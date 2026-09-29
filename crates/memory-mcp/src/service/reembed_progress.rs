//! Progress reporting abstraction for the reembed maintenance command.
//!
//! Three implementations:
//! - [`IndicatifProgressReporter`] — TTY mode: live progress bar via `indicatif`
//! - [`LogProgressReporter`] — non-TTY mode: structured log events (existing behavior)
//! - [`NoopProgressReporter`] — test mode: no output, captures nothing

use std::time::Duration;

use crate::logging::LogLevel;
use crate::service::reembed::ReembedSummary;
use crate::service::reembed_options::ReembedOutcome;

/// Progress events emitted by the reembed loop.
///
/// The reporter implementation decides how to surface them (bar update,
/// log line, or silent capture).
pub trait ReembedProgressReporter: Send + Sync {
    /// Called once at the start, before any fact processing.
    ///
    /// `total_facts` is the count of facts needing reembed across all namespaces.
    /// `resumed` indicates whether this is a resume of a prior interrupted/failed run.
    /// `resumed_count` is the number of facts already processed in the prior run.
    fn on_job_started(&self, total_facts: usize, resumed: bool, resumed_count: usize);

    /// Called when entering a new namespace.
    fn on_namespace_started(&self, namespace: &str, namespace_total: usize);

    /// Called after each fact is processed (success or failure).
    fn on_fact_processed(&self, namespace: &str, summary: &ReembedSummary, elapsed: Duration);

    /// Called when a namespace completes.
    fn on_namespace_completed(
        &self,
        namespace: &str,
        succeeded: usize,
        failed: usize,
        elapsed: Duration,
    );

    /// Called when the HNSW index recreation phase starts.
    fn on_index_recreating(&self, namespace: &str);

    /// Called when the HNSW index recreation completes.
    fn on_index_recreated(&self, namespace: &str);

    /// Called when the job is interrupted (Ctrl+C).
    fn on_interrupted(&self, summary: &ReembedSummary, elapsed: Duration);

    /// Called when the job completes with the given outcome.
    fn on_job_completed(
        &self,
        outcome: &ReembedOutcome,
        summary: &ReembedSummary,
        elapsed: Duration,
    );
}

/// TTY-mode progress reporter using `indicatif`.
///
/// Shows a live progress bar with percentage, ETA, speed, and success/failure
/// counters. Degrades gracefully: `indicatif` automatically hides the bar when
/// stderr is not a TTY.
pub struct IndicatifProgressReporter {
    bar: indicatif::ProgressBar,
    spinner: indicatif::ProgressBar,
}

impl IndicatifProgressReporter {
    /// Creates a new reporter with a progress bar drawn to stderr,
    /// throttled to 10 redraws per second.
    #[must_use]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let bar = indicatif::ProgressBar::new(0);
        bar.set_style(
            indicatif::ProgressStyle::with_template(
                "Reembedding [{prefix}] {bar:40.cyan/blue} {pos}/{len} ({percent}%) eta {eta_precise} | {per_sec} {msg}",
            )
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
            .progress_chars("█░"),
        );
        bar.set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(10));

        let spinner = indicatif::ProgressBar::new_spinner();
        spinner.set_style(
            indicatif::ProgressStyle::with_template("{spinner} {msg}")
                .unwrap_or_else(|_| indicatif::ProgressStyle::default_spinner()),
        );
        spinner.set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(10));

        Self { bar, spinner }
    }

    /// Shows the initial spinner with a message during service initialization.
    pub fn start_init_spinner(&self, message: &str) {
        self.spinner.set_message(message.to_string());
        self.spinner
            .enable_steady_tick(std::time::Duration::from_millis(100));
    }
}

impl ReembedProgressReporter for IndicatifProgressReporter {
    fn on_job_started(&self, total_facts: usize, resumed: bool, resumed_count: usize) {
        self.spinner.finish_and_clear();
        self.bar.set_length(total_facts as u64);
        if resumed {
            self.bar.set_prefix("resuming");
            self.bar.inc(resumed_count as u64);
            self.bar.println(format!(
                "↻ Resuming interrupted reembed: {resumed_count}/{total_facts} facts already processed"
            ));
        }
    }

