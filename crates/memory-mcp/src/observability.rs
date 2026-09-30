//! Optional Prometheus recorder/listener installation.
//!
//! Zero-config by design: without the `prometheus` feature or without a
//! valid `MEMORY_PROMETHEUS_LISTEN_ADDR`, no socket opens and the
//! `metrics` facade stays no-op. `127.0.0.1:0` is supported for tests.

use std::net::SocketAddr;
use std::time::Instant;

use crate::error::MemoryError;

/// Total logical operations by bounded operation and outcome.
pub const METRIC_OPERATIONS_TOTAL: &str = "memory_operation_calls_total";
/// Logical operation duration in seconds by bounded operation and outcome.
pub const METRIC_OPERATION_DURATION_SECONDS: &str = "memory_operation_duration_seconds";
/// Bounded domain result counts by operation and result kind.
pub const METRIC_OPERATION_RESULTS_TOTAL: &str = "memory_operation_results_total";

/// Filesystem-watch metric family: revision outcomes.
pub const METRIC_FS_WATCH_REVISIONS_TOTAL: &str = "memory_fs_watch_revisions_total";
/// Filesystem-watch metric family: retry counts by bounded stage and reason.
pub const METRIC_FS_WATCH_RETRIES_TOTAL: &str = "memory_fs_watch_retries_total";
/// Filesystem-watch metric family: startup-scan file outcomes.
pub const METRIC_FS_WATCH_SCAN_FILES_TOTAL: &str = "memory_fs_watch_scan_files_total";
/// Filesystem-watch gauge: claimable queue depth.
pub const METRIC_FS_WATCH_QUEUE_DEPTH: &str = "memory_fs_watch_queue_depth";
/// Filesystem-watch gauge: in-flight revisions.
pub const METRIC_FS_WATCH_INFLIGHT: &str = "memory_fs_watch_inflight";
/// Filesystem-watch gauge: degraded watcher state.
pub const METRIC_FS_WATCH_DEGRADED: &str = "memory_fs_watch_degraded";
/// Filesystem-watch histogram: revision duration by bounded outcome.
pub const METRIC_FS_WATCH_REVISION_DURATION_SECONDS: &str =
    "memory_fs_watch_revision_duration_seconds";

/// HTTP counter: requests served, by method class and status class.
///
/// The first golden signal for a service whose only entry point is HTTP. It is
/// a counter rather than something derived from the access log because a log
/// line is sampled and unstructured: answering "how much traffic did this
/// deployment serve" from logs means parsing them, and the answer is wrong the
/// moment a line is dropped.
pub const METRIC_HTTP_REQUESTS_TOTAL: &str = "memory_http_requests_total";

/// HTTP histogram: request duration in seconds, by method and status class.
///
/// Prometheus's own convention is that the latency histogram carries a
/// `le` label; the buckets this recorder installs are the default set, which
/// spans 5 ms to 10 s and covers everything from an in-memory hit to a
/// database round trip.
pub const METRIC_HTTP_REQUEST_DURATION_SECONDS: &str = "memory_http_request_duration_seconds";

/// HTTP gauge: requests currently in flight.
///
/// The saturation signal for this service. A count that only rises and falls
/// between scrapes is invisible, and "the queue is growing" is the question an
/// on-call engineer asks before anything else when latency climbs.
pub const METRIC_HTTP_REQUESTS_INFLIGHT: &str = "memory_http_requests_inflight";

/// Counter: background job outcomes, by job family and outcome.
///
/// A background job is the one failure class with no request attached: it
/// cannot be seen in a status code, and it does not appear in the request
/// metrics because it is not a request. Logged alone, a failing job is a line
/// somebody has to be reading the right log to notice. Counted, it is an alert
/// on a rate, which is how it should be found.
pub const METRIC_BACKGROUND_JOBS_TOTAL: &str = "memory_background_jobs_total";

/// Histogram: background job duration in seconds, by job family.
pub const METRIC_BACKGROUND_JOB_DURATION_SECONDS: &str = "memory_background_job_duration_seconds";

const KNOWN_OPERATIONS: &[&str] = &[
    "ingest",
    "extract",
    "resolve",
    "assemble_context",
    "explain",
    "invalidate",
    "lifecycle_dashboard",
    "lifecycle_archive_candidates",
    "lifecycle_restore_archived",
    "lifecycle_recompute_decay",
    "lifecycle_rebuild_communities",
];

const KNOWN_RESULTS: &[&str] = &[
    "episodes",
    "entities",
    "facts",
    "links",
    "warnings",
    "items",
    "explanations",
    "invalidations",
    "archived",
    "restored",
    "decay_invalidated",
    "communities",
    "active_facts",
    "archival_candidates",
];

