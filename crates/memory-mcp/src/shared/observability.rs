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

/// Gauge: the build this exposition belongs to, carried as a `version` label.
///
/// The value is always 1 — the number carries no information, the label is the
/// payload. It is the only series that answers "which version are these
/// numbers from": a dashboard opened during a rollout, an alert firing against
/// a release, or two versions running side by side telling themselves apart
/// rather than appearing as one unexplained doubling of everything else.
pub const METRIC_BUILD_INFO: &str = "memory_build_info";

/// Sign-ins that completed, counted with no labels at all.
///
/// Deliberately the opposite of [`METRIC_AUTH_REFUSALS_TOTAL`]: a refusal
/// needs its branch to be actionable, a sign-in needs nothing but the count,
/// and every label it could carry — account, subject, tenant — would turn a
/// traffic measure into a disclosure. Sign-ups cannot be told from sign-ins
/// here; what is countable without naming anyone is that someone arrived.
pub const METRIC_AUTH_SIGNINS_TOTAL: &str = "memory_auth_signins_total";

/// When the newest knowledge was written, as seconds since the Unix epoch.
///
/// A timestamp, not a duration: the value is read as `time() - gauge`, because
/// the question is "how old is what we know", and only the collector knows
/// what "now" is — a duration measured inside the process would answer "how
/// long since this process wrote something", which is a different question
/// that happens to agree most of the time.
///
/// Written by the capture path only. A recall reads knowledge and leaves it
/// as old as it was, and stamping on read would make a quiet service look
/// fresh — the exact failure a freshness gauge exists to catch.
pub const METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS: &str =
    "memory_knowledge_last_write_timestamp_seconds";

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

/// One metric's kind, for the exposition's `# TYPE` and `# HELP` lines.
///
/// The kind is here rather than inferred from the name because the name
/// cannot be trusted to carry it: a family called `..._total` is not
/// necessarily a counter, and one that is a gauge says nothing about whether
/// it can fall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Monotonically increasing.
    Counter,
    /// A level that goes up and down.
    Gauge,
    /// A distribution. Rendered as a summary, not as buckets: the recorder is
    /// built without `set_buckets`, so quantile lines and `_sum` are what a
    /// scrape carries.
    Histogram,
}

/// What a metric family is and what it means, registered at install time.
///
/// Without this, `/metrics` served bare `name{labels} value` lines. A panel
/// author had only the name to go on — no unit, no meaning, no type — so every
/// dashboard was written against someone's memory of a name, and a mistake in
/// that memory is a panel that is quietly wrong rather than visibly broken.
///
/// Lives beside the names, not in the recorder: what a measurement *means* is
/// domain knowledge, and ADR-0058 asks that a bounded context be able to say so
/// without acquiring the metrics facade.
pub struct MetricDescription {
    /// The metric name this describes.
    pub name: &'static str,
    /// Counter, gauge or histogram.
    pub kind: MetricKind,
    /// The unit of the value, or `None` for a dimensionless count.
    ///
    /// `metrics::Unit` rather than a string, because the exporter matches on
    /// the enum to render the unit and would ignore a name it did not
    /// recognise — so a typo here would be a silently missing unit.
    pub unit: Option<metrics::Unit>,
    /// The `# HELP` text. One line: the exposition has no continuation.
    pub help: &'static str,
}

