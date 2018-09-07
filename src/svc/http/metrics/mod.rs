use indexmap::IndexMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use linkerd2_metrics::{latency, Counter, FmtLabels, Histogram};

use svc::http::Classify;

mod class;
mod report;
mod service;

pub use self::report::Report;
pub use self::service::{New, Measure, Mod};

pub fn new<Base, Config, C>(base: Base) -> (Mod<Config, C>, Report<Base, Config, C::Class>)
where
    Base: FmtLabels + Clone,
    Config: FmtLabels + Clone + Hash + Eq,
    C: Classify,
    C::Class: FmtLabels + Clone + Hash + Eq,
{
    let registry = Arc::new(Mutex::new(Registry::default()));
    (Mod::new(registry.clone()), Report::new(base, registry))
}

#[derive(Debug)]
struct Registry<Config, Class>
where
    Config: Hash + Eq,
    Class: Hash + Eq,
{
    by_config: IndexMap<Config, Arc<Mutex<Metrics<Class>>>>,
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

impl<Config, Class> Default for Registry<Config, Class>
where
    Config: Hash + Eq,
    Class: Hash + Eq,
{
    fn default() -> Self {
        Self {
            by_config: IndexMap::default(),
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
