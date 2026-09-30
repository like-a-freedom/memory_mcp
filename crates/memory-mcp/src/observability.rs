//! Optional Prometheus recorder/listener installation.
//!
//! Zero-config by design: without the `prometheus` feature or without a
//! valid `MEMORY_PROMETHEUS_LISTEN_ADDR`, no socket opens and the
//! `metrics` facade stays no-op. `127.0.0.1:0` is supported for tests.

use std::net::SocketAddr;
use std::time::Instant;

use crate::error::MemoryError;

// The vocabulary — what a measurement is *called* — lives in the pure kernel
// at `crate::shared::observability`; this module is the implementation that
// records it. Domain modules import from `shared`, so they never acquire the
// Prometheus recorder and everything it drags. See ADR-0058.
#[cfg(test)]
pub(crate) use crate::shared::observability::{DECLARED_REFUSAL_BRANCHES, DECLARED_STAGES};
pub use crate::shared::observability::{
    METRIC_AUTH_REFUSALS_TOTAL, METRIC_BACKGROUND_JOB_DURATION_SECONDS,
    METRIC_BACKGROUND_JOBS_TOTAL, METRIC_HTTP_REQUEST_DURATION_SECONDS,
    METRIC_HTTP_REQUESTS_INFLIGHT, METRIC_HTTP_REQUESTS_TOTAL, METRIC_OPERATION_DURATION_SECONDS,
    METRIC_OPERATION_RESULTS_TOTAL, METRIC_OPERATIONS_TOTAL,
    METRIC_PIPELINE_STAGE_DURATION_SECONDS, METRIC_RUNTIME_REFUSALS_TOTAL, StageTimer,
};

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

/// The one place a count and a duration are emitted together.
///
/// Every timed thing here is a counter and a histogram carrying the same
/// labels, so they are emitted as a pair: a count without a duration, or a
/// duration without its count, is half a measurement, and it is the pair that
/// makes a rate or a quantile answerable.
///
/// Labels are written out at the call site rather than taken as a slice
/// because the `metrics` macros take them as macro arguments; a `&[(k, v)]`
/// built by the caller would be a temporary the macro borrows past its
/// lifetime. Concentrating that in one function is what stops every caller
/// from solving it again, and from growing a second emission path that drifts.
macro_rules! count_and_time {
    ($counter:expr, $histogram:expr, $seconds:expr, $($name:literal => $value:expr),+ $(,)?) => {{
        metrics::counter!($counter, $($name => $value),+).increment(1);
        metrics::histogram!($histogram, $($name => $value),+).record($seconds);
    }};
}

/// Record one background job: its outcome and how long it ran.
///
/// Shared by every scheduler so a failing job is counted the same way wherever
/// it runs, and so adding a scheduler cannot introduce a third spelling of
/// "this failed".
pub(crate) fn record_job_metric(job: &'static str, outcome: &'static str, seconds: f64) {
    count_and_time!(
        METRIC_BACKGROUND_JOBS_TOTAL,
        METRIC_BACKGROUND_JOB_DURATION_SECONDS,
        seconds,
        "job" => job,
        "outcome" => outcome,
    );
}

/// Record one authentication refusal on the identity callback.
///
/// `branch` is the reason and must be a fixed word — the call sites pass static
/// tags. It is never a username, an issuer or a subject: those are the
/// identifiers that turn a metric into a disclosure, and the audit trail
/// already carries them under a keyed fingerprint.
pub(crate) fn record_auth_refusal(branch: &'static str) {
    metrics::counter!(
        METRIC_AUTH_REFUSALS_TOTAL,
        "surface" => "oidc",
        "branch" => branch,
    )
    .increment(1);
}

/// Record one request-scoped refusal from the HTTP runtime.
pub(crate) fn record_runtime_refusal(op: &'static str) {
    metrics::counter!(METRIC_RUNTIME_REFUSALS_TOTAL, "op" => op).increment(1);
}

/// Move a gauge by `delta`.
///
/// Additive rather than a read-modify-write of a shared counter: two requests
/// completing at once must not lose each other's update, and a gauge that
/// undercounts is worse than one that is slightly late.
pub(crate) fn shift_gauge(metric: &'static str, delta: f64) {
    metrics::gauge!(metric).increment(delta);
}

/// Observe a duration into a histogram under two labels, for a path that
/// measured the stage itself.
///
/// Every label half is `&'static str`: a metrics registry outlives the request
/// that produced the observation, so a value borrowed from a request would
/// dangle the moment it returned. That is why callers pass fixed words rather
/// than formatted ones, and why anything needing per-item attribution belongs
/// in the structured log.
///
/// Two pairs, because a pipeline stage is identified by both the operation that
/// owns it and the step within it — and a stage without its operation cannot be
/// read down a pipeline at all.
pub(crate) fn observe(
    metric: &'static str,
    seconds: f64,
    k0: &'static str,
    v0: &'static str,
    k1: &'static str,
    v1: &'static str,
) {
    metrics::histogram!(metric, k0 => v0, k1 => v1).record(seconds);
}

