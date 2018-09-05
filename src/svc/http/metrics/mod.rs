use indexmap::IndexMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, Weak};

use linkerd2_metrics::{
    latency,
    Counter,
    FmtLabels,
    FmtMetric,
    FmtMetrics,
    Histogram,
    Metric,
};

mod body;
mod class;
mod report;
mod service;

pub use self::report::Report;
pub use self::service::{Make, NewMeasure, Measure};

pub fn new<C, B, S>(base: B) -> (Make<S, C>, Report<C, B, S>)
where
    C: FmtLabels + Clone,
    B: FmtLabels + Clone,
    S: FmtLabels + Clone + Hash + Eq,
{
    let registry = Arc::new(Mutex::new(Registry::<S, C>::default()));
    (Make::new(registry.clone()), Report { base, registry })
}

#[derive(Debug)]
struct Registry<T, C>
where
    T: FmtLabels + Hash + Eq,
    C: FmtLabels + Hash + Eq,
{
    by_target: IndexMap<T, Arc<Mutex<Metrics<C>>>>,
}

#[derive(Debug)]
struct Metrics<C>
where
    C: FmtLabels + Hash + Eq,
{
    pending: Arc<()>,
    total: Counter,
    by_class: IndexMap<C, ClassMetrics>,
}

#[derive(Debug, Default)]
pub struct ClassMetrics {
    total: Counter,
    latency: Histogram<latency::Ms>,
}

struct RequestSensor {
    metrics: Arc<Mutex<Metrics>>,
    state: Arc<Mutex<RequestState>>,
    response: Weak<Mutex<ResponseState>>,
}

struct RequestState {}
struct ResponseState {}

struct NewResponseSensor {
    metrics: Arc<Mutex<Metrics>>,
    state: Arc<Mutex<ResponseState>>,
    request: Weak<Mutex<RequestState>>,
}

struct ResponseSensor {
    metrics: Arc<Mutex<Metrics>>,
    state: Arc<Mutex<ResponseState>>,
    request: Weak<Mutex<RequestState>>,
}

fn new_stream_sensors(metrics: Arc<Mutex<Metrics>>) -> (RequestSensor, NewResponseSensor) {
    let request = Arc::new(Mutex::new(RequestState {}));
    let response = Arc::new(Mutex::new(ResponseState {}));
    let req = RequestSensor {
        metrics: metrics.clone(),
        state: request.clone(),
        response: Arc::downgrade(&response),
    };
    let rsp = NewResponseSensor {
        metrics,
        state: response,
        request: Arc::downgrade(&request),
    };
    (req, rsp)
}

struct ResponseSensor {
    metrics: Arc<Mutex<Metrics>>,
}

fn new_stream_sensor(metrics: Arc<Mutex<Metrics>>) -> (RequestSensor, NewResponseSensor) {
    let req = RequestSensor {
        metrics: metrics.clone(),
    };
    let rsp = NewResponseSensor {
        metrics: metrics.clone(),
    };
    (req, rsp)
}

// ===== impl Registry =====

impl<S> Default for Registry<S>
where
    S: FmtLabels + Hash + Eq,
{
    fn default() -> Self {
        Registry { by_target: IndexMap::default() }
    }
}
