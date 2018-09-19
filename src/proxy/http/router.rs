use futures::{Future, Poll};
use h2;
use http;
use http::header::CONTENT_LENGTH;
use std::{fmt, error};
use std::sync::Arc;
use std::time::Duration;

use ctx;
use svc;

extern crate linkerd2_router;

use self::linkerd2_router::Error;
pub use self::linkerd2_router::{Recognize, Router};

pub struct Layer<R>
where
    R: Recognize,
{
    recognize: R,
    capacity: usize,
    max_idle_age: Duration,
}

pub struct Make<R, M>
where
    R: Recognize,
    M: svc::Make<R::Target>,
    M::Output: svc::Service<Request = R::Request>,
    <M::Output as svc::Service>::Error: error::Error,
    M::Error: fmt::Display,
{
    router: Router<R, M>,
}

pub struct Service<R, M>
where
    R: Recognize,
    M: svc::Make<R::Target>,
    M::Output: svc::Service<Request = R::Request>,
    <M::Output as svc::Service>::Error: error::Error,
    M::Error: fmt::Display,
{
    inner: Router<R, M>,
}

/// Catches errors from the inner future and maps them to 500 responses.
pub struct ResponseFuture<F> {
    inner: F,
}

// ===== impl Layer =====

impl<R, A> Layer<R>
where
    R: Recognize<Request = http::Request<A>> + Send + Sync + 'static,
    A: Send + 'static,
{
    pub fn new(recognize: R, capacity: usize, max_idle_age: Duration) -> Self {
        Self { recognize, capacity, max_idle_age }
    }
}

impl<R, M, A, B> svc::Layer<M> for Layer<R>
where
    R: Recognize<Request = http::Request<A>> + Clone + Send + Sync + 'static,
    M: svc::Make<R::Target> + Send + Sync + 'static,
    M::Output: svc::Service<Request = R::Request, Response = http::Response<B>>,
    <M::Output as svc::Service>::Error: error::Error,
    M::Error: fmt::Display,
    A: Send + 'static,
    B: Default + Send + 'static,
{
    type Bound = Make<R, M>;

    fn bind(&self, next: M) -> Self::Bound {
        let router = Router::new(
            self.recognize.clone(),
            next,
            self.capacity,
            self.max_idle_age
        );
        Make { router }
    }
}

// ===== impl Make =====

impl<R, M> Clone for Make<R, M>
where
    R: Recognize,
    M: svc::Make<R::Target>,
    M::Output: svc::Service<Request = R::Request>,
    <M::Output as svc::Service>::Error: error::Error,
    M::Error: fmt::Display,
{
    fn clone(&self) -> Self {
        Self {
            router: self.router.clone(),
        }
    }
}

impl<R, M, A, B> svc::Make<Arc<ctx::transport::Server>> for Make<R, M>
where
    R: Recognize<Request = http::Request<A>> + Send + Sync + 'static,
    M: svc::Make<R::Target> + Send + Sync + 'static,
    M::Output: svc::Service<Request = R::Request, Response = http::Response<B>>,
    <M::Output as svc::Service>::Error: error::Error,
    M::Error: fmt::Display,
    A: Send + 'static,
    B: Default + Send + 'static,
{
    type Output = Service<R, M>;
    type Error = ();

    fn make(&self, _: &Arc<ctx::transport::Server>) -> Result<Self::Output, Self::Error> {
        let inner = self.router.clone();
        Ok(Service { inner })
    }
}

fn route_err_to_5xx<E, F>(e: Error<E, F>) -> http::StatusCode
where
    E: fmt::Display,
    F: fmt::Display,
{
    match e {
        Error::Route(r) => {
            error!("router error: {}", r);
            http::StatusCode::INTERNAL_SERVER_ERROR
        }
        Error::Inner(i) => {
            error!("service error: {}", i);
            http::StatusCode::INTERNAL_SERVER_ERROR
        }
        Error::NotRecognized => {
            error!("could not recognize request");
            http::StatusCode::INTERNAL_SERVER_ERROR
        }
        Error::NoCapacity(capacity) => {
            // TODO For H2 streams, we should probably signal a protocol-level
            // capacity change.
            error!("router at capacity ({})", capacity);
            http::StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

// ===== impl Service =====

impl<R, M, B> svc::Service for Service<R, M>
where
    R: Recognize + Send + Sync + 'static,
    M: svc::Make<R::Target> + Send + Sync + 'static,
    M::Output: svc::Service<Request = R::Request, Response = http::Response<B>>,
    <M::Output as svc::Service>::Error: error::Error,
    M::Error: fmt::Display,
    B: Default + Send + 'static,
{
    type Request = <Router<R, M> as svc::Service>::Request;
    type Response = <Router<R, M> as svc::Service>::Response;
    type Error = h2::Error;
    type Future = ResponseFuture<<Router<R, M> as svc::Service>::Future>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready().map_err(|e| {
            error!("router failed to become ready: {}", e);
            h2::Reason::INTERNAL_ERROR.into()
        })
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        let inner = self.inner.call(request);
        ResponseFuture { inner }
    }
}

// ===== impl ResponseFuture =====

impl<F, E, G, B> Future for ResponseFuture<F>
where
    F: Future<Item = http::Response<B>, Error = Error<E, G>>,
    E: error::Error,
    G: fmt::Display,
    B: Default,
{
    type Item = F::Item;
    type Error = h2::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.inner.poll().or_else(|e| {
            let response = http::Response::builder()
                .status(route_err_to_5xx(e))
                .header(CONTENT_LENGTH, "0")
                .body(B::default())
                .unwrap();

            Ok(response.into())
        })
    }
}
