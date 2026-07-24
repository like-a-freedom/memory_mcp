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
    pub fn new() -> Self {
        let bar = indicatif::ProgressBar::new(0);
        bar.set_style(
            indicatif::ProgressStyle::with_template(
                "Reembedding [{prefix}] {bar:40.cyan/blue} {pos}/{len} ({percent}%) eta {eta_precise} | {per_sec} {msg}",
            )
            .expect("valid indicatif template")
            .progress_chars("█░"),
        );
        bar.set_draw_target(indicatif::ProgressDrawTarget::stderr_with_hz(10));

        let spinner = indicatif::ProgressBar::new_spinner();
        spinner.set_style(
            indicatif::ProgressStyle::with_template("{spinner} {msg}")
                .expect("valid spinner template"),
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

    /// Finishes the init spinner with a completion message.
    pub fn finish_init(&self, message: &str) {
        self.spinner.finish_with_message(message.to_string());
    }
}

impl Default for IndicatifProgressReporter {
    fn default() -> Self {
        Self::new()
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
        let mut event: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::from([("op".to_string(), serde_json::json!(op))]);
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

    #[test]
    fn noop_reporter_compiles_and_runs() {
        let reporter = NoopProgressReporter;
        let summary = ReembedSummary {
            total_facts: 100,
            processed_facts: 50,
            succeeded_facts: 45,
            failed_facts: 5,
            ..ReembedSummary::default()
        };
        reporter.on_job_started(100, false, 0);
        reporter.on_namespace_started("org", 100);
        reporter.on_fact_processed("org", &summary, Duration::from_secs(10));
        reporter.on_namespace_completed("org", 45, 5, Duration::from_secs(10));
        reporter.on_index_recreating("org");
        reporter.on_index_recreated("org");
        reporter.on_interrupted(&summary, Duration::from_secs(10));
        reporter.on_job_completed(
            &ReembedOutcome::Completed,
            &summary,
            Duration::from_secs(10),
        );
    }
}
