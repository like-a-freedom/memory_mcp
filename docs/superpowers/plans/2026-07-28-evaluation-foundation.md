# Evaluation Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Build a private Rust `eval-harness` crate that gives local evaluation one truthful outcome model, deterministic profiles, typed artifacts, and enforceable quality gates.

**Architecture:** Convert the repository root into a package-plus-workspace and add `crates/eval-harness`, which depends on `memory_mcp` but is never linked into the shipped binary. Domain types, metrics, gates, orchestration, artifacts, and suite adapters are focused modules; the CLI is a thin adapter. The first usable slice migrates deterministic retrieval, extraction, and claim-reconciliation evaluation into the `pr` profile while retaining temporary compatibility launchers.

**Tech Stack:** Rust 2024, Tokio 1.53, Serde/serde_json, Clap 4.6, thiserror 2, SHA-256, existing `memory_mcp` test infrastructure.

## Global Constraints

- Keep exactly eight public MCP tools; evaluation must not add an MCP tool or production CLI command.
- Every selected case has exactly one outcome: `passed`, `quality_failed`, or `invalid`.
- Empty, incomplete, malformed, timed-out, or setup-failed suites are invalid and cannot pass.
- Development splits are diagnostic; frozen test splits gate.
- Retrieval-only and end-to-end headline metrics are never aggregated.
- A gate combines use-case-derived hard floors with an approved-baseline regression budget.
- Run all safely executable cases, assemble the artifact, then evaluate gates.
- JSON is the source of truth; human output is derived from JSON.
- Production code continues to use `MemoryError`; eval-harness uses its own `EvalError`.
- No `unwrap`, `expect`, or `panic` for recoverable harness failures.
- Never hold a lock guard across `.await`; concurrency added in later plans must be bounded.
- Preserve the existing `default = []` production feature policy.
- Do not modify or discard unrelated changes already present in `tests/eval_extraction.rs` or `tests/eval_retrieval.rs`.
- Each task ends with `cargo fmt --all --check`, targeted tests, and a focused commit.

---

## File Map

| Path | Responsibility |
|---|---|
| `Cargo.toml` | Workspace membership and workspace-owned build profiles |
| `crates/eval-harness/Cargo.toml` | Private eval-only dependencies and `memory-eval` binary |
| `crates/eval-harness/src/lib.rs` | Narrow re-exports and top-level `run` entry point |
| `crates/eval-harness/src/main.rs` | Parse CLI, call library, map result to exit code |
| `crates/eval-harness/src/error.rs` | `EvalError` and contextual error variants |
| `crates/eval-harness/src/domain.rs` | Profiles, modes, case IDs, outcomes, trust and split enums |
| `crates/eval-harness/src/artifact.rs` | Versioned JSON artifact and deterministic serialization |
| `crates/eval-harness/src/metrics.rs` | Cutoff-aware retrieval and classification metrics |
| `crates/eval-harness/src/gate.rs` | Absolute floors, regression budgets, and gate decisions |
| `crates/eval-harness/src/profile.rs` | Validated JSON profile manifests |
| `crates/eval-harness/src/runner.rs` | Suite registry, run-all behavior, artifact assembly |
| `crates/eval-harness/src/suites.rs` | Suite module declarations and registry construction |
| `crates/eval-harness/src/suites/retrieval.rs` | Deterministic local retrieval evaluator |
| `crates/eval-harness/src/suites/extraction.rs` | Deterministic extraction evaluator |
| `crates/eval-harness/src/suites/claims.rs` | Claim-reconciliation evaluator with exact matching |
| `evals/profiles/pr.json` | Declared local PR suites, coverage, and gates |
| `evals/baselines/pr.json` | Reviewed comparison artifact for PR regression budgets |
| `evals/schema/eval-artifact-v1.json` | Checked-in JSON Schema for artifacts |
| `tests/eval_support/metrics.rs` | Temporary compatibility delegation to honest metrics |
| `tests/eval_retrieval.rs` | Temporary ignored launcher for the harness suite |
| `tests/eval_extraction.rs` | Temporary ignored launcher for the harness suite |
| `tests/eval_claim_reconciliation.rs` | Temporary ignored launcher for the harness suite |
| `Makefile` | Thin `eval-pr` compatibility command |

