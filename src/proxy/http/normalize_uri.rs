use http;
use futures::Poll;
use std::marker::PhantomData;

use super::h1;
use svc;

pub struct Layer<T, M>(PhantomData<fn() -> (T, M)>);

pub struct Make<T, N: svc::Make<T>> {
    inner: N,
    _p: PhantomData<T>
}

#[derive(Copy, Clone, Debug)]
pub struct Service<S> {
    inner: S,
}

// === impl Layer ===

impl<T, B, M> Layer<T, M>
where
    M: svc::Make<T>,
    M::Value: svc::Service<Request = http::Request<B>>,
{
    pub fn new() -> Self {
        Layer(PhantomData)
    }
}

impl<T, B, M> Clone for Layer<T, M>
where
    M: svc::Make<T>,
    M::Value: svc::Service<Request = http::Request<B>>,
{
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl<T, B, M> svc::Layer<T, T, M> for Layer<T, M>
where
    M: svc::Make<T>,
    M::Value: svc::Service<Request = http::Request<B>>,
{
    type Value = <Make<T, M> as svc::Make<T>>::Value;
    type Error = <Make<T, M> as svc::Make<T>>::Error;
    type Make = Make<T, M>;

    fn bind(&self, inner: M) -> Self::Make {
        Make { inner, _p: PhantomData }
    }
}

// === impl Make ===

impl<T, B, M> svc::Make<T> for Make<T, M>
where
    M: svc::Make<T>,
    M::Value: svc::Service<Request = http::Request<B>>,
{
    type Value = Service<M::Value>;
    type Error = M::Error;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let inner = self.inner.make(&target)?;
        Ok(Service { inner })
    }
}

// === impl Service ===

impl<S, B> svc::Service for Service<S>
where
    S: svc::Service<Request = http::Request<B>>,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self) -> Poll<(), S::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, mut request: S::Request) -> Self::Future {
        debug!("normalizing {}", request.uri());
        h1::normalize_our_view_of_uri(&mut request);
        debug!("normalized {}", request.uri());
        self.inner.call(request)
    }
}
