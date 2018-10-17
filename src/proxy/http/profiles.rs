#![allow(dead_code)]

extern crate tower_discover;
extern crate void;

use self::tower_discover::Discover;
pub use self::void::Void;
use futures::Stream;
use http;
use indexmap::IndexMap;
use regex::Regex;

pub trait CanGetDestination {
    fn get_destination(&self) -> String;
}

pub type Routes = Vec<(RequestMatch, Route)>;

pub trait GetRoutes {
    type Stream: Stream<Item = Routes, Error = Void>;

    fn get_routes(&self, dst: String) -> Self::Stream;
}

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
    pub labels: IndexMap<String, String>,
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

pub mod router {
    use futures::{Async, Future, Poll, Stream};
    use http;
    use std::fmt;

    use svc;

    use super::*;

    pub fn layer<G: GetRoutes + Clone>(get_routes: G) -> Layer<G, ()> {
        Layer {
            get_routes,
            route_layer: (),
            default_route: Route::default(),
        }
    }

    #[derive(Clone, Debug)]
    pub struct Layer<G, R = ()> {
        get_routes: G,
        route_layer: R,
        default_route: Route,
    }

    #[derive(Clone, Debug)]
    pub struct Stack<D, G, R = ()> {
        discover: D,
        get_routes: G,
        route_layer: R,
        default_route: Route,
    }

    pub enum Error<D, R> {
        Discover(D),
        Route(R),
    }

    pub struct Service<D, G, R>
    where
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
        R: svc::Stack<Route>,
        R::Value: svc::Service,
    {
        stack: R,
        discover_bg: Option<shared_discover::Background<D>>,
        route_stream: G,
        routes: Vec<(RequestMatch, R::Value)>,
        default_route: R::Value,
    }

    impl<G> Layer<G, ()>
    where
        G: GetRoutes + Clone,
    {
        pub fn with_route_layer<D, R>(self, route_layer: R) -> Layer<G, R>
        where
            D: Discover,
            D::Key: Clone,
            D::Service: Clone,
            R: svc::Layer<Route, Route, shared_discover::Stack<D>> + Clone,
            R::Value: svc::Service,
        {
            Layer {
                route_layer,
                get_routes: self.get_routes,
                default_route: self.default_route,
            }
        }
    }

    impl<T, D, G, R> svc::Layer<T, T, D> for Layer<G, R>
    where
        T: CanGetDestination,
        D: svc::Stack<T>,
        D::Value: Discover,
        <D::Value as Discover>::Key: Clone,
        <D::Value as Discover>::Service: Clone,
        G: GetRoutes + Clone,
        R: svc::Layer<Route, Route, shared_discover::Stack<D::Value>> + Clone,
        R::Value: svc::Service,
    {
        type Value = <Stack<D, G, R> as svc::Stack<T>>::Value;
        type Error = <Stack<D, G, R> as svc::Stack<T>>::Error;
        type Stack = Stack<D, G, R>;

        fn bind(&self, discover: D) -> Self::Stack {
            Stack {
                discover,
                get_routes: self.get_routes.clone(),
                route_layer: self.route_layer.clone(),
                default_route: self.default_route.clone(),
            }
        }
    }