### Task 1: Create the private workspace crate and typed domain

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/eval-harness/Cargo.toml`
- Create: `crates/eval-harness/src/lib.rs`
- Create: `crates/eval-harness/src/main.rs`
- Create: `crates/eval-harness/src/error.rs`
- Create: `crates/eval-harness/src/domain.rs`

**Interfaces:**
- Produces: `EvalProfile`, `EvalMode`, `EvalCaseId`, `EvalCaseOutcome`, `CaseStatus`, `CorpusSplit`, `LabelTrust`, `RunCompleteness`, `ShardSpec`, `EvalError`.
- Produces: `RunRequest { profile_path, artifact_path, baseline_path, suite_filter, shard: Option<ShardSpec> }`.
- Produces: `pub async fn run(request: RunRequest) -> Result<RunArtifact, EvalError>` declared in `lib.rs`; implemented after runner work.
- Consumes: `memory_mcp` only as a path dependency; the production crate never depends on `eval-harness`.

- [x] **Step 1: Add a failing domain serialization test**

```rust
#[test]
fn case_status_serializes_with_the_truth_contract_names() {
    assert_eq!(serde_json::to_string(&CaseStatus::Passed).unwrap(), "\"passed\"");
    assert_eq!(
        serde_json::to_string(&CaseStatus::QualityFailed).unwrap(),
        "\"quality_failed\""
    );
    assert_eq!(serde_json::to_string(&CaseStatus::Invalid).unwrap(), "\"invalid\"");
}
```

- [x] **Step 2: Run the new crate test and verify the crate is absent**

Run: `cargo test -p eval-harness case_status_serializes_with_the_truth_contract_names`

Expected: FAIL because package `eval-harness` does not exist.

- [x] **Step 3: Add workspace metadata and the private package**

Use this root structure without moving the existing package:

```toml
[workspace]
members = [".", "crates/eval-harness"]
resolver = "3"
```

Create the package with `publish = false`, library name `eval_harness`, binary
name `memory-eval`, and these dependencies: path dependency on `memory_mcp`,
`async-trait`, `chrono` with `serde` and `clock`, `clap` with `derive`,
`hex`, `serde` with `derive`, `serde_json`, `sha2`, `thiserror`, and `tokio`
with `macros`, `rt-multi-thread`, `sync`, and `time`.

- [x] **Step 4: Implement validated domain enums and IDs**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Passed,
    QualityFailed,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvalCaseId(String);

impl EvalCaseId {
    pub fn parse(raw: impl Into<String>) -> Result<Self, EvalError> {
        let value = raw.into();
        if value.trim().is_empty() {
            return Err(EvalError::InvalidConfig("case id must not be empty".into()));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
```

Define `EvalProfile::{Pr, Release, Nightly}`,
`EvalMode::{RetrievalOnly, EndToEnd, Lifecycle, Performance}`,
`CorpusSplit::{Development, Test}`, and
`LabelTrust::{Official, Reviewed, Weak}` as exhaustively matched Serde enums.
Define `RunCompleteness::{Complete, Shard { index, count }}` and validate
`count > 0` plus `index < count`; this describes run coverage and never adds a
fourth case outcome.
Define `EvalCaseOutcome` with required `case_id`, `suite_id`, `mode`, `split`,
`label_trust`, `status`, `metrics`, `invalid_reason`, `failures`, `duration_ms`,
and `attempts`. Validate that only `Invalid` carries `invalid_reason`.

- [x] **Step 5: Implement contextual harness errors**

```rust
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("invalid evaluation configuration: {0}")]
    InvalidConfig(String),
    #[error("evaluation input is invalid: {0}")]
    InvalidInput(String),
    #[error("evaluation I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("evaluation artifact serialization failed: {0}")]
    Artifact(#[from] serde_json::Error),
    #[error("evaluation suite failed: {0}")]
    Suite(String),
}
```

