use futures::Poll;
use http;
use linkerd2_metrics::FmtLabels;
use std::hash::Hash;
use std::marker::PhantomData;

use svc::{MakeService, Service, Stack};

/// Determines how a request's response should be classified.
pub trait Classify<Error> {
    type Class;

    /// Classifies responses.
    ///
    /// Instances are intended to be used as an `http::Extension` that may be
    /// cloned to inner stack layers. Cloned instances are **not** intended to
    /// share state. Each clone should maintain its own internal state.
    type ClassifyResponse: ClassifyResponse<Error, Class = Self::Class>
        + Clone
        + Send
        + Sync
        + 'static;

    fn classify(&self, req: &http::request::Parts) -> Self::ClassifyResponse;
}

/// Classifies a single response.
pub trait ClassifyResponse<Error> {
    /// A response classification.
    type Class;

    /// Update the classifier with the response headers.
    ///
    /// If this is enough data to classify a response, a classification may be
    /// returned.
    ///
    /// This is expected to be called only once.
    fn start(&mut self, headers: &http::response::Parts) -> Option<Self::Class>;

    /// Update the classifier with the response trailers.
    ///
    /// Because trailers indicate an EOS, a classification must be returned.
    ///
    /// This is expected to be called only once.
    fn end(&mut self, headers: &http::HeaderMap) -> Self::Class;

    /// Update the classifier with an underlying error.
    ///
    /// Because errors indicate an end-of-stream, a classification must be
    /// returned.
    ///
    /// This is expected to be called only once.
    fn error(&mut self, error: &Error) -> Self::Class;
}

/// A `Stack` module that adds a `ClassifyResponse` instance to each
pub struct Mod<C, E>(C, PhantomData<E>);

pub struct Make<C, N> {
    classify: C,
    inner: N,
}

pub struct ExtendRequest<C, N> {
    classify: C,
    inner: N,
}

impl<E, C: Classify<E> + Clone> Mod<C, E> {
    fn new(c: C) -> Self {
        Mod(c, PhantomData)
    }
}

impl<N, A, B, C> Stack<N> for Mod<C, <N::Service as Service>::Error>
where
    N: MakeService,
    N::Config: FmtLabels + Clone + Hash + Eq,
    N::Service: Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify<<N::Service as Service>::Error> + Clone,
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
    N::Service: Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify<<N::Service as Service>::Error> + Clone,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = ExtendRequest<C, N::Service>;

    fn make_service(&self, config: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.make_service(config)?;
        Ok(ExtendRequest {
            classify: self.classify.clone(),
            inner,
        })
    }
}

// ===== impl ExtendRequest =====

impl<S, A, B, C> Service for ExtendRequest<C, S>
where
    S: Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify<S::Error>,
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
