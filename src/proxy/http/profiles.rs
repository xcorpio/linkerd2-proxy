#![allow(dead_code)]

extern crate tower_discover;

use futures::Stream;
use http;
use indexmap::IndexMap;
use regex::Regex;
use std::iter::FromIterator;
use std::sync::Arc;
use std::{error, fmt};

use transport::DnsNameAndPort;

pub trait CanGetDestination {
    fn get_destination(&self) -> Option<&DnsNameAndPort>;
}

pub type Routes = Vec<(RequestMatch, Route)>;

pub trait GetRoutes {
    type Stream: Stream<Item = Routes, Error = Error>;

    fn get_routes(&self, dst: &DnsNameAndPort) -> Option<Self::Stream>;
}

pub trait WithRoute {
    type Output;

    fn with_route(self, route: Route) -> Self::Output;
}

#[derive(Debug)]
pub enum Error {}

#[derive(Debug, Clone)]
pub enum RequestMatch {
    All(Vec<RequestMatch>),
    Any(Vec<RequestMatch>),
    Not(Box<RequestMatch>),
    Path(Regex),
    Method(http::Method),
}

// TODO provide a `Classify` implementation derived from api::destination::ResponseClass,
#[derive(Clone, Debug, Default)]
pub struct Route {
    labels: Arc<IndexMap<String, String>>,
}

impl Route {
    pub fn new<'i, I>(label_iter: I) -> Self
    where
        I: Iterator<Item = (String, String)>,
    {
        let labels = {
            let mut pairs = label_iter.collect::<Vec<(String, String)>>();
            pairs.sort_by(|(k0, _), (k1, _)| k0.cmp(k1));
            Arc::new(IndexMap::from_iter(pairs))
        };
        Self { labels }
    }

    pub fn labels(&self) -> &IndexMap<String, String> {
        self.labels.as_ref()
    }
}

impl RequestMatch {
    fn is_match<B>(&self, req: &http::Request<B>) -> bool {
        match self {
            RequestMatch::Method(ref method) => req.method() == *method,
            RequestMatch::Path(ref re) => re.is_match(req.uri().path()),
            RequestMatch::Not(ref m) => !m.is_match(req),
            RequestMatch::All(ref matches) => {
                for ref m in matches {
                    if !m.is_match(req) {
                        return false;
                    }
                }
                true
            }
            RequestMatch::Any(ref matches) => {
                for ref m in matches {
                    if m.is_match(req) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
        unreachable!()
    }
}

impl error::Error for Error {}

pub mod router {
    use futures::{Async, Poll, Stream};
    use http;
    use std::{error, fmt};

    use svc;

    use super::*;

    pub fn layer<T, G, M, R>(get_routes: G, route_layer: R) -> Layer<G, M, R>
    where
        T: CanGetDestination + WithRoute + Clone,
        M: svc::Stack<T>,
        M::Value: Clone,
        G: GetRoutes + Clone,
        R: svc::Layer<
                <T as WithRoute>::Output,
                <T as WithRoute>::Output,
                svc::shared::Stack<M::Value>,
            >
            + Clone,
        R::Value: svc::Service,
    {
        Layer {
            get_routes,
            route_layer,
            default_route: Route::default(),
            _p: ::std::marker::PhantomData,
        }
    }

    #[derive(Clone, Debug)]
    pub struct Layer<G, M, R = ()> {
        get_routes: G,
        route_layer: R,
        default_route: Route,
        _p: ::std::marker::PhantomData<fn() -> M>,
    }

    #[derive(Clone, Debug)]
    pub struct Stack<G, M, R = ()> {
        inner: M,
        get_routes: G,
        route_layer: R,
        default_route: Route,
    }

    #[derive(Debug)]
    pub enum Error<D, R> {
        Inner(D),
        Route(R),
    }

    pub struct Service<G, T, R>
    where
        T: WithRoute,
        R: svc::Stack<T::Output>,
        R::Value: svc::Service,
    {
        target: T,
        stack: R,
        route_stream: Option<G>,
        routes: Vec<(RequestMatch, R::Value)>,
        default_route: R::Value,
    }

    impl<D: fmt::Display, R: fmt::Display> fmt::Display for Error<D, R> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                Error::Inner(e) => fmt::Display::fmt(&e, f),
                Error::Route(e) => fmt::Display::fmt(&e, f),
            }
        }
    }