    impl<T, D, G, R> svc::Stack<T> for Stack<D, G, R>
    where
        T: CanGetDestination,
        D: svc::Stack<T>,
        D::Value: Discover,
        <D::Value as Discover>::Key: Clone,
        <D::Value as Discover>::Service: Clone,
        G: GetRoutes,
        R: svc::Layer<Route, Route, shared_discover::Stack<D::Value>> + Clone,
        R::Value: svc::Service,
    {
        type Value = Service<D::Value, G::Stream, R::Stack>;
        type Error = Error<D::Error, R::Error>;

        fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
            let (stack, discover_bg) = {
                let discover = self.discover.make(&target).map_err(Error::Discover)?;
                let (share, bg) = shared_discover::new(discover);
                let stack = self.route_layer.bind(share);
                (stack, bg)
            };

            let default_route = stack.make(&self.default_route).map_err(Error::Route)?;

            let route_stream = self.get_routes.get_routes(target.get_destination());

            Ok(Service {
                stack,
                route_stream,
                default_route,
                discover_bg: Some(discover_bg),
                routes: Vec::new(),
            })
        }
    }

    impl<D, G, R> Service<D, G, R>
    where
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
        R: svc::Stack<Route> + Clone,
        R::Value: svc::Service,
    {
        fn update_routes(&mut self, mut routes: Routes) {
            self.routes = Vec::with_capacity(routes.len());
            for (req_match, route) in routes.drain(..) {
                match self.stack.make(&route) {
                    Ok(svc) => self.routes.push((req_match, svc)),
                    Err(_) => error!(
                        "failed to build service for route: req_match={:?}; route={:?}",
                        req_match, route
                    ),
                }
            }
        }
    }

    impl<D, G, R, B> svc::Service for Service<D, G, R>
    where
        G: Stream<Item = Routes, Error = Void>,
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
        D::DiscoverError: fmt::Debug,
        R: svc::Stack<Route> + Clone,
        R::Value: svc::Service<Request = http::Request<B>>,
    {
        type Request = <R::Value as svc::Service>::Request;
        type Response = <R::Value as svc::Service>::Response;
        type Error = <R::Value as svc::Service>::Error;
        type Future = <R::Value as svc::Service>::Future;

        fn poll_ready(&mut self) -> Poll<(), Self::Error> {
            match self.discover_bg.as_mut().map(|f| f.poll()) {
                Some(Ok(Async::NotReady)) | None => {}
                Some(Err(e)) => {
                    error!("discover background task failed: {:?}", e);
                    self.discover_bg = None;
                }
                Some(Ok(Async::Ready(()))) => {
                    debug!("discover background task finished");
                    self.discover_bg = None;
                }
            }

            while let Async::Ready(r) = self.route_stream.poll().unwrap() {
                let routes = r.expect("profile stream must be infinite");
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

pub mod per_endpoint {
    use futures::Poll;

    use svc::{self, Stack as _Stack};

    use super::tower_discover::{Change, Discover};

    pub fn layer<K, S, E>(endpoint_layer: E) -> Layer<E>
    where
        S: svc::Service + Clone,
        E: svc::Layer<K, K, svc::Shared<K, S>> + Clone,
    {
        Layer { endpoint_layer }
    }

    pub struct Layer<E = ()> {
        endpoint_layer: E,
    }

    pub struct Stack<D, E = ()> {
        discover: D,
        endpoint_layer: E,
    }

    pub struct PerEndpoint<D, E = ()> {
        discover: D,
        endpoint_layer: E,
    }

    pub enum Error<D, E> {
        Discover(D),
        Endpoint(E),
    }

    impl<T, D, E> svc::Layer<T, T, D> for Layer<E>
    where
        D: svc::Stack<T>,
        D::Value: Discover,
        <D::Value as Discover>::Service: Clone,
        E: svc::Layer<
                <D::Value as Discover>::Key,
                <D::Value as Discover>::Key,
                svc::Shared<<D::Value as Discover>::Key, <D::Value as Discover>::Service>,
            >
            + Clone,
        E::Value: svc::Service + Clone,
    {
        type Value = <Stack<D, E> as svc::Stack<T>>::Value;
        type Error = <Stack<D, E> as svc::Stack<T>>::Error;
        type Stack = Stack<D, E>;

        fn bind(&self, discover: D) -> Self::Stack {
            Stack {
                discover,
                endpoint_layer: self.endpoint_layer.clone(),
            }
        }
    }

    impl<T, D, E> svc::Stack<T> for Stack<D, E>
    where
        D: svc::Stack<T>,
        D::Value: Discover,
        <D::Value as Discover>::Service: Clone,
        E: svc::Layer<
                <D::Value as Discover>::Key,
                <D::Value as Discover>::Key,
                svc::Shared<<D::Value as Discover>::Key, <D::Value as Discover>::Service>,
            >
            + Clone,
        E::Value: svc::Service + Clone,
    {
        type Value = PerEndpoint<D::Value, E>;
        type Error = D::Error;

        fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
            let discover = self.discover.make(&target)?;
            Ok(PerEndpoint {
                discover,
                endpoint_layer: self.endpoint_layer.clone(),
            })
        }
    }

    impl<D, E> Discover for PerEndpoint<D, E>
    where
        D: Discover,
        D::Service: Clone,
        E: svc::Layer<D::Key, D::Key, svc::Shared<D::Key, D::Service>> + Clone,
        E::Value: svc::Service + Clone,
    {
        type Key = D::Key;
        type Request = <E::Value as svc::Service>::Request;
        type Response = <E::Value as svc::Service>::Response;
        type Error = <E::Value as svc::Service>::Error;
        type Service = E::Value;
        type DiscoverError = Error<D::DiscoverError, E::Error>;

        fn poll(&mut self) -> Poll<Change<Self::Key, Self::Service>, Self::DiscoverError> {
            match try_ready!(self.discover.poll().map_err(Error::Discover)) {
                Change::Insert(key, svc) => {
                    let svc = self
                        .endpoint_layer
                        .bind(svc::Shared::new(svc))
                        .make(&key)
                        .map_err(Error::Endpoint)?;
                    Ok(Change::Insert(key, svc).into())
                }
                Change::Remove(key) => Ok(Change::Remove(key).into()),
            }
        }
    }
}

pub mod shared_discover {
    use futures::{sync::mpsc, Async, Future, Poll, Stream};
    use indexmap::IndexMap;
    use std::collections::VecDeque;
    use std::fmt;

    use svc;

    use super::tower_discover::{Change, Discover};
    use super::void::Void;

    pub fn new<D>(discover: D) -> (Stack<D>, Background<D>)
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

    // === impl Stack ===

    impl<T, D> svc::Stack<T> for Stack<D>
    where
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
    {
        type Value = SharedDiscover<D>;
        type Error = Void;

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
        type DiscoverError = D::DiscoverError;

        fn poll(&mut self) -> Poll<Change<Self::Key, Self::Service>, Self::DiscoverError> {
            match self.rx.poll() {
                Ok(Async::Ready(Some(c))) => Ok(Async::Ready(c)),
                Ok(Async::Ready(None)) => Ok(Async::NotReady),
                Ok(Async::NotReady) => Ok(Async::NotReady),
                Err(_) => Ok(Async::NotReady),
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
        fn update_from_cache(&self, tx: &Notify<D>) -> Result<(), ()> {
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

        fn poll_notify_rx(&mut self) {
            loop {
                match self
                    .notify_rx
                    .as_mut()
                    .map(|ref mut notify_rx| notify_rx.poll())
                {
                    Some(Ok(Async::NotReady)) => return,
                    None | Some(Err(_)) | Some(Ok(Async::Ready(None))) => {
                        self.notify_rx = None;
                        return;
                    }
                    Some(Ok(Async::Ready(Some(tx)))) => {
                        if self.update_from_cache(&tx).is_ok() {
                            self.notify_txs.push_back(tx);
                        }
                    }
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
                    return Ok(Async::Ready(()));
                }

                let change = try_ready!(self.discover.poll());
                self.notify_notify_txs(&change);
                match change {
                    Change::Insert(key, svc) => {
                        self.cache.insert(key, svc);
                    }
                    Change::Remove(key) => {
                        self.cache.remove(&key);
                    }
                }
            }
        }
    }
}
