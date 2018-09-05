use futures::Poll;
use http;
use linkerd2_metrics::FmtLabels;
use std::hash::Hash;
use std::marker::PhantomData;

use svc::{Stack, MakeService, Service};

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

pub trait ClassifyResponse<Error> {
    type Class;

    fn start(&mut self, headers: &http::response::Parts) -> Option<Self::Class>;
    fn end(&mut self, headers: &http::HeaderMap) -> Self::Class;
    fn error(&mut self, error: &Error) -> Self::Class;
}

/// A `Stack` module that adds a `ClassifyResponse` instance to each
pub struct Mod<C, E>(C, PhantomData<E>);

pub struct NewAddRequestExtension<C, N> {
    classify: C,
    inner: N,
}

pub struct AddRequestExtension<C, N> {
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
    type Service = <NewAddRequestExtension<C, N> as MakeService>::Service;
    type MakeService = NewAddRequestExtension<C, N>;

    fn build(&self, inner: N) -> Self::MakeService {
        NewAddRequestExtension {
            classify: self.0.clone(),
            inner,
        }
    }
}

// ===== impl NewMeasure =====

impl<N, A, B, C> MakeService for NewAddRequestExtension<C, N>
where
    N: MakeService,
    N::Config: FmtLabels + Clone + Hash + Eq,
    N::Service: Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify<<N::Service as Service>::Error> + Clone,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = AddRequestExtension<C, N::Service>;

    fn make_service(&self, config: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.make_service(config)?;
        Ok(AddRequestExtension {
            classify: self.classify.clone(),
            inner,
        })
    }
}

// ===== impl AddRequestExtension =====

impl<S, A, B, C> Service for AddRequestExtension<C, S>
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