    impl<D: error::Error, R: error::Error> error::Error for Error<D, R> {}

    impl<T, G, M, R> svc::Layer<T, T, M> for Layer<G, M, R>
    where
        T: CanGetDestination + WithRoute + Clone,
        G: GetRoutes + Clone,
        M: svc::Stack<T>,
        M::Value: Clone,
        R: svc::Layer<
                <T as WithRoute>::Output,
                <T as WithRoute>::Output,
                svc::shared::Stack<M::Value>,
            >
            + Clone,
        R::Value: svc::Service,
    {
        type Value = <Stack<G, M, R> as svc::Stack<T>>::Value;
        type Error = <Stack<G, M, R> as svc::Stack<T>>::Error;
        type Stack = Stack<G, M, R>;

        fn bind(&self, inner: M) -> Self::Stack {
            Stack {
                inner,
                get_routes: self.get_routes.clone(),
                route_layer: self.route_layer.clone(),
                default_route: self.default_route.clone(),
            }
        }
    }

    impl<T, G, M, R> svc::Stack<T> for Stack<G, M, R>
    where
        T: CanGetDestination + WithRoute + Clone,
        M: svc::Stack<T>,
        M::Value: Clone,
        G: GetRoutes,
        R: svc::Layer<
                <T as WithRoute>::Output,
                <T as WithRoute>::Output,
                svc::shared::Stack<M::Value>,
            >
            + Clone,
        R::Value: svc::Service,
    {
        type Value = Service<G::Stream, T, R::Stack>;
        type Error = Error<M::Error, R::Error>;

        fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
            let inner = self.inner.make(&target).map_err(Error::Inner)?;
            let stack = self.route_layer.bind(svc::shared::stack(inner));

            let default_route = {
                let t = target.clone().with_route(self.default_route.clone());
                stack.make(&t).map_err(Error::Route)?
            };

            let route_stream = target.get_destination()
                .and_then(|d| self.get_routes.get_routes(&d));

            Ok(Service {
                target: target.clone(),
                stack,
                route_stream,
                default_route,
                routes: Vec::new(),
            })
        }
    }

    impl<G, T, R> Service<G, T, R>
    where
        G: Stream<Item = Routes, Error = super::Error>,
        T: WithRoute + Clone,
        R: svc::Stack<T::Output> + Clone,
        R::Value: svc::Service,
    {
        fn update_routes(&mut self, mut routes: Routes) {
            self.routes = Vec::with_capacity(routes.len());
            for (req_match, route) in routes.drain(..) {
                match self
                    .stack
                    .make(&self.target.clone().with_route(route.clone()))
                {
                    Ok(svc) => self.routes.push((req_match, svc)),
                    Err(_) => error!("failed to build service for route: route={:?}", route),
                }
            }
        }

        fn poll_route_stream(&mut self) -> Option<Async<Option<Routes>>> {
            self.route_stream.as_mut()
                .and_then(|ref mut s| s.poll().ok())
        }
    }

    impl<G, T, R, B> svc::Service for Service<G, T, R>
    where
        G: Stream<Item = Routes, Error = super::Error>,
        T: WithRoute + Clone,
        R: svc::Stack<T::Output> + Clone,
        R::Value: svc::Service<Request = http::Request<B>>,
    {
        type Request = <R::Value as svc::Service>::Request;
        type Response = <R::Value as svc::Service>::Response;
        type Error = <R::Value as svc::Service>::Error;
        type Future = <R::Value as svc::Service>::Future;

        fn poll_ready(&mut self) -> Poll<(), Self::Error> {
            while let Some(Async::Ready(Some(routes))) = self.poll_route_stream() {
                self.update_routes(routes);
            }

            Ok(Async::Ready(()))
        }

        fn call(&mut self, req: Self::Request) -> Self::Future {
            for (ref condition, ref mut service) in &mut self.routes {
                if condition.is_match(&req) {
                    return service.call(req);
                }
            }

            self.default_route.call(req)
        }
    }
}

