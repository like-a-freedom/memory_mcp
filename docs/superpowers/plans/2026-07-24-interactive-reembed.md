# Interactive Reembed with Progress, Cancellation, and Continue-on-Error

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

Transform `memory_mcp reembed` from a silent background operation into an interactive, observable, and resilient maintenance workflow with live progress, graceful Ctrl+C handling, and continue-on-error semantics.

**Architecture:** Add an `indicatif`-based progress reporter abstraction that wraps the existing `reembed_all_facts` loop. Introduce a `ReembedOptions` config struct passed from CLI flags into the service layer. Add `CancellationToken` for graceful interrupt. Extend the persisted job state with `last_failed_fact_ids` per namespace and new statuses (`interrupted`, `completed_with_errors`). Keep all progress reporting behind a trait so the service layer stays testable without a real terminal.

**Tech Stack:** Rust 2024, `indicatif` (new dep), `tokio-util` `CancellationToken` (already available via tokio-util), `clap` derive, existing `MemoryService` / `reembed.rs`.

## Global Constraints

- Rust 2024 edition, `resolver = "3"`.
- Zero `unwrap()` / `expect()` / `panic!()` in production code (`.expect("lock")` on Mutex is acceptable per project convention).
- No lock guard held across `.await`.
- Public MCP tool surface is frozen at 8 tools — `reembed` is a CLI-only maintenance command, not an MCP tool. No new MCP tools.
- `reembed` is not a hidden subcommand — it is a documented public CLI command.
- Feature flags are additive: `default = []`.
- `cargo clippy --all-targets` must produce zero warnings.
- `cargo fmt --all --check` must produce zero diff.
- `cargo test` must produce zero failures.
- All new public/pub(crate) items must have `///` doc comments.
- ADR-0018 must be created to record the architectural decision.
- README.md and AGENTS.md must be updated to document the new UX.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `src/service/reembed.rs` | Core reembed logic — modify loop for continue-on-error, cancellation checks, progress callbacks |
| `src/service/reembed_progress.rs` | **NEW** — `ReembedProgressReporter` trait + `IndicatifProgressReporter` (TTY) + `LogProgressReporter` (non-TTY fallback) |
| `src/service/reembed_options.rs` | **NEW** — `ReembedOptions` struct (max_failures, retry_failed) + `ReembedOutcome` enum |
| `src/cli/args.rs` | Add `ReembedArgs` clap struct with `--max-failures` and `--retry-failed` flags |
| `src/cli/runtime.rs` | Wire `ReembedArgs` → `ReembedOptions` → `MemoryService::reembed_all_facts`, build progress reporter, handle Ctrl+C |
| `src/cli.rs` | Update `Command::Reembed` from unit variant to `Command::Reembed(ReembedArgs)` |
| `src/lib.rs` | Re-export `ReembedOptions`, `ReembedOutcome`, `ReembedSummary` |
| `Cargo.toml` | Add `indicatif = "0.17"` dependency |
| `docs/adr/0018-reembed-interactive-progress.md` | **NEW** — ADR documenting the decision |
| `README.md` | Update reembed sections |
| `AGENTS.md` | Update reembed references |
| `tests/eval_reembed_progress.rs` | **NEW** — integration tests for progress reporter, cancellation, continue-on-error |

---

## Task 1: Add `indicatif` dependency and `ReembedOptions`/`ReembedOutcome` types

**Files:**
- Modify: `Cargo.toml`
- Create: `src/service/reembed_options.rs`
- Modify: `src/service.rs` (add `mod reembed_options;`)
- Modify: `src/lib.rs` (re-export)

**Interfaces:**
- Produces: `ReembedOptions { max_failures: Option<usize>, retry_failed: bool }`, `ReembedOutcome { Completed, CompletedWithErrors, Failed, Interrupted, NothingToDo }`, `ReembedSummary` (already exists, extended with `failed_fact_ids: Vec<String>`)

- [ ] **Step 1: Add `indicatif` to Cargo.toml**

In `Cargo.toml`, under `[dependencies]`, add after the `clap` line:

```toml
indicatif = "0.17"
```

- [ ] **Step 2: Create `src/service/reembed_options.rs`**

```rust
//! Configuration and outcome types for the reembed maintenance command.

/// Options controlling reembed behavior.
///
/// Passed from CLI flags into `MemoryService::reembed_all_facts`.
#[derive(Debug, Clone)]
pub struct ReembedOptions {
    /// Maximum number of failed facts before aborting the run.
    ///
    /// `None` means use the default quota (10% of total, minimum 10).
    /// `Some(0)` means fail-fast on the first error (legacy behavior).
    pub max_failures: Option<usize>,
    /// If true, retry only facts marked as failed in a previous run.
    pub retry_failed: bool,
}

impl Default for ReembedOptions {
    fn default() -> Self {
        Self {
            max_failures: None,
            retry_failed: false,
        }
    }
}

impl ReembedOptions {
    /// Returns the effective max failures cap, applying the default quota
    /// (10% of total, minimum 10) when `max_failures` is `None`.
    pub fn effective_max_failures(&self, total_facts: usize) -> usize {
        match self.max_failures {
            Some(0) => 0,
            Some(n) => n,
            None => {
                let ten_percent = total_facts / 10;
                ten_percent.max(10)
            }
        }
    }
}

/// Final outcome of a reembed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReembedOutcome {
    /// All facts processed successfully, no failures.
    Completed,
    /// All facts processed, but some failures occurred within quota.
    CompletedWithErrors,
    /// Aborted because failure count exceeded the quota.
    Failed,
    /// Interrupted by the user (Ctrl+C).
    Interrupted,
    /// Nothing to do — all embeddings already match the target signature
    /// and no failed facts to retry.
    NothingToDo,
}

impl ReembedOutcome {
    /// Returns the process exit code for this outcome.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Completed | Self::CompletedWithErrors | Self::NothingToDo => 0,
            Self::Failed => 1,
            Self::Interrupted => 130,
        }
    }
}
```