- [x] **Step 6: Make the binary a thin error-to-exit adapter**

`main.rs` parses no suite policy. It calls `eval_harness::cli::parse()` and
`eval_harness::run(request).await`; invalid or failed gates exit non-zero after
the artifact has been written.

- [x] **Step 7: Run checks and commit**

Run:

```bash
cargo test -p eval-harness
cargo check --workspace
cargo fmt --all --check
```

Expected: all pass with zero warnings.

Commit:

```bash
git add Cargo.toml Cargo.lock crates/eval-harness
git commit -m "build(evals): add private evaluation harness crate"
```

### Task 2: Add the versioned artifact and Truth Contract validation

**Files:**
- Create: `crates/eval-harness/src/artifact.rs`
- Create: `evals/schema/eval-artifact-v1.json`
- Modify: `crates/eval-harness/src/lib.rs`

**Interfaces:**
- Consumes: `EvalCaseOutcome`, `EvalProfile`, `EvalMode`.
- Produces: `RunArtifact::validate() -> Result<(), EvalError>`.
- Produces: `write_artifact(path: &Path, artifact: &RunArtifact) -> Result<(), EvalError>`.
- Produces: schema version constant `EVAL_ARTIFACT_SCHEMA_V1`.

- [x] **Step 1: Write failing tests for empty and duplicate coverage**

```rust
#[test]
fn empty_run_is_invalid() {
    let artifact = RunArtifact::fixture(Vec::new(), vec!["case-1".into()]);
    assert!(matches!(artifact.validate(), Err(EvalError::InvalidInput(_))));
}

#[test]
fn selected_case_must_appear_exactly_once() {
    let outcome = passed_fixture("case-1");
    let artifact = RunArtifact::fixture(
        vec![outcome.clone(), outcome],
        vec!["case-1".into()],
    );
    assert!(artifact.validate().is_err());
}
```

- [x] **Step 2: Run tests and verify both fail**

Run: `cargo test -p eval-harness artifact::tests -- --nocapture`

Expected: FAIL because `RunArtifact` is undefined.

- [x] **Step 3: Implement artifact types and invariants**

```rust
pub const EVAL_ARTIFACT_SCHEMA_V1: &str = "memory-mcp-eval/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifact {
    pub schema_version: String,
    pub run_id: String,
    pub profile: EvalProfile,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub expected_case_ids: Vec<EvalCaseId>,
    pub outcomes: Vec<EvalCaseOutcome>,
    pub suite_summaries: Vec<SuiteSummary>,
    pub gates: Vec<GateDecision>,
    pub fingerprint: RunFingerprint,
}
```

`validate` must reject empty expected coverage, empty outcomes, duplicate
expected IDs, duplicate outcomes, missing outcomes, unexpected outcomes,
inconsistent status/reason combinations, non-finite metrics, and an
unsupported schema version. Sort outcomes by `(suite_id, case_id)` before
serialization.

- [x] **Step 4: Write the artifact atomically**

Serialize to a sibling `*.tmp` file, call `sync_all`, then rename. Return
`EvalError::Io` with the exact path on every filesystem failure. Do not print
from the library.

- [x] **Step 5: Check in a strict JSON Schema**

Set `additionalProperties: false` on artifact/domain objects, require all Truth
Contract fields, constrain status enum values, and require schema version
`memory-mcp-eval/v1`. Add a test that parses the schema and asserts its
`$id == "https://memory-mcp.dev/schemas/eval-artifact-v1.json"`.

- [x] **Step 6: Run checks and commit**

Run:

```bash
cargo test -p eval-harness artifact
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src evals/schema
git commit -m "feat(evals): add truthful evaluation artifact"
```

### Task 3: Replace optimistic metrics with cutoff-aware results

**Files:**
- Create: `crates/eval-harness/src/metrics.rs`
- Modify: `crates/eval-harness/src/lib.rs`
- Modify: `tests/eval_support/metrics.rs`