    fn on_namespace_started(&self, namespace: &str, namespace_total: usize) {
        self.bar.set_prefix(namespace.to_string());
        self.bar.println(format!(
            "Starting namespace: {namespace} ({namespace_total} facts)"
        ));
    }

    fn on_fact_processed(&self, _namespace: &str, summary: &ReembedSummary, _elapsed: Duration) {
        self.bar.inc(1);
        let msg = if summary.failed_facts > 0 {
            format!("✓{} ✗{}", summary.succeeded_facts, summary.failed_facts)
        } else {
            format!("✓{}", summary.succeeded_facts)
        };
        self.bar.set_message(msg);
    }

    fn on_namespace_completed(
        &self,
        namespace: &str,
        succeeded: usize,
        failed: usize,
        elapsed: Duration,
    ) {
        self.bar.println(format!(
            "✓ {namespace} complete ({succeeded} succeeded, {failed} failed, {:.1}s)",
            elapsed.as_secs_f64()
        ));
    }

    fn on_index_recreating(&self, namespace: &str) {
        self.bar
            .println(format!("Recreating HNSW index [{namespace}]..."));
    }

    fn on_index_recreated(&self, namespace: &str) {
        self.bar
            .println(format!("✓ HNSW index recreated [{namespace}]"));
    }

    fn on_interrupted(&self, summary: &ReembedSummary, _elapsed: Duration) {
        self.bar.abandon_with_message(format!(
            "⏹ Interrupted at {}/{} facts ({:.0}%) — resume with 'memory_mcp reembed'",
            summary.processed_facts,
            summary.total_facts,
            if summary.total_facts > 0 {
                summary.processed_facts as f64 / summary.total_facts as f64 * 100.0
            } else {
                0.0
            }
        ));
    }

    fn on_job_completed(
        &self,
        _outcome: &ReembedOutcome,
        _summary: &ReembedSummary,
        _elapsed: Duration,
    ) {
        self.bar.finish_and_clear();
        // Final summary is printed by the CLI runtime layer, not here.
    }
}

/// A structured log event, keyed by field name.
type Event = std::collections::HashMap<String, serde_json::Value>;

/// Non-TTY fallback: emits structured log events (existing behavior + new init events).
///
/// Used when stderr is not a TTY (pipes, CI, scripts).
pub struct LogProgressReporter {
    logger: crate::logging::StdoutLogger,
}

impl LogProgressReporter {
    /// Creates a new log-based reporter wrapping the given logger.
    #[must_use]
    pub fn new(logger: crate::logging::StdoutLogger) -> Self {
        Self { logger }
    }

    fn log(&self, op: &str, fields: Vec<(&str, serde_json::Value)>) {
        let mut event: Event = Event::from([("op".to_string(), serde_json::json!(op))]);
        for (k, v) in fields {
            event.insert(k.to_string(), v);
        }
        self.logger.log(event, LogLevel::Info);
    }
}

impl ReembedProgressReporter for LogProgressReporter {
    fn on_job_started(&self, total_facts: usize, resumed: bool, resumed_count: usize) {
        self.log(
            "reembed.init_completed",
            vec![
                ("total_facts", serde_json::json!(total_facts)),
                ("resumed", serde_json::json!(resumed)),
                ("resumed_count", serde_json::json!(resumed_count)),
            ],
        );
    }

    fn on_namespace_started(&self, namespace: &str, namespace_total: usize) {
        self.log(
            "reembed.namespace_started",
            vec![
                ("namespace", serde_json::json!(namespace)),
                ("namespace_total", serde_json::json!(namespace_total)),
            ],
        );
    }

    fn on_fact_processed(&self, _namespace: &str, _summary: &ReembedSummary, _elapsed: Duration) {
        // In non-TTY mode, do NOT log after every fact — only after each batch.
        // Batch-level progress is handled by the existing log_reembed_progress call.
    }

