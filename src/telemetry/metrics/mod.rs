//! Records and serves Prometheus metrics.

use std::fmt;

mod counter;
mod gauge;
mod histogram;
pub mod latency;
pub mod prom;
mod scopes;
mod serve;

pub use self::counter::Counter;
pub use self::gauge::Gauge;
pub use self::histogram::Histogram;
pub use self::prom::{FmtMetrics, FmtLabels, FmtMetric};
pub use self::scopes::Scopes;
pub use self::serve::Serve;
use super::{http, process, tls_config_reload, transport};

/// The root scope for all runtime metrics.
#[derive(Clone, Debug, Default)]
pub struct Report {
    http: http::Report,
    transports: transport::Report,
    tls_config_reload: tls_config_reload::Report,
    process: process::Report,
}

// ===== impl Report =====

impl Report {
    pub(super) fn new(
        http: http::Report,
        transports: transport::Report,
        tls_config_reload: tls_config_reload::Report,
        process: process::Report,
    ) -> Self {
        Self {
            http,
            transports,
            tls_config_reload,
            process,
        }
    }
}

// ===== impl Report =====

impl FmtMetrics for Report {
    fn fmt_metrics(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.http.fmt_metrics(f)?;
        self.transports.fmt_metrics(f)?;
        self.tls_config_reload.fmt_metrics(f)?;
        self.process.fmt_metrics(f)?;

        Ok(())
    }
}
