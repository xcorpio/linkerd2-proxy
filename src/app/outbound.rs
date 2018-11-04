use http;
use std::fmt;

use app::classify;
use control::destination::{Metadata, ProtocolHint};
use proxy::{
    http::{
        classify::CanClassify,
        client, h1,
        normalize_uri::ShouldNormalizeUri,
        profiles::{self, CanGetDestination},
        router, Settings,
    },
    Source,
};
use svc::{self, stack_per_request::ShouldStackPerRequest};
use tap;
use transport::{connect, tls};
use {HostPort, NamePort};

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub destination: HostPort,
    pub settings: Settings,
    pub connect: connect::Target,
    pub metadata: Metadata,
    _p: (),
}

#[derive(Clone, Debug)]
pub struct Route {
    pub destination: HostPort,
    pub route: profiles::Route,
}

#[derive(Copy, Clone, Debug)]
pub struct RecognizeUnbound;

#[derive(Copy, Clone, Debug)]
pub struct RecognizeSettings;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Unbound(pub HostPort);

// === impl Endpoint ===

impl Endpoint {
    pub fn can_use_orig_proto(&self) -> bool {
        match self.metadata.protocol_hint() {
            ProtocolHint::Unknown => false,
            ProtocolHint::Http2 => true,
        }
    }
}

impl ShouldNormalizeUri for Endpoint {
    fn should_normalize_uri(&self) -> bool {
        !self.settings.is_http2() && !self.dst.settings.was_absolute_form()
    }
}

impl ShouldStackPerRequest for Endpoint {
    fn should_stack_per_request(&self) -> bool {
        !self.settings.is_http2() && !self.dst.settings.can_reuse_clients()
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.connect.addr.fmt(f)
    }
}

impl svc::watch::WithUpdate<tls::ConditionalClientConfig> for Endpoint {
    type Updated = Self;

    fn with_update(&self, client_config: &tls::ConditionalClientConfig) -> Self::Updated {
        let mut ep = self.clone();
        ep.connect.tls = ep.metadata.tls_identity().and_then(|identity| {
            client_config.as_ref().map(|config| tls::ConnectionConfig {
                server_identity: identity.clone(),
                config: config.clone(),
            })
        });
        ep
    }
}

// Makes it possible to build a client::Stack<Endpoint>.
impl From<Endpoint> for client::Config {
    fn from(ep: Endpoint) -> Self {
        client::Config::new(ep.connect, ep.settings)
    }
}

impl From<Endpoint> for tap::Endpoint {
    fn from(ep: Endpoint) -> Self {
        // TODO add route labels...
        tap::Endpoint {
            direction: tap::Direction::Out,
            labels: ep.metadata.labels().clone(),
            client: ep.into(),
        }
    }
}

// === impl Route ===

impl CanClassify for Route {
    type Classify = classify::Request;

    fn classify(&self) -> classify::Request {
        self.route.response_classes().clone().into()
    }
}

// === impl RecognizeUnbound ===

impl<B> router::Recognize<http::Request<B>> for RecognizeUnbound {
    type Target = Unbound;

    fn recognize(&self, req: &http::Request<B>) -> Option<Self::Target> {
        const DEFAULT_PORT: u16 = 80;

        let host_port = req.uri()
            .authority_part()
            .and_then(|a| HostPort::from_authority_with_default_port(a, DEFAULT_PORT).ok())
            .or_else(|| {
                h1::authority_from_host(req)
                    .and_then(|a| HostPort::from_authority_with_default_port(&a, DEFAULT_PORT).ok())
            })
            .or_else(|| {
                req.extensions()
                    .get::<Source>()
                    .and_then(|src| src.orig_dst_if_not_local())
                    .map(HostPort::Addr)
            })?;
        debug!("recognize: unbound={:?}", host_port);

        Some(Unbound(host_port))
    }
}

// === impl RecognizeSettings ===

impl<B> router::Recognize<http::Request<B>> for RecognizeSettings {
    type Target = Settings;

    fn recognize(&self, req: &http::Request<B>) -> Option<Self::Target> {
        Some(Settings::detect(req))
    }
}

// === impl Unbound ===

impl CanGetDestination for Unbound {
    fn get_destination(&self) -> Option<&NamePort> {
        match self.0 {
            HostPort::Name(ref name) => Some(name),
            HostPort::Addr(_) => None,
        }
    }
}

impl fmt::Display for Unbound {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

// TODO this should only work with Bound destinations
impl profiles::WithRoute for Unbound {
    type Output = Route;

    fn with_route(self, route: profiles::Route) -> Self::Output {
        Route { destination: self.0, route }
    }
}

pub mod discovery {
    use futures::{Async, Poll};
    use std::net::SocketAddr;

    use super::{Destination, Endpoint};
    use control::destination::Metadata;
    use proxy::resolve;
    use transport::{connect, tls};
    use {Conditional, HostPort, NamePort};