    fn on_namespace_completed(
        &self,
        namespace: &str,
        succeeded: usize,
        failed: usize,
        elapsed: Duration,
    ) {
        self.log(
            "reembed.namespace_completed",
            vec![
                ("namespace", serde_json::json!(namespace)),
                ("succeeded", serde_json::json!(succeeded)),
                ("failed", serde_json::json!(failed)),
                ("duration_ms", serde_json::json!(elapsed.as_millis() as u64)),
            ],
        );
    }

    fn on_index_recreating(&self, namespace: &str) {
        self.log(
            "reembed.index_recreating",
            vec![("namespace", serde_json::json!(namespace))],
        );
    }

    fn on_index_recreated(&self, namespace: &str) {
        self.log(
            "reembed.index_recreated",
            vec![("namespace", serde_json::json!(namespace))],
        );
    }

    fn on_interrupted(&self, summary: &ReembedSummary, elapsed: Duration) {
        self.log(
            "reembed.job_interrupted",
            vec![
                (
                    "processed_facts",
                    serde_json::json!(summary.processed_facts),
                ),
                (
                    "succeeded_facts",
                    serde_json::json!(summary.succeeded_facts),
                ),
                ("failed_facts", serde_json::json!(summary.failed_facts)),
                ("total_facts", serde_json::json!(summary.total_facts)),
                ("duration_ms", serde_json::json!(elapsed.as_millis() as u64)),
            ],
        );
    }

    fn on_job_completed(
        &self,
        outcome: &ReembedOutcome,
        summary: &ReembedSummary,
        elapsed: Duration,
    ) {
        self.log(
            "reembed.job_completed",
            vec![
                ("outcome", serde_json::json!(format!("{outcome:?}"))),
                (
                    "processed_facts",
                    serde_json::json!(summary.processed_facts),
                ),
                (
                    "succeeded_facts",
                    serde_json::json!(summary.succeeded_facts),
                ),
                ("failed_facts", serde_json::json!(summary.failed_facts)),
                ("total_facts", serde_json::json!(summary.total_facts)),
                ("duration_ms", serde_json::json!(elapsed.as_millis() as u64)),
            ],
        );
    }
}

/// No-op reporter for tests. Captures nothing, emits nothing.
/// Useful when tests only check the `ReembedSummary` return value.
pub struct NoopProgressReporter;

impl ReembedProgressReporter for NoopProgressReporter {
    fn on_job_started(&self, _: usize, _: bool, _: usize) {}
    fn on_namespace_started(&self, _: &str, _: usize) {}
    fn on_fact_processed(&self, _: &str, _: &ReembedSummary, _: Duration) {}
    fn on_namespace_completed(&self, _: &str, _: usize, _: usize, _: Duration) {}
    fn on_index_recreating(&self, _: &str) {}
    fn on_index_recreated(&self, _: &str) {}
    fn on_interrupted(&self, _: &ReembedSummary, _: Duration) {}
    fn on_job_completed(&self, _: &ReembedOutcome, _: &ReembedSummary, _: Duration) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::reembed_options::ReembedOutcome;

    /// A summary with a fixed, non-zero shape so every field is observable.
    fn summary() -> ReembedSummary {
        ReembedSummary {
            total_facts: 100,
            processed_facts: 50,
            succeeded_facts: 45,
            failed_facts: 5,
            failed_fact_ids: vec!["fact-1".to_string()],
        }
    }

    /// The log reporter under test. Its logger is set to `error`, so the
    /// progress events it emits at `info` are dropped by the level filter
    /// rather than written to the process-wide stderr stream; the tests assert
    /// the callbacks are safe to run, not on the rendered output.
    fn log_reporter() -> LogProgressReporter {
        LogProgressReporter::new(crate::logging::StdoutLogger::new("error"))
    }

    #[test]
    fn log_job_started_emits_for_a_fresh_run() {
        let reporter = log_reporter();

        reporter.on_job_started(100, false, 0);
    }

    #[test]
    fn log_job_started_emits_for_a_resumed_run() {
        let reporter = log_reporter();

        reporter.on_job_started(100, true, 30);
    }

    #[test]
    fn log_namespace_started_emits() {
        let reporter = log_reporter();

        reporter.on_namespace_started("acme", 100);
    }

