use bytes;
use http;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::{error, fmt};
use tower_h2::Body;

use proxy::http::{client, orig_proto, router, Settings};
use proxy::server::Source;
use svc;
use transport::{connect, tls};
use Conditional;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub addr: SocketAddr,
    pub settings: Settings,
}

// === Recognize ===

#[derive(Clone, Debug, Default)]
pub struct Recognize {
    default_addr: Option<SocketAddr>,
}

impl Recognize {
    pub fn new(default_addr: Option<SocketAddr>) -> Self {
        Self { default_addr }
    }
}

impl<A> router::Recognize<http::Request<A>> for Recognize {
    type Target = Endpoint;

    fn recognize(&self, req: &http::Request<A>) -> Option<Self::Target> {
        let source = req.extensions().get::<Source>()?;
        trace!(
            "recognize local={} orig={:?}",
            source.local,
            source.orig_dst
        );

        let addr = source.orig_dst_if_not_local().or(self.default_addr)?;
        let settings = Settings::detect(req);
        Some(Endpoint { addr, settings })
    }
}

impl fmt::Display for Recognize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "in")
    }
}

pub fn orig_proto_downgrade<M>() -> LayerDowngrade<M> {
    LayerDowngrade(PhantomData)
}

#[derive(Debug)]
pub struct LayerDowngrade<M>(PhantomData<fn() -> (M)>);

#[derive(Clone, Debug)]
pub struct MakeDowngrade<M>
where
    M: svc::Make<Source>,
{
    inner: M,
}

impl<M> Clone for LayerDowngrade<M> {
    fn clone(&self) -> Self {
        LayerDowngrade(PhantomData)
    }
}

impl<M, A, B> svc::Layer<Source, Source, M> for LayerDowngrade<M>
where
    M: svc::Make<Source>,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
{
    type Value = <MakeDowngrade<M> as svc::Make<Source>>::Value;
    type Error = <MakeDowngrade<M> as svc::Make<Source>>::Error;
    type Make = MakeDowngrade<M>;

    fn bind(&self, inner: M) -> Self::Make {
        MakeDowngrade { inner }
    }
}

impl<M, A, B> svc::Make<Source> for MakeDowngrade<M>
where
    M: svc::Make<Source>,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
{
    type Value = orig_proto::Downgrade<M::Value>;
    type Error = M::Error;

    fn make(&self, target: &Source) -> Result<Self::Value, Self::Error> {
        let inner = self.inner.make(&target)?;
        Ok(inner.into())
    }
}

#[derive(Debug)]
pub struct Client<C, B>
where
    C: svc::Make<connect::Target>,
    C::Value: connect::Connect + Clone + Send + Sync + 'static,
    B: Body + 'static,
{
    inner: client::Make<C, B>,
}

impl<C, B> Client<C, B>
where
    C: svc::Make<connect::Target>,
    C::Value: connect::Connect + Clone + Send + Sync + 'static,
    <C::Value as connect::Connect>::Connected: Send,
    <C::Value as connect::Connect>::Future: Send + 'static,
    <C::Value as connect::Connect>::Error: error::Error + Send + Sync,
    B: Body + Send + 'static,
    <B::Data as bytes::IntoBuf>::Buf: Send + 'static,
{
    pub fn new(connect: C) -> Client<C, B> {
        Self {
            inner: client::Make::new("in", connect),
        }
    }
}

impl<C, B> Clone for Client<C, B>
where
    C: svc::Make<connect::Target> + Clone,
    C::Value: connect::Connect + Clone + Send + Sync + 'static,
    <C::Value as connect::Connect>::Connected: Send,
    <C::Value as connect::Connect>::Future: Send + 'static,
    <C::Value as connect::Connect>::Error: error::Error + Send + Sync,
    B: Body + Send + 'static,
    <B::Data as bytes::IntoBuf>::Buf: Send + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<C, B> svc::Make<Endpoint> for Client<C, B>
where
    C: svc::Make<connect::Target>,
    C::Value: connect::Connect + Clone + Send + Sync + 'static,
    <C::Value as connect::Connect>::Connected: Send,
    <C::Value as connect::Connect>::Future: Send + 'static,
    <C::Value as connect::Connect>::Error: error::Error + Send + Sync,
    B: Body + Send + 'static,
    <B::Data as bytes::IntoBuf>::Buf: Send + 'static,
{
    type Value = <client::Make<C, B> as svc::Make<client::Config>>::Value;
    type Error = <client::Make<C, B> as svc::Make<client::Config>>::Error;

    fn make(&self, ep: &Endpoint) -> Result<Self::Value, Self::Error> {
        let tls = Conditional::None(tls::ReasonForNoTls::InternalTraffic);
        let target = connect::Target::new(ep.addr, tls);
        let config = client::Config::new(target, ep.settings.clone());
        self.inner.make(&config)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.addr.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use std::net;

    use http;
    use proxy::http::router::Recognize as _Recognize;
    use proxy::http::settings::{Host, Settings};

    use super::{Endpoint, Recognize};
    use ctx;
    use transport::tls;
    use Conditional;

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
