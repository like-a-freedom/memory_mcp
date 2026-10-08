//! Optional Prometheus recorder/listener installation.
//!
//! Zero-config by design: without the `prometheus` feature or without a
//! valid `MEMORY_PROMETHEUS_LISTEN_ADDR`, no socket opens and the
//! `metrics` facade stays no-op. `127.0.0.1:0` is supported for tests.

#[cfg(feature = "prometheus")]
use std::net::SocketAddr;
use std::time::Instant;

use crate::error::MemoryError;

// The vocabulary — what a measurement is *called* — lives in the pure kernel
// at `crate::shared::observability`; this module is the implementation that
// records it. Domain modules import from `shared`, so they never acquire the
// Prometheus recorder and everything it drags. See ADR-0058.
#[cfg(all(test, feature = "prometheus"))]
pub(crate) use crate::shared::observability::{DECLARED_REFUSAL_BRANCHES, DECLARED_STAGES};
pub use crate::shared::observability::{
    METRIC_AUTH_REFUSALS_TOTAL, METRIC_AUTH_SIGNINS_TOTAL, METRIC_BACKGROUND_JOB_DURATION_SECONDS,
    METRIC_BACKGROUND_JOB_LAST_RUN_TIMESTAMP_SECONDS, METRIC_BACKGROUND_JOBS_TOTAL,
    METRIC_BUILD_INFO, METRIC_HTTP_REQUEST_DURATION_SECONDS, METRIC_HTTP_REQUESTS_INFLIGHT,
    METRIC_HTTP_REQUESTS_TOTAL, METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS,
    METRIC_OPERATION_DURATION_SECONDS, METRIC_OPERATION_RESULTS_TOTAL, METRIC_OPERATION_STOCK,
    METRIC_OPERATIONS_TOTAL, METRIC_PIPELINE_STAGE_DURATION_SECONDS, METRIC_RUNTIME_REFUSALS_TOTAL,
    StageTimer,
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

/// Record one background job: its outcome and how long it ran.
///
/// Shared by every scheduler so a failing job is counted the same way wherever
/// it runs, and so adding a scheduler cannot introduce a third spelling of
/// "this failed".
///
/// The label is `pass`, not `job`, and that is not a style choice. `job` is the
/// collector's own label: a scrape of this exposition with `job_name:
/// memory_mcp` puts `job="memory_mcp"` on every series and renames the
/// exposition's `job` to `exported_job`. Every `by (job)` aggregation would
/// then collapse to the one scrape target, and every `{job="lease"}` filter
/// would match nothing — silently, with the dashboards still rendering. The
/// scheduler dimension is called `pass` here because that is what the code
/// calls a run of one.
#[cfg_attr(
    not(feature = "prometheus"),
    allow(
        dead_code,
        reason = "recorders have no caller without the HTTP profile"
    )
)]
pub(crate) fn record_job_metric(pass: &'static str, outcome: &'static str, seconds: f64) {
    // Through `count_timed`, like every other counter-and-histogram pair. A
    // macro used to live here as well, so a timed thing had two spellings to
    // choose from — one that emitted the pair itself, one that delegated — and a
    // pair emitted by a new caller could pick the wrong one and drift from the
    // single emission path the rest of the file shares.
    count_timed(
        METRIC_BACKGROUND_JOBS_TOTAL,
        METRIC_BACKGROUND_JOB_DURATION_SECONDS,
        seconds,
        &[("pass", pass), ("outcome", outcome)],
    );
    // Stamped here rather than at the schedulers, because every scheduler
    // already reports through this one function and a per-scheduler call is a
    // fourth thing to remember when a new one is added.
    //
    // `degraded` stamps as well as `ok`: a degraded pass walked its tenants and
    // some of them failed, which is a run — and its degradation is what the
    // outcome label and the unhealthy rate are for. Only a pass that failed
    // outright leaves the clock alone.
    #[cfg(feature = "prometheus")]
    if outcome != "error" {
        metrics::gauge!(
            METRIC_BACKGROUND_JOB_LAST_RUN_TIMESTAMP_SECONDS,
            "pass" => pass,
        )
        .set(unix_seconds());
    }
}

/// Record one authentication refusal on the identity callback.
///
/// `branch` is the reason and must be a fixed word — the call sites pass static
/// tags. It is never a username, an issuer or a subject: those are the
/// identifiers that turn a metric into a disclosure, and the audit trail
/// already carries them under a keyed fingerprint.
#[cfg_attr(
    not(feature = "prometheus"),
    allow(
        dead_code,
        reason = "recorders have no caller without the HTTP profile"
    )
)]
pub(crate) fn record_auth_refusal(surface: &'static str, branch: &'static str) {
    metrics::counter!(
        METRIC_AUTH_REFUSALS_TOTAL,
        "surface" => surface,
        "branch" => branch,
    )
    .increment(1);
}

/// Record one request-scoped refusal from the HTTP runtime.
#[cfg_attr(
    not(feature = "prometheus"),
    allow(
        dead_code,
        reason = "recorders have no caller without the HTTP profile"
    )
)]
pub(crate) fn record_runtime_refusal(op: &'static str) {
    metrics::counter!(METRIC_RUNTIME_REFUSALS_TOTAL, "op" => op).increment(1);
}

