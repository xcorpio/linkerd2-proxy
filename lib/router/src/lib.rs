extern crate futures;
extern crate indexmap;
extern crate linkerd2_stack as stack;
extern crate tower_service as svc;

use futures::{Future, Poll};

use std::{error, fmt, mem};
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod cache;

use self::cache::Cache;

/// Routes requests based on a configurable `Key`.
pub struct Router<R, M>
where
    R: Recognize,
    M: stack::Make<R::Target>,
    M::Output: svc::Service<Request = R::Request>,
{
    inner: Arc<Inner<R, M>>,
}

/// Provides a strategy for routing a Request to a Service.
///
/// Implementors must provide a `Key` type that identifies each unique route. The
/// `recognize()` method is used to determine the target for a given request. This target is
/// used to look up a route in a cache (i.e. in `Router`), or can be passed to
/// `bind_service` to instantiate the identified route.
pub trait Recognize {
    /// Requests handled by the discovered services
    type Request;

    /// Identifies a Route.
    type Target: Clone + Eq + Hash;

    /// Determines the target for a route to handle the given request.
    fn recognize(&self, req: &Self::Request) -> Option<Self::Target>;
}

#[derive(Debug, PartialEq)]
pub enum Error<T, U> {
    Inner(T),
    Route(U),
    NoCapacity(usize),
    NotRecognized,
}

pub struct ResponseFuture<F, E>
where
    F: Future,
{
    state: State<F, E>,
}

struct Inner<R, M>
where
    R: Recognize,
    M: stack::Make<R::Target>,
    M::Output: svc::Service<Request = R::Request>,
{
    recognize: R,
    make: M,
    cache: Mutex<Cache<R::Target, M::Output>>,
}

enum State<F, E>
where
    F: Future,
{
    Inner(F),
    RouteError(E),
    NoCapacity(usize),
    NotRecognized,
    Invalid,
}

// ===== impl Router =====

impl<R, M> Router<R, M>
where
    R: Recognize,
    M: stack::Make<R::Target>,
    M::Output: svc::Service<Request = R::Request>,
{
    pub fn new(recognize: R, make: M, capacity: usize, max_idle_age: Duration) -> Self {
        Router {
            inner: Arc::new(Inner {
                recognize,
                make,
                cache: Mutex::new(Cache::new(capacity, max_idle_age)),
            }),
        }
    }
}

impl<R, M> svc::Service for Router<R, M>
where
    R: Recognize,
    M: stack::Make<R::Target>,
    M::Output: svc::Service<Request = R::Request>,
{
    type Request = <M::Output as svc::Service>::Request;
    type Response = <M::Output as svc::Service>::Response;
    type Error = Error<<M::Output as svc::Service>::Error, M::Error>;
    type Future = ResponseFuture<<M::Output as svc::Service>::Future, M::Error>;

    /// Always ready to serve.
    ///
    /// Graceful backpressure is **not** supported at this level, since each request may
    /// be routed to different resources. Instead, requests should be issued and each
    /// route should support a queue of requests.
    ///
    /// TODO Attempt to free capacity in the router.
    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        Ok(().into())
    }

    /// Routes the request through an underlying service.
    ///
    /// The response fails when the request cannot be routed.
    fn call(&mut self, request: Self::Request) -> Self::Future {
        let target = match self.inner.recognize.recognize(&request) {
            Some(target) => target,
            None => return ResponseFuture::not_recognized(),
        };

        let cache = &mut *self.inner.cache.lock().expect("lock router cache");

        // First, try to load a cached route for `target`.
        if let Some(mut service) = cache.access(&target) {
            return ResponseFuture::new(service.call(request));
        }

        // Since there wasn't a cached route, ensure that there is capacity for a
        // new one.
        let reserve = match cache.reserve() {
            Ok(r) => r,
            Err(cache::CapacityExhausted { capacity }) => {
                return ResponseFuture::no_capacity(capacity);
            }
        };

        // Bind a new route, send the request on the route, and cache the route.
        let mut service = match self.inner.make.make(&target) {
            Ok(svc) => svc,
            Err(e) => return ResponseFuture { state: State::RouteError(e) },
        };

        let response = service.call(request);
        reserve.store(target, service);

        ResponseFuture::new(response)
    }
}

impl<R, M> Clone for Router<R, M>
where
    R: Recognize,
    M: stack::Make<R::Target>,
    M::Output: svc::Service<Request = R::Request>,
{
    fn clone(&self) -> Self {
        Router { inner: self.inner.clone() }
    }
}

// ===== impl ResponseFuture =====

impl<F, E> ResponseFuture<F, E>
where
    F: Future,
{
    fn new(inner: F) -> Self {
        ResponseFuture { state: State::Inner(inner) }
    }

    fn not_recognized() -> Self {
        ResponseFuture { state: State::NotRecognized }
    }

    fn no_capacity(capacity: usize) -> Self {
        ResponseFuture { state: State::NoCapacity(capacity) }
    }
}

