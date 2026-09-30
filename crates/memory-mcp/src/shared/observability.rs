//! Observability vocabulary — the names, not the instrumentation.
//!
//! This module is the pure kernel: it owns what a measurement is *called*, and
//! nothing about how it is recorded. It holds no metrics facade, no exporter,
//! no socket. That split is the whole point, and it is why it lives under
//! `shared` rather than beside the exporter.
//!
//! The arrangement follows ADR-0058: a domain or application module must not
//! acquire infrastructure through a shared re-export. Before this module,
//! `memory::retrieval`, `tools::ingest` and `embedding::service` all reached
//! into a crate-root `observability` to time a stage — pulling in the
//! Prometheus recorder and everything it drags — purely to learn that the
//! stage is called `ann_search`. They now name it here and hand the name to
//! [`StageTimer`], whose implementation is the only place that knows a metric
//! exists.
//!
//! Two consequences worth stating, because they are what the boundary buys:
//!
//! * A context can be tested, reasoned about and reused with no exporter
//!   present, and every measurement it takes is a no-op rather than an error.
//! * A stage name cannot drift from the stage that emits it, because the name
//!   and the guard that records it are one unit here rather than a string at
//!   the call site and a lookup somewhere else.
//!
//! Bounded labels are a property of this vocabulary, not of the call sites: a
//! stage, an operation and a refusal branch are all fixed words, because an
//! unbounded one is how a metrics backend falls over. Anything needing
//! per-item attribution belongs in the structured log, which carries a keyed
//! fingerprint for it.

use std::time::Instant;

/// Total logical operations by bounded operation and outcome.
pub const METRIC_OPERATIONS_TOTAL: &str = "memory_operation_calls_total";
/// Logical operation duration in seconds by bounded operation and outcome.
pub const METRIC_OPERATION_DURATION_SECONDS: &str = "memory_operation_duration_seconds";
/// Bounded domain result counts by operation and result kind.
pub const METRIC_OPERATION_RESULTS_TOTAL: &str = "memory_operation_results_total";

/// How much a thing exists right now, by operation and result kind.
///
/// A gauge, and not another `results_total`. That counter answers "how much
/// work did this operation produce", so a level belongs nowhere in it: a
/// stock added to a counter reports the sum of every snapshot ever taken, a
/// dashboard that reads its own inventory grows every time it is opened, and
/// its derivative reports dashboard traffic rather than growth in the data.
pub const METRIC_OPERATION_STOCK: &str = "memory_operation_stock";

/// Histogram: a named stage inside a pipeline, in seconds.
///
/// An operation's total latency says *that* something is slow. Only its stages
/// say *which* part: a query that takes two seconds might be waiting on an
/// embedding provider, on the vector index, or on its own post-processing, and
/// the three need different fixes.
pub const METRIC_PIPELINE_STAGE_DURATION_SECONDS: &str = "memory_pipeline_stage_duration_seconds";

/// Counter: background job outcomes, by job family and outcome.
///
/// A background job is the one failure class with no request attached: it
/// cannot be seen in a status code, and it does not appear in the request
/// metrics because it is not a request.
pub const METRIC_BACKGROUND_JOBS_TOTAL: &str = "memory_background_jobs_total";

/// Histogram: background job duration in seconds, by job family.
pub const METRIC_BACKGROUND_JOB_DURATION_SECONDS: &str = "memory_background_job_duration_seconds";

/// Counter: authentication refusals, by surface and reason.
pub const METRIC_AUTH_REFUSALS_TOTAL: &str = "memory_auth_refusals_total";

/// Counter: request-scoped refusals raised by the HTTP runtime.
pub const METRIC_RUNTIME_REFUSALS_TOTAL: &str = "memory_runtime_refusals_total";

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
/// The recorder's default buckets span 5 ms to 10 s, which covers everything
/// from an in-memory hit to a database round trip.
pub const METRIC_HTTP_REQUEST_DURATION_SECONDS: &str = "memory_http_request_duration_seconds";

/// HTTP gauge: requests currently in flight.
///
/// The saturation signal for this service. A count that only rises and falls
/// between scrapes is invisible, and "the queue is growing" is the question an
/// on-call engineer asks before anything else when latency climbs.
pub const METRIC_HTTP_REQUESTS_INFLIGHT: &str = "memory_http_requests_inflight";

/// Measures one named stage of a pipeline.
///
/// A guard rather than a pair of calls around a block, because a stage is often
/// left early — a skipped embedding, a cache hit, an error — and a duration
/// written only on the success path would be missing exactly the cases worth
/// seeing. Dropping the guard records the observation on every path.
pub struct StageTimer {
    operation: &'static str,
    stage: &'static str,
    started_at: Instant,
}

impl StageTimer {
    /// Begin measuring `stage` inside `operation`.
    ///
    /// Both are fixed words. A stage named after a record, a tenant or a query
    /// would make the series unbounded, and the call site is the only place
    /// that could get that wrong — so the vocabulary below is the checked one.
    pub fn new(operation: &'static str, stage: &'static str) -> Self {
        Self {
            operation,
            stage,
            started_at: Instant::now(),
        }
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        record_stage_metric(
            self.operation,
            self.stage,
            self.started_at.elapsed().as_secs_f64(),
        );
    }
}

/// Record one stage observation.
///
/// The seam the vocabulary is defined against: [`crate::observability`] is the
/// implementation, and it is the only place in the crate that knows a metric
/// exists. Routing through a function rather than calling the metrics facade
/// directly is what keeps this module pure — the import is the dependency, and
/// it points inward at the recording side rather than at an exporter.
pub fn record_stage_metric(operation: &'static str, stage: &'static str, seconds: f64) {
    crate::observability::observe(
        METRIC_PIPELINE_STAGE_DURATION_SECONDS,
        seconds,
        "operation",
        operation,
        "stage",
        stage,
    );
}

/// The stages the pipelines report, as a closed vocabulary.
///
/// A test reads the sources and asserts each one is actually measured, so a
/// stage that is declared and never attached — which produces a metric that
/// always reads zero, indistinguishable from a fast pipeline — fails rather
/// than being noticed by somebody staring at a flat histogram.
#[cfg(test)]
pub const DECLARED_STAGES: &[&str] = &[
    "query_embedding",
    "ann_search",
    "extraction",
    "embedding_provider",
    "store_write",
];

/// The refusal branches the identity callback reports.
///
/// Bounded for the same reason stages are: a branch carrying a subject, an
/// issuer or an authorization code would be a disclosure, and the audit trail
/// already carries those under a keyed fingerprint.
#[cfg(test)]
pub const DECLARED_REFUSAL_BRANCHES: &[&str] = &[
    "provider_error",
    "take_oidc_request",
    "state_mismatch",
    "rfc9207_issuer_mismatch",
    "missing_code",
    "id_token",
    "nonce",
    "signup_invite_only",
];
