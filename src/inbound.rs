use http;
//use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bind::Protocol;
use ctx;
use proxy::http::{router, orig_proto};

pub fn router<A>(
    default_addr: Option<SocketAddr>,
    capacity: usize,
    max_idle_age: Duration
) -> router::Layer<http::Request<A>, Recognize>
where
    A: Send + 'static,
{
    let r = Recognize { default_addr };
    router::Layer::new(r, capacity, max_idle_age)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    addr: SocketAddr,
    protocol: Protocol,
}

#[derive(Clone, Debug, Default)]
pub struct Recognize {
    default_addr: Option<SocketAddr>,
}

// ===== impl Recognize =====

impl Recognize {
    pub fn new(default_addr: SocketAddr) -> Self {
        Self {
            default_addr: Some(default_addr),
        }
    }
}

impl<A> router::Recognize<http::Request<A>> for Recognize {
    type Target = Target;

    fn recognize(&self, req: &http::Request<A>) -> Option<Self::Target> {
        let ctx = req.extensions().get::<Arc<ctx::transport::Server>>()?;
        trace!("recognize local={} orig={:?}", ctx.local, ctx.orig_dst);

        let addr = ctx.orig_dst_if_not_local().or(self.default_addr)?;
        let protocol = orig_proto::detect(req);
        Some(Target { addr, protocol })
    }
}

#[cfg(test)]
mod tests {
    use std::net;

    use http;
    use proxy::http::router::Recognize as _Recognize;

    use super::{Recognize, Target};
    use bind::{self, Host};
    use ctx;
    use conditional::Conditional;
    use tls;

    fn make_target_http1(addr: net::SocketAddr) -> Target {
        let protocol = bind::Protocol::Http1 {
            host: Host::NoAuthority,
            is_h1_upgrade: false,
            was_absolute_form: false,
        };
        Target { addr, protocol }
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
            let inbound = default.map(Recognize::new).unwrap_or_default();

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

            let inbound = default.map(Recognize::new).unwrap_or_default();

            let req = http::Request::new(());

            inbound.recognize(&req) == default.map(make_target_http1)
        }

        fn recognize_default_no_loop(
            default: Option<net::SocketAddr>,
            local: net::SocketAddr,
            remote: net::SocketAddr
        ) -> bool {
            let inbound = default.map(Recognize::new).unwrap_or_default();

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