// pub mod per_endpoint {
//     use futures::Poll;
//     use std::{error, fmt};
//
//     use proxy::resolve::HasEndpoint;
//     use svc::{self, Stack as _Stack};
//
//     use super::tower_discover::{Change, Discover};
//     use super::Route;
//
//     pub fn layer<S, E>(endpoint_layer: E) -> Layer<E>
//     where
//         S: svc::Service + Clone + HasEndpoint,
//         <S as HasEndpoint>::Endpoint: WithRoute,
//         E: svc::Layer<
//                 <<S as HasEndpoint>::Endpoint as WithRoute>::Output,
//                 <<S as HasEndpoint>::Endpoint as WithRoute>::Output,
//                 svc::shared::Stack<S>,
//             >
//             + Clone,
//     {
//         Layer { endpoint_layer }
//     }
//
//     pub trait WithRoute {
//         type Output;
//
//         fn with_route(&self, route: Route) -> Self::Output;
//     }
//
//     #[derive(Clone, Debug)]
//     pub struct Layer<E = ()> {
//         endpoint_layer: E,
//     }
//
//     #[derive(Clone, Debug)]
//     pub struct Stack<D, E = ()> {
//         discover: D,
//         endpoint_layer: E,
//     }
//
//     pub struct PerEndpoint<D, E = ()> {
//         discover: D,
//         route: Route,
//         endpoint_layer: E,
//     }
//
//     #[derive(Debug)]
//     pub enum Error<D, E> {
//         Discover(D),
//         Endpoint(E),
//     }
//
//     impl<D, E> svc::Layer<Route, Route, D> for Layer<E>
//     where
//         D: svc::Stack<Route>,
//         D::Value: Discover,
//         <D::Value as Discover>::Service: HasEndpoint + Clone,
//         <<D::Value as Discover>::Service as HasEndpoint>::Endpoint: WithRoute + Clone,
//         E: svc::Layer<
//                 <<<D::Value as Discover>::Service as HasEndpoint>::Endpoint as WithRoute>::Output,
//                 <<<D::Value as Discover>::Service as HasEndpoint>::Endpoint as WithRoute>::Output,
//                 svc::shared::Stack<<D::Value as Discover>::Service>,
//             >
//             + Clone,
//         E::Value: svc::Service + Clone,
//     {
//         type Value = <Stack<D, E> as svc::Stack<Route>>::Value;
//         type Error = <Stack<D, E> as svc::Stack<Route>>::Error;
//         type Stack = Stack<D, E>;
//
//         fn bind(&self, discover: D) -> Self::Stack {
//             Stack {
//                 discover,
//                 endpoint_layer: self.endpoint_layer.clone(),
//             }
//         }
//     }
//
//     impl<D, E> svc::Stack<Route> for Stack<D, E>
//     where
//         D: svc::Stack<Route>,
//         D::Value: Discover,
//         <D::Value as Discover>::Service: HasEndpoint + Clone,
//         <<D::Value as Discover>::Service as HasEndpoint>::Endpoint: WithRoute + Clone,
//         E: svc::Layer<
//                 <<<D::Value as Discover>::Service as HasEndpoint>::Endpoint as WithRoute>::Output,
//                 <<<D::Value as Discover>::Service as HasEndpoint>::Endpoint as WithRoute>::Output,
//                 svc::shared::Stack<<D::Value as Discover>::Service>,
//             >
//             + Clone,
//         E::Value: svc::Service + Clone,
//     {
//         type Value = PerEndpoint<D::Value, E>;
//         type Error = D::Error;
//
//         fn make(&self, route: &Route) -> Result<Self::Value, Self::Error> {
//             let discover = self.discover.make(&route)?;
//             Ok(PerEndpoint {
//                 discover,
//                 route: route.clone(),
//                 endpoint_layer: self.endpoint_layer.clone(),
//             })
//         }
//     }
//
//     impl<D, E> Discover for PerEndpoint<D, E>
//     where
//         D: Discover,
//         D::Service: HasEndpoint + Clone,
//         <D::Service as HasEndpoint>::Endpoint: WithRoute + Clone,
//         E: svc::Layer<
//                 <<D::Service as HasEndpoint>::Endpoint as WithRoute>::Output,
//                 <<D::Service as HasEndpoint>::Endpoint as WithRoute>::Output,
//                 svc::shared::Stack<D::Service>,
//             >
//             + Clone,
//         E::Value: svc::Service + Clone,
//     {
//         type Key = D::Key;
//         type Request = <E::Value as svc::Service>::Request;
//         type Response = <E::Value as svc::Service>::Response;
//         type Error = <E::Value as svc::Service>::Error;
//         type Service = E::Value;
//         type DiscoverError = Error<D::DiscoverError, E::Error>;
//
//         fn poll(&mut self) -> Poll<Change<Self::Key, Self::Service>, Self::DiscoverError> {
//             match try_ready!(self.discover.poll().map_err(Error::Discover)) {
//                 Change::Insert(key, svc) => {
//                     let endpoint = svc.endpoint().with_route(self.route.clone());
//                     let svc = self
//                         .endpoint_layer
//                         .bind(svc::shared::stack(svc))
//                         .make(&endpoint)
//                         .map_err(Error::Endpoint)?;
//                     Ok(Change::Insert(key, svc).into())
//                 }
//                 Change::Remove(key) => Ok(Change::Remove(key).into()),
//             }
//         }
//     }
//
//     // === impl Error ===
//
//     impl<D: fmt::Display, E: fmt::Display> fmt::Display for Error<D, E> {
//         fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
//             match self {
//                 Error::Discover(d) => fmt::Display::fmt(d, f),
//                 Error::Endpoint(e) => fmt::Display::fmt(e, f),
//             }
//         }
//     }
//
//     impl<D: error::Error, E: error::Error> error::Error for Error<D, E> {
//         fn cause(&self) -> Option<&error::Error> {
//             match self {
//                 Error::Discover(d) => error::Error::cause(d),
//                 Error::Endpoint(e) => error::Error::cause(e),
//             }
//         }
//     }
// }

