#![allow(dead_code)]

extern crate linkerd2_shared_discover as shared;
extern crate tower_discover;
extern crate void;

use self::tower_discover::Discover;
pub use self::void::Void;
use futures::{Async, Future, Poll, Stream};
use http;
use indexmap::IndexMap;
use regex;
use std::fmt;

use svc;

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
    Path(regex::Regex),
    Method(http::Method),
}

// TODO provide a `Classify` implementation derived from api::destination::ResponseClass,
#[derive(Clone, Debug, Default)]
pub struct Route {
    pub labels: IndexMap<String, String>,
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

pub enum StackError<D, R> {
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
    discover_bg: Option<shared::Background<D>>,
    route_stream: G,
    routes: Vec<(RequestMatch, R::Value)>,
    default_route: R::Value,
}

pub struct ShareDiscoverStack<D: Discover>(shared::Share<D>);

impl<G> Layer<G, ()>
where
    G: GetRoutes + Clone,
{
    pub fn new(get_routes: G) -> Self {
        Layer {
            get_routes,
            route_layer: (),
            default_route: Route::default(),
        }
    }

    pub fn with_route_layer<D, R>(self, route_layer: R) -> Layer<G, R>
    where
        D: Discover,
        D::Key: Clone,
        D::Service: Clone,
        R: svc::Layer<Route, Route, ShareDiscoverStack<D>> + Clone,
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
    R: svc::Layer<Route, Route, ShareDiscoverStack<D::Value>> + Clone,
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
    R: svc::Layer<Route, Route, ShareDiscoverStack<D::Value>> + Clone,
    R::Value: svc::Service,
{
    type Value = Service<D::Value, G::Stream, R::Stack>;
    type Error = StackError<D::Error, R::Error>;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let (stack, discover_bg) = {
            let discover = self.discover.make(&target).map_err(StackError::Discover)?;
            let (share, bg) = shared::new(discover);
            let stack = self.route_layer.bind(ShareDiscoverStack(share));
            (stack, bg)
        };

        let route_stream = self.get_routes.get_routes(target.get_destination());

        let default_route = stack.make(&self.default_route)
            .map_err(StackError::Route)?;

        Ok(Service {
            stack,
            route_stream,
            default_route,
            discover_bg: Some(discover_bg),
            routes: Vec::new(),
        })
    }
}

impl<T, D> svc::Stack<T> for ShareDiscoverStack<D>
where
    D: Discover,
    D::Key: Clone,
    D::Service: Clone,
{
    type Value = shared::SharedDiscover<D>;
    type Error = Void;

    fn make(&self, _: &T) -> Result<Self::Value, Self::Error> {
        Ok(self.0.share())
    }
}

impl<D: Discover> Clone for ShareDiscoverStack<D> {
    fn clone(&self) -> Self {
        ShareDiscoverStack(self.0.clone())
    }
}

impl<D: Discover> fmt::Debug for ShareDiscoverStack<D> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ShareDiscoverStack").finish()
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
