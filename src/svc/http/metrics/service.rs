use futures::Poll;
use http;
use std::default::Default;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use linkerd2_metrics::FmtLabels;

use svc::{Service, MakeClient, NewClient};
use svc::http::metrics::{Metrics, TargetMetrics};

const GRPC_STATUS: &str = "grpc-status";

#[derive(Clone, Debug)]
pub struct Make<S>
where
    S: FmtLabels + Clone + Hash + Eq,
{
    metrics: Arc<Mutex<Metrics<S>>>,
}

#[derive(Clone, Debug)]
pub struct NewMeasure<N>
where
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
{
    metrics: Arc<Mutex<Metrics<N::Target>>>,
    inner: N,
}

#[derive(Clone, Debug)]
pub struct Measure<S: Service> {
    metrics: Arc<Mutex<TargetMetrics>>,
    inner: S,
}

// ===== impl Make =====

impl<S> Make<S>
where
    S: FmtLabels + Clone + Hash + Eq,
{
    pub(super) fn new(metrics: Arc<Mutex<Metrics<S>>>) -> Self {
        Make { metrics }
    }
}

impl<N, A, B> MakeClient<N> for Make<N::Target>
where
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
    N::Client: Service<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = <NewMeasure<N> as NewClient>::Client;
    type NewClient = NewMeasure<N>;

    fn make_client(&self, inner: N) -> Self::NewClient {
        NewMeasure {
            metrics: self.metrics.clone(),
            inner,
        }
    }
}

// ===== impl NewMeasure =====

impl<N, A, B> NewClient for NewMeasure<N>
where
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
    N::Client: Service<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = Measure<N::Client>;

    fn new_client(&self, target: &Self::Target) -> Result<Self::Client, Self::Error> {
        let inner = self.inner.new_client(target)?;
        let metrics = self.metrics
            .lock().expect("FIXME error type")
            .by_target.entry(target.clone())
            .or_insert_with(|| Default::default())
            .clone();
        Ok(Measure {
            metrics,
            inner,
        })
    }
}

// ===== impl Measure =====

impl<S, A, B> Service for Measure<S>
where
    S: Service<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        self.inner.call(req)
    }
}
