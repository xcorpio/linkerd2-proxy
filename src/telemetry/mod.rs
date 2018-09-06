use linkerd2_metrics as metrics;

mod errno;
pub mod process;
mod report;
pub mod tls_config_reload;

pub use self::errno::Errno;
pub use self::report::Report;

pub type ServeMetrics = metrics::Serve<Report>;