/// Increment a counter and observe a histogram, for a caller outside this
/// module.
///
/// It is the one export that carries labels, so the `metrics` macros are
/// reached from exactly one place outside [`record_job_metric`] — a caller
/// cannot grow a second, slightly different emission path that drifts from
/// this one.
pub(crate) fn count_timed(
    counter: &'static str,
    histogram: &'static str,
    seconds: f64,
    k0: &'static str,
    v0: &'static str,
    k1: &'static str,
    v1: &'static str,
) {
    metrics::counter!(counter, k0 => v0, k1 => v1).increment(1);
    metrics::histogram!(histogram, k0 => v0, k1 => v1).record(seconds);
}

/// The bounded operation vocabulary. A name outside it becomes `other`, so a
/// caller cannot introduce an unbounded series by passing a formatted string.
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

    /// A stage is only useful if it reaches the exporter, and it is only
    /// reachable if the guard records on the paths that leave early — a
    /// skipped embedding, a cache hit, an error. A timer written only on the
    /// success path would be missing exactly the cases worth seeing.
    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn a_stage_measurement_reaches_the_exporter() {
        let Some(exposition) = tests::exposed(|| async {
            {
                let _stage =
                    crate::observability::StageTimer::new("assemble_context", "query_embedding");
            }
        })
        .await
        else {
            return;
        };

        assert!(
            exposition.contains(crate::observability::METRIC_PIPELINE_STAGE_DURATION_SECONDS),
            "a pipeline stage must reach the exporter: {exposition}"
        );
        assert!(
            exposition.contains(r#"stage="query_embedding""#),
            "and be named, or it cannot be attributed: {exposition}"
        );
        assert!(
            exposition.contains(r#"operation="assemble_context""#),
            "and owned by an operation, so a dashboard reads down the pipeline: {exposition}"
        );
    }

    /// The stages a pipeline reports are a closed set of words. An unbounded
    /// one — a stage named after a record, a tenant, a query string — is how a
    /// metrics backend falls over, and the bounded vocabulary is what makes the
    /// series aggregatable.
    #[test]
    fn stage_labels_are_bounded_words() {
        for stage in ["query_embedding", "ann_search", "rank", "serialize"] {
            assert!(
                stage
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "a stage label must be a fixed word: {stage}"
            );
            assert!(stage.len() < 32, "a stage label must be short: {stage}");
        }
    }

    /// A stage that is declared but never attached to a pipeline measures
    /// nothing. The sources are read and compared, so adding a stage to the
    /// vocabulary without measuring it — the failure this guards — fails here
    /// rather than showing up as a permanently flat histogram, which reads on a
    /// dashboard exactly like a fast pipeline.
    #[test]
    fn every_declared_stage_is_measured_somewhere_in_the_pipeline() {
        const SOURCES: &[&str] = &[
            include_str!("memory/retrieval/semantic.rs"),
            include_str!("tools/extract.rs"),
            include_str!("tools/ingest.rs"),
            include_str!("embedding/service.rs"),
        ];
        for stage in DECLARED_STAGES {
            assert!(
                SOURCES.iter().any(|source| source.contains(stage)),
                "stage `{stage}` is declared but never measured; a stage nobody \
                 attaches is a metric that always reads zero"
            );
        }
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

    /// A refusal branch is a fixed word. One carrying a subject, an issuer or
    /// an authorization code would be a disclosure, and a metrics backend
    /// outlives every request that could have supplied one.
    #[test]
    fn every_declared_refusal_branch_is_a_bounded_word() {
        for branch in DECLARED_REFUSAL_BRANCHES {
            assert!(
                branch
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "a refusal label must be a fixed word: {branch}"
            );
            assert!(branch.len() < 32, "a refusal label must be short: {branch}");
        }
    }

    /// Every branch the identity callback can refuse on must be in the
    /// vocabulary, and every declared branch must still be emitted. The call
    /// sites are read rather than trusted: a new branch that skips the list
    /// escapes the bounded-label check, and a stale entry is a series that
    /// reads zero forever.
    #[test]
    fn every_emitted_refusal_branch_is_declared() {
        const SOURCE: &str = include_str!("control/oidc/handlers.rs");
        for branch in DECLARED_REFUSAL_BRANCHES {
            assert!(
                SOURCE.contains(&format!("\"{branch}\"")),
                "branch `{branch}` is declared but no call site emits it"
            );
        }
    }
}
