use futures::Poll;
use http;
use linkerd2_metrics::FmtLabels;
use std::hash::Hash;
use std::marker::PhantomData;

use svc::{MakeService, Service, Stack};

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

    /// Update the classifier with the response trailers.
    ///
    /// Because trailers indicate an EOS, a classification must be returned.
    ///
    /// This is expected to be called only once.
    fn end(&mut self, headers: &http::HeaderMap) -> Self::Class;

    /// Update the classifier indicating that the request was canceled.
    ///
    /// Because errors indicate an end-of-stream, a classification must be
    /// returned.
    ///
    /// This is expected to be called only once.
    fn cancel(&mut self) -> Self::Class;

    /// Update the classifier with an underlying error.
    ///
    /// Because errors indicate an end-of-stream, a classification must be
    /// returned.
    ///
    /// This is expected to be called only once.
    fn error(&mut self, error: &Self::Error) -> Self::Class;
}

/// A `Stack` module that adds a `ClassifyResponse` instance to each
pub struct Mod<C, E>(C);

pub struct Make<C, N> {
    classify: C,
    inner: N,
}

pub struct ExtendRequest<C, N> {
    classify: C,
    inner: N,
}

impl<C: Classify + Clone> Mod<C> {
    fn new(c: C) -> Self {
        Mod(c)
    }
}

impl<N, A, B, C> Stack<N> for Mod<C>
where
    N: MakeService,
    N::Config: FmtLabels + Clone + Hash + Eq,
    N::Service: Service<Request = http::Request<A>, Response = http::Response<B>, Error = C::Error>,
    C: Classify + Clone,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = <Make<C, N> as MakeService>::Service;
    type MakeService = Make<C, N>;

    fn build(&self, inner: N) -> Self::MakeService {
        Make {
            classify: self.0.clone(),
            inner,
        }
    }
}

// ===== impl NewMeasure =====

impl<N, A, B, C> MakeService for Make<C, N>
where
    N: MakeService,
    N::Config: FmtLabels + Clone + Hash + Eq,
    N::Service: Service<Request = http::Request<A>, Response = http::Response<B>, Error = C::Error>,
    C: Classify + Clone,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = ExtendRequest<C, N::Service>;

    fn make_service(&self, config: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.make_service(config)?;
        let classify = self.classify.clone();

        Ok(ExtendRequest { classify, inner })
    }
}

// ===== impl ExtendRequest =====

impl<S, A, B, C> Service for ExtendRequest<C, S>
where
    N::Service: Service<Request = http::Request<A>, Response = http::Response<B>, Error = C::Error>,
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