**Interfaces:**
- Produces: `retrieval_metrics(cases: &[RetrievalObservation], cutoff: NonZeroUsize) -> Result<RetrievalMetrics, EvalError>`.
- Produces: `classification_metrics(counts: ClassificationCounts) -> Result<ClassificationMetrics, EvalError>`.
- Produces: `RetrievalObservation { relevant_ids: BTreeSet<String>, ranked_ids: Vec<String> }`.

- [x] **Step 1: Write failing metric-contract tests**

```rust
#[test]
fn recall_at_five_ignores_a_hit_at_rank_six() {
    let observation = RetrievalObservation {
        relevant_ids: ["expected".to_string()].into_iter().collect(),
        ranked_ids: ["a", "b", "c", "d", "e", "expected"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    };
    let metrics = retrieval_metrics(&[observation], NonZeroUsize::new(5).unwrap()).unwrap();
    assert_eq!(metrics.recall_at_k, 0.0);
}

#[test]
fn empty_metric_input_is_invalid() {
    assert!(retrieval_metrics(&[], NonZeroUsize::new(5).unwrap()).is_err());
}
```

- [x] **Step 2: Run the tests and verify the old behavior cannot satisfy them**

Run: `cargo test -p eval-harness metrics`

Expected: FAIL because cutoff-aware metrics are not implemented.

- [x] **Step 3: Implement exact denominators and ranks**

For each case, inspect only `ranked_ids[..min(k, len)]` for recall@k. Compute
MRR from the first relevant rank in the full returned ranking and top-1 from
rank one. Reject a case with no relevant IDs as invalid input rather than
removing it from a denominator. Use tolerance-based float assertions in tests.

- [x] **Step 4: Implement zero-denominator classification behavior**

`classification_metrics` returns `None` for precision when no positives were
predicted and `None` for recall when no positives were expected. It returns an
error when the entire classification population is zero. It never substitutes
`1.0`.

- [x] **Step 5: Make legacy metric helpers delegate during migration**

Change `tests/eval_support/metrics.rs` so compatibility callers use the new
cutoff-aware functions. Rename `recall_at_5` to `recall_at_k(NonZeroUsize)` and
update its tests; remove the test that expects perfect empty metrics.

- [x] **Step 6: Run checks and commit**

Run:

```bash
cargo test -p eval-harness metrics
cargo test --test eval_retrieval eval_support::metrics
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src/metrics.rs tests/eval_support/metrics.rs
git commit -m "fix(evals): enforce exact metric denominators"
```

### Task 4: Add typed profiles and two-level quality gates

**Files:**
- Create: `crates/eval-harness/src/profile.rs`
- Create: `crates/eval-harness/src/gate.rs`
- Create: `evals/profiles/pr.json`
- Modify: `crates/eval-harness/src/lib.rs`

**Interfaces:**
- Produces: `ProfileManifest::load(path: &Path) -> Result<Self, EvalError>`.
- Produces: `evaluate_gates(summary: &RunSummary, policy: &GatePolicy, baseline: Option<&RunArtifact>) -> Vec<GateDecision>`.
- Produces: `GateDecision { metric, observed, hard_floor, baseline, regression_budget, status, reason }`.

- [x] **Step 1: Write failing profile validation tests**

```rust
#[test]
fn pr_profile_rejects_missing_expected_coverage() {
    let raw = r#"{"schema_version":"memory-mcp-eval-profile/v1","profile":"pr",
        "time_budget_seconds":600,"suites":[],"gates":[]}"#;
    assert!(ProfileManifest::parse(raw).is_err());
}
```

- [x] **Step 2: Write failing two-level gate tests**

```rust
#[test]
fn regression_fails_even_above_the_hard_floor() {
    let decision = evaluate_metric_gate(0.94, 0.90, Some(0.98), Some(0.02));
    assert_eq!(decision.status, GateStatus::Failed);
    assert_eq!(decision.reason, GateFailureReason::RegressionBudgetExceeded);
}
```