- [ ] **Step 3: Register module in `src/service.rs`**

Add to `src/service.rs` (alongside the existing `mod reembed;`):

```rust
pub mod reembed_options;
```

- [ ] **Step 4: Re-export from `src/lib.rs`**

Add to the existing re-exports in `src/lib.rs`:

```rust
pub use crate::service::reembed_options::{ReembedOptions, ReembedOutcome};
```

- [ ] **Step 5: Write unit test for `effective_max_failures`**

In `src/service/reembed_options.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_quota_is_ten_percent_minimum_ten() {
        let opts = ReembedOptions::default();
        assert_eq!(opts.effective_max_failures(100), 10);
        assert_eq!(opts.effective_max_failures(50), 10);
        assert_eq!(opts.effective_max_failures(1000), 100);
        assert_eq!(opts.effective_max_failures(0), 10);
    }

    #[test]
    fn explicit_max_failures_overrides_default() {
        let opts = ReembedOptions {
            max_failures: Some(5),
            retry_failed: false,
        };
        assert_eq!(opts.effective_max_failures(1000), 5);
    }

    #[test]
    fn zero_max_failures_means_fail_fast() {
        let opts = ReembedOptions {
            max_failures: Some(0),
            retry_failed: false,
        };
        assert_eq!(opts.effective_max_failures(1000), 0);
    }

    #[test]
    fn exit_codes_match_conventions() {
        assert_eq!(ReembedOutcome::Completed.exit_code(), 0);
        assert_eq!(ReembedOutcome::CompletedWithErrors.exit_code(), 0);
        assert_eq!(ReembedOutcome::NothingToDo.exit_code(), 0);
        assert_eq!(ReembedOutcome::Failed.exit_code(), 1);
        assert_eq!(ReembedOutcome::Interrupted.exit_code(), 130);
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib reembed_options`
Expected: 4 tests pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/service/reembed_options.rs src/service.rs src/lib.rs
git commit -m "feat(reembed): add ReembedOptions, ReembedOutcome types and indicatif dep"
```

---

## Task 2: Create `ReembedProgressReporter` trait and implementations

**Files:**
- Create: `src/service/reembed_progress.rs`
- Modify: `src/service.rs` (add `mod reembed_progress;`)

**Interfaces:**
- Consumes: `ReembedSummary` (from `reembed.rs`), `ReembedOutcome` (from Task 1)
- Produces: `ReembedProgressReporter` trait, `IndicatifProgressReporter`, `LogProgressReporter`, `NoopProgressReporter`

- [ ] **Step 1: Create `src/service/reembed_progress.rs`**

```rust
//! Progress reporting abstraction for the reembed maintenance command.
//!
//! Three implementations:
//! - `IndicatifProgressReporter` — TTY mode: live progress bar via `indicatif`
//! - `LogProgressReporter` — non-TTY mode: structured log events (existing behavior)
//! - `NoopProgressReporter` — test mode: no output, captures state for assertions

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
    /// `total_facts` is the count of facts needing reembed across all namespaces.
    fn on_job_started(&self, total_facts: usize, resumed: bool, resumed_count: usize);

    /// Called when entering a new namespace.
    fn on_namespace_started(&self, namespace: &str, namespace_total: usize);

    /// Called after each fact is processed (success or failure).
    fn on_fact_processed(&self, namespace: &str, summary: &ReembedSummary, elapsed: Duration);

    /// Called when a namespace completes.
    fn on_namespace_completed(&self, namespace: &str, succeeded: usize, failed: usize, elapsed: Duration);

    /// Called when the HNSW index recreation phase starts.
    fn on_index_recreating(&self, namespace: &str);

    /// Called when the HNSW index recreation completes.
    fn on_index_recreated(&self, namespace: &str);

    /// Called when the job is interrupted (Ctrl+C).
    fn on_interrupted(&self, summary: &ReembedSummary, elapsed: Duration);

    /// Called when the job completes with the given outcome.
    fn on_job_completed(&self, outcome: &ReembedOutcome, summary: &ReembedSummary, elapsed: Duration);
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
        self.bar.println(format!("Starting namespace: {namespace} ({namespace_total} facts)"));
    }

    fn on_fact_processed(&self, namespace: &str, summary: &ReembedSummary, elapsed: Duration) {
        self.bar.inc(1);
        let msg = if summary.failed_facts > 0 {
            format!("✓{} ✗{}", summary.succeeded_facts, summary.failed_facts)
        } else {
            format!("✓{}", summary.succeeded_facts)
        };
        self.bar.set_message(msg);
    }

    fn on_namespace_completed(&self, namespace: &str, succeeded: usize, failed: usize, elapsed: Duration) {
        self.bar.println(format!(
            "✓ {namespace} complete ({succeeded} succeeded, {failed} failed, {:.1}s)",
            elapsed.as_secs_f64()
        ));
    }

    fn on_index_recreating(&self, namespace: &str) {
        self.bar.println(format!("Recreating HNSW index [{namespace}]..."));
    }

    fn on_index_recreated(&self, namespace: &str) {
        self.bar.println(format!("✓ HNSW index recreated [{namespace}]"));
    }

    fn on_interrupted(&self, summary: &ReembedSummary, elapsed: Duration) {
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

    fn on_job_completed(&self, outcome: &ReembedOutcome, summary: &ReembedSummary, elapsed: Duration) {
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

    fn on_fact_processed(&self, namespace: &str, summary: &ReembedSummary, _elapsed: Duration) {
        // In non-TTY mode, do NOT log after every fact — only after each batch.
        // Batch-level progress is handled by the existing log_reembed_progress call.
    }

    fn on_namespace_completed(&self, namespace: &str, succeeded: usize, failed: usize, elapsed: Duration) {
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
                ("processed_facts", serde_json::json!(summary.processed_facts)),
                ("succeeded_facts", serde_json::json!(summary.succeeded_facts)),
                ("failed_facts", serde_json::json!(summary.failed_facts)),
                ("total_facts", serde_json::json!(summary.total_facts)),
                ("duration_ms", serde_json::json!(elapsed.as_millis() as u64)),
            ],
        );
    }

    fn on_job_completed(&self, outcome: &ReembedOutcome, summary: &ReembedSummary, elapsed: Duration) {
        self.log(
            "reembed.job_completed",
            vec![
                ("outcome", serde_json::json!(format!("{outcome:?}"))),
                ("processed_facts", serde_json::json!(summary.processed_facts)),
                ("succeeded_facts", serde_json::json!(summary.succeeded_facts)),
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
```

- [ ] **Step 2: Register module in `src/service.rs`**

Add to `src/service.rs`:

```rust
pub mod reembed_progress;
```

- [ ] **Step 3: Write unit tests for `NoopProgressReporter` compilation**

In `src/service/reembed_progress.rs`, add at the bottom:

```rust
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
        };
        reporter.on_job_started(100, false, 0);
        reporter.on_namespace_started("org", 100);
        reporter.on_fact_processed("org", &summary, Duration::from_secs(10));
        reporter.on_namespace_completed("org", 45, 5, Duration::from_secs(10));
        reporter.on_index_recreating("org");
        reporter.on_index_recreated("org");
        reporter.on_interrupted(&summary, Duration::from_secs(10));
        reporter.on_job_completed(&ReembedOutcome::Completed, &summary, Duration::from_secs(10));
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib reembed_progress`
Expected: 1 test passes.

- [ ] **Step 5: Commit**

```bash
git add src/service/reembed_progress.rs src/service.rs
git commit -m "feat(reembed): add ReembedProgressReporter trait with indicatif, log, and noop impls"
```

---

## Task 3: Extend `ReembedSummary` with `failed_fact_ids` and update job state schema

**Files:**
- Modify: `src/service/reembed.rs`

**Interfaces:**
- Consumes: `ReembedSummary` (existing struct)
- Produces: `ReembedSummary` with new `failed_fact_ids: Vec<String>` field, updated `update_namespace_progress` storing failed IDs, updated `persist_reembed_job` serializing them

- [ ] **Step 1: Extend `ReembedSummary` struct**

In `src/service/reembed.rs`, update the struct:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReembedSummary {
    pub total_facts: usize,
    pub processed_facts: usize,
    pub succeeded_facts: usize,
    pub failed_facts: usize,
    /// IDs of facts that failed during this run (for `--retry-failed`).
    pub failed_fact_ids: Vec<String>,
}
```

- [ ] **Step 2: Update `update_namespace_progress` to store failed fact IDs**

The function currently stores counters. Extend it to also store a `failed_fact_ids` array per namespace. Find the existing `update_namespace_progress` function and replace it with:

```rust
fn update_namespace_progress(
    namespace_progress: &mut serde_json::Map<String, Value>,
    namespace: &str,
    status: &str,
    processed: usize,
    succeeded: usize,
    failed: usize,
    last_completed_fact_id: Option<&str>,
    failed_fact_ids: &[String],
) {
    let entry = serde_json::json!({
        "status": status,
        "processed": processed,
        "succeeded": succeeded,
        "failed": failed,
        "last_completed_fact_id": last_completed_fact_id,
        "failed_fact_ids": failed_fact_ids,
    });
    namespace_progress.insert(namespace.to_string(), entry);
}
```

- [ ] **Step 3: Update all call sites of `update_namespace_progress`**

There are 3 call sites in `reembed_all_facts`:
1. After a successful fact rewrite (line ~260)
2. After a failed fact rewrite (line ~300)
3. After namespace completion (line ~392)

Each call must now pass a `failed_fact_ids` slice. For the success case, pass the accumulated `namespace_failed_fact_ids`. For the failure case, append the fact_id to the list first.

Add a `Vec<String>` for `namespace_failed_fact_ids` at the start of the namespace loop:

```rust
let mut namespace_failed_fact_ids: Vec<String> = Vec::new();
```

Then update each call site to pass `&namespace_failed_fact_ids`.

- [ ] **Step 4: Update `persist_reembed_job` signature**

The function already serializes `namespace_progress`. No signature change needed — the `failed_fact_ids` are embedded in the namespace_progress entries.

- [ ] **Step 5: Run existing reembed tests to verify nothing broke**

Run: `cargo test --lib reembed`
Expected: All existing tests pass (some may need updating if they call `update_namespace_progress` directly — check and fix).

- [ ] **Step 6: Fix any test compilation errors from the signature change**

If existing tests in `reembed.rs` `mod tests` call `update_namespace_progress`, update them to pass `&[]` for the new parameter.

- [ ] **Step 7: Commit**

```bash
git add src/service/reembed.rs
git commit -m "feat(reembed): extend ReembedSummary with failed_fact_ids and update job state"
```

---

## Task 4: Refactor `reembed_all_facts` to accept `ReembedOptions`, `ReembedProgressReporter`, and `CancellationToken`

**Files:**
- Modify: `src/service/reembed.rs`

**Interfaces:**
- Consumes: `ReembedOptions`, `ReembedProgressReporter` trait, `tokio_util::sync::CancellationToken`
- Produces: Updated `reembed_all_facts` signature returning `Result<(ReembedSummary, ReembedOutcome), MemoryError>`

- [ ] **Step 1: Add imports to `src/service/reembed.rs`**

```rust
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::reembed_options::{ReembedOptions, ReembedOutcome};
use super::reembed_progress::ReembedProgressReporter;
```

- [ ] **Step 2: Change `reembed_all_facts` signature**

Replace:
```rust
pub async fn reembed_all_facts(&self) -> Result<ReembedSummary, MemoryError>
```

With:
```rust
pub async fn reembed_all_facts(
    &self,
    options: &ReembedOptions,
    progress: &dyn ReembedProgressReporter,
    cancel_token: &CancellationToken,
) -> Result<(ReembedSummary, ReembedOutcome), MemoryError>
```

- [ ] **Step 3: Add spinner/init phase logging**

At the very start of the function, before the existing `self.logger.log` call for `reembed.job_started`, the CLI runtime will start a spinner. The service just calls `progress.on_job_started` after computing `total_facts`.

Replace the existing `reembed.job_started` log block to also call `progress.on_job_started`:

```rust
let resumed_count = namespace_progress
    .values()
    .map(|v| v.get("processed").and_then(Value::as_u64).unwrap_or(0) as usize)
    .sum::<usize>();

progress.on_job_started(summary.total_facts, resumed, resumed_count);
```

- [ ] **Step 4: Add cancellation check in the fact loop**

Inside the `for fact in batch` loop, at the top of each iteration, add:

```rust
if cancel_token.is_cancelled() {
    progress.on_interrupted(&summary, started_at.elapsed());
    self.persist_reembed_job(
        &summary,
        &target_signature,
        target_dimension,
        &namespace_progress,
        Some(&started_at_rfc3339),
        Some(&chrono::Utc::now().to_rfc3339()),
        "interrupted",
        None,
        None,
        started_at.elapsed(),
    )
    .await?;
    return Ok((summary, ReembedOutcome::Interrupted));
}
```

- [ ] **Step 5: Replace fail-fast with continue-on-error**

In the `Err(err)` branch of the `match self.rewrite_fact_embedding(...)` block, replace the `return Err(...)` with:

```rust
// Continue-on-error: record the failure and proceed.
summary.processed_facts += 1;
summary.failed_facts += 1;
namespace_processed += 1;
namespace_failed += 1;
namespace_failed_fact_ids.push(fact_id.clone());
summary.failed_fact_ids.push(fact_id.clone());
// Advance the cursor past the failed fact so we don't loop on it.
last_completed_fact_id = Some(fact_id.clone());

update_namespace_progress(
    &mut namespace_progress,
    namespace,
    "running",
    namespace_processed,
    namespace_succeeded,
    namespace_failed,
    last_completed_fact_id.as_deref(),
    &namespace_failed_fact_ids,
);

// ... existing persist_reembed_job call with status "running" ...
// ... existing log calls ...

// Check quota
let max_failures = options.effective_max_failures(summary.total_facts);
if summary.failed_facts > max_failures {
    // Exceeded quota — abort.
    self.write_embedding_state(namespace, "failed", None, Some(REEMBED_JOB_ID)).await?;
    self.persist_reembed_job(
        &summary,
        &target_signature,
        target_dimension,
        &namespace_progress,
        Some(&started_at_rfc3339),
        Some(&chrono::Utc::now().to_rfc3339()),
        "failed",
        Some(namespace),
        Some(&format!("exceeded max_failures ({max_failures})")),
        started_at.elapsed(),
    ).await?;
    progress.on_job_completed(&ReembedOutcome::Failed, &summary, started_at.elapsed());
    return Ok((summary, ReembedOutcome::Failed));
}
```

- [ ] **Step 6: Add `progress.on_fact_processed` call after each fact**

After the `match` block (both Ok and Err branches), add:

```rust
progress.on_fact_processed(namespace, &summary, started_at.elapsed());
```

- [ ] **Step 7: Add `progress.on_namespace_started` and `on_namespace_completed` calls**

At the start of each namespace loop iteration:
```rust
progress.on_namespace_started(namespace, /* namespace_total */ 0);
```

After the namespace loop completes:
```rust
progress.on_namespace_completed(namespace, namespace_succeeded, namespace_failed, started_at.elapsed());
```

- [ ] **Step 8: Add `progress.on_index_recreating`/`on_index_recreated` calls**

Around the HNSW index recreation loop:
```rust
for namespace in &self.namespaces {
    progress.on_index_recreating(namespace);
    self.define_embedding_index(namespace, target_dimension).await.map_err(|err| { ... })?;
    progress.on_index_recreated(namespace);
}
```

- [ ] **Step 9: Determine final outcome and call `on_job_completed`**

At the end of the function, before returning, compute the outcome:

```rust
let outcome = if summary.failed_facts == 0 {
    ReembedOutcome::Completed
} else {
    ReembedOutcome::CompletedWithErrors
};

progress.on_job_completed(&outcome, &summary, started_at.elapsed());

// ... existing persist_reembed_job with status "completed" or "completed_with_errors" ...

Ok((summary, outcome))
```

Also update the final `persist_reembed_job` call to use `"completed"` if `failed_facts == 0`, else `"completed_with_errors"`.

- [ ] **Step 10: Handle `NothingToDo` case**

After computing `summary.total_facts`, add an early return:

```rust
if summary.total_facts == 0 && !options.retry_failed {
    progress.on_job_completed(&ReembedOutcome::NothingToDo, &summary, Duration::ZERO);
    return Ok((summary, ReembedOutcome::NothingToDo));
}
```

- [ ] **Step 11: Update all existing tests in `reembed.rs` `mod tests`**

Every test currently calls `service.reembed_all_facts().await`. Update them to:

```rust
use crate::service::reembed_options::ReembedOptions;
use crate::service::reembed_progress::NoopProgressReporter;
use tokio_util::sync::CancellationToken;

let options = ReembedOptions::default();
let progress = NoopProgressReporter;
let cancel = CancellationToken::new();
let (summary, outcome) = service.reembed_all_facts(&options, &progress, &cancel).await.unwrap();
```

- [ ] **Step 12: Run tests**

Run: `cargo test --lib reembed`
Expected: All tests pass.

- [ ] **Step 13: Commit**

```bash
git add src/service/reembed.rs
git commit -m "feat(reembed): refactor reembed_all_facts with options, progress, cancellation, continue-on-error"
```

---

## Task 5: Add `--max-failures` and `--retry-failed` CLI flags

**Files:**
- Modify: `src/cli/args.rs`
- Modify: `src/cli.rs` (update `Command` enum)

**Interfaces:**
- Produces: `ReembedArgs { max_failures: Option<usize>, retry_failed: bool }`

- [ ] **Step 1: Add `ReembedArgs` struct to `src/cli/args.rs`**

Add to `src/cli/args.rs`:

```rust
/// Arguments for the `reembed` maintenance command.
#[derive(Debug, Clone, Args)]
pub struct ReembedArgs {
    /// Maximum number of failed facts before aborting.
    ///
    /// Default: 10% of total (minimum 10). Use 0 for fail-fast behavior.
    #[arg(long)]
    pub max_failures: Option<usize>,

    /// Retry only facts that failed in a previous reembed run.
    #[arg(long)]
    pub retry_failed: bool,
}
```

- [ ] **Step 2: Update `Command` enum in `src/cli.rs`**

Find the `Command` enum (or wherever `Reembed` is defined) and change:

```rust
// Before:
Reembed,

// After:
Reembed(ReembedArgs),
```

Make sure `ReembedArgs` is imported.

- [ ] **Step 3: Update `src/runner.rs` dispatch**

In `src/runner.rs`, update the `Some(Command::Reembed)` arm:

```rust
Some(Command::Reembed(args)) => {
    run_reembed_mode(logger, args)
        .await
        .map_err(boxed_to_failure)
}
```

- [ ] **Step 4: Update `cli_reembed_subcommand` test**

In `src/cli/runtime.rs` tests:

```rust
#[test]
fn cli_reembed_subcommand() {
    let cli = Cli::parse_from(["memory_mcp", "reembed"]);
    assert!(matches!(cli.command, Some(Command::Reembed(_))));
}

#[test]
fn cli_reembed_with_flags() {
    let cli = Cli::parse_from(["memory_mcp", "reembed", "--max-failures", "5", "--retry-failed"]);
    match cli.command {
        Some(Command::Reembed(args)) => {
            assert_eq!(args.max_failures, Some(5));
            assert!(args.retry_failed);
        }
        _ => panic!("expected Reembed command"),
    }
}
```

- [ ] **Step 5: Run CLI tests**

Run: `cargo test --lib cli`
Expected: All tests pass including the new flag test.

- [ ] **Step 6: Commit**

```bash
git add src/cli/args.rs src/cli.rs src/runner.rs src/cli/runtime.rs
git commit -m "feat(reembed): add --max-failures and --retry-failed CLI flags"
```

---

## Task 6: Wire `run_reembed_mode` with progress reporter selection, Ctrl+C handling, and final summary

**Files:**
- Modify: `src/cli/runtime.rs`

**Interfaces:**
- Consumes: `ReembedArgs`, `ReembedOptions`, `ReembedProgressReporter` impls, `CancellationToken`, `ReembedOutcome`
- Produces: Updated `run_reembed_mode` that builds the right reporter, sets up Ctrl+C, runs reembed, prints summary

- [ ] **Step 1: Update `run_reembed_mode` signature**

In `src/cli/runtime.rs`:

```rust
pub async fn run_reembed_mode(
    logger: &StdoutLogger,
    args: ReembedArgs,
) -> Result<(), Box<dyn std::error::Error>> {
```

- [ ] **Step 2: Build `ReembedOptions` from `ReembedArgs`**

```rust
let options = ReembedOptions {
    max_failures: args.max_failures,
    retry_failed: args.retry_failed,
};
```

- [ ] **Step 3: Show spinner during service init**

```rust
use indicatif::ProgressBar;
use indicatif::ProgressStyle;

let spinner = ProgressBar::new_spinner();
spinner.set_style(ProgressStyle::with_template("{spinner} Initializing reembed service...").expect("valid template"));
spinner.enable_steady_tick(std::time::Duration::from_millis(100));
```

- [ ] **Step 4: Build service**

```rust
let memory_service =
    build_memory_service(logger, EmbeddingActivationMode::ForceEnabledForReembed).await?;
spinner.finish_with_message("Service initialized");
```

- [ ] **Step 5: Select progress reporter based on TTY**

```rust
use crate::service::reembed_progress::{IndicatifProgressReporter, LogProgressReporter, ReembedProgressReporter};
use crate::service::reembed_options::ReembedOutcome;

let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
let reporter: Box<dyn ReembedProgressReporter> = if is_tty {
    Box::new(IndicatifProgressReporter::new())
} else {
    Box::new(LogProgressReporter::new(logger.clone()))
};
```

- [ ] **Step 6: Set up Ctrl+C handler with `CancellationToken`**

```rust
use tokio_util::sync::CancellationToken;

let cancel_token = CancellationToken::new();
let cancel_for_handler = cancel_token.clone();
tokio::spawn(async move {
    if tokio::signal::ctrl_c().await.is_ok() {
        cancel_for_handler.cancel();
    }
});
```

- [ ] **Step 7: Run reembed and capture outcome**

```rust
let started_at = std::time::Instant::now();
let (summary, outcome) = memory_service
    .reembed_all_facts(&options, reporter.as_ref(), &cancel_token)
    .await
    .map_err(|err| log_and_return_error(logger, "main.reembed_failed", err))?;
let elapsed = started_at.elapsed();
```

- [ ] **Step 8: Print final summary to stdout**

```rust
print_reembed_summary(&outcome, &summary, elapsed, &memory_service);
```

Add the `print_reembed_summary` function:

```rust
fn print_reembed_summary(
    outcome: &ReembedOutcome,
    summary: &crate::service::reembed::ReembedSummary,
    elapsed: std::time::Duration,
    _service: &crate::service::MemoryService,
) {
    println!();
    match outcome {
        ReembedOutcome::Completed => println!("✓ Reembed completed"),
        ReembedOutcome::CompletedWithErrors => println!("✓ Reembed completed (with errors)"),
        ReembedOutcome::Failed => println!("✗ Reembed failed"),
        ReembedOutcome::Interrupted => println!("⏹ Reembed interrupted"),
        ReembedOutcome::NothingToDo => println!("✓ Nothing to do — all embeddings already match target signature"),
    }
    println!();
    println!("  Total:       {} facts", summary.total_facts);
    println!("  Processed:   {} ({} succeeded, {} failed)",
        summary.processed_facts, summary.succeeded_facts, summary.failed_facts);
    println!("  Duration:    {:.1}s", elapsed.as_secs_f64());
    if summary.processed_facts > 0 {
        println!("  Speed:       {:.0} facts/sec", summary.processed_facts as f64 / elapsed.as_secs_f64());
    }
    println!();
    if summary.failed_facts > 0 {
        match outcome {
            ReembedOutcome::CompletedWithErrors => {
                println!("  {} facts failed. Re-run with --retry-failed to retry only failures.", summary.failed_facts);
            }
            ReembedOutcome::Failed => {
                println!("  {} facts failed (quota exceeded). Fix the provider and re-run with --retry-failed.", summary.failed_facts);
            }
            ReembedOutcome::Interrupted => {
                println!("  Resume with 'memory_mcp reembed' to continue from where it stopped.");
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 9: Update exit code based on outcome**

```rust
memory_service.shutdown_lifecycle_background_workers().await;

if outcome.exit_code() != 0 {
    return Err(Box::new(std::io::Error::other(
        format!("reembed exited with status: {outcome:?}"),
    )));
}
Ok(())
```

- [ ] **Step 10: Run existing runtime tests**

Run: `cargo test --lib runtime`
Expected: Tests pass (update `cli_reembed_subcommand` if needed — it should still pass since `reembed` with no args produces default `ReembedArgs`).

- [ ] **Step 11: Commit**

```bash
git add src/cli/runtime.rs
git commit -m "feat(reembed): wire progress reporter, Ctrl+C, final summary in run_reembed_mode"
```

---

## Task 7: Integration tests for progress, cancellation, and continue-on-error

**Files:**
- Create: `tests/eval_reembed_progress.rs`

**Interfaces:**
- Consumes: `ReembedOptions`, `NoopProgressReporter`, `CancellationToken`, `MemoryService`

- [ ] **Step 1: Create `tests/eval_reembed_progress.rs`**

```rust
//! Integration tests for reembed progress reporting, cancellation, and continue-on-error.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use memory_mcp::service::reembed::ReembedSummary;
use memory_mcp::service::reembed_options::{ReembedOptions, ReembedOutcome};
use memory_mcp::service::reembed_progress::{NoopProgressReporter, ReembedProgressReporter};
use tokio_util::sync::CancellationToken;
```

- [ ] **Step 2: Write test — continue-on-error does not stop the run**

This test uses the existing `SequenceTestEmbeddingProvider` pattern (fails on call N) from the unit tests. Since that provider is private, this integration test uses the public `MemoryService` builder with a mock that fails on specific calls. If that's not feasible from integration tests, write it as a unit test inside `reembed.rs` instead.

Add to `src/service/reembed.rs` `mod tests`:

```rust
#[tokio::test]
async fn reembed_continue_on_error_completes_within_quota() {
    let db = make_in_memory_db(&["org"]).await;
    // Seed 5 facts; provider fails on 2nd call but succeeds on others.
    for i in 0..5 {
        seed_fact_with_embedding(
            &db,
            "org",
            &format!("fact:{i}"),
            "old-sig",
            3,
            &format!("content {i}"),
        )
        .await;
    }

    let provider = Arc::new(SequenceTestEmbeddingProvider::fails_on_call(3, 2));
    let service = make_reembed_service(db, provider, vec!["org".to_string()]);

    let options = ReembedOptions { max_failures: Some(10), retry_failed: false };
    let progress = NoopProgressReporter;
    let cancel = CancellationToken::new();

    let (summary, outcome) = service
        .reembed_all_facts(&options, &progress, &cancel)
        .await
        .expect("reembed should not hard-fail within quota");

    assert_eq!(outcome, ReembedOutcome::CompletedWithErrors);
    assert_eq!(summary.total_facts, 5);
    assert_eq!(summary.processed_facts, 5);
    assert_eq!(summary.succeeded_facts, 4);
    assert_eq!(summary.failed_facts, 1);
    assert_eq!(summary.failed_fact_ids.len(), 1);
}
```

- [ ] **Step 3: Write test — fail-fast when max_failures is 0**

```rust
#[tokio::test]
async fn reembed_fail_fast_when_max_failures_zero() {
    let db = make_in_memory_db(&["org"]).await;
    for i in 0..5 {
        seed_fact_with_embedding(
            &db,
            "org",
            &format!("fact:{i}"),
            "old-sig",
            3,
            &format!("content {i}"),
        )
        .await;
    }

    let provider = Arc::new(SequenceTestEmbeddingProvider::fails_on_call(3, 2));
    let service = make_reembed_service(db, provider, vec!["org".to_string()]);

    let options = ReembedOptions { max_failures: Some(0), retry_failed: false };
    let progress = NoopProgressReporter;
    let cancel = CancellationToken::new();

    let (summary, outcome) = service
        .reembed_all_facts(&options, &progress, &cancel)
        .await
        .expect("reembed returns outcome, not error, on quota exceeded");

    assert_eq!(outcome, ReembedOutcome::Failed);
    assert!(summary.failed_facts >= 1);
}
```

- [ ] **Step 4: Write test — cancellation produces Interrupted outcome**

```rust
#[tokio::test]
async fn reembed_cancellation_produces_interrupted_outcome() {
    let db = make_in_memory_db(&["org"]).await;
    for i in 0..10 {
        seed_fact_with_embedding(
            &db,
            "org",
            &format!("fact:{i}"),
            "old-sig",
            3,
            &format!("content {i}"),
        )
        .await;
    }

    let provider = Arc::new(SequenceTestEmbeddingProvider::new(3));
    let service = make_reembed_service(db, provider, vec!["org".to_string()]);

    let options = ReembedOptions::default();
    let progress = NoopProgressReporter;
    let cancel = CancellationToken::new();

    // Cancel before starting — simulates Ctrl+C during processing.
    cancel.cancel();

    let (summary, outcome) = service
        .reembed_all_facts(&options, &progress, &cancel)
        .await
        .expect("interrupted reembed returns outcome, not error");

    assert_eq!(outcome, ReembedOutcome::Interrupted);
    // The job state should be "interrupted" in DB.
    let job = service.load_reembed_job().await.expect("load job");
    assert_eq!(job.as_ref().and_then(|j| j.get("status")).and_then(|v| v.as_str()), Some("interrupted"));
}
```

- [ ] **Step 5: Write test — NothingToDo when all facts already match signature**

```rust
#[tokio::test]
async fn reembed_nothing_to_do_when_all_match_signature() {
    let db = make_in_memory_db(&["org"]).await;
    // Seed facts that already match the target signature.
    for i in 0..3 {
        seed_fact_with_embedding(
            &db,
            "org",
            &format!("fact:{i}"),
            "test",  // matches provider name "test" → same signature
            3,
            &format!("content {i}"),
        )
        .await;
    }

    let provider = Arc::new(SequenceTestEmbeddingProvider::new(3));
    let service = make_reembed_service(db, provider, vec!["org".to_string()]);

    let options = ReembedOptions::default();
    let progress = NoopProgressReporter;
    let cancel = CancellationToken::new();

    let (summary, outcome) = service
        .reembed_all_facts(&options, &progress, &cancel)
        .await
        .expect("nothing-to-do is not an error");

    assert_eq!(outcome, ReembedOutcome::NothingToDo);
    assert_eq!(summary.total_facts, 0);
}
```

- [ ] **Step 6: Run all reembed tests**

Run: `cargo test --lib reembed`
Expected: All tests pass including the 4 new ones.

- [ ] **Step 7: Commit**

```bash
git add src/service/reembed.rs tests/eval_reembed_progress.rs
git commit -m "test(reembed): add integration tests for continue-on-error, cancellation, nothing-to-do"
```

---

## Task 8: Create ADR-0018

**Files:**
- Create: `docs/adr/0018-reembed-interactive-progress.md`

- [ ] **Step 1: Create the ADR**

```markdown
# ADR-0018: Interactive Reembed with Progress, Cancellation, and Continue-on-Error

> Status: Accepted (2026-07-24)
> Related: ADR-0016 (public surface freeze — reembed is CLI-only, not an MCP tool)

## Context

The `memory_mcp reembed` command rewrites all fact embeddings after an embedding
provider switch. Prior to this ADR, the command:

1. Logged structured events to stderr, but with no live progress bar — the
   process appeared to "go silent" and users could not tell if it had started.
2. Failed fast on the first fact error, stopping the entire run even for
   transient remote-provider failures.
3. Had no Ctrl+C handling — interrupting the process left the HNSW index
   dropped and embedding states in "rebuilding" with no clear recovery path.
4. Only logged ETA as a raw `eta_seconds` integer inside a JSON-ish log line.

After switching providers, operators reported confusion: "the app went to the
background and it's unclear whether reembed started or what the status is."

## Decision

Transform `reembed` into an interactive, observable, and resilient maintenance
command with four changes:

### 1. Live TTY progress bar via `indicatif`

When stderr is a TTY, show a live progress bar:
- Percentage, processed/total, ETA in human-readable format, facts/sec
- Success/failure counters (`✓1230 ✗10`)
- Namespace label in the bar prefix
- Spinner during service initialization (model loading)
- Redraw throttled to 10 Hz (`stderr_with_hz(10)`)

When stderr is not a TTY (pipes, CI, scripts), degrade to the existing
structured log events plus two new init-phase events
(`reembed.init_started`, `reembed.init_completed`).

### 2. Graceful Ctrl+C with `CancellationToken`

Register a `tokio::signal::ctrl_c()` handler that cancels a
`CancellationToken`. The fact-processing loop checks `is_cancelled()` after
each fact. On interrupt:

- Finish the current fact (no mid-rewrite abort).
- Persist job state with status `"interrupted"`.
- Show: `⏹ Interrupted at 1240/3000 (41%). Resume with 'memory_mcp reembed'.`
- Exit code 130 (standard SIGINT convention).
- On next `reembed` run, resume from the last cursor; show a resume hint.

### 3. Continue-on-error with quota

Instead of fail-fast, continue processing after a fact error:

- Record the failure, advance the cursor past the failed fact.
- Default quota: 10% of total facts (minimum 10).
- `--max-failures N` CLI flag overrides (0 = fail-fast, legacy behavior).
- If quota exceeded → status `"failed"`, exit code 1.
- If all processed with some failures → status `"completed_with_errors"`,
  exit code 0.
- Persist `failed_fact_ids` per namespace in job state for `--retry-failed`.

New job statuses: `running`, `completed`, `completed_with_errors`, `failed`,
`interrupted`.

### 4. `--retry-failed` flag

After a `completed_with_errors` or `failed` run, `--retry-failed` processes
only the facts in `failed_fact_ids`. If all retries succeed, status becomes
`completed`.

### 5. Final summary in stdout

After the progress bar clears, print a compact summary to stdout:

```
✓ Reembed completed (with errors)

  Total:       3000 facts
  Processed:   3000 (2990 succeeded, 10 failed)
  Duration:    135.2s
  Speed:       22 facts/sec

  10 facts failed. Re-run with --retry-failed to retry only failures.
```

## Consequences

- **New dependency:** `indicatif = "0.17"` — widely used, minimal, no transitive
  bloat. Already compatible with the existing `console` crate ecosystem.
- **Public CLI surface change:** `Command::Reembed` becomes
  `Command::Reembed(ReembedArgs)` with two optional flags. This is a
  backward-compatible change — `memory_mcp reembed` with no flags still works.
- **No MCP tool surface change:** reembed remains CLI-only. ADR-0016 public
  surface freeze is respected.
- **Job state schema extended:** `namespace_progress[ns]` now includes
  `failed_fact_ids` array. Old job records without this field are handled
  gracefully (defaults to empty array).
- **New statuses:** `interrupted` and `completed_with_errors` are persisted in
  the job record. The startup embedding-state check already treats any
  non-`ready` state as "semantic retrieval disabled", so these are safe.
```

- [ ] **Step 2: Commit**

```bash
git add docs/adr/0018-reembed-interactive-progress.md
git commit -m "docs(adr): ADR-0018 — interactive reembed with progress, cancellation, continue-on-error"
```

---

## Task 9: Update README.md and AGENTS.md documentation

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`

- [ ] **Step 1: Update README.md "The `reembed` maintenance command" section**

Update the section to document the new flags and behavior. Add:

- `--max-failures N` description
- `--retry-failed` description
- Ctrl+C behavior
- Example of TTY progress bar output
- Example of final summary output
- Reference to ADR-0018

- [ ] **Step 2: Update README.md "Progress, status, and logging" section**

Update to describe:
- TTY vs non-TTY behavior
- New init-phase events
- New `interrupted` and `completed_with_errors` statuses
- The `failed_fact_ids` job state field

- [ ] **Step 3: Update README.md "Subcommands" table**

Update the `reembed` row description to mention the new flags.

- [ ] **Step 4: Update AGENTS.md "Common Tasks" section**

Update the reembed entry to reflect new flags and behavior. Add a brief
reference to ADR-0018 for architectural details.

- [ ] **Step 5: Run quality gate**

Run:
```bash
cargo fmt --all
cargo clippy --all-targets
cargo test
```
Expected: All pass with zero warnings.

- [ ] **Step 6: Commit**

```bash
git add README.md AGENTS.md
git commit -m "docs(reembed): update README and AGENTS.md with interactive reembed UX"
```

---

## Task 10: Full quality gate and self-review

- [ ] **Step 1: Run full quality gate**

```bash
cargo fmt --all --check
cargo clippy --all-targets
cargo test
```

- [ ] **Step 2: Fix any clippy warnings**

If clippy reports warnings, fix them. Common issues to watch for:
- Unused imports from the refactoring
- `clippy::too_many_arguments` on `update_namespace_progress` — if triggered, consider a params struct, but the existing codebase already has functions with similar arity.

- [ ] **Step 3: Run reembed eval tests specifically**

```bash
cargo test --lib reembed -- --nocapture
```

- [ ] **Step 4: Verify CLI help output**

```bash
cargo run -- reembed --help
```
Expected: Shows `--max-failures` and `--retry-failed` flags with descriptions.

- [ ] **Step 5: Final commit if any fixes were needed**

```bash
git add -A
git commit -m "fix(reembed): quality gate fixes — clippy, fmt, test adjustments"
```

- [ ] **Step 6: Self-review checklist**

Verify against the grilling decisions:
- [ ] `indicatif` progress bar in TTY mode with %, ETA, speed, success/failure
- [ ] Non-TTY fallback to structured logs + init events
- [ ] `inc(1)` after each fact + `stderr_with_hz(10)` throttle
- [ ] Graceful Ctrl+C with `CancellationToken`, status "interrupted", exit 130
- [ ] Continue-on-error with 10% quota (min 10), `--max-failures` override
- [ ] New statuses: `completed`, `completed_with_errors`, `failed`, `interrupted`
- [ ] `--retry-failed` flag retries only `failed_fact_ids`
- [ ] Final summary in stdout
- [ ] `failed_fact_ids` persisted in job state
- [ ] ADR-0018 created
- [ ] README.md updated
- [ ] AGENTS.md updated
- [ ] All tests pass
- [ ] Zero clippy warnings
- [ ] Zero fmt diff