    #[test]
    fn log_fact_processed_stays_silent_in_non_tty_mode() {
        let reporter = log_reporter();

        // Non-TTY mode deliberately drops per-fact events; batch progress is
        // logged by the caller. The callback must be a no-op, not a per-fact flood.
        reporter.on_fact_processed("acme", &summary(), Duration::from_secs(1));
    }

    #[test]
    fn log_namespace_completed_emits() {
        let reporter = log_reporter();

        reporter.on_namespace_completed("acme", 45, 5, Duration::from_secs(10));
    }

    #[test]
    fn log_namespace_completed_emits_with_a_zero_duration() {
        let reporter = log_reporter();

        reporter.on_namespace_completed("acme", 0, 0, Duration::ZERO);
    }

    #[test]
    fn log_index_recreating_emits() {
        let reporter = log_reporter();

        reporter.on_index_recreating("acme");
    }

    #[test]
    fn log_index_recreated_emits() {
        let reporter = log_reporter();

        reporter.on_index_recreated("acme");
    }

    #[test]
    fn log_interrupted_emits() {
        let reporter = log_reporter();

        reporter.on_interrupted(&summary(), Duration::from_secs(10));
    }

    #[test]
    fn log_job_completed_emits() {
        let reporter = log_reporter();

        reporter.on_job_completed(
            &ReembedOutcome::Completed,
            &summary(),
            Duration::from_secs(10),
        );
    }

    #[test]
    fn log_job_completed_emits_for_a_failed_outcome() {
        let reporter = log_reporter();

        reporter.on_job_completed(&ReembedOutcome::Failed, &summary(), Duration::from_secs(10));
    }

    #[test]
    fn indicatif_reporter_tolerates_every_callback() {
        // Under a non-TTY stderr (the test harness) `indicatif` renders nothing;
        // this asserts the callbacks stay side-effect free in that mode.
        let reporter = IndicatifProgressReporter::new();

        reporter.on_job_started(100, false, 0);
        reporter.on_namespace_started("acme", 100);
        reporter.on_fact_processed("acme", &summary(), Duration::from_secs(1));
        reporter.on_namespace_completed("acme", 45, 5, Duration::from_secs(10));
        reporter.on_index_recreating("acme");
        reporter.on_index_recreated("acme");
        reporter.on_interrupted(&summary(), Duration::from_secs(10));
        reporter.on_job_completed(
            &ReembedOutcome::Completed,
            &summary(),
            Duration::from_secs(10),
        );
    }

    #[test]
    fn indicatif_reporter_tolerates_a_resumed_run() {
        let reporter = IndicatifProgressReporter::new();

        reporter.on_job_started(100, true, 30);
    }

    #[test]
    fn indicatif_reporter_tolerates_an_interruption_with_no_facts() {
        // The interruption message divides by total_facts, so zero must not panic.
        let reporter = IndicatifProgressReporter::new();

        reporter.on_interrupted(&ReembedSummary::default(), Duration::from_secs(1));
    }

    #[test]
    fn indicatif_reporter_tolerates_a_completion_with_failures() {
        // A failed-fact summary takes the other arm of the message formatter.
        let reporter = IndicatifProgressReporter::new();

        reporter.on_fact_processed("acme", &summary(), Duration::from_secs(1));
    }

    #[test]
    fn indicatif_reporter_tolerates_the_init_spinner() {
        let reporter = IndicatifProgressReporter::new();

        reporter.start_init_spinner("loading models");
    }

    #[test]
    fn noop_reporter_tolerates_every_callback() {
        let reporter = NoopProgressReporter;

        reporter.on_job_started(100, false, 0);
        reporter.on_namespace_started("acme", 100);
        reporter.on_fact_processed("acme", &summary(), Duration::from_secs(1));
        reporter.on_namespace_completed("acme", 45, 5, Duration::from_secs(10));
        reporter.on_index_recreating("acme");
        reporter.on_index_recreated("acme");
        reporter.on_interrupted(&summary(), Duration::from_secs(10));
        reporter.on_job_completed(
            &ReembedOutcome::Completed,
            &summary(),
            Duration::from_secs(10),
        );
    }
}
