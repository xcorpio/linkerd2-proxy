use http;
use std::default::Default;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use linkerd2_metrics::FmtLabels;

use svc::{Service, MakeClient, NewClient};
use svc::http::metrics::{Metrics, ServiceMetrics};

const GRPC_STATUS: &str = "grpc-status";

#[derive(Clone, Debug)]
pub struct Make<S>(Arc<Mutex<Metrics<S>>>);

#[derive(Clone, Debug)]
pub struct NewMeasure<N: NewClient> {
    metrics: Arc<Mutex<Metrics<N::Target>>>,
    inner: N,
}

#[derive(Clone, Debug)]
pub struct Measure<S: Service> {
    metrics: Arc<Mutex<ServiceMetrics>>,
    inner: S,
}

// ===== impl Make =====

impl<S> Make<S>
where
    S: FmtLabels + Clone + Hash + Eq,
{
    pub(super) fn new(metrics: Arc<Mutex<Metrics<S>>>) -> Self {
        Make(metrics)
    }
}

impl<N> MakeClient<N> for Make<N::Target>
where
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = <NewMeasure<N::Client> as NewClient>::Client;
    type NewClient = NewMeasure<N::Client>;

    fn make_client(&self, inner: N) -> Self::NewClient {
        NewMeasure {
            metrics: self.0.clone(),
            inner,
        }
    }
}

// ===== impl NewMeasure =====

impl<N> NewClient for NewMeasure<N>
where
    N: NewClient,
    N::Target: FmtLabels + Clone + Hash + Eq,
    Measure<N::Client>: Service,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = Measure<N::Client>;

    fn new_client(&self, target: &Self::Target) -> Result<N::Client, N::Error> {
        let inner = self.new_client(target)?;
        let mut reg = self.metrics.lock().expect("FIXME error type");
        let metrics = reg.by_service.entry(target.clone()).or_insert_with(|| Default::default());
        Ok(Measure {
            metrics,
            inner,
        })
    }
}

// ===== impl NewMeasure =====

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

}