impl<F, E> Future for ResponseFuture<F, E>
where
    F: Future,
{
    type Item = F::Item;
    type Error = Error<F::Error, E>;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        use self::State::*;

        match self.state {
            Inner(ref mut fut) => fut.poll().map_err(Error::Inner),
            RouteError(..) => {
                match mem::replace(&mut self.state, Invalid) {
                    RouteError(e) => Err(Error::Route(e)),
                    _ => unreachable!(),
                }
            }
            NotRecognized => Err(Error::NotRecognized),
            NoCapacity(capacity) => Err(Error::NoCapacity(capacity)),
            Invalid => panic!("response future polled after ready"),
        }
    }
}

// ===== impl Error =====

impl<T, U> fmt::Display for Error<T, U>
where
    T: fmt::Display,
    U: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::Inner(ref why) => fmt::Display::fmt(why, f),
            Error::Route(ref why) =>
                write!(f, "route recognition failed: {}", why),
            Error::NotRecognized => f.pad("route not recognized"),
            Error::NoCapacity(capacity) => write!(f, "router capacity reached ({})", capacity),
        }
    }
}

impl<T, U> error::Error for Error<T, U>
where
    T: error::Error,
    U: error::Error,
{
    fn cause(&self) -> Option<&error::Error> {
        match *self {
            Error::Inner(ref why) => Some(why),
            Error::Route(ref why) => Some(why),
            _ => None,
        }
    }

    fn description(&self) -> &str {
        match *self {
            Error::Inner(_) => "inner service error",
            Error::Route(_) => "route recognition failed",
            Error::NoCapacity(_) => "router capacity reached",
            Error::NotRecognized => "route not recognized",
        }
    }
}

#[cfg(test)]
mod test_util {
    use futures::{Poll, future};
    use stack::Make;
    use svc::Service;

    pub struct Recognize;

    #[derive(Debug)]
    pub struct MultiplyAndAssign(usize);

    #[derive(Debug)]
    pub enum Request {
        NotRecognized,
        Recognized(usize),
    }

    // ===== impl Recognize =====

    impl super::Recognize for Recognize {
        type Request = Request;
        type Target = usize;

        fn recognize(&self, req: &Self::Request) -> Option<Self::Target> {
            match *req {
                Request::NotRecognized => None,
                Request::Recognized(n) => Some(n),
            }
        }
    }

    impl Make<usize> for Recognize {
        type Output = MultiplyAndAssign;
        type Error = ();

        fn make(&self, _: &Self::Key) -> Result<Self::Output, Self::Error> {
            Ok(MultiplyAndAssign(1))
        }
    }

    // ===== impl MultiplyAndAssign =====

    impl Default for MultiplyAndAssign {
        fn default() -> Self {
            MultiplyAndAssign(1)
        }
    }

    impl Service for MultiplyAndAssign {
        type Request = Request;
        type Response = usize;
        type Error = ();
        type Future = future::FutureResult<usize, ()>;

        fn poll_ready(&mut self) -> Poll<(), ()> {
            unimplemented!()
        }

        fn call(&mut self, req: Self::Request) -> Self::Future {
            let n = match req {
                Request::NotRecognized => unreachable!(),
                Request::Recognized(n) => n,
            };
            self.0 *= n;
            future::ok(self.0)
        }
    }

    impl From<usize> for Request {
        fn from(n: usize) -> Request {
            Request::Recognized(n)
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::Future;
    use std::time::Duration;
    use test_util::*;
    use tower_service::Service;
    use super::{Error, Router};

    impl Router<Recognize> {
        fn call_ok(&mut self, req: Request) -> usize {
            self.call(req).wait().expect("should route")
        }

        fn call_err(&mut self, req: Request) -> super::Error<(), ()> {
            self.call(req).wait().expect_err("should not route")
        }
    }

    #[test]
    fn invalid() {
        let mut router = Router::new(Recognize, 1, Duration::from_secs(0));

        let rsp = router.call_err(Request::NotRecognized);
        assert_eq!(rsp, Error::NotRecognized);
    }

    #[test]
    fn cache_limited_by_capacity() {
        let mut router = Router::new(Recognize, 1, Duration::from_secs(1));

        let rsp = router.call_ok(2.into());
        assert_eq!(rsp, 2);

        let rsp = router.call_err(3.into());
        assert_eq!(rsp, Error::NoCapacity(1));
    }

    #[test]
    fn services_cached() {
        let mut router = Router::new(Recognize, 1, Duration::from_secs(0));

        let rsp = router.call_ok(2.into());
        assert_eq!(rsp, 2);

        let rsp = router.call_ok(2.into());
        assert_eq!(rsp, 4);
    }
}