pub mod shared_discover {
    use futures::{sync::mpsc, Async, Future, Poll, Stream};
    use indexmap::IndexMap;
    use std::collections::VecDeque;
    use std::{error, fmt};

    use svc;

    use super::tower_discover::{Change, Discover};

    pub(super) fn new<D>(discover: D) -> (Stack<D>, Background<D>)
    where
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
    {
        let (notify_tx, notify_rx) = mpsc::unbounded();
        let stack = Stack { notify_tx };
        let bg = Background {
            discover,
            notify_rx: Some(notify_rx),
            notify_txs: VecDeque::new(),
            cache: IndexMap::new(),
        };
        (stack, bg)
    }

    pub struct Stack<D: Discover> {
        notify_tx: mpsc::UnboundedSender<Notify<D>>,
    }

    pub struct SharedDiscover<D: Discover> {
        rx: mpsc::UnboundedReceiver<Change<D::Key, D::Service>>,
    }

    pub struct Background<D: Discover> {
        discover: D,
        notify_rx: Option<mpsc::UnboundedReceiver<Notify<D>>>,
        notify_txs: VecDeque<Notify<D>>,
        cache: IndexMap<D::Key, D::Service>,
    }

    struct Notify<D: Discover> {
        tx: mpsc::UnboundedSender<Change<D::Key, D::Service>>,
    }

    #[derive(Copy, Clone, Debug)]
    pub struct LostBackground;

    // === impl Stack ===

