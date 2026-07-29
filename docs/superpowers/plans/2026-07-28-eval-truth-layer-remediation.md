# Evaluation Truth Layer Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make evaluation artifacts, suite summaries, time budgets, gates, and benchmark reports mathematically correct and impossible to pass with incomplete evidence.

**Architecture:** Replace the current “first metric wins” summary logic with suite-owned typed reducers over case evidence, namespace every case and gate, and bump the artifact contract to v2. The runner consumes the exact loaded profile, measures real elapsed time, validates exact coverage, and writes an artifact even for invalid execution. Reports and baselines are generated and validated from the same artifact contract.

**Tech Stack:** Rust 2024, Tokio 1.53, Serde/serde_json, thiserror, SHA-256, existing `eval-harness` workspace crate.

## Global Constraints

- Every selected case has exactly one outcome: `passed`, `quality_failed`, or `invalid`.
- Empty, incomplete, duplicated, or unexpected coverage is invalid.
- A metric is never copied from an arbitrary case; its reducer and denominator are explicit.
- Development and test splits are summarized separately; only declared test slices gate.
- Retrieval-only, end-to-end, lifecycle, and performance metrics are never merged.
- A declared regression budget requires a compatible approved baseline.
- The exact profile passed to `memory-eval` controls suites, gates, budget, and fingerprint.
- The artifact is written before returning exit 1 or exit 2 whenever an output path is available.
- The human report is derived from the artifact and contains no manually reconstructed metrics.
- Existing `target/evals/pr.json` and `target/evals/release.json` are diagnostic evidence only and must not become approved baselines.

---

## File Map

| Path | Responsibility |
|---|---|
| `crates/eval-harness/src/domain.rs` | Namespaced case/gate keys and typed metric evidence |
| `crates/eval-harness/src/artifact.rs` | Artifact v2, validation, invalid-run artifacts |
| `crates/eval-harness/src/runner.rs` | Exact profile execution, duration, coverage, reducers |
| `crates/eval-harness/src/reducer.rs` | Suite metric reducer trait and shared count helpers |
| `crates/eval-harness/src/gate.rs` | Slice-qualified hard floors and baseline comparison |
| `crates/eval-harness/src/profile.rs` | Exact coverage and qualified gate declarations |
| `crates/eval-harness/src/report.rs` | Deterministic Markdown/console rendering |
| `crates/eval-harness/src/main.rs` | Thin run adapter and exit mapping |
| `crates/eval-harness/src/merge.rs` | Recompute summaries/gates from merged outcomes |
| `evals/schema/eval-artifact-v2.json` | Strict artifact schema |
| `evals/profiles/pr.json` | Exact PR suite coverage and qualified gates |
| `evals/profiles/release.json` | Exact release suite coverage and qualified gates |
| `evals/baselines/pr.json` | Replaced only after corrected artifact review |
| `docs/evals/BENCHMARK_RUN_REPORT_2026-07-28.md` | Remains historical; corrected run gets a new dated report |

### Task 1: Introduce namespaced case identity and typed metric evidence

**Files:**
- Modify: `crates/eval-harness/src/domain.rs`
- Create: `crates/eval-harness/src/reducer.rs`
- Modify: `crates/eval-harness/src/lib.rs`

**Interfaces:**
- Produces: `CaseKey { suite_id: SuiteId, case_id: EvalCaseId }`.
- Produces: `MetricKey { suite_id, mode, split, label_trust, metric }`.
- Produces: `MetricEvidence::{Retrieval, Classification, Count, Ratio, Duration}`.
- Produces: `SuiteReducer::reduce(&self, outcomes: &[EvalCaseOutcome]) -> Result<SuiteSummary, EvalError>`.

- [ ] **Step 1: Write failing identity tests**

```rust
#[test]
fn same_local_id_in_two_suites_is_not_a_duplicate() {
    let first = CaseKey::parse("retrieval", "case-1").unwrap();
    let second = CaseKey::parse("claims", "case-1").unwrap();
    assert_ne!(first, second);
}

#[test]
fn empty_suite_or_case_id_is_rejected() {
    assert!(CaseKey::parse("", "case-1").is_err());
    assert!(CaseKey::parse("retrieval", "").is_err());
}
```

- [ ] **Step 2: Run tests and verify the current global ID contract fails**

Run: `cargo test -p eval-harness domain::tests::same_local_id_in_two_suites_is_not_a_duplicate`

Expected: FAIL because `CaseKey` is not defined.

