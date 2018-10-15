#![allow(dead_code)]

extern crate void;

pub use self::void::Void;
use futures::{Async, Poll, Stream};
use http;
use indexmap::IndexMap;
use regex;

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
pub struct Layer<G> {
    get_routes: G,
    default_route: Route,
}

#[derive(Clone, Debug)]
pub struct Stack<G, S> {
    get_routes: G,
    inner: S,
    default_route: Route,
}

#[derive(Debug)]
pub struct Service<G, S: svc::Stack<Route>> {
    route_stream: G,
    stack: S,
    routes: Vec<(RequestMatch, S::Value)>,
    default_route: S::Value,
}

impl<G> Layer<G>
where
    G: GetRoutes + Clone,
{
    pub fn new(get_routes: G) -> Self {
        let default_route = Route::default();
        Layer { get_routes, default_route, }
    }
}

impl<T, G, S> svc::Layer<T, Route, S> for Layer<G>
where
    T: CanGetDestination,
    G: GetRoutes + Clone,
    S: svc::Stack<Route> + Clone,
{
    type Value = <Stack<G, S> as svc::Stack<T>>::Value;
    type Error = <Stack<G, S> as svc::Stack<T>>::Error;
    type Stack = Stack<G, S>;

    fn bind(&self, inner: S) -> Self::Stack {
        Stack {
            get_routes: self.get_routes.clone(),
            default_route: self.default_route.clone(),
            inner,
        }
    }
}

impl<T, G, S> svc::Stack<T> for Stack<G, S>
where
    T: CanGetDestination,
    G: GetRoutes,
    S: svc::Stack<Route> + Clone,
{
    type Value = Service<G::Stream, S>;
    type Error = S::Error;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let default_route = self.inner.make(&self.default_route)?;
        let dst = target.get_destination();
        let route_stream = self.get_routes.get_routes(dst);
        Ok(Service {
            route_stream,
            default_route,
            stack: self.inner.clone(),
            routes: Vec::new(),
        })
    }
}

impl<G, S> Service<G, S>
where
    G: Stream<Item = Routes, Error = Void>,
    S: svc::Stack<Route>,
    S::Value: svc::Service,
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

impl<G, S, B> svc::Service for Service<G, S>
where
    G: Stream<Item = Routes, Error = Void>,
    S: svc::Stack<Route>,
    S::Value: svc::Service<Request = http::Request<B>>,
{
    type Request = <S::Value as svc::Service>::Request;
    type Response = <S::Value as svc::Service>::Response;
    type Error = <S::Value as svc::Service>::Error;
    type Future = <S::Value as svc::Service>::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
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
