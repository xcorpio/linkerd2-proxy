use http;
use futures::Poll;
use std::marker::PhantomData;

use super::h1::normalize_our_view_of_uri;
use svc;

pub struct Layer<T>(PhantomData<T>);

pub struct Make<T, N: svc::Make<T>> {
    inner: N,
    _p: PhantomData<T>
}

#[derive(Copy, Clone, Debug)]
pub struct Service<S> {
    inner: S,
}

// === impl Layer ===

pub fn layer<T>() -> Layer<T> {
    Layer(PhantomData)
}

impl<T, N, B> svc::Layer<N> for Layer<T>
where
    N: svc::Make<T>,
    N::Output: svc::Service<Request = http::Request<B>>,
{
    type Bound = Make<T, N>;

    fn bind(&self, inner: N) -> Self::Bound {
        Make { inner, _p: PhantomData }
    }
}

// === impl Make ===

impl<T, N, B> svc::Make<T> for Make<T, N>
where
    N: svc::Make<T>,
    N::Output: svc::Service<Request = http::Request<B>>,
{
    type Output = Service<N::Output>;
    type Error = N::Error;

    fn make(&self, target: &T) -> Result<Self::Output, Self::Error> {
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
        normalize_our_view_of_uri(&mut request);
        self.inner.call(request)
    }
}
