#![allow(unused_imports)]

use indexmap::IndexMap;
use std::net::SocketAddr;
use std::sync::Arc;

use http::{self, uri};
use tower_h2;

use ctx;
use ctx::transport::TlsStatus;
use telemetry;
use proxy;
use proxy::http::Settings;
use svc::{self, Layer, stack::watch};
use transport;
use tls;

/// An HTTP `Service` that is created for each `Endpoint` and `Settings`.
pub type Stack<B> = proxy::http::orig_proto::Upgrade<
    proxy::http::normalize_uri::Service<
        WatchTls<B>
    >
>;

type WatchTls<B> = svc::stack::watch::Service<tls::ConditionalClientConfig, RebindTls<B>>;

/// An HTTP `Service` that is created for each `Endpoint`, `Settings`, and client
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
    settings: Settings,
}

pub struct SetTlsMake<M> {
    endpoint: Endpoint,
    settings: Settings,
    inner: M,
}

// ===== impl IntoClient =====

impl<M> svc::Make<tls::ConditionalClientConfig> for SetTlsMake<M>
where
    M: svc::Make<Endpoint>,
    M::Value: svc::Service,
{
    type Value = M::Value;
    type Error = M::Error;

    fn make(
        &self,
        tls_client_config: &tls::ConditionalClientConfig
    ) -> Result<Self::Value, Self::Error> {
        debug!(
            "rebinding endpoint stack for {:?}:{:?} on TLS config change",
            self.endpoint, self.settings,
        );

        let tls = self.endpoint.tls_identity().and_then(|identity| {
            tls_client_config.as_ref().map(|config| {
                tls::ConnectionConfig {
                    server_identity: identity.clone(),
                    config: config.clone(),
                }
            })
        });

        self.inner.make(&client_ctx)
    }
}

// fn bind_with_tls(
//     ep: &Endpoint,
//     settings: &Settings,
//     tls_client_config: &tls::ConditionalClientConfig,
// ) {
//     debug!("bind_with_tls endpoint={:?}, settings={:?}", ep, settings);
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
//             proxy::http::Client::new(settings, connect, log.executor())
//         )
//     )
// }

// fn bind_stack(&self, ep: &Endpoint, settings: &Settings) {
//     debug!("bind_stack: endpoint={:?}, settings={:?}", ep, settings);
//     let rebind = RebindTls {
//         bind: self.clone(),
//         endpoint: ep.clone(),
//         settings: settings.clone(),
//     };
//     let watch_tls = watch::Service::try(self.tls_client_config.clone(), rebind)
//         .expect("tls client must not fail");
// }

// pub fn bind_service(&self, ep: &Endpoint, settings: &Settings) {
//     // If the endpoint is another instance of this proxy, AND the usage
//     // of HTTP/1.1 Upgrades are not needed, then bind to an HTTP2 service
//     // instead.
//     //
//     // The related `orig_proto` middleware will automatically translate
//     // if the settings was originally HTTP/1.
//     let settings = if ep.can_use_orig_proto() && !settings.is_h1_upgrade() {
//         &Settings::Http2
//     } else {
//         settings
//     };
// }