/// Move a gauge by `delta`.
///
/// Additive rather than a read-modify-write of a shared counter: two requests
/// completing at once must not lose each other's update, and a gauge that
/// undercounts is worse than one that is slightly late.
#[cfg_attr(
    not(feature = "prometheus"),
    allow(
        dead_code,
        reason = "recorders have no caller without the HTTP profile"
    )
)]
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
///
/// Takes a slice of alternating key and value rather than positional
/// parameters: the `metrics` macros take labels as token pairs, so a variable
/// count would otherwise need one macro per arity and a nine-argument
/// function that clippy rightly refuses. Every side is `&'static str`, so no
/// label borrows from a request — a registry outlives the request that
/// produced it.
#[cfg_attr(
    not(feature = "prometheus"),
    allow(
        dead_code,
        reason = "recorders have no caller without the HTTP profile"
    )
)]
pub(crate) fn count_timed(
    counter: &'static str,
    histogram: &'static str,
    seconds: f64,
    labels: &[(&'static str, &'static str)],
) {
    // The slice is iterated by reference, so the arms bind references to
    // references; the macros want the values.
    match *labels {
        [(k0, v0), (k1, v1)] => {
            metrics::counter!(counter, k0 => v0, k1 => v1).increment(1);
            metrics::histogram!(histogram, k0 => v0, k1 => v1).record(seconds);
        }
        [(k0, v0), (k1, v1), (k2, v2)] => {
            metrics::counter!(counter, k0 => v0, k1 => v1, k2 => v2).increment(1);
            metrics::histogram!(histogram, k0 => v0, k1 => v1, k2 => v2).record(seconds);
        }
        _ => {
            // A label set this function does not know. Failing loudly beats
            // recording a series with the wrong labels: the two families would
            // drift apart and a panel would read one of them as missing.
            debug_assert!(
                false,
                "a counter and histogram pair needs a known label arity, got {}",
                labels.len()
            );
        }
    }
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

    /// Record work *produced* by this operation: a flow.
    ///
    /// The value is added to a counter, so it answers "how much did this
    /// operation produce" — bytes written, entities extracted, rows
    /// archived. A `rate()` over it is meaningless, and its value grows with
    /// the process lifetime by design.
    ///
    /// For a *stock* — how much exists, which goes up and down — use
    /// [`Self::record_stock`]. A stock added to a counter is a metric that
    /// cannot be read: see that method.
    pub(crate) fn record_result(&self, result: &str, count: usize) {
        metrics::counter!(
            METRIC_OPERATION_RESULTS_TOTAL,
            "operation" => self.operation,
            "result" => result_label(result),
        )
        .increment(count as u64);
    }

    /// Record how much *exists*: a stock.
    ///
    /// Set, not incremented. A stock is a level — ten thousand active facts
    /// is ten thousand whether nobody has looked in a week or a thousand
    /// people have. Adding it to a counter means the metric reports the sum of
    /// every snapshot ever taken: opening the lifecycle dashboard once would
    /// add its whole inventory, and the value would grow with every read
    /// rather than with the data. Its derivative — the only thing anyone would
    /// plot — would report dashboard traffic, not growth.
    ///
    /// The same shape as `claim_relations_active`, which was already a gauge
    /// for exactly this reason.
    pub(crate) fn record_stock(&self, stock: &str, count: usize) {
        metrics::gauge!(
            METRIC_OPERATION_STOCK,
            "operation" => self.operation,
            "result" => result_label(stock),
        )
        .set(count as f64);
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

/// Duration of one rolling summary bucket. The exporter multiplies this by
/// [`summary_buckets`] to get the full quantile window.
///
/// Five 60-second buckets provide the five-minute history claimed by the
/// recording rules while letting observations roll out one minute at a time.
/// Buckets are *not* configured, which would switch the exporter from summaries
/// to `_bucket` exposition process-wide and cost every duration metric its
/// quantile series.
#[cfg(feature = "prometheus")]
pub const SUMMARY_BUCKET_DURATION: std::time::Duration = std::time::Duration::from_secs(60);

/// Number of rolling buckets in each summary's quantile window.
///
/// A function rather than a `const` because `NonZeroU32::new` is not const on
/// this toolchain. The value is fixed at compile time either way, and the
/// call inlines to the same thing.
#[cfg(feature = "prometheus")]
pub fn summary_buckets() -> std::num::NonZeroU32 {
    std::num::NonZeroU32::new(5).expect("5 is non-zero")
}

/// A quantile can only be as old as the window the recorder was given, so the
/// window has to cover the shortest window any rule or panel claims.
///
/// The exporter's own default is three buckets of twenty seconds — about a
/// minute. The recording rules name their latency series `p95_5m`, and
/// alerting on a one-minute quantile under a five-minute name is a lie the
/// panel cannot detect: the percentile decays to nothing a minute after the
/// last request, so a sustained regression reads as a series of spikes, and a
/// long one cannot be reconstructed at all because the observations behind it
/// are already gone.
///
/// Asserted here rather than at the call sites, because there are three
/// `PrometheusBuilder`s in two files and a fourth added later would silently
/// keep the default.
#[test]
#[cfg(feature = "prometheus")]
fn summary_bucket_duration_and_count_make_the_claimed_five_minute_window() {
    assert_eq!(
        SUMMARY_BUCKET_DURATION,
        std::time::Duration::from_secs(60),
        "each summary bucket rolls out after one minute"
    );
    assert_eq!(
        summary_buckets().get(),
        5,
        "the recording rules require a five-minute rolling window"
    );
    assert_eq!(
        SUMMARY_BUCKET_DURATION * summary_buckets().get(),
        std::time::Duration::from_secs(300),
        "bucket duration is multiplied by bucket count exactly once"
    );
}

/// Register every family's description with the installed recorder.
///
/// Called once per profile, right after the recorder is installed. Registering
/// is separate from recording because the recorder is global: by the time the
/// first metric is emitted the description is already needed, and a description
/// registered after a series exists only fills in for the next render.
#[cfg(feature = "prometheus")]
pub fn describe_metrics() {
    use crate::shared::observability::{DESCRIPTIONS, MetricKind};

    for described in DESCRIPTIONS {
        // Typed explicitly: `.into()` alone is ambiguous here, because the
        // two macro forms below each accept a different concrete type.
        let help: metrics::SharedString = described.help.into();
        // Two macro forms: the three-argument one takes a `Unit` outright, the
        // two-argument one means "dimensionless". There is no `Option<Unit>`
        // form, so the choice is made here rather than by the dictionary.
        match (described.kind, described.unit) {
            // The unit is dropped on purpose for the first two, because
            // dimensionless is a rule over the dictionary rather than a per-family
            // choice: `only_histograms_carry_a_unit` pins it across every entry,
            // so a counter that later grows a unit fails that test instead of
            // being described here without the unit nobody should have added.
            (MetricKind::Counter, _) => {
                metrics::describe_counter!(described.name, help);
            }
            (MetricKind::Gauge, _) => {
                metrics::describe_gauge!(described.name, help);
            }
            (MetricKind::Histogram, Some(unit)) => {
                metrics::describe_histogram!(described.name, unit, help);
            }
            (MetricKind::Histogram, None) => {
                metrics::describe_histogram!(described.name, help);
            }
        }
    }
}

/// Stamp the exposition with the build it came from.
///
/// Recorded wherever [`describe_metrics`] is, so every profile that describes
/// its metrics also identifies the build: `memory_build_info` is how an
/// operator reading a dashboard or firing alert knows *which version* these
/// numbers came from, and it is what makes a partial rollout visible as two
/// versions rather than as one doubled set of series with no explanation.
///
/// A gauge set once, at install time, because the build cannot change while
/// the process runs — a value that moved would be a lie about the binary.
#[cfg(feature = "prometheus")]
pub(crate) fn record_build_info() {
    metrics::gauge!(
        METRIC_BUILD_INFO,
        "version" => env!("CARGO_PKG_VERSION")
    )
    .set(1.0);
}

/// Seconds since the Unix epoch, the unit a timestamp gauge is read in.
///
/// Clock skew and a pre-epoch host cannot be represented, and neither is worth
/// a lie: the value clamps to `0.0`, which reads as "written at the epoch" —
/// obviously wrong on a dashboard, rather than silently plausible.
#[cfg(feature = "prometheus")]
fn unix_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs_f64())
        .unwrap_or(0.0)
}

/// Record that new knowledge has landed, at the moment it landed.
///
/// The freshness question — "how old is what this service knows" — has no
/// answer in any other series, so it gets one written by the paths that know:
/// the episode write and the extraction write. Both are shared funnels every
/// capture transport passes through, which is the only reason the gauge reads
/// anything on a deployment that never calls an MCP tool.
///
/// Deliberately not called from [`OperationMetrics`]: a call that arrived and
/// failed is traffic, not knowledge, and a stamp keyed to transport success
/// would keep a broken capture loop looking freshly fed.
pub(crate) fn record_knowledge_write() {
    #[cfg(feature = "prometheus")]
    metrics::gauge!(METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS).set(unix_seconds());
}

/// Record a sign-in that completed.
///
/// Called where the session is issued rather than where the flow ends, so it
/// counts sign-ins that actually produced a usable session and cannot double
/// count a retry of a flow that failed after authentication.
///
/// Gated on `control-plane` because that is the only caller — the OIDC browser
/// flow. The body is gated on `prometheus` separately, so a control-plane
/// build without metrics records nothing and says so at the call site rather
/// than by vanishing.
#[cfg(feature = "control-plane")]
pub(crate) fn record_signin_success() {
    #[cfg(feature = "prometheus")]
    metrics::counter!(METRIC_AUTH_SIGNINS_TOTAL).increment(1);
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
            let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
                .set_bucket_duration(SUMMARY_BUCKET_DURATION)
                .expect("SUMMARY_BUCKET_DURATION is non-zero")
                .set_bucket_count(summary_buckets())
                .install_recorder()
                .ok();
            if handle.is_some() {
                describe_metrics();
                record_build_info();
            }
            handle
        })
        .clone()
}

