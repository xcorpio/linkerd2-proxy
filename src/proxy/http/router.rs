use futures::{Future, Poll};
use h2;
use http;
use http::header::CONTENT_LENGTH;
use std::{error, fmt};
use std::marker::PhantomData;
use std::time::Duration;

use svc;

extern crate linkerd2_router;

use self::linkerd2_router::Error;
pub use self::linkerd2_router::{Recognize, Router};

#[derive(Debug)]
pub struct Config<Req, Rec>
where
    Rec: Recognize<Req>,
{
    recognize: Rec,
    capacity: usize,
    max_idle_age: Duration,
    _p: PhantomData<fn() -> Req>,
}

#[derive(Debug)]
pub struct Layer();

pub struct Make<T, Mk>
where
    Mk: svc::Make<T> + Clone,
{
    inner: Mk,
    _p: PhantomData<fn() -> T>,
}

pub struct Service<Req, Rec, Mk>
where
    Rec: Recognize<Req>,
    Mk: svc::Make<Rec::Target>,
    Mk::Value: svc::Service<Request = Req>,
{
    inner: Router<Req, Rec, Mk>,
}

/// Catches errors from the inner future and maps them to 500 responses.
pub struct ResponseFuture<F> {
    inner: F,
}

// === impl Config ===

impl<Req, Rec> Config<Req, Rec>
where
    Rec: Recognize<Req>,
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

impl<Req, Rec> Clone for Config<Req, Rec>
where
    Rec: Recognize<Req> + Clone,
{
    fn clone(&self) -> Self {
        Self::new(self.recognize.clone(), self.capacity, self.max_idle_age)
    }
}

impl<Req, Rec> fmt::Display for Config<Req, Rec>
where
    Rec: Recognize<Req> + Clone + fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.recognize.fmt(f)
    }
}


// === impl Layer ===

impl Layer {
    pub fn new() -> Self {
        Layer()
    }
}

impl<T, U, Mk, B> svc::Layer<T, U, Mk> for Layer
where
    Mk: svc::Make<U> + Clone + Send + Sync + 'static,
    Mk::Value: svc::Service<Response = http::Response<B>>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Debug,
    B: Default + Send + 'static,
    Make<U, Mk>: svc::Make<T>,
{
    type Value = <Make<U, Mk> as svc::Make<T>>::Value;
    type Error = <Make<U, Mk> as svc::Make<T>>::Error;
    type Make = Make<U, Mk>;

    fn bind(&self, inner: Mk) -> Self::Make {
        Make { inner, _p: PhantomData }
    }
}

// === impl Make ===

impl<T, Mk> Clone for Make<T, Mk>
where
    Mk: svc::Make<T> + Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _p: PhantomData
        }
    }
}

impl<Req, Rec, Mk, B> svc::Make<Config<Req, Rec>> for Make<Rec::Target, Mk>
where
    Rec: Recognize<Req> + Clone + Send + Sync + 'static,
    Mk: svc::Make<Rec::Target> + Clone + Send + Sync + 'static,
    Mk::Value: svc::Service<Request = Req, Response = http::Response<B>>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Debug,
    B: Default + Send + 'static,
{
    type Value = Service<Req, Rec, Mk>;
    type Error = ();

    fn make(&self, config: &Config<Req, Rec>) -> Result<Self::Value, Self::Error> {
        let inner = Router::new(
            config.recognize.clone(),
            self.inner.clone(),
            config.capacity,
            config.max_idle_age,
        );
        Ok(Service { inner })
    }
}

fn route_err_to_5xx<E, F>(e: Error<E, F>) -> http::StatusCode
where
    E: error::Error,
    F: fmt::Debug,
{
    match e {
        Error::Route(r) => {
            error!("router error: {:?}", r);
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

// === impl Service ===

impl<Req, Rec, Mk, B> svc::Service for Service<Req, Rec, Mk>
where
    Rec: Recognize<Req> + Send + Sync + 'static,
    Mk: svc::Make<Rec::Target> + Send + Sync + 'static,
    Mk::Value: svc::Service<Request = Req, Response = http::Response<B>>,
    <Mk::Value as svc::Service>::Error: error::Error,
    Mk::Error: fmt::Debug,
    B: Default + Send + 'static,
{
    type Request = <Router<Req, Rec, Mk> as svc::Service>::Request;
    type Response = <Router<Req, Rec, Mk> as svc::Service>::Response;
    type Error = h2::Error;
    type Future = ResponseFuture<<Router<Req, Rec, Mk> as svc::Service>::Future>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready().map_err(|e| {
            error!("router failed to become ready: {:?}", e);
            h2::Reason::INTERNAL_ERROR.into()
        })
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        let inner = self.inner.call(request);
        ResponseFuture { inner }
    }
}

impl<Req, Rec, Mk> Clone for Service<Req, Rec, Mk>
where
    Rec: Recognize<Req>,
    Mk: svc::Make<Rec::Target>,
    Mk::Value: svc::Service<Request = Req>,
    Router<Req, Rec, Mk>: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// === impl ResponseFuture ===

impl<F, E, G, B> Future for ResponseFuture<F>
where
    F: Future<Item = http::Response<B>, Error = Error<E, G>>,
    E: error::Error,
    G: fmt::Debug,
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