    #[derive(Clone, Debug)]
    pub struct Resolve<R: resolve::Resolve<NamePort>>(R);

    #[derive(Debug)]
    pub enum Resolution<R: resolve::Resolution> {
        Name(Destination, R),
        Addr(Destination, Option<SocketAddr>),
    }

    // === impl Resolve ===

    impl<R> Resolve<R>
    where
        R: resolve::Resolve<NamePort, Endpoint = Metadata>,
    {
        pub fn new(resolve: R) -> Self {
            Resolve(resolve)
        }
    }

    impl<R> resolve::Resolve<Destination> for Resolve<R>
    where
        R: resolve::Resolve<NamePort, Endpoint = Metadata>,
    {
        type Endpoint = Endpoint;
        type Resolution = Resolution<R::Resolution>;

        fn resolve(&self, dst: &Destination) -> Self::Resolution {
            match dst.host_port {
                HostPort::Name(ref name) => Resolution::Name(dst.clone(), self.0.resolve(&name)),
                HostPort::Addr(ref addr) => Resolution::Addr(dst.clone(), Some(*addr)),
            }
        }
    }

    // === impl Resolution ===

    impl<R> resolve::Resolution for Resolution<R>
    where
        R: resolve::Resolution<Endpoint = Metadata>,
    {
        type Endpoint = Endpoint;
        type Error = R::Error;

        fn poll(&mut self) -> Poll<resolve::Update<Self::Endpoint>, Self::Error> {
            match self {
                Resolution::Name(ref dst, ref mut res) => match try_ready!(res.poll()) {
                    resolve::Update::Remove(addr) => {
                        Ok(Async::Ready(resolve::Update::Remove(addr)))
                    }
                    resolve::Update::Add(addr, metadata) => {
                        // If the endpoint does not have TLS, note the reason.
                        // Otherwise, indicate that we don't (yet) have a TLS
                        // config. This value may be changed by a stack layer that
                        // provides TLS configuration.
                        let tls = match metadata.tls_identity() {
                            Conditional::None(reason) => reason.into(),
                            Conditional::Some(_) => tls::ReasonForNoTls::NoConfig,
                        };
                        let ep = Endpoint {
                            dst: dst.clone(),
                            connect: connect::Target::new(addr, Conditional::None(tls)),
                            metadata,
                            _p: (),
                        };
                        Ok(Async::Ready(resolve::Update::Add(addr, ep)))
                    }
                },
                Resolution::Addr(ref dst, ref mut addr) => match addr.take() {
                    Some(addr) => {
                        let tls = tls::ReasonForNoIdentity::NoAuthorityInHttpRequest;
                        let ep = Endpoint {
                            dst: dst.clone(),
                            connect: connect::Target::new(addr, Conditional::None(tls.into())),
                            metadata: Metadata::none(tls),
                            _p: (),
                        };
                        let up = resolve::Update::Add(addr, ep);
                        Ok(Async::Ready(up))
                    }
                    None => Ok(Async::NotReady),
                },
            }
        }
    }
}

pub mod orig_proto_upgrade {
    use http;

    use super::Endpoint;
    use proxy::http::{orig_proto, Settings};
    use svc;

    #[derive(Debug, Clone)]
    pub struct Layer;

    #[derive(Clone, Debug)]
    pub struct Stack<M>
    where
        M: svc::Stack<Endpoint>,
    {
        inner: M,
    }

    pub fn layer() -> Layer {
        Layer
    }

    impl<M, A, B> svc::Layer<Endpoint, Endpoint, M> for Layer
    where
        M: svc::Stack<Endpoint>,
        M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    {
        type Value = <Stack<M> as svc::Stack<Endpoint>>::Value;
        type Error = <Stack<M> as svc::Stack<Endpoint>>::Error;
        type Stack = Stack<M>;

        fn bind(&self, inner: M) -> Self::Stack {
            Stack { inner }
        }
    }

    // === impl Stack ===

    impl<M, A, B> svc::Stack<Endpoint> for Stack<M>
    where
        M: svc::Stack<Endpoint>,
        M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    {
        type Value = svc::Either<orig_proto::Upgrade<M::Value>, M::Value>;
        type Error = M::Error;

        fn make(&self, endpoint: &Endpoint) -> Result<Self::Value, Self::Error> {
            if endpoint.can_use_orig_proto()
                && !endpoint.dst.settings.is_http2()
                && !endpoint.dst.settings.is_h1_upgrade()
            {
                let mut upgraded = endpoint.clone();
                upgraded.dst.settings = Settings::Http2;
                self.inner.make(&upgraded).map(|i| svc::Either::A(i.into()))
            } else {
                self.inner.make(&endpoint).map(svc::Either::B)
            }
        }
    }
}