/// Read the listener address, keeping the environment out of the rule.
///
/// The rule — unset and blank mean "no listener", a valid `ip:port` is the
/// address, anything else is a configuration error naming the variable — is
/// [`listener_addr_from`], which takes a value and returns a decision. Reading
/// the process environment is a single line here so the rules themselves can be
/// tested with plain inputs: environment variables are process-global, so a
/// test that set one would be racing every other test in the binary.
#[cfg(feature = "prometheus")]
fn parse_listen_addr() -> Result<Option<SocketAddr>, MemoryError> {
    match std::env::var(ENV_PROMETHEUS_LISTEN_ADDR) {
        Ok(raw) => listener_addr_from(Some(&raw)),
        Err(_) => listener_addr_from(None),
    }
}

/// The listener decision, as a pure function of the configured value.
#[cfg(feature = "prometheus")]
pub(crate) fn listener_addr_from(raw: Option<&str>) -> Result<Option<SocketAddr>, MemoryError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
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

#[cfg(feature = "prometheus")]
fn install_with_addr(addr: SocketAddr) -> Result<(), MemoryError> {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .set_bucket_duration(SUMMARY_BUCKET_DURATION)
        .expect("SUMMARY_BUCKET_DURATION is non-zero")
        .set_bucket_count(summary_buckets())
        .with_http_listener(addr)
        .install()
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!(
                "failed to install Prometheus exporter on {addr}: {err}"
            ))
        })?;
    describe_metrics();
    record_build_info();
    Ok(())
}

// Gated on the same feature as the exporter it reads: reading an exposition
// needs the exporter crate, and the `prometheus` profile is the only build in
// which a metric is scrapeable at all. Outside it there is nothing to assert.
#[cfg(all(test, feature = "prometheus"))]
pub(crate) mod tests {
    use super::*;

    /// Run `body` under the shared recorder lock and return what it produced.
    ///
    /// One recorder, one lock. `metrics::set_global_recorder` succeeds once per
    /// process, so six tests each installing their own returned nothing on
    /// every run but one, and which one won was a race. Taking the lock across
    /// the body also stops two tests interleaving their metrics into one
    /// exposition and reading each other's series.
    ///
    /// Generic in what the body returns so a test can take a reading on each
    /// side of the work it does, which is what makes a counter assertion exact
    /// without depending on what sibling tests have already recorded.
    pub(crate) async fn exposed<F, Fut, T>(body: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        static LOCK: std::sync::OnceLock<
            tokio::sync::Mutex<metrics_exporter_prometheus::PrometheusHandle>,
        > = std::sync::OnceLock::new();
        let handle = shared_test_handle().expect("prometheus enabled");
        let lock = LOCK.get_or_init(|| tokio::sync::Mutex::new(handle));
        let _guard = lock.lock().await;
        body().await
    }

    /// The exposition as it stands. Read it *inside* [`exposed`] when the test
    /// also writes to the recorder — outside the lock, a sibling test can move
    /// a counter between the two readings.
    pub(crate) fn render() -> String {
        shared_test_handle().expect("prometheus enabled").render()
    }

    /// The value of one labelled series, by metric name and the labels it must
    /// carry.
    ///
    /// Filtering by the labels is not optional. The recorder is process-global, so
    /// `memory_http_requests_total` carries a series per method-and-outcome
    /// combination; taking the first match returns whichever one another test
    /// happened to record first, and an assertion against it is a coin flip. The
    /// metric name is also matched on a boundary — `foo` must not match `foo_bar` —
    /// so a sibling name cannot be read by accident.
    /// The value of a series that carries no labels at all.
    ///
    /// A separate reader from [`sample_series`] because a label-less line is
    /// rendered `name 1`, with no braces — so asking `sample_series` for it
    /// with an empty label set builds the prefix `name{}` and matches nothing.
    /// That returned `None` rather than an error, which made a test asserting
    /// "the gauge was never raised" pass on a gauge that was raised on every
    /// single request: an assertion that could not fail, on the exact signal
    /// the dashboard tells the reader to distrust.
    pub(crate) fn sample_bare(exposition: &str, metric: &str) -> Option<f64> {
        exposition
            .lines()
            .find(|line| line.starts_with(metric) && line[metric.len()..].starts_with(' '))
            .and_then(|line| line.split_whitespace().last())
            .and_then(|value| value.parse().ok())
    }

