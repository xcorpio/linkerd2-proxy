use http;
use std::fmt;

use app::classify;
use control::destination::{Metadata, ProtocolHint};
use proxy::http::{
    classify::CanClassify,
    profiles::{self, CanGetDestination},
    router, settings,
};
use svc;
use tap;
use transport::{connect, tls};
use {Addr, NameAddr};

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub dst_name: Option<NameAddr>,
    pub connect: connect::Target,
    pub metadata: Metadata,
}

#[derive(Clone, Debug)]
pub struct Route {
    pub dst_addr: Addr,
    pub route: profiles::Route,
}

#[derive(Copy, Clone, Debug)]
pub struct RecognizeDstAddr;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DstAddr(Addr);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CanonicalDstAddr(Addr);

// === impl Endpoint ===

impl Endpoint {
    pub fn can_use_orig_proto(&self) -> bool {
        match self.metadata.protocol_hint() {
            ProtocolHint::Unknown => false,
            ProtocolHint::Http2 => true,
        }
    }
}

impl settings::router::HasConnect for Endpoint {
    fn connect(&self) -> connect::Target {
        self.connect.clone()
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

impl From<Endpoint> for tap::Endpoint {
    fn from(ep: Endpoint) -> Self {
        // TODO add route labels...
        tap::Endpoint {
            direction: tap::Direction::Out,
            labels: ep.metadata.labels().clone(),
            target: ep.connect.clone(),
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

// === impl RecognizeDstAddr ===

impl<B> router::Recognize<http::Request<B>> for RecognizeDstAddr {
    type Target = DstAddr;

    fn recognize(&self, req: &http::Request<B>) -> Option<Self::Target> {
        let addr = super::http_request_addr(req).ok()?;
        debug!("recognize: dst={:?}", addr);
        Some(DstAddr(addr))
    }
}

// === impl DstAddr ===

impl fmt::Display for DstAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

// === impl CanonicalDstAddr ===

impl fmt::Display for CanonicalDstAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl CanGetDestination for CanonicalDstAddr {
    fn get_destination(&self) -> Option<&NameAddr> {
        match self.0 {
            Addr::Name(ref name) => Some(name),
            Addr::Socket(_) => None,
        }
    }
}

// TODO this should only work with Bound destinations
impl profiles::WithRoute for CanonicalDstAddr {
    type Output = Route;

    fn with_route(self, route: profiles::Route) -> Self::Output {
        Route {
            dst_addr: self.0,
            route,
        }
    }
}

pub mod discovery {
    use futures::{Async, Poll};
    use std::net::SocketAddr;

    use super::{Endpoint, CanonicalDstAddr};
    use control::destination::Metadata;
    use proxy::resolve;
    use transport::{connect, tls};
    use {Addr, Conditional, NameAddr};

    #[derive(Clone, Debug)]
    pub struct Resolve<R: resolve::Resolve<NameAddr>>(R);

    #[derive(Debug)]
    pub enum Resolution<R: resolve::Resolution> {
        Name(NameAddr, R),
        Addr(Option<SocketAddr>),
    }

    // === impl Resolve ===

    impl<R> Resolve<R>
    where
        R: resolve::Resolve<NameAddr, Endpoint = Metadata>,
    {
        pub fn new(resolve: R) -> Self {
            Resolve(resolve)
        }
    }

    impl<R> resolve::Resolve<CanonicalDstAddr> for Resolve<R>
    where
        R: resolve::Resolve<NameAddr, Endpoint = Metadata>,
    {
        type Endpoint = Endpoint;
        type Resolution = Resolution<R::Resolution>;

        fn resolve(&self, dst: &CanonicalDstAddr) -> Self::Resolution {
            match dst {
                CanonicalDstAddr(Addr::Name(ref name)) => {
                    Resolution::Name(name.clone(), self.0.resolve(&name))
                }
                CanonicalDstAddr(Addr::Socket(ref addr)) => Resolution::Addr(Some(*addr)),
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
                Resolution::Name(ref name, ref mut res) => match try_ready!(res.poll()) {
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
                            dst_name: Some(name.clone()),
                            connect: connect::Target::new(addr, Conditional::None(tls)),
                            metadata,
                        };
                        Ok(Async::Ready(resolve::Update::Add(addr, ep)))
                    }
                },
                Resolution::Addr(ref mut addr) => match addr.take() {
                    Some(addr) => {
                        let tls = tls::ReasonForNoIdentity::NoAuthorityInHttpRequest;
                        let ep = Endpoint {
                            dst_name: None,
                            connect: connect::Target::new(addr, Conditional::None(tls.into())),
                            metadata: Metadata::none(tls),
                        };
                        Ok(Async::Ready(resolve::Update::Add(addr, ep)))
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
    use proxy::http::orig_proto;
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
            if endpoint.can_use_orig_proto() {
                self.inner.make(&endpoint).map(|i| svc::Either::A(i.into()))
            } else {
                self.inner.make(&endpoint).map(svc::Either::B)
            }
        }
    }
}

pub mod canonicalize {
    use futures::{Async, Future, Poll, future};
    use http;
    use std::{error, fmt};
    use std::time::Duration;
    use tokio_timer::{clock, Delay};

    use dns;
    use svc;
    use {Addr, NameAddr};

    use super::{DstAddr, CanonicalDstAddr};

    // Duration to wait before polling DNS again after an error (or a NXDOMAIN
    // response with no TTL).
    const DNS_ERROR_TTL: Duration = Duration::from_secs(5);

    #[derive(Debug, Clone)]
    pub struct Layer(dns::Resolver);

    #[derive(Clone, Debug)]
    pub struct Stack<M: svc::Stack<CanonicalDstAddr>> {
        resolver: dns::Resolver,
        inner: M,
    }

    pub struct Service<M: svc::Stack<CanonicalDstAddr>> {
        original_addr: NameAddr,
        resolver: dns::Resolver,
        service: Option<M::Value>,
        service_name: Option<CanonicalDstAddr>,
        stack: M,
        state: State,
    }

    enum State {
        Pending(dns::RefineFuture),
        ValidUntil(Delay),
    }

    #[derive(Debug)]
    pub enum Error<M, S> {
        Stack(M),
        Service(S),
    }

    pub fn layer(resolver: dns::Resolver) -> Layer {
        Layer(resolver)
    }

    impl<M, A, B> svc::Layer<DstAddr, CanonicalDstAddr, M> for Layer
    where
        M: svc::Stack<CanonicalDstAddr> + Clone,
        M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    {
        type Value = <Stack<M> as svc::Stack<DstAddr>>::Value;
        type Error = <Stack<M> as svc::Stack<DstAddr>>::Error;
        type Stack = Stack<M>;

        fn bind(&self, inner: M) -> Self::Stack {
            let resolver = self.0.clone();
            Stack { inner, resolver }
        }
    }

    // === impl Stack ===

    impl<M, A, B> svc::Stack<DstAddr> for Stack<M>
    where
        M: svc::Stack<CanonicalDstAddr> + Clone,
        M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    {
        type Value = svc::Either<Service<M>, M::Value>;
        type Error = M::Error;

        fn make(&self, dst_addr: &DstAddr) -> Result<Self::Value, Self::Error> {
            match dst_addr {
                DstAddr(Addr::Name(na)) => {
                    let fut = self.resolver.refine(na.name());
                    let svc = Service {
                        original_addr: na.clone(),
                        stack: self.inner.clone(),
                        resolver: self.resolver.clone(),
                        state: State::Pending(fut),
                        service: None,
                        service_name: None,
                    };
                    Ok(svc::Either::A(svc))
                }
                DstAddr(addr @ Addr::Socket(_)) => {
                    let target = CanonicalDstAddr(addr.clone());
                    self.inner.make(&target).map(svc::Either::B)
                }
            }
        }
    }

    impl<M, A, B> Service<M>
    where
        M: svc::Stack<CanonicalDstAddr>,
        M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    {
        fn poll_state(&mut self) -> Poll<(), M::Error> {
            loop {
                self.state = match self.state {
                    State::Pending(ref mut fut) => match fut.poll() {
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Ok(Async::Ready(refine)) => {
                            let addr = NameAddr::new(refine.name, self.original_addr.port());
                            let service_name = CanonicalDstAddr(addr.into());

                            if self.service_name.as_ref() != Some(&service_name) {
                                let svc = self.stack.make(&service_name)?;
                                self.service = Some(svc);
                                self.service_name = Some(service_name);
                            }

                            State::ValidUntil(Delay::new(refine.valid_until))
                        }
                        Err(e) => {
                            if self.service.is_none() {
                                let service_name = CanonicalDstAddr(self.original_addr.clone().into());
                                let svc = self.stack.make(&service_name)?;
                                self.service = Some(svc);
                                self.service_name = Some(service_name);
                            }

                            let valid_until = match e.kind() {
                                dns::ResolveErrorKind::NoRecordsFound { valid_until, .. } => {
                                    valid_until.unwrap_or_else(|| clock::now() + DNS_ERROR_TTL)
                                }
                                _ => clock::now() + DNS_ERROR_TTL,
                            };
                            State::ValidUntil(Delay::new(valid_until))
                        }
                    },

                    State::ValidUntil(ref mut f) => match f.poll().expect("timer must not fail") {
                        Async::NotReady => return Ok(Async::NotReady),
                        Async::Ready(()) => {
                            State::Pending(self.resolver.refine(self.original_addr.name()))
                        }
                    },
                };
            }
        }
    }

    impl<M, A, B> svc::Service for Service<M>
    where
        M: svc::Stack<CanonicalDstAddr>,
        M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    {
        type Request = http::Request<A>;
        type Response = http::Response<B>;
        type Error = Error<M::Error, <M::Value as svc::Service>::Error>;
        type Future = future::MapErr<
            <M::Value as svc::Service>::Future,
            fn(<M::Value as svc::Service>::Error) -> Self::Error,
        >;

        fn poll_ready(&mut self) -> Poll<(), Self::Error> {
            self.poll_state().map_err(Error::Stack)?;

            match self.service.as_mut() {
                None => Ok(Async::NotReady),
                Some(ref mut svc) => svc.poll_ready().map_err(Error::Service),
            }
        }

        fn call(&mut self, req: Self::Request) -> Self::Future {
            let svc = self.service.as_mut().expect("poll_ready must be called first");
            svc.call(req).map_err(Error::Service)
        }
    }

    impl<M: fmt::Display, S: fmt::Display> fmt::Display for Error<M, S> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                Error::Stack(e) => e.fmt(f),
                Error::Service(e) => e.fmt(f),
            }
        }
    }

    impl<M: error::Error, S: error::Error> error::Error for Error<M, S> {
        fn cause(&self) -> Option<&error::Error> {
            match self {
                Error::Stack(e) => e.cause(),
                Error::Service(e) => e.cause(),
            }
        }
    }
}
