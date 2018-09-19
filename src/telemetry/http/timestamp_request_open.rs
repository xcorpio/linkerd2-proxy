use futures::Poll;
use http;
use std::marker::PhantomData;
use std::time::Instant;

use svc;

/// A `RequestOpen` timestamp.
///
/// This is added to a request's `Extensions` by the `TimestampRequestOpen`
/// middleware. It's a newtype in order to distinguish it from other
/// `Instant`s that may be added as request extensions.
#[derive(Copy, Clone, Debug)]
pub struct RequestOpen(pub Instant);

/// Middleware that adds a `RequestOpen` timestamp to requests.
///
/// This is a separate middleware from `sensor::Http`, because we want
/// to install it at the earliest point in the stack. This is in order
/// to ensure that request latency metrics cover the overhead added by
/// the proxy as accurately as possible.
#[derive(Copy, Clone, Debug)]
pub struct TimestampRequestOpen<S> {
    inner: S,
}

/// Layers a `TimestampRequestOpen` middleware on an HTTP client.
#[derive(Clone, Debug)]
pub struct Layer<T, B>(PhantomData<fn() -> (T, B)>);

/// Uses an `M`-typed `Make` to build a `TimestampRequestOpen` service.
#[derive(Clone, Debug)]
pub struct Make<M>(M);

// === impl TimestampRequsetOpen ===

impl<S, B> svc::Service for TimestampRequestOpen<S>
where
    S: svc::Service<Request = http::Request<B>>,
{
    type Request = http::Request<B>;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, mut req: Self::Request) -> Self::Future {
        req.extensions_mut().insert(RequestOpen(Instant::now()));
        self.inner.call(req)
    }
}

// === impl Layer ===

impl<T, B> Layer<T, B> {
    pub fn new() -> Self {
        Layer(PhantomData)
    }
}

impl<N, T, B> svc::Layer<N> for Layer<T, B>
where
    N: svc::Make<T>,
    N::Service: svc::Service<Request = http::Request<B>>,
{
    type Bound = Make<N>;

    fn bind(&self, next: N) -> Make<N> {
        Make(next)
    }
}

// === impl Make ===

impl<N, T, B> svc::Make<T> for Make<N>
where
    N: svc::Make<T>,
    N::Service: svc::Service<Request = http::Request<B>>,
{
    type Service = TimestampRequestOpen<N::Service>;
    type Error = N::Error;

    fn make(&self, target: &T) -> Result<Self::Service, Self::Error> {
        let inner = self.0.make(target)?;
        Ok(TimestampRequestOpen { inner })
    }
}