    pub(crate) fn sample_series(exposition: &str, metric: &str, labels: &str) -> Option<f64> {
        let prefix = format!("{metric}{{{labels}}}");
        exposition
            .lines()
            .find(|line| line.starts_with(&prefix))
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
        let exposition = tests::exposed(|| async {
            {
                let _stage =
                    crate::observability::StageTimer::new("assemble_context", "query_embedding");
            }
            tests::render()
        })
        .await;

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

    /// The refusal's `surface` is a bounded label: the bearer path and the OIDC
    /// path do not share a series, so a refusal is attributable to the
    /// credential kind as well as the branch.
    #[tokio::test]
    async fn the_auth_refusal_metric_surface_is_bounded() {
        let exposition = exposed(|| async {
            record_auth_refusal("bearer", "verify_failed");
            render()
        })
        .await;

        assert!(
            exposition.contains(METRIC_AUTH_REFUSALS_TOTAL)
                && exposition.contains(r#"surface="bearer""#)
                && exposition.contains(r#"branch="verify_failed""#),
            "the surface must be a bounded label: {exposition}"
        );
    }

    /// A stage timer must live in a block that closes before the work it names
    /// ends — otherwise it measures everything after it.
    ///
    /// Bound at function scope, a guard stays alive until the function returns.
    /// That is not a subtle drift: `query_embedding` came to measure the whole
    /// retrieval including the vector search, and two stages ended up nested, so
    /// a dashboard subtracting one from the other read zero — which is the
    /// discrimination the stages exist to provide.
    ///
    /// The check is that the guard's own line opens no block, so the statement
    /// holding it cannot be scoped: a scoped timer is always the *first* thing
    /// inside a `let … = {`, and the line above it ends in `{`. An unscoped one
    /// is a plain `let _stage = …` and the line above it is unrelated.
    #[test]
    fn every_stage_timer_is_scoped_to_its_block() {
        const SOURCES: &[&str] = &[
            include_str!("memory/retrieval/semantic.rs"),
            include_str!("tools/extract.rs"),
            include_str!("tools/ingest.rs"),
            include_str!("embedding/service.rs"),
        ];
        for source in SOURCES {
            let lines: Vec<&str> = source.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                if !line.trim_start().starts_with("let _") {
                    continue;
                }
                // The statement runs to its terminating `;`, inclusive. The
                // length is found rather than collected with `take_while`,
                // which drops the line that satisfied the predicate — and that
                // line is the one carrying the call, so the statement came out
                // as a bare `let _stage =` and the test never fired.
                let length = lines[index..]
                    .iter()
                    .position(|candidate| candidate.trim_end().ends_with(';'))
                    .map_or(1, |offset| offset + 1);
                let statement: String = lines[index..index + length].concat();
                if !statement.contains("StageTimer::new") {
                    continue;
                }
                // Scoped: the block opening the guard is on the line above and
                // its brace is what the formatter left at the end of that line.
                let line_above = index
                    .checked_sub(1)
                    .and_then(|up| lines.get(up))
                    .copied()
                    .unwrap_or_default();
                let scoped = line_above.trim_end().ends_with('{');
                assert!(
                    scoped,
                    "{}: `{}` opens no block, so it measures everything after it \
                     rather than the call it names",
                    index + 1,
                    line.trim(),
                );
            }
        }
    }

    /// A stage that is declared but never attached to a pipeline measures
    /// nothing. The sources are read and compared, so adding a stage to the
    /// vocabulary without measuring it — the failure this guards — fails here
    /// rather than showing up as a permanently flat histogram, which reads on a
    /// dashboard exactly like a fast pipeline.
    ///
    /// The match is on the constructed call rather than on the stage name as a
    /// substring: a bare `contains("ann_search")` is satisfied by the same word
    /// in a doc comment or a string literal, so the check passed for the right
    /// answer and would keep passing for the wrong one.
    #[test]
    fn every_declared_stage_is_measured_somewhere_in_the_pipeline() {
        const SOURCES: &[&str] = &[
            include_str!("memory/retrieval/semantic.rs"),
            include_str!("tools/extract.rs"),
            include_str!("tools/ingest.rs"),
            include_str!("embedding/service.rs"),
        ];
        for stage in DECLARED_STAGES {
            let attached = SOURCES.iter().any(|source| {
                source.contains(&format!(
                    "StageTimer::new(\"assemble_context\", \"{stage}\")"
                )) || source.contains(&format!("StageTimer::new(\"extract\", \"{stage}\")"))
                    || source.contains(&format!("StageTimer::new(\"ingest\", \"{stage}\")"))
            });
            assert!(
                attached,
                "stage `{stage}` is declared but no `StageTimer::new` constructs it; \
                 a stage nobody attaches is a metric that always reads zero"
            );
        }
    }

    #[test]
    fn operation_metrics_are_safe_without_recorder() {
        let mut metrics = OperationMetrics::new("ingest");
        metrics.record_result("episodes", 1);
        metrics.success();
    }

    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn operation_metrics_emit_expected_families() {
        // Through `exposed`, so this holds the same lock as every other metric
        // test. Reading the shared recorder directly while another test writes to
        // it is a race, and the operation counts below would drift.
        let output = exposed(|| async {
            let mut metrics = OperationMetrics::new("ingest");
            metrics.record_result("episodes", 2);
            metrics.success();
            drop(metrics);
            render()
        })
        .await;

        assert!(output.contains("memory_operation_calls_total{"));
        assert!(output.contains("operation=\"ingest\""));
        assert!(output.contains("outcome=\"success\""));
        assert!(output.contains("memory_operation_duration_seconds"));
        assert!(output.contains("memory_operation_results_total{"));
        assert!(output.contains("result=\"episodes\""));
    }

    /// A recorded write stamps the knowledge clock.
    ///
    /// Freshness is what a memory product is trusted on, and no existing series
    /// can answer it: `timestamp()` of a counter that stopped growing reports
    /// the last *scrape*, so a service that stopped learning a day ago looks as
    /// recently touched as one that learned a minute ago, and the `increase()`
    /// rules read "nothing new" for both. The clock therefore has to be written
    /// where knowledge lands — and stamped with the moment it landed, not the
    /// moment it was read, or an age panel would show a staircase of its own
    /// refresh.
    #[tokio::test]
    async fn a_recorded_write_stamps_the_knowledge_clock() {
        let (exposition, before, after) = exposed(|| async {
            let before = unix_seconds();
            record_knowledge_write();
            (render(), before, unix_seconds())
        })
        .await;

        assert!(
            exposition.contains(&format!(
                "# HELP {METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS} "
            )),
            "the freshness stamp must be described: a panel author given only a \
             name cannot tell a timestamp from a duration: {exposition}"
        );
        let stamped = sample_bare(&exposition, METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS)
            .unwrap_or_else(|| {
                panic!(
                    "a knowledge write must stamp when it happened; without it \
                     freshness is unreadable and every capture stops being \
                     visible: {exposition}"
                )
            });
        assert!(
            (before..=after).contains(&stamped),
            "the stamp must be the moment of the write, not of the read: \
             expected {before}..={after}, got {stamped}: {exposition}"
        );
    }

    /// Only the two knowledge writes stamp the clock — a wiring lint, not a
    /// scenario.
    ///
    /// It is a lint because the property is *where* a call sits across modules,
    /// which no behavioural test can observe without a database behind every
    /// path. The behaviour it protects is real: a stamp that also followed
    /// recall, or one left on the MCP adapter, makes a service that is not
    /// learning look freshly fed, and the first leaves the gauge absent for
    /// every deployment whose knowledge arrives unattended — which is the whole
    /// reason the gauge exists.
    #[test]
    fn only_the_knowledge_writes_stamp_the_clock() {
        let writers = [
            ("memory/ingestion.rs", include_str!("memory/ingestion.rs")),
            (
                "memory/episode/fact_extraction.rs",
                include_str!("memory/episode/fact_extraction.rs"),
            ),
        ];
        for (name, source) in writers {
            assert!(
                source.contains("record_knowledge_write()"),
                "{name} is a knowledge write and must stamp the freshness \
                 clock: without it the gauge reads absent on every deployment \
                 that reaches the store through it"
            );
        }

        // The read paths must not stamp: each of these reads the knowledge the
        // clock measures, and a stamp in any of them would make an idle service
        // look fed. `resolve` also records `entities`, which is why it is on this
        // list rather than being treated as a writer.
        for (name, source) in [
            ("memory/retrieval.rs", include_str!("memory/retrieval.rs")),
            ("memory/lifecycle.rs", include_str!("memory/lifecycle.rs")),
            (
                "tools/assemble_context.rs",
                include_str!("tools/assemble_context.rs"),
            ),
            ("tools/resolve.rs", include_str!("tools/resolve.rs")),
            ("tools/explain.rs", include_str!("tools/explain.rs")),
        ] {
            assert!(
                !source.contains("record_knowledge_write()"),
                "{name} reads or maintains knowledge without adding any, so it \
                 must not stamp the freshness clock — a read that refreshes it \
                 makes a service that stopped learning look freshly fed"
            );
        }

        // The transport adapter is where this used to live, and it is the
        // tempting place to put it back: a call that arrived and failed is
        // traffic, not knowledge. Scoped to the block itself, because this file
        // also names the recorder — in its definition and in the test above.
        let operation_metrics = include_str!("observability.rs")
            .split("impl OperationMetrics")
            .nth(1)
            .unwrap_or_default()
            .split("impl Drop for OperationMetrics")
            .next()
            .unwrap_or_default();
        assert!(
            !operation_metrics.contains("record_knowledge_write()"),
            "the freshness stamp belongs to the write that landed, not to the \
             MCP call that asked for it: a capture that arrives and fails would \
             otherwise keep the clock fresh"
        );
    }

    /// A background pass stamps when it last ran.
    ///
    /// A pass counter answers "how many times did this run", which cannot tell a
    /// scheduler that is running every minute from one that ran an hour ago and
    /// has not run since — and the second is the failure nobody sees, because a
    /// job that stops running reports no errors, no refusals and no latency. It
    /// is the background half of the same question the knowledge clock answers
    /// for captures: how long since anything happened.
    #[tokio::test]
    async fn a_background_pass_stamps_when_it_last_ran() {
        let (exposition, before, after) = exposed(|| async {
            let before = unix_seconds();
            record_job_metric("lifecycle_decay", "ok", 0.01);
            (render(), before, unix_seconds())
        })
        .await;

        assert!(
            exposition.contains(&format!(
                "# HELP {METRIC_BACKGROUND_JOB_LAST_RUN_TIMESTAMP_SECONDS} "
            )),
            "the run stamp must be described: a panel author given only a name \
             cannot tell a timestamp from a duration: {exposition}"
        );
        let series = format!(
            "{METRIC_BACKGROUND_JOB_LAST_RUN_TIMESTAMP_SECONDS}{{pass=\"lifecycle_decay\"}}"
        );
        let stamped = sample_series(
            &exposition,
            METRIC_BACKGROUND_JOB_LAST_RUN_TIMESTAMP_SECONDS,
            r#"pass="lifecycle_decay""#,
        )
        .unwrap_or_else(|| {
            panic!(
                "a completed pass must stamp when it ran, per job; without \
                     it a scheduler that stopped is indistinguishable from one \
                     that is simply quiet: {exposition}"
            )
        });
        assert!(
            (before..=after).contains(&stamped),
            "the stamp must be the moment of the pass: expected \
             {before}..={after}, got {stamped}: {exposition}"
        );
        assert!(
            exposition.contains(&series),
            "`{series}` is absent, so the age of this job cannot be read: \
             {exposition}"
        );
    }

    /// A pass that failed is not a pass that ran.
    ///
    /// This is the whole reason the stamp exists alongside the pass counter. If
    /// a failure refreshed it, a job that fails every minute would look
    /// permanently fresh — the alert would have nothing left to measure, and the
    /// one signal that distinguishes "running and broken" from "running" would
    /// be the one that lied.
    #[tokio::test]
    async fn a_failed_pass_does_not_refresh_when_the_job_last_ran() {
        let (exposition, before, after) = exposed(|| async {
            // One successful pass first, so the comparison is between two
            // readings rather than between an absence and an absence.
            record_job_metric("lease", "ok", 0.01);
            let before = sample_series(
                &render(),
                METRIC_BACKGROUND_JOB_LAST_RUN_TIMESTAMP_SECONDS,
                r#"pass="lease""#,
            );
            record_job_metric("lease", "error", 0.01);
            let after = sample_series(
                &render(),
                METRIC_BACKGROUND_JOB_LAST_RUN_TIMESTAMP_SECONDS,
                r#"pass="lease""#,
            );
            (render(), before, after)
        })
        .await;

        assert!(
            before.is_some(),
            "the successful pass above must have left a stamp, or this test \
             compares two absences and passes against an implementation that \
             never stamps at all: {exposition}"
        );
        assert_eq!(
            after, before,
            "a failing pass stamped the same clock a successful one does, so a \
             job that never succeeds looks permanently fresh: {exposition}"
        );
    }

    /// No exposition may declare a `job` label — a lint, not a scenario.
    ///
    /// `job` belongs to the collector: a scrape attaches its own `job_name` to
    /// every series and renames an exposition's `job` to `exported_job` when
    /// `honor_labels` is false, which is the default and what the shipped scrape
    /// configurations use. Nothing fails when that happens. `by (job)` collapses
    /// to one target, `{job="lease"}` matches nothing, and a dashboard still
    /// renders — one merged series where five were meant, an alert that can
    /// never fire. Found the hard way, by scraping a fixture: the background
    /// job panels looked fine and were reading the wrong thing.
    ///
    /// The file list mirrors the checkers' `SOURCES` in
    /// `observability/check_rules.py`, which is the set of files that actually
    /// declare a metric or a label. Listing files that declare no `metrics::`
    /// call instead would have left the lint guarding nothing: the first version
    /// of this test scanned `memory/ingestion.rs` and `memory/lifecycle.rs`,
    /// neither of which records anything, and missed the three that do.
    #[test]
    fn no_metric_declares_a_job_label() {
        for source in [
            include_str!("observability.rs"),
            include_str!("shared/observability.rs"),
            include_str!("knowledge/claims_policy/telemetry.rs"),
            include_str!("http/registry/provisioning.rs"),
            include_str!("service/fs_watch/telemetry.rs"),
        ] {
            for (number, line) in source.lines().enumerate() {
                let code = line.trim();
                if code.starts_with("//") {
                    continue;
                }
                assert!(
                    !code.contains("\"job\""),
                    "observability.rs:{} declares a `job` label: {code}\n\
                     `job` is the collector's label and is renamed to \
                     `exported_job` on scrape; name the dimension for what it \
                     is (`pass`, `operation`, `route`) or every grouping and \
                     filter written against it silently reads the scrape \
                     target instead",
                    number + 1,
                );
            }
        }
    }

    /// Every declared family reaches the exposition.
    ///
    /// The other direction is covered — `every_named_family_is_described` fails
    /// when something is exported without a description — but nothing stopped a
    /// family from being *declared and never recorded*, and that failure is
    /// invisible from inside the process: `describe_metrics` registers the help
    /// line, the scrape looks healthy, and the series simply is not there. A
    /// panel reading it then renders *No data*, which is what an unexercised
    /// subsystem looks like too.
    ///
    /// So this drives every recorder this module and its neighbours own — the
    /// operations, the stage timers, the background pass, the refusals, the
    /// sign-in and the knowledge clock, and the claims and filesystem-watch
    /// telemetry facades — and then reads the exposition and asks whether each
    /// declared family is in it.
    ///
    /// The three HTTP families are the exception, and the exception is not a
    /// loophole: they are recorded by the request-logging middleware, which needs
    /// a served request to run. That is what
    /// `http::metrics::tests::a_served_request_reaches_the_exposition` covers,
    /// and the two tests between them cover the whole list — a family added to
    /// `DESCRIPTIONS` is missing from one of them or the other until something
    /// records it.
    ///
    /// Gated on the full profile because the recorders it drives are: a sign-in
    /// count belongs to the control plane, and registry reconciliation to the
    /// HTTP profile. A narrower build is a legal build, but this test has no
    /// honest claim about the families that build does not have.
    #[tokio::test]
    #[cfg(all(
        feature = "prometheus",
        feature = "streamable-http",
        feature = "control-plane"
    ))]
    async fn every_declared_family_reaches_the_exposition() {
        use crate::knowledge::claims_policy::telemetry as claims_telemetry;
        use crate::knowledge::claims_policy::telemetry::{ClaimMatchMode, ClaimMetricStage};
        use crate::models::claim::ClaimSchemaFamily;
        use crate::models::inbox_revision::InboxFailureClass;
        use crate::service::fs_watch::processor::ProcessOutcome;
        use crate::service::fs_watch::telemetry::FsWatchTelemetry;

        // These families are written by HTTP middleware or periodic owner
        // upkeep. Dedicated integration tests exercise those production writers
        // rather than recording synthetic samples here.
        const HTTP_MIDDLEWARE: [&str; 5] = [
            METRIC_HTTP_REQUESTS_TOTAL,
            METRIC_HTTP_REQUEST_DURATION_SECONDS,
            METRIC_HTTP_REQUESTS_INFLIGHT,
            crate::shared::observability::METRIC_HTTP_PREFLIGHT_BODY_BYTES,
            crate::shared::observability::METRIC_HTTP_PREFLIGHT_REFUSALS_TOTAL,
        ];
        const HTTP_OWNER_SNAPSHOT: [&str; 8] = [
            crate::shared::observability::METRIC_HTTP_PREFLIGHT_RESERVED_REQUESTS,
            crate::shared::observability::METRIC_HTTP_PREFLIGHT_RESERVED_BYTES,
            crate::shared::observability::METRIC_HTTP_TENANT_RUNTIME_COUNT,
            crate::shared::observability::METRIC_HTTP_CONTEXT_CACHE_ACCOUNTED_BYTES,
            crate::shared::observability::METRIC_HTTP_QUERY_CACHE_ACCOUNTED_BYTES,
            crate::shared::observability::METRIC_HTTP_BACKGROUND_EMBEDDING_ADMITTED_TASKS,
            crate::shared::observability::METRIC_HTTP_BACKGROUND_EMBEDDING_RUNNING_TASKS,
            crate::shared::observability::METRIC_HTTP_BACKGROUND_EMBEDDING_RETAINED_BYTES,
        ];

        let exposition = exposed(|| async {
            let mut ingest = OperationMetrics::new("ingest");
            ingest.record_result("episodes", 1);
            ingest.record_stock("active_facts", 2);
            ingest.success();
            drop(ingest);

            let mut extract = OperationMetrics::new("extract");
            extract.record_result("facts", 1);
            extract.success();
            drop(extract);

            let _ = crate::shared::observability::StageTimer::new("ingest", "store_write");

            record_job_metric("lease", "ok", 0.01);
            record_auth_refusal("oidc", "nonce");
            record_runtime_refusal("quota");
            record_signin_success();
            record_knowledge_write();
            record_build_info();

            claims_telemetry::record_pipeline_event(
                ClaimMetricStage::Project,
                ClaimSchemaFamily::Attribute,
                "duplicate",
                "none",
            );
            claims_telemetry::record_pipeline_duration(
                ClaimMetricStage::Reconcile,
                ClaimSchemaFamily::Attribute,
                "duplicate",
                std::time::Duration::from_millis(5),
            );
            claims_telemetry::record_candidate_count(
                ClaimSchemaFamily::Attribute,
                ClaimMatchMode::Exact,
                2,
            );
            claims_telemetry::set_active_relations(ClaimSchemaFamily::Attribute, "duplicate", 1.0);
            claims_telemetry::record_backfill_fact("projected", "none");
            crate::http::registry::provisioning::record_registry_reconciliation(
                "missing_namespace",
            );

            let telemetry = FsWatchTelemetry::new();
            telemetry.record_revision(ProcessOutcome::Processed);
            telemetry.record_retry("backend", InboxFailureClass::Io);
            telemetry.record_scan_file("enqueued");
            telemetry.set_queue_depth(1);
            telemetry.set_inflight(1);
            telemetry.set_degraded(false);
            telemetry.record_revision_duration(
                ProcessOutcome::Processed,
                std::time::Duration::from_millis(3),
            );

            render()
        })
        .await;

        let missing: Vec<&str> = crate::shared::observability::DESCRIPTIONS
            .iter()
            .map(|described| described.name)
            .filter(|name| {
                !exposition.contains(&format!("# HELP {name} "))
                    && !HTTP_MIDDLEWARE.contains(name)
                    && !HTTP_OWNER_SNAPSHOT.contains(name)
            })
            .collect();
        assert!(
            missing.is_empty(),
            "these families are declared and described, so a panel author is \
             promised them, but nothing recorded into them: a declared family \
             no recorder writes is absent from every scrape, and a panel \
             reading it looks like a subsystem that is switched off: {missing:?}"
        );
    }

    /// A listener address that parses is taken, and its port is kept verbatim.
    ///
    /// `127.0.0.1:0` is the interesting one: port zero means "any free port",
    /// which is what the tests and a developer machine use, and a rule that
    /// rejected it would have made the whole listener untestable.
    #[cfg(feature = "prometheus")]
    #[test]
    fn a_configured_listener_address_is_the_one_that_was_configured() {
        let addr = listener_addr_from(Some(" 127.0.0.1:0 "))
            .expect("a valid address is not an error")
            .expect("a configured address opens a listener");

        assert_eq!(
            addr,
            "127.0.0.1:0"
                .parse::<SocketAddr>()
                .expect("the literal parses"),
            "the address must be the configured one, whitespace and all"
        );
    }

    /// Unset and blank both mean "no listener", and neither is an error.
    ///
    /// Blank is separated from unset because a deployment template that renders
    /// an empty value is a normal way to end up here, and refusing it would send
    /// an operator looking for a syntax error that is not one.
    #[cfg(feature = "prometheus")]
    #[test]
    fn an_unset_or_blank_listener_address_means_no_listener() {
        assert_eq!(
            listener_addr_from(None).expect("unset is not an error"),
            None,
            "unset means no listener, not a failure"
        );
        assert_eq!(
            listener_addr_from(Some("   ")).expect("blank is not an error"),
            None,
            "a blank value is the same as unset: an empty template variable is \
             not a syntax error"
        );
    }

    /// A malformed address is refused, and the message says which variable.
    ///
    /// The message quotes the offending value and names the variable, because
    /// this is a startup error a person has to act on without a stack trace: the
    /// two facts they need are which setting is wrong and what they put in it.
    #[cfg(feature = "prometheus")]
    #[test]
    fn a_malformed_listener_address_names_the_variable_and_the_value() {
        let error =
            listener_addr_from(Some("localhost:9100")).expect_err("a hostname is not a SocketAddr");

        let message = error.to_string();
        assert!(
            message.contains(ENV_PROMETHEUS_LISTEN_ADDR),
            "the error must name the variable to change: {message}"
        );
        assert!(
            message.contains("localhost:9100"),
            "and quote the value that was rejected, so it can be found without \
             reading the source: {message}"
        );
    }

    /// No counter or gauge carries a unit.
    ///
    /// One-directional on purpose, and not an oversight: `describe_metrics`
    /// drops the unit for both, so a unit added to a counter would be silently
    /// discarded from the exposition. This is the only thing that notices.
    /// A histogram without a unit is legitimate — a count of candidates has no
    /// unit but is still a distribution — so the rule stops at the boundary.
    ///
    /// It is a dictionary invariant rather than a per-family check, so it also
    /// covers the families written after this one.
    #[test]
    fn counters_and_gauges_are_dimensionless() {
        for described in crate::shared::observability::DESCRIPTIONS {
            if matches!(
                described.kind,
                crate::shared::observability::MetricKind::Histogram
            ) {
                continue;
            }
            assert!(
                described.unit.is_none(),
                "`{}` is a {:?}, and `describe_metrics` drops the unit for both: \
                 a unit added here would never reach the exposition",
                described.name,
                described.kind
            );
        }
    }

    /// A label set the helper does not know is refused loudly.
    ///
    /// The alternative is a counter and a histogram recorded with different
    /// labels, which no scrape would report and no panel would explain. The
    /// guard is a `debug_assert`, so the test is gated the same way: in a
    /// release build there is nothing here to catch.
    #[cfg(all(debug_assertions, feature = "prometheus"))]
    #[test]
    #[should_panic(expected = "known label arity")]
    fn an_unknown_label_arity_is_refused_rather_than_recorded() {
        count_timed(
            "memory_test_unmatched_counter",
            "memory_test_unmatched_histogram",
            0.01,
            &[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")],
        );
    }

    /// A stock reported as a flow again is the original defect, and nothing
    /// about it is visible at the call site: `record_stock(name, x.len())`
    /// and `record_result(name, x.len())` read the same at the line. The
    /// inventory read is what makes it a stock — the counts are bounded by
    /// page limits, not produced by the work.
    #[test]
    fn an_inventory_read_is_never_counted_as_work_produced() {
        const LIFECYCLE: &str = include_str!("memory/lifecycle.rs");
        // The dashboard reads three levels and performs no work. A counter
        // entry there is the defect; the archive, restore, decay and rebuild
        // paths below it do produce work and are left alone.
        let dashboard = LIFECYCLE
            .split("pub async fn archive_candidates")
            .next()
            .expect("the dashboard precedes archive_candidates");
        assert!(
            !dashboard.contains("record_result"),
            "the lifecycle dashboard reports levels, not work produced; \
             a `record_result` in it re-accumulates the whole inventory on \
             every read: {dashboard}"
        );
        assert!(
            dashboard.contains("record_stock"),
            "and it must still report its inventory, or the panel goes blank"
        );
    }

    /// An exported family has to describe itself.
    ///
    /// A dashboard author opening `/metrics` saw bare
    /// `memory_http_requests_total{method="read",outcome="2xx"} 42` and nothing
    /// else: no unit, no meaning, no type. Grafana cannot infer any of it, so
    /// every panel had to be written against a name someone remembered, and a
    /// mistake in that memory is a panel that is silently wrong rather than
    /// visibly broken. The `# HELP` line is what the description becomes.
    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn every_exported_family_carries_a_description() {
        let exposition = exposed(|| async {
            let mut metrics = OperationMetrics::new("ingest");
            metrics.record_result("episodes", 1);
            metrics.record_stock("active_facts", 1);
            metrics.success();
            drop(metrics);
            render()
        })
        .await;

        // A family reaches the exposition only once something has recorded into
        // it, so each is emitted above; a description for a family no series
        // exists for would be invisible to a scrape anyway.
        // `METRIC_BUILD_INFO` belongs here because the recorder install above
        // stamps it, so this exposition does carry it. The sign-in counter and
        // the freshness stamp are deliberately absent: a family reaches the
        // exposition only once something records into it, and neither a store
        // write nor an OIDC callback happens here. Each has a test that drives
        // its own path — `a_completed_sign_in_is_counted_for_the_exporter` and
        // `a_recorded_write_stamps_the_knowledge_clock` — and a family checked
        // in a test that cannot produce it would only be a test of the list.
        for family in [
            METRIC_OPERATIONS_TOTAL,
            METRIC_OPERATION_DURATION_SECONDS,
            METRIC_OPERATION_RESULTS_TOTAL,
            METRIC_OPERATION_STOCK,
            METRIC_BUILD_INFO,
        ] {
            assert!(
                exposition.contains(&format!("# HELP {family} ")),
                "`{family}` is exported without a description, so a panel \
                 author has nothing but the name to go on: {exposition}"
            );
        }
    }

    /// The exposition says which build produced it.
    ///
    /// An incident review that cannot name the version behind a dashboard is
    /// read against the wrong release: the fix gets looked for in the code
    /// that shipped after the numbers were taken, and the release that
    /// actually caused it is never opened.
    ///
    /// What this covers is narrow, and the width matters: the shared handle
    /// stamps the build itself when it installs the recorder, so this proves
    /// that *a* recorder install stamps — not that both composition roots
    /// remember to call [`record_build_info`]. That half is asserted where the
    /// risk lives, in the HTTP composition root's own source-scan test
    /// (`http::metrics::tests::the_recorder_installs_from_the_composition_root`),
    /// which checks `install_recorder` next to the stamp. Keeping the two claims
    /// apart is why this wording does not claim more than it reads.
    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn the_exposition_identifies_the_build_it_came_from() {
        let exposition = render();

        assert!(
            exposition.contains(&format!("# HELP {METRIC_BUILD_INFO} ")),
            "the build series is exported without a description, so the label \
             it carries has nothing to explain it: {exposition}"
        );
        let stamped = format!(
            "{METRIC_BUILD_INFO}{{version=\"{}\"}}",
            env!("CARGO_PKG_VERSION")
        );
        assert!(
            exposition.contains(&stamped),
            "`{stamped}` is absent, so a reader cannot tell which version these \
             numbers came from — the install path forgot record_build_info: \
             {exposition}"
        );
    }

    /// A label-less series has to be readable, and reading it must not be a
    /// silent miss.
    ///
    /// `sample_series` builds the prefix `name{labels}`, so asking it for a
    /// series that has no labels at all produces `name{}` and matches nothing.
    /// It returns `None` for that, indistinguishable from a series that was
    /// never written — which is how a test asserting "the in-flight gauge was
    /// never raised" came to pass on a gauge raised on every request.
    #[test]
    fn a_series_without_labels_is_read_by_its_own_reader() {
        const EXPOSITION: &str = concat!(
            "# TYPE memory_http_requests_inflight gauge\n",
            "memory_http_requests_inflight 3\n",
            "memory_http_requests_total{method=\"read\"} 7\n",
        );

        assert_eq!(
            sample_bare(EXPOSITION, "memory_http_requests_inflight"),
            Some(3.0)
        );
        // The prefixed reader must not claim it, and must not silently return
        // a *neighbouring* series either.
        assert_eq!(
            sample_series(EXPOSITION, "memory_http_requests_inflight", ""),
            None,
            "the prefixed reader must not pretend to read a label-less series"
        );
        // A name that is a prefix of another must not read the longer one.
        assert_eq!(
            sample_bare(EXPOSITION, "memory_http_requests"),
            None,
            "a bare reader must respect the name boundary, or `memory_http_requests` \
             reads `memory_http_requests_total`"
        );
    }

    /// Every family the crate exports has to be in the dictionary.
    ///
    /// A metric constant with no description is the failure this prevents, and
    /// it fails quietly: the series appears, the name looks self-explanatory,
    /// and nobody notices there was prose to write. Adding a name without an
    /// entry is the only way that happens.
    ///
    /// The list is scanned out of the sources rather than written by hand,
    /// because a hand-written list is the same kind of thing that goes stale:
    /// a family added last week is simply not in it, and the test still passes.
    #[test]
    fn every_named_family_is_described() {
        const SOURCES: &[&str] = &[
            include_str!("shared/observability.rs"),
            include_str!("observability.rs"),
            include_str!("knowledge/claims_policy/telemetry.rs"),
            include_str!("http/registry/provisioning.rs"),
            include_str!("service/fs_watch/telemetry.rs"),
        ];

        let mut exported: std::collections::BTreeSet<&'static str> =
            std::collections::BTreeSet::new();
        for source in SOURCES {
            for line in source.lines() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for quote in code.split('"').skip(1).step_by(2) {
                    // A family name is a bare snake-case identifier. The guards
                    // reject what a naive scan also picks up: a prefix someone
                    // assembles (`memory_claim_`), and a name glued to a label
                    // list by concatenation in a test (`…_total{`).
                    let is_family = quote.starts_with("memory_")
                        && quote
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                        && quote.len() > "memory_".len() + 3
                        && !quote.ends_with('_')
                        // A family name carries its own shape: a namespace, at
                        // least one underscore-separated word, and a suffix
                        // that says what it is. Without this, a name a *test*
                        // writes — a prefix it is checking the boundary of, say
                        // — is read as a family nobody described.
                        && (quote.ends_with("_total")
                            || quote.contains("_seconds")
                            || quote.ends_with("_count")
                            || quote.ends_with("_active")
                            || quote.ends_with("_depth")
                            || quote.ends_with("_inflight")
                            || quote.ends_with("_degraded")
                            || quote.ends_with("_considered"));
                    if is_family {
                        // `line` borrows from the `include_str!` source, which
                        // is `'static`, so the copied substring outlives it.
                        exported.insert(quote.to_string().leak());
                    }
                }
            }
        }

        let undescribed: Vec<_> = exported
            .iter()
            .filter(|name| {
                !crate::shared::observability::DESCRIPTIONS
                    .iter()
                    .any(|described| described.name == **name)
            })
            .collect();
        assert!(
            undescribed.is_empty(),
            "these families are exported but nothing describes them, so a panel \
             author gets a name and nothing else: {undescribed:?}"
        );
    }

    /// A description is a single line.
    ///
    /// The text exposition has no continuation for `# HELP`, so a newline in
    /// the middle yields a line that is neither a comment nor a series — a
    /// scrape error rather than a formatting quirk.
    #[test]
    fn no_description_spans_two_lines() {
        for described in crate::shared::observability::DESCRIPTIONS {
            assert!(
                !described.help.contains('\n'),
                "`{}` has a multi-line description; the exposition has no \
                 continuation for `# HELP`",
                described.name
            );
        }
    }

    /// The descriptions are the vocabulary's to carry, not the recorder's.
    ///
    /// What a family means is knowledge about the domain; where it is recorded
    /// is an infrastructure detail. The split is ADR-0058's: a bounded context
    /// must be able to name and describe its own measurements without acquiring
    /// the Prometheus facade. So the prose lives beside the metric-name
    /// constants in `shared`, and the recorder only registers what it is told.
    ///
    /// Asserted structurally rather than by behaviour: the dictionary is a
    /// constant, so a description in the wrong module is a compile error for
    /// every caller and needs no test. What this pins is that the type carries
    /// the prose at all — a `MetricDescription` without a `help` field would
    /// make the whole arrangement impossible and is the regression worth
    /// naming.
    #[test]
    fn a_description_carries_its_prose_and_its_unit() {
        let described = crate::shared::observability::DESCRIPTIONS
            .iter()
            .find(|described| described.name == METRIC_OPERATION_STOCK)
            .expect("every family is in the dictionary");
        assert!(
            described.help.contains("level"),
            "the prose has to say what the number means, not restate the name: \
             {:?}",
            described.help
        );
        assert_eq!(described.unit, None, "a count of things is dimensionless");
    }

    /// A stock is a level; a flow is a total. The two are the same number at
    /// first glance and behave nothing alike afterwards.
    ///
    /// `lifecycle_dashboard` reported its inventory — active facts, archival
    /// candidates, communities — through `record_result`, which increments. So
    /// the counter held the sum of every inventory ever read: opening the
    /// dashboard added ~5 000 to it, and its derivative reported dashboard
    /// traffic rather than growth in the data. A panel reading "how much is
    /// stored" from that counter is wrong, and wrong in the direction that
    /// looks like success.
    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn a_stock_is_set_rather_than_accumulated() {
        let (first, second, exposition) = exposed(|| async {
            let metrics = OperationMetrics::new("lifecycle_dashboard");
            metrics.record_stock("active_facts", 10_000);
            let first = sample_series(
                &render(),
                METRIC_OPERATION_STOCK,
                r#"operation="lifecycle_dashboard",result="active_facts""#,
            );
            // A second read of an unchanged store reports the same level.
            metrics.record_stock("active_facts", 10_000);
            let second = sample_series(
                &render(),
                METRIC_OPERATION_STOCK,
                r#"operation="lifecycle_dashboard",result="active_facts""#,
            );
            (first, second, render())
        })
        .await;

        assert_eq!(
            first,
            Some(10_000.0),
            "a stock must be reported at its level: {exposition}"
        );
        assert_eq!(
            second, first,
            "reading the same inventory twice must not add it twice: {exposition}"
        );
    }

    /// The counterpart: a flow really does accumulate. Without this, the test
    /// above would also pass if `record_result` had been turned into a
    /// `set` — the two must be told apart, not merely both made small.
    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn a_flow_accumulates_across_calls() {
        let (first, second) = exposed(|| async {
            let metrics = OperationMetrics::new("lifecycle_rebuild_communities");
            metrics.record_result("communities", 40);
            let first = sample_series(
                &render(),
                METRIC_OPERATION_RESULTS_TOTAL,
                r#"operation="lifecycle_rebuild_communities",result="communities""#,
            );
            metrics.record_result("communities", 40);
            let second = sample_series(
                &render(),
                METRIC_OPERATION_RESULTS_TOTAL,
                r#"operation="lifecycle_rebuild_communities",result="communities""#,
            );
            (first, second)
        })
        .await;

        assert_eq!(first, Some(40.0));
        assert_eq!(
            second,
            Some(80.0),
            "work produced must add up; a flow that reports a level reports \
             only the last call"
        );
    }
    /// site saying so — that default is the whole point of the guard, since
    /// every early return and every unexpected failure becomes visible without
    /// a `match` arm written for it.
    ///
    /// Only the success branch was tested, so the default could be changed to
    /// `success` and every failure would be reported as a success — the exact
    /// inversion that makes an error rate meaningless.
    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn an_operation_that_returns_early_is_recorded_as_an_error() {
        // Through `exposed`, so this takes the same lock as every other metric
        // test. Writing to the shared recorder directly and then reading the
        // counter as `1` is a race: another test's operation lands in the same
        // exposition, and this failed on roughly one run in three.
        let exposition = exposed(|| async {
            // No `success()` call: this is what an early return looks like.
            let metrics = OperationMetrics::new("resolve");
            drop(metrics);
            render()
        })
        .await;

        let failures = exposition
            .lines()
            .filter(|line| {
                line.starts_with("memory_operation_calls_total")
                    && line.contains("operation=\"resolve\"")
                    && line.contains("outcome=\"error\"")
            })
            .count();
        assert_eq!(
            failures, 1,
            "an early return must be counted as an error: {exposition}"
        );
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

/// The no-recorder contract, in the build that has no recorder.
///
/// Outside the `prometheus` feature there is no exposition to assert against,
/// so this module holds the one property that build still has: every recorder
/// is callable and does nothing. It lives here rather than in the module above
/// because that one does not compile in this configuration — which would leave
/// the contract untested in exactly the build it is about.
#[cfg(all(test, not(feature = "prometheus")))]
mod without_a_recorder {
    use super::*;

    /// Installing without the feature is a no-op that always succeeds.
    ///
    /// This is the whole contract of a build that does not ask for metrics: the
    /// call the composition root makes is still there, and it succeeds without
    /// opening a socket or touching a recorder.
    #[test]
    fn install_is_a_no_op_without_the_feature() {
        install().expect("install succeeds without the prometheus feature");
    }

    /// Every recorder is callable in a build that has no recorder installed.
    ///
    /// These call sites are not empty in this build: `record_auth_refusal`,
    /// `record_runtime_refusal` and `shift_gauge` reach the `metrics` macros
    /// unconditionally, and `count_timed` still checks its label arity. What the
    /// macros do with a missing recorder is the facade's business, but *calling
    /// into* it is this crate's, and that is the contract: a profile that never
    /// installs a recorder still runs every one of these on its request path, so
    /// a call site that grew an `.expect` on a global handle — or an unwrap on a
    /// registry lookup — would pass every test in the metrics build, where a
    /// recorder does exist, and take down the profile that installs none.
    ///
    /// `record_knowledge_write` is deliberately absent: its body is compiled out
    /// without the feature, so calling it would cover nothing. So is
    /// `install_recorder`, which only exists in the profile that has a recorder.
    #[test]
    fn every_recorder_is_safe_without_a_recorder_installed() {
        record_auth_refusal("oidc", "nonce");
        record_runtime_refusal("quota");
        record_job_metric("lease", "ok", 0.01);
        shift_gauge("memory_test_gauge", 1.0);
        count_timed(
            "memory_test_counter",
            "memory_test_histogram",
            0.01,
            &[("pass", "lease"), ("outcome", "ok")],
        );
        count_timed(
            "memory_test_counter_three",
            "memory_test_histogram_three",
            0.01,
            &[
                ("route", "/metrics"),
                ("method", "read"),
                ("outcome", "2xx"),
            ],
        );
        let mut metrics = OperationMetrics::new("ingest");
        metrics.record_result("episodes", 1);
        metrics.record_stock("active_facts", 1);
        metrics.success();
        drop(metrics);
        let _stage = crate::shared::observability::StageTimer::new("ingest", "store_write");
        drop(_stage);
    }
}
