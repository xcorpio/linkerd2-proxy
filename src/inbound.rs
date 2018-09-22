use http;
use std::net::SocketAddr;

use proxy::http::{router, orig_proto, Settings};
use proxy::server::Source;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint {
    addr: SocketAddr,
    settings: Settings,
}

#[derive(Clone, Debug, Default)]
pub struct Recognize {
    default_addr: Option<SocketAddr>,
}

// ===== impl Recognize =====

impl Recognize {
    pub fn new(default_addr: Option<SocketAddr>) -> Self {
        Self {
            default_addr,
        }
    }
}

impl<A> router::Recognize<http::Request<A>> for Recognize {
    type Target = Endpoint;

    fn recognize(&self, req: &http::Request<A>) -> Option<Self::Target> {
        let source = req.extensions().get::<Source>()?;
        trace!("recognize local={} orig={:?}", source.local, source.orig_dst);

        let addr = source.orig_dst_if_not_local().or(self.default_addr)?;
        let settings = orig_proto::detect(req);
        Some(Endpoint { addr, settings })
    }
}

#[cfg(test)]
mod tests {
    use std::net;

    use http;
    use proxy::http::router::Recognize as _Recognize;
    use proxy::http::settings::{Host, Settings};

    use super::{Recognize, Endpoint};
    use ctx;
    use conditional::Conditional;
    use transport::tls;

    fn make_target_http1(addr: net::SocketAddr) -> Endpoint {
        let settings = Settings::Http1 {
            host: Host::NoAuthority,
            is_h1_upgrade: false,
            was_absolute_form: false,
        };
        Endpoint { addr, settings }
    }

    const TLS_DISABLED: Conditional<(), tls::ReasonForNoTls> =
        Conditional::None(tls::ReasonForNoTls::Disabled);

    quickcheck! {
        fn recognize_orig_dst(
            orig_dst: net::SocketAddr,
            local: net::SocketAddr,
            remote: net::SocketAddr
        ) -> bool {
            let ctx = ctx::Proxy::Inbound;

            let inbound = Recognize::default();

            let srv_ctx = ctx::transport::Server::new(
                ctx, &local, &remote, &Some(orig_dst), TLS_DISABLED);

            let rec = srv_ctx.orig_dst_if_not_local().map(make_target_http1);

            let mut req = http::Request::new(());
            req.extensions_mut()
                .insert(srv_ctx);

            inbound.recognize(&req) == rec
        }

        fn recognize_default_no_orig_dst(
            default: Option<net::SocketAddr>,
            local: net::SocketAddr,
            remote: net::SocketAddr
        ) -> bool {
            let inbound = Recognize::new(default);

            let mut req = http::Request::new(());
            req.extensions_mut()
                .insert(ctx::transport::Server::new(
                    ctx::Proxy::Inbound,
                    &local,
                    &remote,
                    &None,
                    TLS_DISABLED,
                ));

            inbound.recognize(&req) == default.map(make_target_http1)
        }

        fn recognize_default_no_ctx(default: Option<net::SocketAddr>) -> bool {
            let ctx = ctx::Proxy::Inbound;

            let inbound = Recognize::new(default);

            let req = http::Request::new(());

            inbound.recognize(&req) == default.map(make_target_http1)
        }

        fn recognize_default_no_loop(
            default: Option<net::SocketAddr>,
            local: net::SocketAddr,
            remote: net::SocketAddr
        ) -> bool {
            let inbound = Recognize::new(default);

            let mut req = http::Request::new(());
            req.extensions_mut()
                .insert(ctx::transport::Server::new(
                    ctx::Proxy::Inbound,
                    &local,
                    &remote,
                    &Some(local),
                    TLS_DISABLED,
                ));

            inbound.recognize(&req) == default.map(make_target_http1)
        }
    }
}
