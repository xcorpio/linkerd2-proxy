use std::marker::PhantomData;
use tower_h2;

use ctx;
use ctx::transport::TlsStatus;
use endpoint::Endpoint;
use svc::NewClient;
use telemetry;
use transparency;
use transport::{self, tls};

use super::Protocol;

/// Binds a `Service` from a `SocketAddr`.
///
/// The returned `Service` buffers request until a connection is established.
///
/// # TODO
///
/// Buffering is not bounded and no timeouts are applied.
pub struct NewEndpoint<B> {
    proxy: ctx::Proxy,
    sensors: telemetry::Sensors,
    transport_registry: transport::metrics::Registry,
    tls_client_config: tls::ClientConfigWatch,
    _p: PhantomData<fn() -> B>,
}

impl<B> NewEndpoint<B> {
    pub fn new(
        proxy: ctx::Proxy,
        sensors: telemetry::Sensors,
        transport_registry: transport::metrics::Registry,
        tls_client_config: tls::ClientConfigWatch
    ) -> Self {
        Self {
            proxy,
            sensors,
            transport_registry,
            tls_client_config,
            _p: PhantomData,
        }
    }
}

impl<B> Clone for NewEndpoint<B> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy,
            sensors: self.sensors.clone(),
            transport_registry: self.transport_registry.clone(),
            tls_client_config: self.tls_client_config.clone(),
            _p: PhantomData,
        }
    }
}

impl<B> NewClient for NewEndpoint<B>
where
    B: tower_h2::Body + Send + 'static,
    <B::Data as ::bytes::IntoBuf>::Buf: Send,
{
    type Target = (Endpoint, Protocol, tls::ConditionalClientConfig);
    type Error = ();
    type Client = Reconnect<orig_proto::Upgrade<NormalizeUri<NewHttp<B>>>>;

    /// Creates a client for a specific endpoint.
    ///
    /// This binds a service stack that comprises the client for that individual
    /// endpoint. It will have to be rebuilt if the TLS configuration changes.
    ///
    /// This includes:
    /// + Reconnects
    /// + URI normalization
    /// + HTTP sensors
    ///
    /// When the TLS client configuration is invalidated, this function will
    /// be called again to bind a new stack.
    fn new_client(&mut self, target: &Self::Target) -> Result<Self::Client, ()> {
        let &(ref ep, ref protocol, ref tls_client_config) = target;

        let tls = ep.tls_identity().and_then(|identity| {
            tls_client_config.as_ref().map(|config| {
                tls::ConnectionConfig {
                    server_identity: identity.clone(),
                    config: config.clone(),
                }
            })
        });
        debug!("new_endpoint endpoint={:?}, protocol={:?}, tls={:?}", ep, protocol, tls);

        let addr = ep.address();
        let client_ctx = ctx::transport::Client::new(
            self.proxy,
            &addr,
            ep.metadata().clone(),
            TlsStatus::from(&tls),
        );

        // Map a socket address to a connection.
        let connect = self.transport_registry
            .new_connect(client_ctx.as_ref(), transport::Connect::new(addr, tls));

        let log = ::logging::Client::proxy(self.ctx, addr).with_protocol(protocol.clone());

        let client = transparency::Client::new(protocol, connect, log.executor());

        let sensors = self.sensors.http(client, &client_ctx);

        // Rewrite the HTTP/1 URI, if the authorities in the Host header
        // and request URI are not in agreement, or are not present.
        let normalize_uri = NormalizeUri::new(sensors, protocol.was_absolute_form());
        let upgrade_orig_proto = orig_proto::Upgrade::new(normalize_uri, protocol.is_http2());

        // Automatically perform reconnects if the connection fails.
        //
        // TODO: Add some sort of backoff logic.
        Ok(Reconnect::new(upgrade_orig_proto))
    }
}


impl<B> NewEndpoint<B>
where
    B: tower_h2::Body + Send + 'static,
    <B::Data as ::bytes::IntoBuf>::Buf: Send,
{
    /// Binds the endpoint stack used to construct a bound service.
    ///
    /// This will wrap the service stack returned by `bind_inner_stack`
    /// with a middleware layer that causes it to be re-constructed when
    /// the TLS client configuration changes.
    ///
    /// This function will itself be called again by `BoundService` if the
    /// service binds per request, or if the initial connection to the
    /// endpoint fails.
    fn bind_stack(&self, ep: &Endpoint, protocol: &Protocol) -> Stack<B> {
        debug!("bind_stack: endpoint={:?}, protocol={:?}", ep, protocol);
        // TODO: Since `BindsPerRequest` bindings are only used for a
        // single request, it seems somewhat unnecessary to wrap them in a
        // `WatchService` middleware so that they can be rebound when the TLS
        // config changes, since they _always_ get rebound regardless. For now,
        // we still add the `WatchService` layer so that the per-request and
        // bound service stacks have the same type.
        let rebind = RebindTls {
            bind: self.clone(),
            endpoint: ep.clone(),
            protocol: protocol.clone(),
        };
        WatchService::new(self.tls_client_config.clone(), rebind)
    }

    pub fn bind_service(&self, ep: &Endpoint, protocol: &Protocol) -> BoundService<B> {
        // If the endpoint is another instance of this proxy, AND the usage
        // of HTTP/1.1 Upgrades are not needed, then bind to an HTTP2 service
        // instead.
        //
        // The related `orig_proto` middleware will automatically translate
        // if the protocol was originally HTTP/1.
        let protocol = if ep.can_use_orig_proto() && !protocol.is_h1_upgrade() {
            &Protocol::Http2
        } else {
            protocol
        };

        let binding = if protocol.can_reuse_clients() {
            Binding::Bound(self.bind_stack(ep, protocol))
        } else {
            Binding::BindsPerRequest {
                next: None
            }
        };

        BoundService {
            bind: self.clone(),
            binding,
            debounce_connect_error_log: false,
            endpoint: ep.clone(),
            protocol: protocol.clone(),
        }
    }
}
