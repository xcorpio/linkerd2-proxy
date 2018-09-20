use indexmap::IndexMap;
use std::net::SocketAddr;
use std::sync::Arc;

use futures::Poll;
use http::{self, uri};
use tower_h2;

use ctx;
use ctx::transport::TlsStatus;
use telemetry;
use proxy;
use svc::{self, Layer, stack::watch};
use transport;
use tls;

/// An HTTP `Service` that is created for each `Endpoint` and `Protocol`.
pub type Stack<B> = proxy::http::orig_proto::Upgrade<
    proxy::http::normalize_uri::Service<
        WatchTls<B>
    >
>;

type WatchTls<B> = svc::stack::watch::Service<tls::ConditionalClientConfig, RebindTls<B>>;

/// An HTTP `Service` that is created for each `Endpoint`, `Protocol`, and client
/// TLS configuration.
pub type TlsStack<B> = telemetry::http::service::Http<HttpService<B>, B, proxy::http::Body>;

type HttpService<B> = proxy::Reconnect<
    Arc<ctx::transport::Client>,
    proxy::http::Client<
        transport::metrics::Connect<transport::Connect>,
        ::logging::ClientExecutor<&'static str, SocketAddr>,
        telemetry::http::service::RequestBody<B>,
    >
>;

/// Identifies a connection from the proxy to another process.
#[derive(Debug)]
pub struct Endpoint {
    pub proxy: ctx::Proxy,
    pub address: SocketAddr,
    pub labels: IndexMap<String, String>,
    pub tls_status: tls::ConditionalClientConfig,
    _p: (),
}

pub struct SetTlsLayer {
    endpoint: Endpoint,
    protocol: Protocol,
}

pub struct SetTlsMake<M> {
    endpoint: Endpoint,
    protocol: Protocol,
    inner: M,
}

/// Protocol portion of the `Recognize` key for a request.
///
/// This marks whether to use HTTP/2 or HTTP/1.x for a request. In
/// the case of HTTP/1.x requests, it also stores a "host" key to ensure
/// that each host receives its own connection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Http1 {
        host: Host,
        /// Whether the request wants to use HTTP/1.1's Upgrade mechanism.
        ///
        /// Since these cannot be translated into orig-proto, it must be
        /// tracked here so as to allow those with `is_h1_upgrade: true` to
        /// use an explicitly HTTP/1 service, instead of one that might
        /// utilize orig-proto.
        is_h1_upgrade: bool,
        /// Whether or not the request URI was in absolute form.
        ///
        /// This is used to configure Hyper's behaviour at the connection
        /// level, so it's necessary that requests with and without
        /// absolute URIs be bound to separate service stacks. It is also
        /// used to determine what URI normalization will be necessary.
        was_absolute_form: bool,
    },
    Http2
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Authority(uri::Authority),
    NoAuthority,
}

// ===== impl IntoClient =====

impl<M> svc::Make<tls::ConditionalClientConfig> for SetTlsMake<M>
where
    M: svc::Make<Endpoint>,
    M::Output: svc::Service,
{
    type Output = M::Output;
    type Error = M::Error;

    fn make(
        &mut self,
        tls_client_config: &tls::ConditionalClientConfig
    ) -> Result<Self::Output, Self::Error> {
        debug!(
            "rebinding endpoint stack for {:?}:{:?} on TLS config change",
            self.endpoint, self.protocol,
        );

        let tls = self.endpoint.tls_identity().and_then(|identity| {
            tls_client_config.as_ref().map(|config| {
                tls::ConnectionConfig {
                    server_identity: identity.clone(),
                    config: config.clone(),
                }
            })
        });

        let client_ctx = ctx::transport::Client::new(
            self.ctx,
            &addr,
            ep.metadata().clone(),
            TlsStatus::from(&tls),
        );

        self.inner.make(&client_ctx)
    }
}

// fn bind_with_tls(
//     ep: &Endpoint,
//     protocol: &Protocol,
//     tls_client_config: &tls::ConditionalClientConfig,
// ) {
//     debug!("bind_with_tls endpoint={:?}, protocol={:?}", ep, protocol);
//     let tls = ep.tls_identity().and_then(|identity| {
//         tls_client_config.as_ref().map(|config| {
//             tls::ConnectionConfig {
//                 server_identity: identity.clone(),
//                 config: config.clone(),
//             }
//         })
//     });
//     let client_ctx = ctx::transport::Client::new(
//         self.ctx,
//         &addr,
//         ep.metadata().clone(),
//         TlsStatus::from(&tls),
//     );
//     // Map a socket address to a connection.
//     let connect = self.transport_registry
//         .new_connect(client_ctx.as_ref(), transport::Connect::new(addr, tls));
//     // TODO: Add some sort of backoff logic between reconnects.
//     self.sensors.http(
//         client_ctx.clone(),
//         proxy::Reconnect::new(
//             client_ctx.clone(),
//             proxy::http::Client::new(protocol, connect, log.executor())
//         )
//     )
// }

// fn bind_stack(&self, ep: &Endpoint, protocol: &Protocol) {
//     debug!("bind_stack: endpoint={:?}, protocol={:?}", ep, protocol);
//     let rebind = RebindTls {
//         bind: self.clone(),
//         endpoint: ep.clone(),
//         protocol: protocol.clone(),
//     };
//     let watch_tls = watch::Service::try(self.tls_client_config.clone(), rebind)
//         .expect("tls client must not fail");
// }

pub fn bind_service(&self, ep: &Endpoint, protocol: &Protocol) {
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
}

// ===== impl Protocol =====

impl Protocol {
    pub fn detect<B>(req: &http::Request<B>) -> Self {
        if req.version() == http::Version::HTTP_2 {
            return Protocol::Http2;
        }

        let was_absolute_form = proxy::http::h1::is_absolute_form(req.uri());
        trace!(
            "Protocol::detect(); req.uri='{:?}'; was_absolute_form={:?};",
            req.uri(), was_absolute_form
        );
        // If the request has an authority part, use that as the host part of
        // the key for an HTTP/1.x request.
        let host = Host::detect(req);

        let is_h1_upgrade = proxy::http::h1::wants_upgrade(req);

        Protocol::Http1 {
            host,
            is_h1_upgrade,
            was_absolute_form,
        }
    }

    /// Returns true if the request was originally received in absolute form.
    pub fn was_absolute_form(&self) -> bool {
        match self {
            &Protocol::Http1 { was_absolute_form, .. } => was_absolute_form,
            _ => false,
        }
    }

    pub fn can_reuse_clients(&self) -> bool {
        match *self {
            Protocol::Http2 | Protocol::Http1 { host: Host::Authority(_), .. } => true,
            _ => false,
        }
    }

    pub fn is_h1_upgrade(&self) -> bool {
        match *self {
            Protocol::Http1 { is_h1_upgrade: true, .. } => true,
            _ => false,
        }
    }

    pub fn is_http2(&self) -> bool {
        match *self {
            Protocol::Http2 => true,
            _ => false,
        }
    }
}

impl Host {
    pub fn detect<B>(req: &http::Request<B>) -> Host {
        req
            .uri()
            .authority_part()
            .cloned()
            .or_else(|| proxy::http::h1::authority_from_host(req))
            .map(Host::Authority)
            .unwrap_or_else(|| Host::NoAuthority)
    }
}
