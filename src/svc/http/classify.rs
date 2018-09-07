use futures::Poll;
use http;
use std::hash::Hash;

use svc::{NewClient, Service, Stack};

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

    fn classify(&self, req: &http::request::Parts) -> Self::ClassifyResponse;
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
    fn start(&mut self, headers: &http::response::Parts) -> Option<Self::Class>;

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
pub struct Mod<C: Classify>(C);

pub struct New<C: Classify, N: NewClient> {
    classify: C,
    inner: N,
}

/// A service
pub struct ExtendRequest<C: Classify, S: Service> {
    classify: C,
    inner: S,
}

impl<C: Classify + Clone> Mod<C> {
    fn new(c: C) -> Self {
        Mod(c)
    }
}

impl<N, A, B, C> Stack<N> for Mod<C>
where
    N: NewClient,
    N::Service: Service<Request = http::Request<A>, Response = http::Response<B>, Error = C::Error>,
    C: Classify + Clone,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = <New<C, N> as NewClient>::Service;
    type NewClient = New<C, N>;

    fn build(&self, inner: N) -> Self::NewClient {
        New {
            classify: self.0.clone(),
            inner,
        }
    }
}

// ===== impl NewMeasure =====

impl<N, A, B, C> NewClient for New<C, N>
where
    N: NewClient,
    N::Service: Service<Request = http::Request<A>, Response = http::Response<B>, Error = C::Error>,
    C: Classify + Clone,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = ExtendRequest<C, N::Service>;

    fn new_client(&self, config: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.new_client(config)?;
        let classify = self.classify.clone();

        Ok(ExtendRequest { classify, inner })
    }
}

// ===== impl ExtendRequest =====

impl<S, A, B, C> Service for ExtendRequest<C, S>
where
    S: Service<Request = http::Request<A>, Response = http::Response<B>, Error = C::Error>,
    C: Classify + Clone,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        let (mut head, body) = req.into_parts();

        let classify_response = self.classify.classify(&head);
        head.extensions.insert(classify_response);

        let req = http::Request::from_parts(head, body);
        self.inner.call(req)
    }
}
