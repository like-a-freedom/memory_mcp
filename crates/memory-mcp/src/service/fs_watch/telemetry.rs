//! Bounded filesystem-watch metrics and structured-event helpers.
//!
//! Metric labels are strictly bounded; paths, hashes, IDs, and error text are
//! never labels. Unknown values map to `other`.
//!
//! Several metric families are emitted by the runtime in a later task; until
//! then they are exercised only by tests, so dead-code analysis is relaxed.
#![allow(dead_code)]

use std::time::Instant;

use crate::models::inbox_revision::InboxFailureClass;
use crate::observability::{
    METRIC_FS_WATCH_DEGRADED, METRIC_FS_WATCH_INFLIGHT, METRIC_FS_WATCH_QUEUE_DEPTH,
    METRIC_FS_WATCH_RETRIES_TOTAL, METRIC_FS_WATCH_REVISION_DURATION_SECONDS,
    METRIC_FS_WATCH_REVISIONS_TOTAL, METRIC_FS_WATCH_SCAN_FILES_TOTAL,
};

use super::processor::ProcessOutcome;

const KNOWN_OUTCOMES: &[&str] = &["processed", "failed", "skipped_duplicate", "interrupted"];

const KNOWN_RETRY_STAGES: &[&str] = &["backend", "read", "ingest", "extract"];

const KNOWN_RETRY_REASONS: &[&str] = &[
    "io",
    "storage",
    "model",
    "timeout",
    "channel",
    "corrupt",
    "validation",
    "other_transient",
];

const KNOWN_SCAN_OUTCOMES: &[&str] = &[
    "enqueued",
    "skipped_symlink",
    "skipped_unsupported",
    "skipped_not_regular",
    "failed_read",
    "interrupted",
];

/// Maps a revision outcome to a bounded label value.
pub(crate) fn revision_outcome_label(outcome: ProcessOutcome) -> &'static str {
    match outcome {
        ProcessOutcome::Processed => "processed",
        ProcessOutcome::FailedNonRetryable | ProcessOutcome::FailedRetriesExhausted => "failed",
        ProcessOutcome::Interrupted => "interrupted",
    }
}

/// Maps a retry stage to a bounded label value.
pub(crate) fn retry_stage_label(stage: &str) -> &'static str {
    KNOWN_RETRY_STAGES
        .iter()
        .copied()
        .find(|known| *known == stage)
        .unwrap_or("other")
}

/// Maps a failure class to a bounded retry reason.
pub(crate) fn retry_reason_label(class: InboxFailureClass) -> &'static str {
    match class {
        InboxFailureClass::Validation => "validation",
        InboxFailureClass::Corrupt => "corrupt",
        InboxFailureClass::Io => "io",
        InboxFailureClass::Storage => "storage",
        InboxFailureClass::Model => "model",
        InboxFailureClass::Timeout => "timeout",
        InboxFailureClass::Channel => "channel",
        InboxFailureClass::OtherTransient => "other_transient",
    }
}

/// Bounded scan outcome label.
pub(crate) fn scan_outcome_label(outcome: &str) -> &'static str {
    KNOWN_SCAN_OUTCOMES
        .iter()
        .copied()
        .find(|known| *known == outcome)
        .unwrap_or("other")
}

/// Telemetry facade for the filesystem-watch pipeline.
#[derive(Clone, Default)]
pub struct FsWatchTelemetry;

impl FsWatchTelemetry {
    pub fn new() -> Self {
        Self
    }

    pub(crate) fn record_revision(&self, outcome: ProcessOutcome) {
        let outcome = revision_outcome_label(outcome);
        metrics::counter!(METRIC_FS_WATCH_REVISIONS_TOTAL, "outcome" => outcome).increment(1);
    }

    pub(crate) fn record_success(&self) {}

    pub(crate) fn record_retry(&self, stage: &str, class: InboxFailureClass) {
        let stage = retry_stage_label(stage);
        let reason = retry_reason_label(class);
        metrics::counter!(METRIC_FS_WATCH_RETRIES_TOTAL, "stage" => stage, "reason" => reason)
            .increment(1);
    }

    pub(crate) fn record_scan_file(&self, outcome: &str) {
        let outcome = scan_outcome_label(outcome);
        metrics::counter!(METRIC_FS_WATCH_SCAN_FILES_TOTAL, "outcome" => outcome).increment(1);
    }

    pub(crate) fn set_queue_depth(&self, depth: usize) {
        metrics::gauge!(METRIC_FS_WATCH_QUEUE_DEPTH).set(depth as f64);
    }

    pub(crate) fn set_inflight(&self, inflight: usize) {
        metrics::gauge!(METRIC_FS_WATCH_INFLIGHT).set(inflight as f64);
    }

    pub(crate) fn set_degraded(&self, degraded: bool) {
        metrics::gauge!(METRIC_FS_WATCH_DEGRADED).set(if degraded { 1.0 } else { 0.0 });
    }

    pub(crate) fn record_revision_duration(
        &self,
        outcome: ProcessOutcome,
        duration: std::time::Duration,
    ) {
        let outcome = revision_outcome_label(outcome);
        metrics::histogram!(METRIC_FS_WATCH_REVISION_DURATION_SECONDS, "outcome" => outcome)
            .record(duration.as_secs_f64());
    }
}

/// Timing helper for revision durations.
#[derive(Debug)]
pub(crate) struct RevisionTimer {
    started: Instant,
}

impl RevisionTimer {
    pub(crate) fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    pub(crate) fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
}
