use h2;
use http;
use indexmap::IndexMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use tower_h2;

use metrics::{latency, Counter, FmtLabels, Histogram};
use proxy::http::Classify;
use svc;

mod class;
mod report;
mod service;

pub use self::report::Report;
pub use self::service::{Layer, Make, Measure, RequestBody};

pub fn new<C, T, M, A, B>() -> (Layer<T, M, C>, Report<T, C::Class>)
where
    T: FmtLabels + Clone + Hash + Eq,
    M: svc::Make<T>,
    M::Value: svc::Service<
        Request = http::Request<RequestBody<A, C::Class>>,
        Response = http::Response<B>,
        Error = h2::Error,
    >,
    C: Classify<Error = h2::Error> + Clone,
    C::Class: FmtLabels + Hash + Eq,
    C::ClassifyResponse: Send + Sync + 'static,
    A: tower_h2::Body,
    B: tower_h2::Body,
{
    let registry = Arc::new(Mutex::new(Registry::default()));
    (Layer::new(registry.clone()), Report::new(registry))
}

#[derive(Debug)]
struct Registry<T, C>
where
    T: Hash + Eq,
    C: Hash + Eq,
{
    by_target: IndexMap<T, Arc<Mutex<Metrics<C>>>>,
}

#[derive(Debug)]
struct Metrics<C>
where
    C: Hash + Eq,
{
    total: Counter,
    by_class: IndexMap<C, ClassMetrics>,
    unclassified: ClassMetrics,
}

#[derive(Debug, Default)]
pub struct ClassMetrics {
    total: Counter,
    latency: Histogram<latency::Ms>,
}

impl<T, C> Default for Registry<T, C>
where
    T: Hash + Eq,
    C: Hash + Eq,
{
    fn default() -> Self {
        Self {
            by_target: IndexMap::default(),
        }
    }
}

impl<C> Default for Metrics<C>
where
    C: Hash + Eq,
{
    fn default() -> Self {
        Self {
            total: Counter::default(),
            by_class: IndexMap::default(),
            unclassified: ClassMetrics::default(),
        }
    }
}
