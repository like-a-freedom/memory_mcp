//! Bounded filesystem-watch metrics and structured-event helpers.
//!
//! Metric labels are strictly bounded; paths, hashes, IDs, and error text are
//! never labels. Unknown values map to `other`.
//!
//! Every family here is written by the runtime or the processor once a watcher
//! starts. A few helpers (`KNOWN_OUTCOMES`, `KNOWN_RETRY_REASONS`,
//! `RevisionTimer`) have no caller in this build, so dead-code analysis stays
//! relaxed for this module.
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
    "skipped_outside_root",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_processed_revision_is_labelled_processed() {
        assert_eq!(
            revision_outcome_label(ProcessOutcome::Processed),
            "processed"
        );
    }

    #[test]
    fn a_non_retryable_failure_is_labelled_failed() {
        let observed = revision_outcome_label(ProcessOutcome::FailedNonRetryable);

        assert_eq!(observed, "failed");
    }

    #[test]
    fn an_exhausted_retry_budget_is_labelled_failed() {
        let observed = revision_outcome_label(ProcessOutcome::FailedRetriesExhausted);

        assert_eq!(observed, "failed");
    }

    #[test]
    fn an_interrupted_revision_is_labelled_interrupted() {
        assert_eq!(
            revision_outcome_label(ProcessOutcome::Interrupted),
            "interrupted"
        );
    }

    #[test]
    fn a_known_retry_stage_is_passed_through() {
        assert_eq!(retry_stage_label("backend"), "backend");
    }

    #[test]
    fn an_unknown_retry_stage_maps_to_other() {
        // A caller-supplied stage must never become a new label value.
        assert_eq!(retry_stage_label("/etc/passwd"), "other");
    }

    #[test]
    fn an_empty_retry_stage_maps_to_other() {
        assert_eq!(retry_stage_label(""), "other");
    }

    #[test]
    fn a_validation_failure_is_labelled_validation() {
        assert_eq!(
            retry_reason_label(InboxFailureClass::Validation),
            "validation"
        );
    }

    #[test]
    fn an_io_failure_is_labelled_io() {
        assert_eq!(retry_reason_label(InboxFailureClass::Io), "io");
    }

    #[test]
    fn a_storage_failure_is_labelled_storage() {
        assert_eq!(retry_reason_label(InboxFailureClass::Storage), "storage");
    }

    #[test]
    fn a_model_failure_is_labelled_model() {
        assert_eq!(retry_reason_label(InboxFailureClass::Model), "model");
    }

    #[test]
    fn a_timeout_failure_is_labelled_timeout() {
        assert_eq!(retry_reason_label(InboxFailureClass::Timeout), "timeout");
    }

    #[test]
    fn a_channel_failure_is_labelled_channel() {
        assert_eq!(retry_reason_label(InboxFailureClass::Channel), "channel");
    }

    #[test]
    fn a_corrupt_failure_is_labelled_corrupt() {
        assert_eq!(retry_reason_label(InboxFailureClass::Corrupt), "corrupt");
    }

    #[test]
    fn an_unclassified_transient_failure_is_labelled_other_transient() {
        assert_eq!(
            retry_reason_label(InboxFailureClass::OtherTransient),
            "other_transient"
        );
    }

    #[test]
    fn a_known_scan_outcome_is_passed_through() {
        assert_eq!(scan_outcome_label("enqueued"), "enqueued");
    }

    #[test]
    fn a_symlink_skip_is_labelled_skipped_symlink() {
        assert_eq!(scan_outcome_label("skipped_symlink"), "skipped_symlink");
    }

    #[test]
    fn an_unsupported_skip_is_labelled_skipped_unsupported() {
        assert_eq!(
            scan_outcome_label("skipped_unsupported"),
            "skipped_unsupported"
        );
    }

    #[test]
    fn a_read_failure_is_labelled_failed_read() {
        assert_eq!(scan_outcome_label("failed_read"), "failed_read");
    }

    #[test]
    fn an_unknown_scan_outcome_maps_to_other() {
        // A file name or path must never become a metric label.
        assert_eq!(scan_outcome_label("/home/user/secret.txt"), "other");
    }

    #[test]
    fn an_empty_scan_outcome_maps_to_other() {
        assert_eq!(scan_outcome_label(""), "other");
    }

    #[test]
    fn recording_a_revision_does_not_panic() {
        let telemetry = FsWatchTelemetry::new();

        telemetry.record_revision(ProcessOutcome::Processed);
    }

    #[test]
    fn recording_a_retry_does_not_panic() {
        let telemetry = FsWatchTelemetry::new();

        telemetry.record_retry("backend", InboxFailureClass::Storage);
    }

    #[test]
    fn recording_an_unknown_retry_stage_does_not_panic() {
        let telemetry = FsWatchTelemetry::new();

        telemetry.record_retry("/etc/passwd", InboxFailureClass::OtherTransient);
    }

    #[test]
    fn recording_a_scan_file_does_not_panic() {
        let telemetry = FsWatchTelemetry::new();

        telemetry.record_scan_file("enqueued");
    }

    #[test]
    fn setting_the_queue_depth_does_not_panic() {
        let telemetry = FsWatchTelemetry::new();

        telemetry.set_queue_depth(7);
    }

    #[test]
    fn setting_the_inflight_count_does_not_panic() {
        let telemetry = FsWatchTelemetry::new();

        telemetry.set_inflight(3);
    }

    #[test]
    fn setting_the_degraded_flag_does_not_panic() {
        let telemetry = FsWatchTelemetry::new();

        telemetry.set_degraded(true);
    }

    #[test]
    fn recording_a_revision_duration_does_not_panic() {
        let telemetry = FsWatchTelemetry::new();

        telemetry.record_revision_duration(
            ProcessOutcome::Processed,
            std::time::Duration::from_millis(5),
        );
    }

    #[test]
    fn a_fresh_revision_timer_reports_a_non_negative_elapsed() {
        let timer = RevisionTimer::start();

        assert!(timer.elapsed() < std::time::Duration::from_secs(60));
    }
}