- [x] **Step 3: Implement strict profile parsing**

Use `#[serde(deny_unknown_fields)]` raw structs, then `TryFrom<RawProfile>` to
validate: non-empty suites, non-empty expected case selectors, positive time
budget, unique suite IDs, test-only release gates, and no weak-label gate.

- [x] **Step 4: Define the initial PR profile**

`evals/profiles/pr.json` must declare a 600-second budget and the deterministic
local retrieval, extraction, and claim suites. Development claim cases are
reported but only test-split metrics appear in gates. Record exact fixture
paths and expected case IDs or a checked fixture digest; do not use wildcard
discovery without an expected count.

- [x] **Step 5: Implement hard-floor and baseline decisions**

Evaluate hard floors first, then regression budgets. A missing required
baseline is invalid, not passed. Reject a baseline with a different schema,
suite, mode, corpus fingerprint, evaluator version, or configuration hash.

- [x] **Step 6: Run checks and commit**

Run:

```bash
cargo test -p eval-harness profile gate
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src evals/profiles/pr.json
git commit -m "feat(evals): add validated profiles and gates"
```

### Task 5: Build run-all orchestration and deterministic reporting

**Files:**
- Create: `crates/eval-harness/src/runner.rs`
- Create: `crates/eval-harness/src/suites.rs`
- Modify: `crates/eval-harness/src/artifact.rs`
- Modify: `crates/eval-harness/src/lib.rs`

**Interfaces:**
- Produces: `#[async_trait] pub trait EvalSuite`.
- Produces: `Runner::run(&self, request: RunRequest) -> Result<RunArtifact, EvalError>`.
- Consumes: `ProfileManifest`, `RunFingerprint`, `Vec<Arc<dyn EvalSuite>>`.

- [x] **Step 1: Write a failing run-all test**

```rust
#[tokio::test]
async fn quality_failure_does_not_prevent_later_cases_from_running() {
    let suites = vec![
        fake_suite("a", CaseStatus::QualityFailed),
        fake_suite("b", CaseStatus::Passed),
    ];
    let artifact = Runner::new(suites).run(fixture_request()).await.unwrap();
    assert_eq!(artifact.outcomes.len(), 2);
    assert!(artifact.outcomes.iter().any(|case| case.case_id.as_str() == "b"));
    assert!(artifact.gates.iter().any(|gate| gate.status == GateStatus::Failed));
}
```

- [x] **Step 2: Define the suite contract**

```rust
#[async_trait]
pub trait EvalSuite: Send + Sync {
    fn id(&self) -> &str;
    fn mode(&self) -> EvalMode;
    fn expected_case_ids(&self) -> &[EvalCaseId];
    async fn run(&self, context: &RunContext) -> Vec<EvalCaseOutcome>;
}
```

Suite implementations convert recoverable per-case errors to `invalid`
outcomes. Only a process-level inability to construct or write the artifact
returns `EvalError`.

- [x] **Step 3: Implement deterministic runner assembly**

Run every selected suite, retain every outcome, sort by suite and case ID,
validate exact expected coverage, calculate summaries, evaluate gates after all
suites, validate the final artifact, and then write it. Do not call
`process::exit` or print inside `Runner`.

- [x] **Step 4: Add fingerprint capture**

Capture Rust version, OS/arch, package version, build profile, enabled features,
provider, model, device, sanitized configuration hash, Git commit when
available, evaluator versions, and profile digest. Never store secrets or raw
credential-bearing environment values.

- [x] **Step 5: Derive the concise summary from the artifact**

Render profile, duration, counts by all three statuses, suite metrics, failed
gates, invalid reasons, and artifact path. Add a golden string test that proves
ordering is independent of completion order.

- [x] **Step 6: Run checks and commit**

Run:

```bash
cargo test -p eval-harness runner artifact
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src
git commit -m "feat(evals): orchestrate complete truthful runs"
```

### Task 6: Migrate deterministic retrieval and extraction suites