fn operation_label(operation: &str) -> &'static str {
    KNOWN_OPERATIONS
        .iter()
        .copied()
        .find(|known| *known == operation)
        .unwrap_or("other")
}

fn result_label(result: &str) -> &'static str {
    KNOWN_RESULTS
        .iter()
        .copied()
        .find(|known| *known == result)
        .unwrap_or("other")
}

/// Record one background job: its outcome and how long it ran.
///
/// Shared by every scheduler so a failing job is counted the same way
/// wherever it runs, and so adding a scheduler cannot introduce a third
/// spelling of "this failed".
pub(crate) fn record_job_metric(job: &'static str, outcome: &'static str, seconds: f64) {
    const JOBS: &str = METRIC_BACKGROUND_JOBS_TOTAL;
    const DURATION: &str = METRIC_BACKGROUND_JOB_DURATION_SECONDS;
    metrics::counter!(
        JOBS,
        "job" => job,
        "outcome" => outcome,
    )
    .increment(1);
    metrics::histogram!(
        DURATION,
        "job" => job,
        "outcome" => outcome,
    )
    .record(seconds);
}

/// Records one logical operation when dropped.
///
/// The default outcome is `error`, which makes early returns and unexpected
/// failures observable without requiring every call site to duplicate a match
/// arm. Successful paths explicitly mark the operation as successful.
pub(crate) struct OperationMetrics {
    operation: &'static str,
    started_at: Instant,
    outcome: &'static str,
}

impl OperationMetrics {
    pub(crate) fn new(operation: &'static str) -> Self {
        Self {
            operation: operation_label(operation),
            started_at: Instant::now(),
            outcome: "error",
        }
    }

    pub(crate) fn success(&mut self) {
        self.outcome = "success";
    }

    pub(crate) fn record_result(&self, result: &str, count: usize) {
        metrics::counter!(
            METRIC_OPERATION_RESULTS_TOTAL,
            "operation" => self.operation,
            "result" => result_label(result),
        )
        .increment(count as u64);
    }
}

impl Drop for OperationMetrics {
    fn drop(&mut self) {
        let duration = self.started_at.elapsed();
        metrics::counter!(
            METRIC_OPERATIONS_TOTAL,
            "operation" => self.operation,
            "outcome" => self.outcome,
        )
        .increment(1);
        metrics::histogram!(
            METRIC_OPERATION_DURATION_SECONDS,
            "operation" => self.operation,
            "outcome" => self.outcome,
        )
        .record(duration.as_secs_f64());
    }
}

/// Environment variable that opts into a Prometheus HTTP listener.
pub const ENV_PROMETHEUS_LISTEN_ADDR: &str = "MEMORY_PROMETHEUS_LISTEN_ADDR";

/// Install the Prometheus recorder/listener when the feature is enabled
/// and `MEMORY_PROMETHEUS_LISTEN_ADDR` is set.
///
/// Without the feature, this is a no-op. With the feature but without the
/// env var, the recorder stays unset and no socket opens. With both, a
/// duplicate recorder or invalid address is a startup error.
pub fn install() -> Result<(), MemoryError> {
    #[cfg(feature = "prometheus")]
    {
        let Some(addr) = parse_listen_addr()? else {
            return Ok(());
        };
        install_with_addr(addr)?;
    }
    Ok(())
}

/// Process-wide handle to the installed Prometheus recorder.
/// First call installs the recorder; subsequent calls return the
/// same handle. Returns `None` when the `prometheus` feature is off.
///
/// Use this from test fixtures and from the HTTP composition root
/// to avoid the double-install panic.
#[cfg(feature = "prometheus")]
pub fn shared_test_handle() -> Option<metrics_exporter_prometheus::PrometheusHandle> {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<Option<metrics_exporter_prometheus::PrometheusHandle>> =
        OnceLock::new();
    HANDLE
        .get_or_init(|| {
            metrics_exporter_prometheus::PrometheusBuilder::new()
                .install_recorder()
                .ok()
        })
        .clone()
}

#[cfg(feature = "prometheus")]
fn parse_listen_addr() -> Result<Option<SocketAddr>, MemoryError> {
    match std::env::var(ENV_PROMETHEUS_LISTEN_ADDR) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let addr: SocketAddr = trimmed.parse().map_err(|_| {
                MemoryError::ConfigInvalid(format!(
                    "{ENV_PROMETHEUS_LISTEN_ADDR}='{trimmed}' is not a valid SocketAddr (use ip:port, e.g. 127.0.0.1:9100)"
                ))
            })?;
            Ok(Some(addr))
        }
        Err(_) => Ok(None),
    }
}

