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

mod class;
mod report;
mod service;

pub use self::report::Report;
pub use self::service::{Stack, NewMeasure, Measure};

pub fn new<C, B, S>(base: B) -> (Mod<S, C>, Report<C, B, S>)
where
    C: FmtLabels + Clone,
    B: FmtLabels + Clone,
    S: FmtLabels + Clone + Hash + Eq,
{
    let registry = Arc::new(Mutex::new(Registry::<S, C>::default()));
    (Mod::new(registry.clone()), Report { base, registry })
}

#[derive(Debug)]
struct Registry<Config, Class>
where
    Config: FmtLabels + Hash + Eq,
    Class: FmtLabels + Hash + Eq,
{
    by_config: IndexMap<Config, Arc<Mutex<Metrics<Class>>>>,
}

#[derive(Debug)]
struct Metrics<Class>
where
    Class: FmtLabels + Hash + Eq,
{
    pending: Arc<()>,
    total: Counter,
    by_class: IndexMap<Class, ClassMetrics>,
}

#[derive(Debug, Default)]
pub struct ClassMetrics {
    total: Counter,
    latency: Histogram<latency::Ms>,
}