    impl<T, D> svc::Stack<T> for Stack<D>
    where
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
    {
        type Value = SharedDiscover<D>;
        type Error = super::Error;

        fn make(&self, _: &T) -> Result<Self::Value, Self::Error> {
            let (tx, rx) = mpsc::unbounded();
            let _ = self.notify_tx.unbounded_send(Notify { tx });

            Ok(SharedDiscover { rx })
        }
    }

    impl<D: Discover> Clone for Stack<D> {
        fn clone(&self) -> Self {
            Self {
                notify_tx: self.notify_tx.clone(),
            }
        }
    }

    impl<D: Discover> fmt::Debug for Stack<D> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.debug_struct("profiles::Stack").finish()
        }
    }

    // === impl SharedDiscovery ===

    impl<D: Discover> Discover for SharedDiscover<D>
    where
        D: Discover,
        D::Service: Clone,
    {
        type Key = D::Key;
        type Request = D::Request;
        type Response = D::Response;
        type Error = D::Error;
        type Service = D::Service;
        type DiscoverError = LostBackground;

        fn poll(&mut self) -> Poll<Change<Self::Key, Self::Service>, Self::DiscoverError> {
            match self.rx.poll() {
                Ok(Async::NotReady) => Ok(Async::NotReady),
                Ok(Async::Ready(Some(c))) => {
                    debug!("SharedDiscover: ready");
                    Ok(Async::Ready(c))
                }
                Err(_) | Ok(Async::Ready(None)) => Err(LostBackground)
            }
        }
    }

    // === impl Background ===

    impl<D> Background<D>
    where
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
    {

        fn poll_notify_rx(&mut self) {
            loop {
                match self
                    .notify_rx
                    .as_mut()
                    .map(|ref mut notify_rx| notify_rx.poll())
                {
                    Some(Ok(Async::NotReady)) => return,
                    None | Some(Err(_)) | Some(Ok(Async::Ready(None))) => {
                        debug!("Background: no more notifiers");
                        self.notify_rx = None;
                        return;
                    }
                    Some(Ok(Async::Ready(Some(tx)))) => {
                        debug!("Background: new notifier");
                        if self.update_from_cache(&tx).is_ok() {
                            self.notify_txs.push_back(tx);
                        }
                    }
                }
            }
        }

        fn update_from_cache(&self, tx: &Notify<D>) -> Result<(), ()> {
            if !self.cache.is_empty() {
                debug!("Background: notifying from cache");
            }
            for (key, svc) in self.cache.iter() {
                tx.tx
                    .unbounded_send(Change::Insert(key.clone(), svc.clone()))
                    .map_err(|_| {})?;
            }

            Ok(())
        }

        fn notify_notify_txs(&mut self, change: &Change<D::Key, D::Service>) {
            for _ in 0..self.notify_txs.len() {
                let tx = self.notify_txs.pop_front().unwrap();
                let c = match change {
                    Change::Insert(ref k, ref s) => Change::Insert(k.clone(), s.clone()),
                    Change::Remove(ref k) => Change::Remove(k.clone()),
                };
                if tx.tx.unbounded_send(c).is_ok() {
                    self.notify_txs.push_back(tx);
                }
            }
        }
    }

    impl<D> Future for Background<D>
    where
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
    {
        type Item = ();
        type Error = D::DiscoverError;

        fn poll(&mut self) -> Poll<(), Self::Error> {
            loop {
                self.poll_notify_rx();

                if self.notify_rx.is_none() && self.notify_txs.is_empty() {
                    debug!("Backgroud: complete");
                    return Ok(Async::Ready(()));
                }

                debug!("polling discover");
                let change = try_ready!(self.discover.poll());
                self.notify_notify_txs(&change);
                match change {
                    Change::Insert(key, svc) => {
                        debug!("Background: adding to cache");
                        self.cache.insert(key, svc);
                    }
                    Change::Remove(key) => {
                        debug!("Background: removing from cache");
                        self.cache.remove(&key);
                    }
                }
            }
        }
    }

    // === impl Error ===

    impl fmt::Display for LostBackground {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "Lost background discovery task")
        }
    }

    impl error::Error for LostBackground {}
}