#[cfg(feature = "prometheus")]
fn install_with_addr(addr: SocketAddr) -> Result<(), MemoryError> {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .with_http_listener(addr)
        .install()
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!(
                "failed to install Prometheus exporter on {addr}: {err}"
            ))
        })?;
    Ok(())
}

#[cfg(not(feature = "prometheus"))]
#[allow(dead_code)]
fn parse_listen_addr() -> Result<Option<SocketAddr>, MemoryError> {
    Ok(None)
}

#[cfg(not(feature = "prometheus"))]
#[allow(dead_code)]
fn install_with_addr(_addr: SocketAddr) -> Result<(), MemoryError> {
    Ok(())
}

// Gated on the same feature as the exporter it reads: reading an exposition
// needs the exporter crate, and the `prometheus` profile is the only build in
// which a metric is scrapeable at all. Outside it there is nothing to assert.
#[cfg(all(test, feature = "prometheus"))]
pub(crate) mod tests {
    use super::*;

    /// A recorder installed into a fresh registry, and the exposition text it
    /// renders.
    ///
    /// The `metrics` facade is process-global, so a test that installs a
    /// recorder cannot do it twice. Every caller goes through [`Recorder`]
    /// instead, which reports clearly when another test already owns it rather
    /// than silently recording into a recorder nobody reads — a failure mode
    /// that would make every metric assertion below vacuously true.
    pub(crate) struct Recorder {
        handle: metrics_exporter_prometheus::PrometheusHandle,
    }

    impl Recorder {
        pub(crate) fn install() -> Option<Self> {
            // The exporter crate, not the `metrics` facade: the handle that can
            // render an exposition is the exporter crate's type, and the facade
            // does not re-export it. The helper is therefore gated on the same
            // feature as the exporter it reads, which is also the only
            // configuration in which a metric is scrapeable at all.
            let exporter = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let handle = exporter.handle().clone();
            metrics::set_global_recorder(exporter).ok()?;
            Some(Self { handle })
        }

        /// The current exposition, as a scrape would see it.
        pub(crate) fn render(&self) -> String {
            self.handle.render()
        }
    }

    /// Install a recorder, run `body`, and return the exposition it produced.
    ///
    /// `None` when another test holds the global recorder, which is not a
    /// failure: the metric is still exercised, it simply cannot be read back
    /// in the same process.
    pub(crate) async fn exposed<F, Fut>(body: F) -> Option<String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let recorder = Recorder::install()?;
        body().await;
        Some(recorder.render())
    }

    /// The value of a counter or gauge in the exposition, by metric name.
    pub(crate) fn sample(exposition: &str, metric: &str) -> Option<f64> {
        exposition
            .lines()
            .find(|line| {
                line.starts_with(metric)
                    && !line.starts_with(&format!("{metric}_bucket"))
                    && !line.starts_with(&format!("{metric}_sum"))
                    && !line.starts_with(&format!("{metric}_count"))
            })
            .and_then(|line| line.split_whitespace().last())
            .and_then(|value| value.parse().ok())
    }

    #[test]
    fn env_var_name_is_stable() {
        assert_eq!(ENV_PROMETHEUS_LISTEN_ADDR, "MEMORY_PROMETHEUS_LISTEN_ADDR");
    }

    #[test]
    fn operation_labels_are_bounded() {
        assert_eq!(operation_label("ingest"), "ingest");
        assert_eq!(operation_label("request:abc"), "other");
    }

    #[test]
    fn result_labels_are_bounded() {
        assert_eq!(result_label("facts"), "facts");
        assert_eq!(result_label("fact:abc"), "other");
    }

    #[test]
    fn operation_metrics_are_safe_without_recorder() {
        let mut metrics = OperationMetrics::new("ingest");
        metrics.record_result("episodes", 1);
        metrics.success();
    }

    #[test]
    #[cfg(feature = "prometheus")]
    fn operation_metrics_emit_expected_families() {
        // Shares the OnceLock with `http::HttpState::test_metrics_handle`
        // so the recorder installs at most once per process.
        let handle = super::shared_test_handle().expect("prometheus enabled");

        let mut metrics = OperationMetrics::new("ingest");
        metrics.record_result("episodes", 2);
        metrics.success();
        drop(metrics);

        let output = handle.render();
        assert!(output.contains("memory_operation_calls_total{"));
        assert!(output.contains("operation=\"ingest\""));
        assert!(output.contains("outcome=\"success\""));
        assert!(output.contains("memory_operation_duration_seconds"));
        assert!(output.contains("memory_operation_results_total{"));
        assert!(output.contains("result=\"episodes\""));
    }

    #[test]
    #[cfg(not(feature = "prometheus"))]
    fn install_is_noop_without_feature() {
        // Without the feature, install always succeeds and never opens a socket.
        install().expect("install succeeds without prometheus feature");
    }
}
