use indexmap::IndexMap;
use std::fmt;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

pub fn new<Base, Svc>(base: Base) -> (Make<Svc>, Report<Base, Svc>)
where
    Base: FmtLabels + Clone,
    Svc: FmtLabels + Hash + Eq,
{
    let metrics = Default::default();
    (Make::new(metrics.clone()), Report { base, metrics })
}

/// Reports HTTP metrics for prometheus.
#[derive(Clone, Debug)]
pub struct Report<Base, Svc>
where
    Base: FmtLabels + Clone,
    Svc: FmtLabels + Hash + Eq,
{
    base: Base,
    metrics: Arc<Mutex<Metrics<Svc>>>
}

#[derive(Debug, Default)]
struct Metrics<Svc>
where
    Svc: FmtLabels + Hash + Eq,
{
    by_service: IndexMap<Svc, Arc<Mutex<ServiceMetrics>>>,
}

#[derive(Debug, Default)]
struct ServiceMetrics
where
{
    total: Counter,
    by_class: IndexMap<class::Class, ClassMetrics>,
}

#[derive(Debug, Default)]
pub struct ClassMetrics {
    total: Counter,
    latency: Histogram<latency::Ms>,
}

// ===== impl Metrics =====

impl<S> Metrics<S>
where
    S: FmtLabels + Hash + Eq,
{
     fn is_empty(&self) -> bool {
        self.by_service.is_empty()
    }

    fn fmt_by_service<F, B, M>(&self, f: &mut fmt::Formatter, metric: Metric<M>, base: B, get_metric: F)
        -> fmt::Result
    where
        F: Fn(&ServiceMetrics) -> &M,
        B: FmtLabels,
        M: FmtMetric,
    {
        for (svc, sm) in &self.by_service {
            if let Ok(m) = sm.lock() {
                let labels = (base, svc);
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
        for (svc, sm) in &self.by_service {
            if let Ok(sm) = sm.lock() {
                for (tgt, tm) in &sm.by_target {
                    for (cls, m) in &sm.by_class {
                        let labels = ((base, svc), cls);
                        get_metric(&*m).fmt_metric_labeled(f, metric.name, labels)?;
                    }
                }
            }
        }

        Ok(())
    }
}

// ===== impl Report =====

impl<B, S> FmtMetrics for Report<B, S> {
    fn fmt_metrics(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let metrics = match self.metrics.lock() {
            Err(_) => return Ok(()),
            Ok(m) => m,
        };

        if metrics.is_empty() {
            return Ok(());
        }

        request_total.fmt_help(f)?;
        metrics.fmt_by_service(f, request_total, &self.base, |s| &s.total)?;

        response_total.fmt_help(f)?;
        metrics.fmt_by_class(f, request_total, &self.base, |s| &s.total)?;

        response_latency_ms.fmt_help(f)?;
        metrics.fmt_by_class(f, request_total, &self.base, |s| &s.latency)?;

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
