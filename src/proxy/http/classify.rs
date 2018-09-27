use futures::Poll;
use http;
use std::marker::PhantomData;

use svc;

/// Determines how a request's response should be classified.
pub trait Classify {
    type Class;
    type Error;

    /// Classifies responses.
    ///
    /// Instances are intended to be used as an `http::Extension` that may be
    /// cloned to inner stack layers. Cloned instances are **not** intended to
    /// share state. Each clone should maintain its own internal state.
    type ClassifyResponse: ClassifyResponse<Class = Self::Class, Error = Self::Error>
        + Clone
        + Send
        + Sync
        + 'static;

    fn classify<B>(&self, req: &http::Request<B>) -> Self::ClassifyResponse;
}

/// Classifies a single response.
pub trait ClassifyResponse {
    /// A response classification.
    type Class;
    type Error;

    /// Update the classifier with the response headers.
    ///
    /// If this is enough data to classify a response, a classification may be
    /// returned. Implementations should expect that `end` or `error` may be
    /// called even when a class is returned.
    ///
    /// This is expected to be called only once.
    fn start<B>(&mut self, headers: &http::Response<B>) -> Option<Self::Class>;

    /// Update the classifier with an EOS.
    ///
    /// Because trailers indicate an EOS, a classification must be returned.
    ///
    /// This is expected to be called only once.
    fn eos(&mut self, trailers: Option<&http::HeaderMap>) -> Self::Class;

    /// Update the classifier with an underlying error.
    ///
    /// Because errors indicate an end-of-stream, a classification must be
    /// returned.
    ///
    /// This is expected to be called only once.
    fn error(&mut self, error: &Self::Error) -> Self::Class;
}

/// A `Stack` module that adds a `ClassifyResponse` instance to each
pub struct Layer<C: Classify, T, M: svc::Make<T>> {
    classify: C,
    _p: PhantomData<fn() -> (T, M)>,
}

pub struct Make<C: Classify, T, M: svc::Make<T>> {
    classify: C,
    inner: M,
    _p: PhantomData<fn() -> T>,
}

/// A service
pub struct ExtendRequest<C: Classify, S: svc::Service> {
    classify: C,
    inner: S,
}

impl<C, T, M, A, B> Layer<C, T, M>
where
    M: svc::Make<T>,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify + Clone,
{
    pub fn new(classify: C) -> Self {
        Layer {
            classify,
            _p: PhantomData,
        }
    }
}

impl<C, T, M, A, B> Clone for Layer<C, T, M>
where
    M: svc::Make<T>,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify + Clone,
{
    fn clone(&self) -> Self {
        Self::new(self.classify.clone())
    }
}

impl<T, M, A, B, C> svc::Layer<T, T, M> for Layer<C, T, M>
where
    M: svc::Make<T>,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify + Clone,
{
    type Value = <Make<C, T, M> as svc::Make<T>>::Value;
    type Error = M::Error;
    type Make = Make<C, T, M>;

    fn bind(&self, inner: M) -> Self::Make {
        Make {
            inner,
            classify: self.classify.clone(),
            _p: PhantomData,
        }
    }
}

// ===== impl Make =====

impl<C, T, M, A, B> Clone for Make<C, T, M>
where
    M: svc::Make<T> + Clone,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify + Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            classify: self.classify.clone(),
            _p: PhantomData
        }
    }
}

impl<T, M, A, B, C> svc::Make<T> for Make<C, T, M>
where
    M: svc::Make<T>,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify + Clone,
{
    type Value = ExtendRequest<C, M::Value>;
    type Error = M::Error;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let inner = self.inner.make(target)?;
        let classify = self.classify.clone();

        Ok(ExtendRequest { classify, inner })
    }
}

// ===== impl ExtendRequest =====

impl<S, A, B, C> svc::Service for ExtendRequest<C, S>
where
    S: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify + Clone,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, mut req: Self::Request) -> Self::Future {
        let classify_response = self.classify.classify(&req);
        req.extensions_mut().insert(classify_response);

        self.inner.call(req)
    }
}
