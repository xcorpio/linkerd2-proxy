use std::fmt;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use linkerd2_metrics::{
    latency,
    Counter,
    FmtLabels,
    FmtMetric,
    FmtMetrics,
    Histogram,
    Metric,
};

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
pub struct Report<B, S>
where
    B: FmtLabels + Clone,
    S: FmtLabels + Hash + Eq,
{
    base: B,
    registry: Arc<Mutex<Registry<S>>>,
}

// ===== impl Report =====

impl<B, S> FmtMetrics for Report<B, S>
where
    B: FmtLabels + Clone,
    S: FmtLabels + Hash + Eq,
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
        fmt_by_target(f, &registry, request_total, &self.base, |s| &s.total)?;

        response_total.fmt_help(f)?;
        fmt_by_class(f, &registry, response_total, &self.base, |s| &s.total)?;

        response_latency_ms.fmt_help(f)?;
        fmt_by_class(f, &registry, response_latency_ms, &self.base, |s| &s.latency)?;

        Ok(())
    }
}

fn fmt_by_target<S, F, B, M>(
    reg: &Registry<S>,
    f: &mut fmt::Formatter,
    metric: Metric<M>,
    base: B,
    get_metric: F
) -> fmt::Result
where
    S: FmtLabels + Hash + Eq,
    F: Fn(&Metrics) -> &M,
    B: FmtLabels,
    M: FmtMetric,
{
    for (tgt, tm) in &reg.by_target {
        if let Ok(m) = tm.lock() {
            let labels = (&base, tgt);
            get_metric(&*m).fmt_metric_labeled(f, metric.name, labels)?;
        }
    }

    Ok(())
}

fn fmt_by_class<S, F, B, M>(
    reg: &Registry<S>,
    f: &mut fmt::Formatter,
    metric: Metric<M>,
    base: B,
    get_metric: F
) -> fmt::Result
where
    S: FmtLabels + Hash + Eq,
    F: Fn(&ClassMetrics) -> &M,
    B: FmtLabels,
    M: FmtMetric,
{
    for (tgt, tm) in &reg.by_target {
        if let Ok(tm) = tm.lock() {
            for (cls, m) in &tm.by_class {
                let labels = ((&base, tgt), cls);
                get_metric(&*m).fmt_metric_labeled(f, metric.name, labels)?;
            }
        }
    }

    Ok(())
}