**Files:**
- Create: `crates/eval-harness/src/suites/retrieval.rs`
- Create: `crates/eval-harness/src/suites/extraction.rs`
- Modify: `crates/eval-harness/src/suites.rs`
- Modify: `tests/eval_retrieval.rs`
- Modify: `tests/eval_extraction.rs`

**Interfaces:**
- Produces: `LocalRetrievalSuite` and `ExtractionSuite`, both implementing `EvalSuite`.
- Consumes: existing JSON fixtures and production `MemoryService` calls.
- Produces: per-case metric maps with versioned names such as `recall_at_5`, `mrr`, `top_1_hit_rate`, `entity_precision`, `entity_recall`, and `fact_type_accuracy`.

- [x] **Step 1: Add a failing retrieval case at rank six**

Create a harness unit test with six ranked IDs and the expected ID at rank six.
Assert `recall_at_5 == 0.0`, `mrr == 1.0 / 6.0`, and status
`quality_failed` when the case requires recall@5 of 1.0.

- [x] **Step 2: Add a failing extraction warning-recall case**

Build an extraction observation with two expected warning IDs and one exact
match. Assert warning recall is 0.5 and the outcome is `quality_failed`, not
`passed`.

- [x] **Step 3: Port fixture parsing without changing labels**

Move evaluation-only parsing and comparison into focused harness modules.
Preserve the existing fixture bytes and IDs. Parse timestamps and expected
fields fallibly; convert malformed cases to `invalid`. Do not change thresholds
or frozen labels in this task.

- [x] **Step 4: Use exact result boundaries**

Retrieval evaluates the returned top-k IDs/content at the declared cutoff.
Extraction compares canonical entity keys, fact types, and warning identities
using explicit normalized equality. Any heuristic comparison is named,
versioned, and diagnostic only.

- [x] **Step 5: Convert old ignored runners to thin launchers**

Each old `run_*_evals` test should invoke `memory-eval` or the harness library
for exactly one suite and fail when the artifact contains a failed/invalid
gate. Remove duplicate metric and report logic only after parity artifacts have
been reviewed.

- [x] **Step 6: Run parity and commit**

Run:

```bash
cargo test -p eval-harness suites::retrieval suites::extraction
cargo test --test eval_retrieval
cargo test --test eval_extraction
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --suite local-retrieval --suite extraction --artifact target/evals/pr-foundation.json
```

Expected: every fixture case appears once; known quality failures fail the
gate rather than printing PASS.

Commit:

```bash
git add crates/eval-harness tests/eval_retrieval.rs tests/eval_extraction.rs
git commit -m "refactor(evals): migrate retrieval and extraction suites"
```

### Task 7: Migrate claim reconciliation with exact evidence

**Files:**
- Create: `crates/eval-harness/src/suites/claims.rs`
- Modify: `crates/eval-harness/src/suites.rs`
- Modify: `tests/eval_claim_reconciliation.rs`
- Modify: `docs/evals/CLAIM_RECONCILIATION.md`

**Interfaces:**
- Produces: `ClaimReconciliationSuite`.
- Consumes: `tests/fixtures/evals/claim_reconciliation_cases.json`.
- Produces: exact per-split confusion counts and case outcomes.

- [x] **Step 1: Write failing tests for the three known truth defects**

```rust
#[test]
fn source_id_substring_is_not_an_exact_match() {
    assert!(!warning_matches("source:12", "source:123"));
}

#[test]
fn invalid_reference_time_invalidates_the_case() {
    assert!(parse_reference_time("not-a-time").is_err());
}

#[test]
fn_expected_isolation_skip_is_not_an_observed_violation() {
    assert_eq!(isolation_violations(&expected_skip_fixture(), &[]), 0);
}
```

- [x] **Step 2: Run tests and verify failures**

Run: `cargo test -p eval-harness suites::claims`

Expected: FAIL until exact reconciliation evaluation exists.

- [x] **Step 3: Implement exact warning/relation matching**

Resolve expected source IDs to the actual episode/fact IDs created during
setup. Compare typed IDs and expected outcomes/reason codes. Never use
`str::contains` for identity. Treat ambiguous multiple matches as invalid.

