use indexmap::IndexMap;
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

mod class;
mod service;

pub use self::service::{Make, NewMeasure, Measure};

metrics! {
    request_total: Counter { "Total count of HTTP requests." },
    response_total: Counter { "Total count of HTTP responses" },
    response_latency_ms: Histogram<latency::Ms> {
        "Elapsed times between a request's headers being received \
        and its response stream completing"
    }
}

pub fn new<B, S>(base: B) -> (Make<S>, Report<B, S>)
where
    B: FmtLabels + Clone,
    S: FmtLabels + Clone + Hash + Eq,
{
    let metrics = Arc::new(Mutex::new(Metrics::<S>::default()));
    (Make::new(metrics.clone()), Report { base, metrics })
}

/// Reports HTTP metrics for prometheus.
#[derive(Clone, Debug)]
pub struct Report<B, S>
where
    B: FmtLabels + Clone,
    S: FmtLabels + Hash + Eq,
{
    base: B,
    metrics: Arc<Mutex<Metrics<S>>>,
}

#[derive(Debug)]
struct Metrics<T: FmtLabels + Hash + Eq> {
    by_target: IndexMap<T, Arc<Mutex<TargetMetrics>>>,
}

#[derive(Debug, Default)]
struct TargetMetrics {
    total: Counter,
    by_class: IndexMap<class::Class, ClassMetrics>,
}

#[derive(Debug, Default)]
pub struct ClassMetrics {
    total: Counter,
    latency: Histogram<latency::Ms>,
}

// ===== impl Metrics =====

impl<S> Default for Metrics<S>
where
    S: FmtLabels + Hash + Eq,
{
    fn default() -> Self {
        Metrics { by_target: IndexMap::default() }
    }
}

impl<S> Metrics<S>
where
    S: FmtLabels + Hash + Eq,
{
     fn is_empty(&self) -> bool {
        self.by_target.is_empty()
    }

    fn fmt_by_target<F, B, M>(&self, f: &mut fmt::Formatter, metric: Metric<M>, base: B, get_metric: F)
        -> fmt::Result
    where
        F: Fn(&TargetMetrics) -> &M,
        B: FmtLabels,
        M: FmtMetric,
    {
        for (tgt, tm) in &self.by_target {
            if let Ok(m) = tm.lock() {
                let labels = (&base, tgt);
                get_metric(&*m).fmt_metric_labeled(f, metric.name, labels)?;
            }
        }

        Ok(())
    }

    fn fmt_by_class<F, B, M>(&self, f: &mut fmt::Formatter, metric: Metric<M>, base: B, get_metric: F)
        -> fmt::Result
    where
        F: Fn(&ClassMetrics) -> &M,
        B: FmtLabels,
        M: FmtMetric,
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

// ===== impl Report =====

impl<B, S> FmtMetrics for Report<B, S>
where
    B: FmtLabels + Clone,
    S: FmtLabels + Hash + Eq,
{

    fn fmt_metrics(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let metrics = match self.metrics.lock() {
            Err(_) => return Ok(()),
            Ok(m) => m,
        };

        if metrics.is_empty() {
            return Ok(());
        }

        request_total.fmt_help(f)?;
        metrics.fmt_by_target(f, request_total, &self.base, |s| &s.total)?;

        response_total.fmt_help(f)?;
        metrics.fmt_by_class(f, response_total, &self.base, |s| &s.total)?;

        response_latency_ms.fmt_help(f)?;
        metrics.fmt_by_class(f, response_latency_ms, &self.base, |s| &s.latency)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use ctx;
    use ctx::test_util::*;
    use super::*;
    use conditional::Conditional;
    use tls;

    const TLS_DISABLED: Conditional<(), tls::ReasonForNoTls> =
        Conditional::None(tls::ReasonForNoTls::Disabled);

    fn mock_route(
        registry: &mut Metrics,
        proxy: ctx::Proxy,
        server: &Arc<ctx::transport::Server>,
        team: &str
    ) {
        let client = client(proxy, indexmap!["team".into() => team.into(),], TLS_DISABLED);
        let (req, rsp) = request("http://nba.com", &server, &client);
        registry.end_request(RequestLabels::new(&req));
        registry.end_response(ResponseLabels::new(&rsp, None), Duration::from_millis(10));
   }

    #[test]
    fn expiry() {
        let proxy = ctx::Proxy::Outbound;

        let server = server(proxy, TLS_DISABLED);

        let inner = Arc::new(Mutex::new(Metrics::default()));
        let mut registry = Metrics(inner.clone());

        let t0 = Instant::now();

        mock_route(&mut registry, proxy, &server, "warriors");
        let t1 = Instant::now();

        mock_route(&mut registry, proxy, &server, "sixers");
        let t2 = Instant::now();

        let mut inner = inner.lock().unwrap();
        assert_eq!(inner.requests.len(), 2);
        assert_eq!(inner.responses.len(), 2);

        inner.retain_since(t0);
        assert_eq!(inner.requests.len(), 2);
        assert_eq!(inner.responses.len(), 2);

        inner.retain_since(t1);
        assert_eq!(inner.requests.len(), 1);
        assert_eq!(inner.responses.len(), 1);

        inner.retain_since(t2);
        assert_eq!(inner.requests.len(), 0);
        assert_eq!(inner.responses.len(), 0);
    }
}
