use futures::{Future, Poll};
use http;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_timer::clock;

use linkerd2_metrics::FmtLabels;

use svc::{Service, MakeClient, NewClient};
use svc::http::classify::{Classify, ClassifyStream};
use svc::http::metrics::{Registry, Metrics};
use svc::http::metrics::body::{RequestBody, ResponseBody};

#[derive(Clone, Debug)]
pub struct Make<S, C>
where
    S: FmtLabels + Clone + Hash + Eq,
    C: Classify + Clone,
{
    classify: C,
    registry: Arc<Mutex<Registry<S, C::Class>>>,
}

#[derive(Clone, Debug)]
pub struct NewMeasure<N, C>
where
    C: Classify,
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
{
    classify: C,
    registry: Arc<Mutex<Registry<N::Target, C::Class>>>,
    inner: N,
}

#[derive(Clone, Debug)]
pub struct Measure<S: Service, C: Classify> {
    classify: C,
    metrics: Arc<Mutex<Metrics<C::Class>>>,
    inner: S,
}

pub struct ResponseFuture<S: Service, C: ClassifyStream> {
    state: Arc<StreamState<C>>,
    inner: S::Future,
}

#[derive(Debug)]
pub struct RequestBody<T, C: ClassifyStream> {
    state: Option<StreamState<C>>,
    inner: T,
}

#[derive(Debug)]
pub struct ResponseBody<T, C: ClassifyStream> {
    state: Option<StreamState<C>>,
    inner: T,
}

struct StreamState<C: ClassifyStream> {
    init_stamp: Instant,
    metrics: Arc<Mutex<Metrics<C::Class>>>,
    classify: Mutex<C>,
}

// ===== impl Make =====

impl<S, C> Make<S, C>
where
    S: FmtLabels + Clone + Hash + Eq,
    C: Classify + Clone,
{
    pub(super) fn new(
        classify: C,
        registry: Arc<Mutex<Registry<S, C::Class>>>,
    ) -> Self {
        Self { classify, registry }
    }
}

impl<C, N, A, B> MakeClient<N> for Make<N::Target, C>
where
    C: Classify + Clone,
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
    N::Client: Service<
        Request = http::Request<RequestBody<A, C::ClassifyStream>>,
        Response = http::Response<B>,
    >,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = <NewMeasure<N, C> as NewClient>::Client;
    type NewClient = NewMeasure<N, C>;

    fn make_client(&self, inner: N) -> Self::NewClient {
        NewMeasure {
            classify: self.classify.clone(),
            registry: self.registry.clone(),
            inner,
        }
    }
}

// ===== impl NewMeasure =====

impl<C, N, A, B> NewClient for NewMeasure<N, C>
where
    C: Classify + Clone,
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
    N::Client: Service<
        Request = http::Request<RequestBody<A, C::Clas>>,
        Response = http::Response<B>,
    >,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = Measure<N::Client, C>;

    fn new_client(&self, target: &Self::Target) -> Result<Self::Client, Self::Error> {
        let inner = self.inner.new_client(target)?;

        let metrics = match self.registry.lock() {
            Ok(mut r) => {
                r.by_target.entry(target.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(Metrics::new())))
                    .clone()
            }
            Err(_) => {
                // If the metrics lock was poisoned, just create a one-off
                // structure that won't be reported.
                Arc::new(Mutex::new(Metrics::new()))
            }
        };

        Ok(Measure {
            classify: self.classify.clone(),
            metrics,
            inner
       })
    }
}

// ===== impl Measure =====

impl<C, S, A, B> Service for Measure<S, C>
where
    C: Classify + Clone,
    S: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
    >,
{
    type Request = http::Request<A>;
    type Response = http::Response<ResponseBody<B>>;
    type Error = S::Error;
    type Future = ResponseFuture<S, C::ClassifyStream>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        let (head, inner) = req.into_parts();
        let state = Arc::new(StreamState {
            init_stamp: clock::now(),
            classify: Mutex::new(self.classify.classify_stream(&head)),
            metrics: self.metrics.clone(),
        });
        let body = RequestBody::new(state.clone(), inner);
        let req = http::Request::from_parts(head, body);

        ResponseFuture {
            state,
            inner: self.inner.call(req),
        }
    }
}

impl<C, S, B> Future for ResponseFuture<S, C>
where
    S: Service<Response = http::Response<B>>,
    C: ClassifyStream,
{
    type Item = http::Response<ResponseBody<B>>;
    type Error = S::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let (head, inner) = try_ready!(self.inner.poll()).into_parts();
        if let Ok(ref mut classify) = self.state.classify.lock() {
            classify.response(&head);
        }
        let body = ResponseBody::new(self.state.clone(), inner);

        let rsp = http::Response::from_parts(head, body);
        Ok(rsp.into())
    }
}
