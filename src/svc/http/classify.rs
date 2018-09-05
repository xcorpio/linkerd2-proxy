use futures::Poll;
use http;
use linkerd2_metrics::FmtLabels;
use std::hash::Hash;
use std::marker::PhantomData;

use svc::{MakeClient, NewClient, Service};

/// Determines how a request's response should be classified.
///
///
pub trait Classify<Error>: Clone {
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

pub struct Make<C>(C);

pub struct NewRequestExtension<C, N> {
    inner: N,
    classify: C,
}

pub struct RequestExtension<C, N> {
    inner: N,
    classify: C,
}

impl<E, C: Classify<E>> Make<C> {
    fn new(c: C) -> Self {
        Make(c)
    }
}

impl<N, A, B, C> MakeClient<N> for Make<C>
where
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
    N::Client: Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify<<N::Client as Service>::Error>,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = <NewRequestExtension<C, N> as NewClient>::Client;
    type NewClient = NewRequestExtension<C, N>;

    fn make_client(&self, inner: N) -> Self::NewClient {
        NewRequestExtension {
            classify: self.0.clone(),
            inner,
        }
    }
}

// ===== impl NewMeasure =====

impl<N, A, B, C> NewClient for NewRequestExtension<C, N>
where
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
    N::Client: Service<Request = http::Request<A>, Response = http::Response<B>>,
    C: Classify<<N::Client as Service>::Error>,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = RequestExtension<C, N::Client>;

    fn new_client(&self, target: &Self::Target) -> Result<Self::Client, Self::Error> {
        let inner = self.inner.new_client(target)?;
        Ok(RequestExtension {
            inner,
            _p: PhantomData,
        })
    }
}

// ===== impl RequestExtension =====

impl<S, A, B, C> Service for RequestExtension<C, S>
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
        head.extensions.insert(self.classify.classify(&head));

        let req = http::Request::from_parts(head, body);
        self.inner.call(req)
    }
}
