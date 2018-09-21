use futures::{Future, Poll};
use h2;
use http;
use http::header::CONTENT_LENGTH;
use std::marker::PhantomData;
use std::time::Duration;
use std::{error, fmt};

use svc;

extern crate linkerd2_router;

use self::linkerd2_router::Error;
pub use self::linkerd2_router::{Recognize, Router};

#[derive(Clone, Debug)]
pub struct Layer<Req, Rec>
where
    Rec: Recognize<Req>,
{
    recognize: Rec,
    capacity: usize,
    max_idle_age: Duration,
    _p: PhantomData<fn() -> Req>,
}

pub struct Make<Req, Rec, Mk>
where
    Rec: Recognize<Req>,
    Mk: svc::Make<Rec::Target>,
    Mk::Value: svc::Service<Request = Req>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Display,
{
    router: Router<Req, Rec, Mk>,
}

pub struct Service<Req, Rec, Mk>
where
    Rec: Recognize<Req>,
    Mk: svc::Make<Rec::Target>,
    Mk::Value: svc::Service<Request = Req>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Display,
{
    inner: Router<Req, Rec, Mk>,
}

/// Catches errors from the inner future and maps them to 500 responses.
pub struct ResponseFuture<F> {
    inner: F,
}

// ===== impl Layer =====

impl<Req, Rec> Layer<Req, Rec>
where
    Rec: Recognize<Req> + Send + Sync + 'static,
    Req: Send + 'static,
{
    pub fn new(recognize: Rec, capacity: usize, max_idle_age: Duration) -> Self {
        Self {
            recognize,
            capacity,
            max_idle_age,
            _p: PhantomData,
        }
    }
}

impl<T, Req, Rec, Mk, B> svc::Layer<T, Rec::Target, Mk>
    for Layer<Req, Rec>
where
    Rec: Recognize<Req> + Clone + Send + Sync + 'static,
    Mk: svc::Make<Rec::Target> + Send + Sync + 'static,
    Mk::Value: svc::Service<Request = Req, Response = http::Response<B>>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Display,
    B: Default + Send + 'static,
    Make<Req, Rec, Mk>: svc::Make<T>,
{
    type Value = <Make<Req, Rec, Mk> as svc::Make<T>>::Value;
    type Error = <Make<Req, Rec, Mk> as svc::Make<T>>::Error;
    type Make = Make<Req, Rec, Mk>;

    fn bind(&self, next: Mk) -> Self::Make {
        let router = Router::new(
            self.recognize.clone(),
            next,
            self.capacity,
            self.max_idle_age,
        );
        Make { router }
    }
}

// ===== impl Make =====

impl<Req, Rec, Mk> Clone for Make<Req, Rec, Mk>
where
    Rec: Recognize<Req>,
    Mk: svc::Make<Rec::Target>,
    Mk::Value: svc::Service<Request = Req>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Display,
{
    fn clone(&self) -> Self {
        Self {
            router: self.router.clone(),
        }
    }
}

impl<T, Req, Rec, Mk, B> svc::Make<T> for Make<Req, Rec, Mk>
where
    Rec: Recognize<Req> + Send + Sync + 'static,
    Mk: svc::Make<Rec::Target> + Send + Sync + 'static,
    Mk::Value: svc::Service<Request = Req, Response = http::Response<B>>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Display,
    B: Default + Send + 'static,
{
    type Value = Service<Req, Rec, Mk>;
    type Error = ();

    fn make(&self, _: &T) -> Result<Self::Value, Self::Error> {
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

impl<Req, Rec, Mk, B> svc::Service for Service<Req, Rec, Mk>
where
    Rec: Recognize<Req> + Send + Sync + 'static,
    Mk: svc::Make<Rec::Target> + Send + Sync + 'static,
    Mk::Value: svc::Service<Request = Req, Response = http::Response<B>>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Display,
    B: Default + Send + 'static,
{
    type Request = <Router<Req, Rec, Mk> as svc::Service>::Request;
    type Response = <Router<Req, Rec, Mk> as svc::Service>::Response;
    type Error = h2::Error;
    type Future = ResponseFuture<<Router<Req, Rec, Mk> as svc::Service>::Future>;

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