- [ ] **Step 3: Implement validated identity types**

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CaseKey {
    pub suite_id: SuiteId,
    pub case_id: EvalCaseId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SuiteId(String);
```

`SuiteId::parse` and `EvalCaseId::parse` reject empty or surrounding-whitespace
values. `EvalCaseOutcome` stores one `CaseKey` rather than separate unvalidated
suite and case strings.

- [ ] **Step 4: Define evidence that can be reduced correctly**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetricEvidence {
    Retrieval {
        relevant: u64,
        hits_at_k: u64,
        first_relevant_rank: Option<u32>,
        cutoff: u32,
    },
    Classification {
        true_positives: u64,
        false_positives: u64,
        false_negatives: u64,
        true_negatives: u64,
    },
    Count { value: u64 },
    Ratio { numerator: u64, denominator: u64 },
    Duration { nanoseconds: u64 },
}
```

Keep user-facing metric values in `SuiteSummary`; store reducer inputs in each
case outcome. Reject zero retrieval cutoffs and zero ratio denominators.

- [ ] **Step 5: Define the reducer seam**

```rust
pub trait SuiteReducer: Send + Sync {
    fn suite_id(&self) -> &SuiteId;
    fn reduce(&self, outcomes: &[EvalCaseOutcome]) -> Result<Vec<SuiteSummary>, EvalError>;
}
```

Return multiple summaries so split, mode, and label-trust slices cannot be
collapsed accidentally.

- [ ] **Step 6: Run checks and commit**

Run:

```bash
cargo test -p eval-harness domain reducer
cargo clippy -p eval-harness --all-targets -- -D warnings
cargo fmt --all --check
```

Commit:

```bash
git add crates/eval-harness/src/domain.rs crates/eval-harness/src/reducer.rs crates/eval-harness/src/lib.rs
git commit -m "refactor(evals): type case identity and metric evidence"
```

### Task 2: Replace first-case summaries with mathematically correct reducers

**Files:**
- Modify: `crates/eval-harness/src/runner.rs`
- Modify: `crates/eval-harness/src/merge.rs`
- Modify: `crates/eval-harness/src/reducer.rs`
- Test: `crates/eval-harness/tests/summary_reduction.rs`

**Interfaces:**
- Consumes: `EvalSuite::reducer() -> &dyn SuiteReducer`.
- Produces: one `SuiteSummary` per `(suite, mode, split, label_trust)` slice.
- Removes: `metrics.entry(key).or_insert(value)` from runner and shard merge.

- [ ] **Step 1: Write a failing retrieval aggregation test**

```rust
#[test]
fn retrieval_summary_uses_all_cases() {
    let outcomes = vec![
        retrieval_outcome("a", 1, 1, Some(1), CaseStatus::Passed),
        retrieval_outcome("b", 0, 1, None, CaseStatus::QualityFailed),
    ];
    let summary = RetrievalReducer::new("local-retrieval", 5)
        .reduce(&outcomes)
        .unwrap()
        .remove(0);
    assert_eq!(summary.metrics["recall_at_5"], 0.5);
    assert_eq!(summary.metrics["mrr"], 0.5);
    assert_eq!(summary.metrics["top_1_hit_rate"], 0.5);
}
```

- [ ] **Step 2: Write a failing classification aggregation test**

```rust
#[test]
fn classification_summary_sums_confusion_counts_before_f1() {
    let outcomes = vec![
        classification_outcome("a", 1, 0, 0, 2),
        classification_outcome("b", 0, 1, 1, 0),
    ];
    let summary = ClassificationReducer::new("extraction", "entity")
        .reduce(&outcomes)
        .unwrap()
        .remove(0);
    assert!((summary.metrics["entity_precision"] - 0.5).abs() < 1e-12);
    assert!((summary.metrics["entity_recall"] - 0.5).abs() < 1e-12);
    assert!((summary.metrics["entity_f1"] - 0.5).abs() < 1e-12);
}
```

- [ ] **Step 3: Run tests and confirm current summaries return first-case values**

Run: `cargo test -p eval-harness --test summary_reduction`

Expected: FAIL; current runner uses `or_insert` and has no reducer.

- [ ] **Step 4: Implement shared reducers**

Sum counts before calculating ratios. Retrieval recall uses total hits divided
by total relevant evidence; MRR and top-1 use the number of valid evaluated
queries. Classification precision/recall remain absent when their denominator
is zero; a required gate over an absent metric becomes invalid.

- [ ] **Step 5: Delegate summary creation to each suite**

Extend `EvalSuite`:

```rust
fn reducer(&self) -> &dyn SuiteReducer;
```

Runner and merge group outcomes by suite, ask the registered reducer to
recompute summaries, and reject outcomes whose evidence kind does not match
their suite reducer.

- [ ] **Step 6: Prove the three misleading report claims disappear**

Build a fixture matching the observed run: 63/66 retrieval passes, 7/9
extraction passes, and 38/42 claim passes. Assert the summary cannot report
retrieval MRR/top-1 as 1.0 when failed cases have zero rank, cannot report
entity F1 from the first case, and cannot report claim precision from an
arbitrary case.

- [ ] **Step 7: Run checks and commit**

Run:

```bash
cargo test -p eval-harness reducer --test summary_reduction
cargo clippy -p eval-harness --all-targets -- -D warnings
cargo fmt --all --check
```

Commit:

```bash
git add crates/eval-harness/src/runner.rs crates/eval-harness/src/merge.rs crates/eval-harness/src/reducer.rs crates/eval-harness/tests/summary_reduction.rs
git commit -m "fix(evals): aggregate every case in suite metrics"
```

### Task 3: Enforce exact profile coverage, real duration, and time budgets

**Files:**
- Modify: `crates/eval-harness/src/profile.rs`
- Modify: `crates/eval-harness/src/runner.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Modify: `crates/eval-harness/src/artifact.rs`
- Modify: `evals/profiles/pr.json`
- Modify: `evals/profiles/release.json`

**Interfaces:**
- Produces: `CoverageDecl::{ExactIds, ManifestPopulation, StableSample}`.
- Produces: `Runner::run(&self, request: &RunRequest) -> RunArtifact`.
- Produces: run-level `budget_status: GateStatus`.

- [ ] **Step 1: Write failing duration and budget tests**

```rust
#[tokio::test(start_paused = true)]
async fn runner_records_elapsed_time_and_fails_the_budget() {
    let suite = DelayedSuite::new(Duration::from_secs(11));
    let artifact = run_with_budget(suite, Duration::from_secs(10)).await;
    assert_eq!(artifact.duration_ms, 11_000);
    assert_eq!(artifact.budget_status, GateStatus::Failed);
}
```

- [ ] **Step 2: Replace minimum-only coverage**

For local fixtures, profiles declare the fixture SHA-256 plus exact expected
case count. For external corpora, they declare a manifest population or stable
sample selection. Reject `min_cases` because it permits silent loss.

- [ ] **Step 3: Pass the loaded manifest into Runner**

Change the runner signature to:

```rust
pub async fn run(&self, request: &RunRequest) -> RunArtifact;

pub struct RunRequest {
    pub manifest: ProfileManifest,
    pub manifest_path: PathBuf,
    pub artifact_path: PathBuf,
    pub baseline: Option<RunArtifact>,
    pub suite_filter: BTreeSet<SuiteId>,
}
```

Remove the hard-coded `evals/profiles/{profile}.json` reload. Hash the exact
loaded bytes into `fingerprint.profile_digest`.

- [ ] **Step 4: Measure the complete run**

Start the monotonic timer before suite construction and stop after reducers and
gates finish. Record per-suite and run durations. Compare the run duration with
`time_budget_seconds`; incomplete execution and timeout are invalid, elapsed
over-budget with complete evidence is a failed budget gate.

- [ ] **Step 5: Preserve an invalid artifact on runner failure**

Convert suite construction failure, unknown suite, panic/join failure, and
coverage mismatch into an invalid run artifact with one invalid outcome per
unmeasured expected case. `main.rs` writes the artifact and then exits 2.

- [ ] **Step 6: Run checks and commit**

Run:

```bash
cargo test -p eval-harness runner profile artifact
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src evals/profiles
git commit -m "fix(evals): enforce exact coverage and time budgets"
```

### Task 4: Qualify gates and require compatible baselines

**Files:**
- Modify: `crates/eval-harness/src/profile.rs`
- Modify: `crates/eval-harness/src/gate.rs`
- Modify: `crates/eval-harness/src/artifact.rs`
- Modify: `crates/eval-harness/src/merge.rs`
- Modify: `evals/profiles/pr.json`
- Modify: `evals/profiles/release.json`

**Interfaces:**
- Produces: `GateTarget { suite_id, mode, split, label_trust, metric }`.
- Produces: `BaselineCompatibility::check(current, baseline, target)`.
- Produces: `GateFailureReason::{MissingBaseline, IncompatibleBaseline, MissingMetric}`.

- [ ] **Step 1: Write failing baseline tests**

```rust
#[test]
fn regression_budget_without_baseline_is_invalid() {
    let decision = evaluate_metric_gate(0.95, Some(0.90), None, Some(0.02));
    assert_eq!(decision.status, GateStatus::Invalid);
    assert_eq!(decision.reason, GateFailureReason::MissingBaseline);
}

#[test]
fn gate_does_not_pick_same_named_metric_from_another_suite() {
    let artifact = artifact_with_metrics([
        ("retrieval-a", "recall_at_5", 1.0),
        ("retrieval-b", "recall_at_5", 0.2),
    ]);
    let decision = evaluate_target(&target("retrieval-b", "recall_at_5"), &artifact, None);
    assert_eq!(decision.observed, Some(0.2));
}
```

- [ ] **Step 2: Make every gate target explicit**

Profile JSON uses:

```json
{
  "target": {
    "suite_id": "local-retrieval",
    "mode": "retrieval_only",
    "split": "test",
    "label_trust": ["official", "reviewed"],
    "metric": "recall_at_5"
  },
  "hard_floor": 0.90,
  "regression_budget": 0.05,
  "baseline_required": true
}
```

- [ ] **Step 3: Validate baseline compatibility**

Require matching artifact schema, profile, profile digest policy, suite and
evaluator versions, corpus/fixture fingerprints, build features, provider,
model, device, configuration hash, and metric target. Return
`IncompatibleBaseline`; never silently ignore a placeholder or unreadable file.

- [ ] **Step 4: Recompute gates after shard merge**

Do not copy gates from the first shard. Merge case evidence, recompute suite
summaries, validate complete population, then evaluate qualified gates against
the baseline.

- [ ] **Step 5: Delete the placeholder baseline**

Remove `evals/baselines/pr.json` until a corrected, schema-valid, reviewed run
exists. Profiles may keep hard floors but must mark regression comparison
invalid until the approved baseline is supplied.

- [ ] **Step 6: Run checks and commit**

Run:

```bash
cargo test -p eval-harness gate merge profile
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src evals/profiles
git add -u evals/baselines/pr.json
git commit -m "fix(evals): require qualified compatible baselines"
```

### Task 5: Version the artifact and generate reports from it

**Files:**
- Modify: `crates/eval-harness/src/artifact.rs`
- Create: `crates/eval-harness/src/report.rs`
- Create: `evals/schema/eval-artifact-v2.json`
- Modify: `crates/eval-harness/src/lib.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Test: `crates/eval-harness/tests/report_rendering.rs`

**Interfaces:**
- Produces: schema `memory-mcp-eval/v2`.
- Produces: `render_markdown(artifact: &RunArtifact) -> Result<String, EvalError>`.
- Produces: `memory-eval report --artifact <path> --output <path>`.

- [ ] **Step 1: Write a failing report consistency test**

```rust
#[test]
fn report_values_are_read_from_the_artifact() {
    let artifact = fixture_artifact_with_recall(0.75);
    let report = render_markdown(&artifact).unwrap();
    assert!(report.contains("| recall_at_5 | 0.7500 |"));
    assert!(!report.contains("recall_at_5 | 1.0000"));
}
```

- [ ] **Step 2: Define artifact v2**

Require namespaced case keys, exact coverage declaration and result, metric
evidence, sliced summaries, budget status, qualified gates, retry history, real
fingerprints, and reducer/evaluator versions. Set `additionalProperties: false`
throughout the JSON Schema.

- [ ] **Step 3: Add semantic artifact validation**

Validate schema version, unique case keys, exact coverage, outcome/status
invariants, summary recomputation equality, gate target existence, budget
status, finite values, and absence of weak labels from release gates.

- [ ] **Step 4: Render deterministic Markdown**

Generate environment, coverage, duration, case status, suite metrics, gates,
failed cases, invalid reasons, corpus/fixture fingerprints, and benchmark
limitations directly from the artifact. Sort all sections by typed keys.

- [ ] **Step 5: Mark the original report as superseded only after rerun**

Do not edit historical values. After corrected execution, create a new report
whose header links the superseded report and explains the invalidated claims:
ceiling retrieval metrics, zero extraction/claim aggregate interpretation,
stable Criterion claim, and verified profile budgets.

- [ ] **Step 6: Run checks and commit**

Run:

```bash
cargo test -p eval-harness artifact report --test report_rendering
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src evals/schema crates/eval-harness/tests/report_rendering.rs
git commit -m "feat(evals): publish self-consistent artifact reports"
```

## Completion Evidence

Before implementing suite-specific fixes:

- prove the observed 3/66 retrieval failures affect aggregate retrieval metrics;
- prove extraction and claim summaries are computed from total confusion counts;
- show non-zero PR and release `duration_ms`;
- show exact expected/observed coverage equality;
- show a regression-budget gate is invalid without a baseline;
- validate artifact v2 against JSON Schema and semantic recomputation;
- generate the corrected Markdown report solely from the artifact.

