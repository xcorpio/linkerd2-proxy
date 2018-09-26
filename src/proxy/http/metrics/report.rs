use std::fmt;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use linkerd2_metrics::{latency, Counter, FmtLabels, FmtMetric, FmtMetrics, Histogram, Metric};

use super::{ClassMetrics, Metrics, Registry};

metrics! {
    request_total: Counter { "Total count of HTTP requests." },
    response_total: Counter { "Total count of HTTP responses" },
    response_latency_ms: Histogram<latency::Ms> {
        "Elapsed times between a request's headers being received \
        and its response stream completing"
    }
}

/// Reports HTTP metrics for prometheus.
#[derive(Clone, Debug)]
pub struct Report<Base, Target, Class>
where
    Base: FmtLabels + Clone,
    Target: FmtLabels + Hash + Eq,
    Class: FmtLabels + Hash + Eq,
{
    base: Base,
    registry: Arc<Mutex<Registry<Target, Class>>>,
}

// ===== impl Report =====

impl<Base, Target, Class> Report<Base, Target, Class>
where
    Base: FmtLabels + Clone,
    Target: FmtLabels + Hash + Eq,
    Class: FmtLabels + Hash + Eq,
{
    pub(super) fn new(base: Base, registry: Arc<Mutex<Registry<Target, Class>>>) -> Self {
        Self { base, registry }
    }
}

impl<Base, Target, Class> FmtMetrics for Report<Base, Target, Class>
where
    Base: FmtLabels + Clone,
    Target: FmtLabels + Hash + Eq,
    Class: FmtLabels + Hash + Eq,
{
    fn fmt_metrics(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let registry = match self.registry.lock() {
            Err(_) => return Ok(()),
            Ok(r) => r,
        };

        if registry.by_target.is_empty() {
            return Ok(());
        }

        request_total.fmt_help(f)?;
        registry.fmt_by_target(f, &self.base, request_total, |s| &s.total)?;

        response_total.fmt_help(f)?;
        registry.fmt_by_class(f, &self.base, response_total, |s| &s.total)?;
        registry.fmt_by_target(f, &self.base, response_total, |s| &s.unclassified.total)?;

        response_latency_ms.fmt_help(f)?;
        registry.fmt_by_class(f, &self.base, response_latency_ms, |s| &s.latency)?;
        registry.fmt_by_target(f, &self.base, response_latency_ms, |s| {
            &s.unclassified.latency
        })?;

        Ok(())
    }
}

impl<Target, Class> Registry<Target, Class>
where
    Target: FmtLabels + Hash + Eq,
    Class: FmtLabels + Hash + Eq,
{
    fn fmt_by_target<B, M, F>(
        &self,
        f: &mut fmt::Formatter,
        base: B,
        metric: Metric<M>,
        get_metric: F,
    ) -> fmt::Result
    where
        B: FmtLabels,
        M: FmtMetric,
        F: Fn(&Metrics<Class>) -> &M,
    {
        for (tgt, tm) in &self.by_target {
            if let Ok(m) = tm.lock() {
                let labels = (&base, tgt);
                get_metric(&*m).fmt_metric_labeled(f, metric.name, labels)?;
            }
        }

        Ok(())
    }

    fn fmt_by_class<B, M, F>(
        &self,
        f: &mut fmt::Formatter,
        base: B,
        metric: Metric<M>,
        get_metric: F,
    ) -> fmt::Result
    where
        B: FmtLabels,
        M: FmtMetric,
        F: Fn(&ClassMetrics) -> &M,
    {
        for (tgt, tm) in &self.by_target {
            if let Ok(tm) = tm.lock() {
                for (cls, m) in &tm.by_class {
                    let labels = ((&base, tgt), cls);
                    get_metric(&*m).fmt_metric_labeled(f, metric.name, labels)?;
                }
            }
        }

        Ok(())
    }
}