/// Every family this crate exports, in the order a reader meets them.
pub const DESCRIPTIONS: &[MetricDescription] = &[
    MetricDescription {
        name: METRIC_BUILD_INFO,
        kind: MetricKind::Gauge,
        unit: None,
        help: "Build this exposition belongs to, as a version label; the \
               value is always 1. Read the label, never the number, and read \
               it per instance: two versions during a rollout are two series, \
               not one series with a bigger number.",
    },
    MetricDescription {
        name: METRIC_AUTH_SIGNINS_TOTAL,
        kind: MetricKind::Counter,
        unit: None,
        help: "Sign-ins that completed and issued a session. No labels: an \
               account, subject or tenant label would make this a disclosure \
               rather than a traffic measure, so the count is all it carries. \
               Read against memory_auth_refusals_total for arrivals; the \
               refusal branch stays where the reason is.",
    },
    MetricDescription {
        name: METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS,
        kind: MetricKind::Gauge,
        // Deliberately no unit. A `seconds` unit makes Grafana render an epoch
        // timestamp as an elapsed duration, which is off by decades and looks
        // plausible; the panel that plots an age applies `dateTimeAsIso` and
        // the age is computed in the query.
        unit: None,
        help: "Unix timestamp of the newest completed capture, in seconds. The \
               age of what this service knows is `time()` minus this, and no \
               other series can answer it: a counter that stopped growing \
               still reports the last scrape as recent. Written only when \
               knowledge lands, so a service that stopped learning looks as \
               old as it is; absent until the first capture.",
    },
    MetricDescription {
        name: METRIC_OPERATIONS_TOTAL,
        kind: MetricKind::Counter,
        unit: None,
        help: "Logical memory operations invoked, by operation and outcome. \
               The outcome is error unless the operation reported success, so \
               every early return and unexpected failure is counted without a \
               match arm at the call site.",
    },
    MetricDescription {
        name: METRIC_OPERATION_DURATION_SECONDS,
        kind: MetricKind::Histogram,
        unit: Some(metrics::Unit::Seconds),
        help: "Wall-clock duration of a memory operation, by operation and \
               outcome. Exported as a summary: read the quantile lines.",
    },
    MetricDescription {
        name: METRIC_OPERATION_RESULTS_TOTAL,
        kind: MetricKind::Counter,
        unit: None,
        help: "Work produced by an operation, by result kind — entities \
               extracted, rows archived, context items returned. A flow, not a \
               level: use increase() over a window, never rate().",
    },
    MetricDescription {
        name: METRIC_OPERATION_STOCK,
        kind: MetricKind::Gauge,
        unit: None,
        help: "How much exists right now, by operation and result kind — \
               active facts, communities. A level, so it is set rather than \
               accumulated: reading the inventory twice reports it twice over \
               the same value, not twice as much.",
    },
    MetricDescription {
        name: METRIC_PIPELINE_STAGE_DURATION_SECONDS,
        kind: MetricKind::Histogram,
        unit: Some(metrics::Unit::Seconds),
        help: "One named stage inside a pipeline, by operation. An operation's \
               total says that something is slow; its stages say which part.",
    },
    MetricDescription {
        name: METRIC_BACKGROUND_JOBS_TOTAL,
        kind: MetricKind::Counter,
        unit: None,
        help: "Background scheduler passes, by job and outcome. Timed from \
               when the job started running, not when it was scheduled, so \
               queue wait is not counted as work.",
    },
    MetricDescription {
        name: METRIC_BACKGROUND_JOB_DURATION_SECONDS,
        kind: MetricKind::Histogram,
        unit: Some(metrics::Unit::Seconds),
        help: "Duration of a background job pass, by job and outcome.",
    },
    MetricDescription {
        name: METRIC_AUTH_REFUSALS_TOTAL,
        kind: MetricKind::Counter,
        unit: None,
        help: "Authentication refusals on the identity callback, by bounded \
               branch. Refusals only: a successful sign-in is not counted, so \
               this cannot be read as a success ratio.",
    },
    MetricDescription {
        name: METRIC_RUNTIME_REFUSALS_TOTAL,
        kind: MetricKind::Counter,
        unit: None,
        help: "HTTP runtime refusals, by operation — quota, lease, task and \
               registry failures. Counted at the single point every warning \
               passes through, so both entry points are counted once.",
    },
    MetricDescription {
        name: METRIC_HTTP_REQUESTS_TOTAL,
        kind: MetricKind::Counter,
        unit: None,
        help: "HTTP requests served, by method class, status class and the \
               route matched. Route is a router pattern with parameters \
               collapsed, so the label set is bounded; a request matching no \
               route reads 'unmatched'.",
    },
    MetricDescription {
        name: METRIC_HTTP_REQUEST_DURATION_SECONDS,
        kind: MetricKind::Histogram,
        unit: Some(metrics::Unit::Seconds),
        help: "HTTP request duration in seconds, by method class, status \
               class and route. Exported as a summary: read the quantile lines.",
    },
    MetricDescription {
        name: METRIC_HTTP_REQUESTS_INFLIGHT,
        kind: MetricKind::Gauge,
        unit: None,
        help: "Requests currently in flight. The saturation signal: a scrape \
               only sees a non-zero value when it lands inside a request, so a \
               flat zero is the normal case and not evidence of an idle \
               server.",
    },
    // The families below are declared in their own modules but described here,
    // with the rest of the vocabulary: a panel author reading `/metrics` has no
    // way to tell which file a family came from, and the description is the only
    // place that says what a number means. Their names are written out rather
    // than imported because `claims_policy` and `fs_watch` sit behind different
    // feature gates, and a shared constant would have to be gated to match both
    // — putting a claim metric behind `fs-watch` to satisfy a constant is the
    // kind of coupling the split exists to prevent.
    MetricDescription {
        name: "memory_http_registry_reconciliation_total",
        kind: MetricKind::Counter,
        unit: None,
        help: "Tenant namespaces registry reconciliation found wrong — \
               registered but unbound, or bound to nothing. One increment per \
               affected namespace per pass, and a pass runs at most once a \
               minute, so a non-zero rate is a standing inconsistency.",
    },
    MetricDescription {
        name: "memory_claim_pipeline_total",
        kind: MetricKind::Counter,
        unit: None,
        help: "Claim pipeline events by stage, schema, outcome and reason. \
               The labels are bounded and carry no claim, subject or project \
               identifier: those are the values that would turn a counter into \
               a disclosure.",
    },
    MetricDescription {
        name: "memory_claim_pipeline_duration_seconds",
        kind: MetricKind::Histogram,
        unit: Some(metrics::Unit::Seconds),
        help: "Claim reconciliation duration, by stage and schema.",
    },
    MetricDescription {
        name: "memory_claim_candidates_considered",
        kind: MetricKind::Histogram,
        unit: None,
        help: "Candidates considered for one claim slot, by schema. A count, \
               not a duration: read the mean as _sum over _count. A quantile \
               is useless here — every observation below the first reported \
               quantile collapses to zero, so 'no candidates' becomes \
               indistinguishable from 'a handful'. The name deliberately does \
               not end in _count, so the exporter appends that suffix to the \
               observation count and keeps it off the bare name the quantile \
               lines share.",
    },
    MetricDescription {
        name: "memory_claim_relations_active",
        kind: MetricKind::Gauge,
        unit: None,
        help: "Claim relations currently stored, by schema and outcome. A \
               series appears only after its first set, so an absent series \
               means 'never written', not zero.",
    },
    MetricDescription {
        name: "memory_claim_backfill_facts_total",
        kind: MetricKind::Counter,
        unit: None,
        help: "Facts backfilled into the claim store, by outcome and reason.",
    },
    MetricDescription {
        name: "memory_fs_watch_revisions_total",
        kind: MetricKind::Counter,
        unit: None,
        help: "Inbox revisions processed, by outcome. This is the product's \
               automatic ingestion path: a rising failed count is knowledge \
               that stopped arriving without anything saying so.",
    },
    MetricDescription {
        name: "memory_fs_watch_retries_total",
        kind: MetricKind::Counter,
        unit: None,
        help: "Inbox revision retries, by pipeline stage and failure class.",
    },
    MetricDescription {
        name: "memory_fs_watch_scan_files_total",
        kind: MetricKind::Counter,
        unit: None,
        help: "Files seen by the startup scan, by outcome. Emitted once at \
               startup and then flat for the process lifetime, so a window \
               longer than a boot shows no movement at all.",
    },
    MetricDescription {
        name: "memory_fs_watch_queue_depth",
        kind: MetricKind::Gauge,
        unit: None,
        help: "Inbox queue depth at the moment of startup recovery, never \
               updated again. A snapshot, not a live backlog: a growing queue is \
               invisible here, so do not alert on it.",
    },
    MetricDescription {
        name: "memory_fs_watch_inflight",
        kind: MetricKind::Gauge,
        unit: None,
        help: "Whether a revision is being processed. Boolean rather than a \
               count: the processor is sequential, so this is only ever 0 or 1 \
               and cannot show concurrency.",
    },
    MetricDescription {
        name: "memory_fs_watch_degraded",
        kind: MetricKind::Gauge,
        unit: None,
        help: "Whether the watcher backend has exhausted its retries. A \
               one-way latch for the process lifetime — once 1 it stays 1, so \
               read it with max_over_time rather than avg.",
    },
    MetricDescription {
        name: "memory_fs_watch_revision_duration_seconds",
        kind: MetricKind::Histogram,
        unit: Some(metrics::Unit::Seconds),
        help: "Inbox revision duration, by outcome. A single revision may run \
               to its attempt timeout, so the upper quantiles can sit well past \
               the last reported bucket.",
    },
];