- [x] **Step 4: Remove current-time fallback**

Parse every fixture timestamp before creating the service. A parse failure
produces one invalid case with the parser error. The evaluator never calls
`Utc::now()` as replacement evidence.

- [x] **Step 5: Separate expected isolation from violations**

Count a violation only when an actual relation crosses scope, project, policy,
subject, or comparison-key boundaries. Expected skip reason codes contribute
to coverage and true-negative evaluation, not to violation counts.

- [x] **Step 6: Gate only the frozen test split**

Emit development and test summaries separately. Keep development diagnostic;
apply hard floors and regression budgets only to `CorpusSplit::Test`. Assert
that zero precision/recall cannot yield a passing gate.

- [x] **Step 7: Run parity and commit**

Run:

```bash
cargo test -p eval-harness suites::claims
cargo test --test eval_claim_reconciliation
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --artifact target/evals/pr-with-claims.json
```

Commit:

```bash
git add crates/eval-harness tests/eval_claim_reconciliation.rs docs/evals/CLAIM_RECONCILIATION.md
git commit -m "fix(evals): make claim reconciliation evidence exact"
```

### Task 8: Expose the PR profile through a thin CLI and Make target

**Files:**
- Create: `crates/eval-harness/src/cli.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Modify: `crates/eval-harness/src/lib.rs`
- Modify: `Makefile`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: `memory-eval run --profile <path> --artifact <path> [--baseline <path>] [--suite <id>]`.
- Consumes: `Runner::run`.
- Produces: exit 0 only for a valid artifact with all required gates passed.

- [x] **Step 1: Write failing CLI parsing tests**

```rust
#[test]
fn run_requires_profile_and_artifact_paths() {
    let parsed = Cli::try_parse_from(["memory-eval", "run"]);
    assert!(parsed.is_err());
}
```

- [x] **Step 2: Implement the thin command**

The CLI loads the profile, constructs the registered suites, runs them, writes
the artifact, prints the derived summary, and returns exit 2 for invalid runs
and exit 1 for quality-gate failure. It must still write the artifact before
returning either non-zero status.

- [x] **Step 3: Replace Make policy with one adapter**

Add:

```make
eval-pr:
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/pr.json \
		--artifact target/evals/pr.json
```

Keep old targets temporarily as aliases with a deprecation message; do not
retain their suite lists or stdout diff.

- [x] **Step 4: Add a non-blocking PR artifact job**

Add a CI job that runs `eval-pr`, always uploads `target/evals/pr.json` with
`if: always()`, and initially reports the result without becoming a required
check until two consecutive representative runs meet the 600-second budget and
baseline review is complete.

- [x] **Step 5: Establish the first reviewed baseline**

After reviewing the full case-level artifact and explaining every difference
from the legacy report, copy the schema-valid artifact to
`evals/baselines/pr.json`. Re-run with
`--baseline evals/baselines/pr.json` and assert both hard floors and regression
budgets are evaluated. Any later baseline replacement follows the same
before/after review.

- [x] **Step 6: Verify the complete foundation**

Run:

```bash
cargo test --workspace --lib --bins --tests
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
make eval-pr
```

Expected: normal tests pass; `make eval-pr` writes schema version
`memory-mcp-eval/v1`, contains every declared case exactly once, and exits
according to its real gate result.

- [x] **Step 7: Commit**

```bash
git add crates/eval-harness Makefile .github/workflows/ci.yml evals/profiles/pr.json evals/baselines/pr.json Cargo.lock
git commit -m "ci(evals): publish the truthful pr evaluation"
```

## Completion Evidence

Before starting the corpus plan, attach:

- `target/evals/pr.json`;
- its schema-validation result;
- selected/observed case-ID equality;
- old-versus-new metric comparison with every intentional difference
  explained;
- wall-clock duration on the declared PR runner;
- confirmation that `cargo build -p memory_mcp --release` does not build or
  link `eval-harness`.
