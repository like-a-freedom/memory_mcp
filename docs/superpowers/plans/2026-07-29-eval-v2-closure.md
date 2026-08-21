# Evaluation V2 Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Status: Task 1 complete; Task 2 in progress

**Goal:** Turn the 2026-07-29 diagnostic v2 run into a truthful, fully gated evaluation whose claim, end-to-end, lifecycle, external-retrieval, and performance results measure the intended production behavior within the PR 10-minute and release 20-minute budgets.

**Architecture:** Keep `memory-eval` as the private orchestration boundary and add an explicit artifact verdict derived from case outcomes, gates, coverage, and the time budget. Give claim evaluation a feature-gated, read-only evidence seam into persisted claims and relations; run end-to-end and lifecycle cases through their real production entry points; keep model and database setup outside single-stage Criterion timing. Promote PR, release, and nightly profiles only after exact coverage, external-corpus execution, benchmark provenance, and a reviewed baseline are all present.

**Tech Stack:** Rust 2024, Tokio 1.53, Serde/serde_json, SurrealDB, Criterion 0.5, Candle GLiNER CPU/Metal, GitHub Actions, existing `memory_mcp` and private `eval-harness` workspace crates.

## Global Constraints

- Preserve the frozen eight-tool MCP public surface.
- Do not add eval behavior to MCP handlers, ordinary CLI commands, or `src/main.rs`.
- The `eval-support` feature is additive, disabled by default, read-only, and unavailable from the production binary unless explicitly enabled.
- A run is `passed` only when coverage is exact, every required case passed, every gate passed, and the time budget passed.
- A run containing any required `quality_failed` case is `quality_failed`, even when the profile declares no metric gates.
- A run containing invalid evidence, a missing selected suite, zero outcomes for a selected suite, an invalid gate, or an incomplete artifact is `invalid`.
- PR and release evaluation failures must fail CI; artifact upload still runs with `if: always()`.
- Nightly may remain a scheduled diagnostic job, but its artifact and report must never label quality failures as passed.
- Fixture source IDs are never compared directly with persisted fact, claim, or relation IDs.
- Claim precision and recall use persisted claim relations and recorded source lineage, not substring matching or warning counts.
- Cross-scope, cross-project, and policy-boundary violations have a hard floor of zero tolerated violations.
- End-to-end entity assertions inspect extraction output; retrieval assertions inspect assembled context.
- Lifecycle poisoning executes `LifecycleCapture`, durable projection, `LifecycleRecall`, and a deterministic attempted-action policy.
- CPU and Metal benchmarks use the same pinned model, labels, threshold, input corpus, timing boundary, and output-parity oracle.
- A Metal benchmark that cannot initialize Metal is unsupported/invalid evidence, never a successful nanosecond measurement.
- Contention reports operations per second and per-operation latency; raw iteration duration alone is not throughput.
- PR wall-clock budget remains at most 600 seconds.
- Release wall-clock budget remains at most 1,200 seconds for the merged complete artifact.
- Existing 2026-07-28 and 2026-07-29 reports remain immutable historical evidence.
- Do not approve a new baseline until the complete corrected release artifact has `verdict = "passed"`.
- Every task follows TDD, ends with focused verification, and is committed independently.

---

## Current Evidence and Scope Boundary

The 2026-07-29 run proves that reducer aggregation, elapsed-time capture, CPU
NER execution, database-backed contention, and stable end-to-end case IDs have
improved. It does not yet prove claim quality, lifecycle poisoning safety,
external-corpus release quality, Metal performance, or the full release time
budget.

This plan closes only the remaining gaps observed in:

- `docs/evals/BENCHMARK_RUN_REPORT_2026-07-29.md`;
- `target/evals/v2-pr.json`;
- `target/evals/v2-release.json`;
- `target/evals/v2-nightly.json`;
- the v2 Criterion output under `target/evals/reports/v2/benches/`.

No new ADR is required. Task 1 completes ADR-0019's truthful profile contract;
Task 6 completes ADR-0017's wired lifecycle evidence gate.

## File Map

| Path | Responsibility |
|---|---|
| `crates/eval-harness/src/domain.rs` | `RunVerdict`, namespaced expected case keys, typed run issues |
| `crates/eval-harness/src/artifact.rs` | Artifact v2, exact coverage validation, stored verdict |
| `crates/eval-harness/src/profile.rs` | Required suite declarations and exact coverage policy |
| `crates/eval-harness/src/runner.rs` | Selected-suite accounting, verdict derivation, budget status |
| `crates/eval-harness/src/gate.rs` | Direction-aware metric gates |
| `crates/eval-harness/src/report.rs` | Truthful machine-derived Markdown result |
| `crates/eval-harness/src/main.rs` | Thin exit-code mapping and report subcommand |
| `evals/schema/eval-artifact-v2.json` | Strict schema for verdict and namespaced coverage |
| `src/eval_support.rs` | Feature-gated read-only persisted claim/relation evidence |
| `src/lib.rs` | Feature-gated export of `eval_support` only |
| `Cargo.toml` | Additive `eval-support` feature |
| `crates/eval-harness/Cargo.toml` | Enable `eval-support`; require `metal` for Metal bench |
| `crates/eval-harness/src/suites/claims.rs` | Lineage-aware persisted claim evaluation |
| `crates/eval-harness/src/suites/extraction.rs` | Exact extraction and contradiction-warning evidence |
| `src/service/claims/schema.rs` | Structural commitment projection for promise facts |
| `src/service/claims/project.rs` | Fact type supplied to the claim projector |
| `tests/fixtures/evals/extraction_cases.json` | Frozen extraction and promise-warning expectations |
| `src/service/context/ranking.rs` | Explicit-time valid-time-first retrieval selection |
| `crates/eval-harness/src/suites/retrieval.rs` | Ranked retrieval identity and temporal diagnostics |
| `tests/fixtures/evals/retrieval_cases.json` | Frozen `ret-063` breadth/time oracle |
| `crates/eval-harness/src/suites/end_to_end.rs` | Separate extraction and retrieval oracles |
| `tests/fixtures/evals/end_to_end_cases.json` | Scope-aware typed end-to-end expectations |
| `crates/eval-harness/src/suites/action_grounding.rs` | Real lifecycle recall and action evidence |
| `crates/eval-harness/src/suites/capacity.rs` | Real capture and persisted growth evidence |
| `crates/eval-harness/src/suites/poisoning.rs` | Capture-to-attempted-action adversarial replay |
| `crates/eval-harness/src/suites/lifecycle.rs` | Aggregate wired lifecycle release gate |
| `tests/fixtures/evals/agent_memory_lifecycle_cases.json` | Frozen lifecycle and poisoning scenarios |
| `crates/eval-harness/src/benchmark.rs` | Shared pinned NER fixture and benchmark provenance |
| `crates/eval-harness/benches/ner_cpu.rs` | Comparable real CPU inference |
| `crates/eval-harness/benches/ner_metal.rs` | Comparable real Metal inference |
| `crates/eval-harness/benches/contention.rs` | Normalized shared-service contention |
| `crates/eval-harness/src/suites/external_retrieval.rs` | Exact IDs, canonical import, bounded workers |
| `crates/eval-harness/src/adapters.rs` | Persisted canonical facts and actual ID mapping |
| `evals/profiles/pr.json` | Required fast suites and stratified external sample |
| `evals/profiles/release.json` | Complete external retrieval and lifecycle gates |
| `evals/profiles/nightly.json` | Required diagnostic suites with truthful verdict |
| `evals/baselines/pr.json` | Reviewed corrected PR comparison artifact, created only after approval |
| `.github/workflows/ci.yml` | Enforced PR/release jobs and always-uploaded artifacts |
| `docs/performance/NER_PERFORMANCE.md` | Correct benchmark contract and measured provenance |
| `docs/evals/BENCHMARK_RUN_REPORT_2026-07-30.md` | Generated corrected-run report |

### Task 1: Make the run verdict and selected-suite coverage truthful

**Files:**
- Modify: `crates/eval-harness/src/domain.rs`
- Modify: `crates/eval-harness/src/artifact.rs`
- Modify: `crates/eval-harness/src/profile.rs`
- Modify: `crates/eval-harness/src/runner.rs`
- Modify: `crates/eval-harness/src/gate.rs`
- Modify: `crates/eval-harness/src/report.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Create: `evals/schema/eval-artifact-v2.json`
- Test: `crates/eval-harness/tests/run_verdict.rs`

**Interfaces:**
- Produces: `RunVerdict::{Passed, QualityFailed, Invalid}`.
- Produces: `RunIssue { stage: RunStage, suite_id: Option<SuiteId>, message: String }`.
- Produces: `RunIssue::empty_suite(suite_id: &str) -> RunIssue`.
- Produces: `derive_run_verdict(outcomes: &[EvalCaseOutcome], gates: &[GateDecision], budget_status: GateStatus, issues: &[RunIssue]) -> RunVerdict`.
- Produces: `GateDirection::{AtLeast, AtMost}` and direction-aware gate evaluation.
- Changes: `RunArtifact.expected_cases` to `Vec<CaseKey>`.
- Changes: `RunArtifact.budget_status` from `Option<GateStatus>` to `GateStatus`; every valid profile has a positive budget.
- Changes: `RunArtifact` to include `verdict: RunVerdict` and `issues: Vec<RunIssue>`.
- Changes: `ExpectedCoverage` to `{ exact_cases: usize }`; `min_cases` is removed.
- Changes: `RunRequest` to carry suite-load issues into the artifact.
- Consumes: the exact suite list loaded from `ProfileManifest`; no implicit registry-wide suites.

- [x] **Step 1: Write failing verdict truth-table tests**

```rust
use eval_harness::{
    derive_run_verdict, CaseStatus, CorpusSplit, EvalCaseOutcome, EvalMode,
    GateStatus, LabelTrust, RunIssue, RunVerdict,
};

fn fixture_outcome(suite: &str, case: &str, status: CaseStatus) -> EvalCaseOutcome {
    let mut outcome = EvalCaseOutcome::new(
        suite,
        case,
        EvalMode::EndToEnd,
        CorpusSplit::Test,
        LabelTrust::Official,
        status,
    );
    if status == CaseStatus::Invalid {
        outcome.invalid_reason = Some("fixture invalid evidence".into());
    }
    outcome
}

#[test]
fn quality_failure_cannot_pass_without_metric_gates() {
    let outcomes = vec![fixture_outcome("end-to-end", "entity", CaseStatus::QualityFailed)];
    assert_eq!(
        derive_run_verdict(&outcomes, &[], GateStatus::Passed, &[]),
        RunVerdict::QualityFailed
    );
}

#[test]
fn invalid_evidence_dominates_failed_quality() {
    let outcomes = vec![
        fixture_outcome("claims", "c1", CaseStatus::QualityFailed),
        fixture_outcome("claims", "c2", CaseStatus::Invalid),
    ];
    assert_eq!(
        derive_run_verdict(&outcomes, &[], GateStatus::Passed, &[]),
        RunVerdict::Invalid
    );
}

#[test]
fn empty_selected_suite_is_invalid() {
    let issues = vec![RunIssue::empty_suite("downstream-qa")];
    assert_eq!(
        derive_run_verdict(&[], &[], GateStatus::Passed, &issues),
        RunVerdict::Invalid
    );
}
```

- [x] **Step 2: Run the focused verdict tests and verify all three fail**

Run:

```bash
cargo test -p eval-harness --test run_verdict -- --nocapture
```

Expected: compilation fails because `RunVerdict`, `RunIssue`, and
`derive_run_verdict` do not exist.

- [x] **Step 3: Add the minimal verdict and issue domain types**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunVerdict {
    Passed,
    QualityFailed,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStage {
    SuiteLoad,
    SuiteRun,
    Coverage,
    Gate,
    Budget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunIssue {
    pub stage: RunStage,
    pub suite_id: Option<SuiteId>,
    pub message: String,
}
```

Implement `derive_run_verdict` with this precedence:

1. any `RunIssue`, invalid outcome, invalid gate, or invalid budget → `Invalid`;
2. any quality-failed outcome, failed gate, or failed budget → `QualityFailed`;
3. otherwise → `Passed`.

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDirection {
    AtLeast,
    AtMost,
}
```

Require every `GateDecl` to carry an explicit direction. `AtLeast` fails when
`observed < hard_floor`; `AtMost` fails when `observed > hard_floor`.
Regression comparison follows the same direction: `AtLeast` fails when
`baseline - observed > budget`, while `AtMost` fails when
`observed - baseline > budget`.

- [x] **Step 4: Replace global expected IDs with namespaced case keys**

Change the artifact field to:

```rust
pub expected_cases: Vec<CaseKey>,
```

Build it from each selected suite as:

```rust
expected_cases.extend(
    suite
        .expected_case_ids()
        .iter()
        .map(|case_id| CaseKey::parse(suite.id(), case_id.as_str()))
        .collect::<Result<Vec<_>, _>>()?,
);
```

Do not sort/deduplicate before validation. Validation must reject duplicate
`CaseKey` values, missing outcomes, and unexpected outcomes.

- [x] **Step 5: Write failing selected-suite accounting tests**

```rust
#[tokio::test]
async fn declared_suite_that_failed_to_load_is_an_invalid_run() {
    let request = fixture_request_with_declared_suite("missing-suite");
    let artifact = Runner::new(vec![]).run(&request).await.unwrap();
    assert_eq!(artifact.verdict, RunVerdict::Invalid);
    assert_eq!(artifact.issues[0].stage, RunStage::SuiteLoad);
}

#[tokio::test]
async fn selected_suite_returning_zero_outcomes_is_invalid() {
    let runner = Runner::new(vec![Box::new(EmptySuite::new("downstream-qa"))]);
    let artifact = runner.run(&fixture_request("downstream-qa")).await.unwrap();
    assert_eq!(artifact.verdict, RunVerdict::Invalid);
}
```

- [x] **Step 6: Make suite construction return evidence instead of warnings**

Add a private registry result in `main.rs`:

```rust
struct LoadedSuites {
    suites: Vec<Box<dyn EvalSuite>>,
    issues: Vec<RunIssue>,
}
```

Every declared suite must produce either one suite instance or one
`RunStage::SuiteLoad` issue. Pass these issues in `RunRequest`; never print a
warning and silently omit a declared suite.

Define the test-only request helpers in `run_verdict.rs`:

```rust
fn fixture_request(suite_id: &str) -> RunRequest {
    request_from_manifest(profile_manifest_with_suite(suite_id))
}

fn fixture_request_with_declared_suite(suite_id: &str) -> RunRequest {
    fixture_request(suite_id)
}
```

`profile_manifest_with_suite` constructs one suite with
`ExpectedCoverage { exact_cases: 1 }`; `request_from_manifest` fills temporary
artifact/profile paths, an empty baseline, and an empty issue list.
`EmptySuite::new(id)` declares one expected case and returns zero outcomes so
the test exercises missing execution evidence rather than an empty
declaration.

- [x] **Step 7: Store verdict in artifact v2 and validate it**

Set:

```rust
pub const EVAL_ARTIFACT_SCHEMA_V2: &str = "memory-mcp-eval/v2";
```

`RunArtifact::validate()` must recompute the verdict and reject an artifact
whose stored verdict differs. Update `eval-artifact-v2.json` with
`additionalProperties: false`, namespaced `expected_cases`, `verdict`, and
`issues`.

- [x] **Step 8: Map the CLI exit code exclusively from the stored verdict**

```rust
match artifact.verdict {
    RunVerdict::Passed => ExitCode::SUCCESS,
    RunVerdict::QualityFailed => ExitCode::from(1),
    RunVerdict::Invalid => ExitCode::from(2),
}
```

Write the artifact and print the generated report before returning any of
these codes.

- [x] **Step 9: Render an unambiguous report result**

The generated Markdown header must contain exactly one of:

```text
**Verdict:** PASSED
**Verdict:** QUALITY FAILED
**Verdict:** INVALID
```

Include separate tables for quality failures, invalid cases, failed gates,
invalid gates, budget, and run issues. A missing gate list must render
`Metric gates: none declared`; it must not imply a passing quality result.

- [x] **Step 10: Run focused and workspace verification**

Run:

```bash
cargo test -p eval-harness --test run_verdict
cargo test -p eval-harness artifact profile runner report
cargo clippy -p eval-harness --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: all commands pass with zero warnings and zero formatting drift.

- [x] **Step 11: Commit the truthful run contract**

```bash
git add crates/eval-harness/src crates/eval-harness/tests/run_verdict.rs evals/schema/eval-artifact-v2.json
git commit -m "fix(evals): derive truthful run verdicts"
```

### Task 2: Evaluate persisted claims and relations through exact source lineage

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Create: `src/eval_support.rs`
- Modify: `crates/eval-harness/Cargo.toml`
- Modify: `crates/eval-harness/src/suites/claims.rs`
- Test: `crates/eval-harness/tests/claim_lineage.rs`
- Modify: `docs/evals/CLAIM_RECONCILIATION.md`

**Interfaces:**
- Produces: feature `eval-support = []`, disabled by default.
- Produces: `ClaimEvidenceReader::new(db: Arc<dyn DbClient>)`.
- Produces: `ClaimEvidenceReader::for_fact_ids(namespace: &str, fact_ids: &[String]) -> Result<PersistedClaimEvidence, MemoryError>`.
- Produces: `PersistedClaimEvidence { claims: Vec<EvaluatedClaim>, relations: Vec<EvaluatedRelation> }`.
- Produces: `SourceLineageMap { by_source_id: BTreeMap<String, SourceLineage> }`.
- Produces: `SourceLineage { episode_id: String, fact_ids: BTreeSet<String> }`.
- Produces: `IsolationBoundary { namespace: String, scope: String, project: Option<String>, policy_fingerprint: String }`.
- Produces: `classify_isolation_violation(relation: &EvaluatedRelation, expected: &IsolationBoundary) -> Option<IsolationViolation>`.
- Consumes: `ExtractResult.facts`, persisted claims, and persisted claim relations.
- Test helpers: `relation(left_fact_id, right_fact_id, outcome) -> EvaluatedRelation`, `expected_relation(setup_source_id, source_id, outcome) -> ExpectedRelation`, and `same_boundary() -> IsolationBoundary`.

Keep lineage matcher types `pub(crate)` and their pure tests inside
`suites/claims.rs`. The integration test invokes the public
`ClaimReconciliationSuite` and inspects its public outcomes.

- [ ] **Step 1: Write failing lineage matching tests**

```rust
#[test]
fn expected_relation_matches_only_the_mapped_fact_pair() {
    let lineage = SourceLineageMap::from_pairs([
        ("setup-1", ["fact:old"]),
        ("source-1", ["fact:new"]),
    ]);
    let actual = relation("fact:old", "fact:new", "contradiction");

    assert!(matches_expected_relation(
        &expected_relation("setup-1", "source-1", "contradiction"),
        &actual,
        &lineage
    ));
    assert!(!matches_expected_relation(
        &expected_relation("setup-1", "other-source", "contradiction"),
        &actual,
        &lineage
    ));
}

#[test]
fn different_fact_ids_are_not_an_isolation_violation() {
    let relation = relation("fact:old", "fact:new", "contradiction");
    assert_eq!(classify_isolation_violation(&relation, &same_boundary()), None);
}
```

- [ ] **Step 2: Run the lineage tests and verify they fail**

Run:

```bash
cargo test -p eval-harness --test claim_lineage -- --nocapture
```

Expected: compilation fails because the lineage and relation evaluator types do
not exist.

- [ ] **Step 3: Add a narrow feature-gated read-only evidence seam**

In the workspace root:

```toml
[features]
default = []
eval-support = []
```

In `src/lib.rs`:

```rust
#[cfg(feature = "eval-support")]
#[doc(hidden)]
pub mod eval_support;
```

`src/eval_support.rs` may call the crate-private `ClaimStore`, but it exposes
only immutable evaluation views:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedRelation {
    pub relation_id: String,
    pub left_fact_id: String,
    pub right_fact_id: String,
    pub outcome: ClaimRelationOutcome,
    pub reason_code: String,
    pub scope: String,
    pub project: Option<String>,
    pub policy_fingerprint: String,
}
```

Do not expose SurrealDB queries, mutation methods, or this module through MCP.

- [ ] **Step 4: Record source lineage during setup and source extraction**

Change `ingest_and_extract` to return:

```rust
struct ExtractedSource {
    source_id: String,
    episode_id: String,
    fact_ids: BTreeSet<String>,
    extraction: ExtractResult,
}
```

Populate `fact_ids` only from `ExtractResult.facts[].fact_id`. Reject duplicate
fixture source IDs as invalid evidence.

- [ ] **Step 5: Replace warning matching with persisted relation matching**

For an expected relation:

```rust
fn matches_expected_relation(
    expected: &ExpectedRelation,
    actual: &EvaluatedRelation,
    lineage: &SourceLineageMap,
) -> bool {
    let left = lineage.fact_ids(&expected.setup_source_id);
    let right = lineage.fact_ids(&expected.source_id);

    left.contains(&actual.left_fact_id)
        && right.contains(&actual.right_fact_id)
        && actual.outcome.to_string() == expected.outcome
}
```

If the persisted relation is explicitly unordered, accept the reversed pair in
the same function. Never use `contains`, source-ID substrings, or warning
content as identity.

- [ ] **Step 6: Define confusion counts over the persisted relation set**

For each case calculate:

```rust
let true_positives = matched_expected_relations.len() as u64;
let false_negatives = expected_positive_relations.len() as u64 - true_positives;
let false_positives = unmatched_actual_relations.len() as u64;
let true_negatives = expected_negative_boundaries_without_relation as u64;
```

Emit `MetricEvidence::classification(tp, fp, fn_, tn)`. A case passes only
when `fn_ == 0`, `fp == 0`, and `isolation_violations == 0`. A missed expected
contradiction must be `quality_failed`.

- [ ] **Step 7: Classify isolation from boundary metadata**

Count a violation only when an actual persisted relation crosses one of:

- namespace;
- scope;
- project;
- policy fingerprint.

Different fact IDs in a valid same-boundary relation are expected and must not
be counted. Missing boundary metadata makes the case `invalid`.

- [ ] **Step 8: Add positive, negative, and boundary integration tests**

```rust
#[tokio::test]
async fn contradiction_case_matches_persisted_relation_by_lineage() {
    let outcome = run_claim_case("cr-001").await;
    assert_eq!(outcome.status, CaseStatus::Passed);
    assert_eq!(outcome.evidence["classification"], MetricEvidence::classification(1, 0, 0, 0));
}

#[tokio::test]
async fn cross_project_case_has_no_relation_and_no_violation() {
    let outcome = run_claim_case("cr-022").await;
    assert_eq!(outcome.status, CaseStatus::Passed);
    assert_eq!(outcome.metrics["isolation_violations"], 0.0);
}

#[tokio::test]
async fn missed_expected_relation_fails_the_case() {
    let outcome = evaluate_fixture_with_empty_relation_store("cr-001").await;
    assert_eq!(outcome.status, CaseStatus::QualityFailed);
}
```

- [ ] **Step 9: Run claim evaluation and inspect denominators**

Run:

```bash
cargo test -p eval-harness --test claim_lineage
cargo test -p eval-harness suites::claims
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/pr.json \
  --artifact target/evals/pr-claim-fixed.json \
  --suites claim-reconciliation
```

Expected:

- 42 namespaced claim outcomes;
- no invalid outcomes;
- non-zero official-test precision and recall denominators;
- case status agrees with its confusion counts;
- zero isolation violations.

- [ ] **Step 10: Document the exact claim metric contract**

Record corpus version, split, persisted evidence tables, lineage mapping,
positive outcomes, negative boundaries, confusion-matrix formula, and the rule
that warning metrics are diagnostic only.

- [ ] **Step 11: Run quality checks and commit**

```bash
cargo clippy -p memory_mcp -p eval-harness --all-targets --features eval-support -- -D warnings
cargo fmt --all --check
git add Cargo.toml src/lib.rs src/eval_support.rs crates/eval-harness docs/evals/CLAIM_RECONCILIATION.md
git commit -m "fix(evals): score persisted claims by source lineage"
```

### Task 3: Close the two promise-warning extraction failures

**Files:**
- Modify: `src/service/claims/schema.rs`
- Modify: `src/service/claims/project.rs`
- Modify: `src/service/episode/fact_extraction.rs`
- Modify: `crates/eval-harness/src/suites/extraction.rs`
- Test: `crates/eval-harness/tests/extraction_promises.rs`
- Read-only oracle: `tests/fixtures/evals/extraction_cases.json`

**Interfaces:**
- Changes: `FactPersistedParams` and `ClaimProjectionInput` to carry `fact_type: &str`.
- Produces: `CommitmentSentence { subject: String, action: String, target: String, deadline: String }`.
- Produces: `parse_commitment_sentence(content: &str) -> Option<CommitmentSentence>`.
- Produces: `ExtractionWarningEvidence { expected: u64, matched: u64, unexpected: u64 }`.
- Consumes: the unchanged `ext-006` and `ext-007` official expectations.
- Test helpers: `project_promise(content) -> ClaimDraftCandidate` and `project_fact(fact_type, content) -> Vec<ClaimDraftCandidate>` invoke the real built-in schema registry.

Parser and matcher tests remain co-located unit tests. The integration helper
`run_fixture_case(id)` runs the public `ExtractionSuite`, then selects the
outcome whose namespaced case ID equals `id`.

- [ ] **Step 1: Write failing production commitment-projection tests**

```rust
#[test]
fn promise_sentence_projects_action_target_and_deadline() {
    let parsed = parse_commitment_sentence(
        "Alice Smith will send Bob Jones the prototype by Friday."
    ).unwrap();

    assert_eq!(parsed.subject, "Alice Smith");
    assert_eq!(parsed.action, "send");
    assert_eq!(parsed.target, "Bob Jones the prototype");
    assert_eq!(parsed.deadline, "Friday");
}

#[test]
fn shifted_deadline_keeps_the_same_comparison_key() {
    let friday = project_promise("Alice Smith will send Bob Jones the prototype by Friday.");
    let monday = project_promise("Alice Smith will send Bob Jones the prototype by Monday.");

    assert_eq!(friday.comparison_key, monday.comparison_key);
    assert_ne!(friday.qualifiers["deadline"], monday.qualifiers["deadline"]);
}
```

- [ ] **Step 2: Run the focused projection tests and verify they fail**

```bash
cargo test -p memory_mcp service::claims::schema::tests::promise_sentence -- --nocapture
cargo test -p memory_mcp service::claims::schema::tests::shifted_deadline -- --nocapture
```

Expected: the sentence parser is absent and prose promise facts do not produce
commitment drafts.

- [ ] **Step 3: Pass the persisted fact type into claim projection**

Add:

```rust
pub(crate) struct FactPersistedParams<'a> {
    pub namespace: &'a str,
    pub fact_id: &'a FactId,
    pub source_episode_id: &'a EpisodeId,
    pub fact_type: &'a str,
    pub content: &'a str,
    pub scope: &'a str,
    pub project: Option<&'a str>,
    pub entity_links: &'a [String],
    pub t_valid: DateTime<Utc>,
}

pub(crate) struct ClaimProjectionInput<'a> {
    pub subject: &'a str,
    pub t_ref: DateTime<Utc>,
    pub fact_type: &'a str,
    pub content: &'a str,
    pub structured_fields: &'a BTreeMap<String, String>,
    pub assertions: &'a [StructuralAssertion],
}
```

Update every constructor and test fixture. The commitment fallback introduced
below runs only when `fact_type == "promise"`; other schemas keep their current
behavior.

- [ ] **Step 4: Add the bounded English commitment sentence parser**

For official v1 promise fixtures, parse this exact grammar:

```text
<subject> " will " <action> <target> " by " <deadline> ["."]
```

Rules:

- subject, action, target, and deadline must all be non-empty;
- action is the first token after `will`;
- target is the remaining text before `by`;
- strip only terminal sentence punctuation from deadline;
- normalize comparison-key components with `NormalizedText`;
- return `None` for text without both ` will ` and ` by `;
- do not add a regex or NLP dependency.

- [ ] **Step 5: Project prose promises through `CommitmentV1`**

When structured `action` and `target` fields are absent and
`fact_type == "promise"`, use the parsed sentence:

```rust
let mut qualifiers = BTreeMap::new();
qualifiers.insert(
    "deadline".to_string(),
    NormalizedText::new(&parsed.deadline).to_string(),
);

ClaimDraftCandidate {
    schema_ref: self.schema_ref(),
    subject: NormalizedText::new(&parsed.subject).to_string(),
    comparison_key: commitment_key(&parsed.action, &parsed.target)?,
    qualifiers,
    value: ClaimValue::Boolean(true),
    cardinality: ClaimCardinality::SingleValued,
    observed_at: input.t_ref,
    valid_from: None,
    valid_to: None,
    validity_source: ClaimValiditySource::Explicit,
    source_lineage: None,
    source_span: None,
}
```

The two promise versions therefore occupy the same slot but have different
deadline qualifiers, allowing the existing reconciliation engine to produce a
contradiction without content heuristics.

- [ ] **Step 6: Add negative projection tests**

```rust
#[test]
fn non_promise_fact_does_not_use_commitment_sentence_fallback() {
    let drafts = project_fact(
        "experience",
        "Alice Smith will send Bob Jones the prototype by Friday."
    );
    assert!(drafts.iter().all(|draft| {
        draft.schema_ref.family != ClaimSchemaFamily::Commitment
    }));
}

#[test]
fn promise_without_deadline_is_not_invented() {
    assert!(parse_commitment_sentence(
        "Alice Smith will send Bob Jones the prototype."
    ).is_none());
}
```

- [ ] **Step 7: Make warning evidence inspectable**

Emit `ExtractionWarningEvidence` per case and include the normalized expected
and actual warning triples:

```text
fact_type
existing_content
new_content
```

Unexpected warnings count as false positives. Missing warnings count as false
negatives. Do not relax `warning_matches` or convert exact official labels to
substring matching.

- [ ] **Step 8: Add the two failing cases as focused integration tests**

```rust
#[tokio::test]
async fn ext_006_detects_shifted_delivery_date() {
    let outcome = run_fixture_case("ext-006").await;
    assert_eq!(outcome.status, CaseStatus::Passed);
    assert_eq!(outcome.metrics["warning_recall"], 1.0);
}

#[tokio::test]
async fn ext_007_detects_shifted_checklist_handoff() {
    let outcome = run_fixture_case("ext-007").await;
    assert_eq!(outcome.status, CaseStatus::Passed);
    assert_eq!(outcome.metrics["warning_recall"], 1.0);
}
```

`run_fixture_case(id)` loads the existing official fixture by exact ID and
invokes the same `run_case` path used by the profile.

- [ ] **Step 9: Run the complete extraction suite**

```bash
cargo test -p eval-harness --test extraction_promises
cargo test -p eval-harness suites::extraction
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/pr.json \
  --artifact target/evals/pr-extraction-fixed.json \
  --suites extraction
```

Expected: nine expected cases, nine outcomes, zero invalid outcomes,
`ext-006` and `ext-007` warning recall equal to `1.0`, and no unexpected
warnings in negative cases.

- [ ] **Step 10: Run quality checks and commit**

```bash
cargo clippy -p memory_mcp -p eval-harness --all-targets --features eval-support -- -D warnings
cargo fmt --all --check
git add src/service/claims/schema.rs src/service/claims/project.rs src/service/episode/fact_extraction.rs crates/eval-harness/src/suites/extraction.rs crates/eval-harness/tests/extraction_promises.rs
git commit -m "fix(claims): project deadlines from promise facts"
```

### Task 4: Resolve the remaining explicit-time retrieval failure

**Files:**
- Modify: `src/service/context/ranking.rs`
- Modify: `crates/eval-harness/src/suites/retrieval.rs`
- Test: `crates/eval-harness/tests/retrieval_temporal_breadth.rs`
- Read-only oracle: `tests/fixtures/evals/retrieval_cases.json`

**Interfaces:**
- Produces: `TemporalCandidatePartition { in_window: Vec<RankedContextFact>, textual_fallback: Vec<RankedContextFact> }`.
- Produces: `partition_temporal_candidates(facts: Vec<RankedContextFact>, focus: &TemporalWindow, query_terms: &[String]) -> TemporalCandidatePartition`.
- Produces: `RetrievalRankEvidence { fact_id: String, rank: usize, source_episode: String, t_valid: DateTime<Utc>, in_temporal_window: bool, matched_query_terms: Vec<String> }`.
- Consumes: existing `infer_temporal_window`, `fact_is_within_temporal_focus`, MMR, source caps, and the unchanged `ret-063` official oracle.
- Test helpers: `ret_063_ranked_facts`, `retrospective_only_facts`, and `mixed_date_facts` construct deterministic `RankedContextFact` values; `selected_ids`, `fixed_cutoff`, `april_window`, and the query-term helpers return exact stable inputs used in the assertions.

- [ ] **Step 1: Write the failing valid-time-first ranking test**

```rust
#[test]
fn explicit_april_query_selects_all_april_updates_before_stale_umbrella_text() {
    let selected = select_ranked_context_facts(
        ret_063_ranked_facts(),
        6,
        infer_temporal_window(
            "april 2026 alpha suite delta control signal monitor orbit portal updates",
            fixed_cutoff(),
        ),
        query_terms(),
    );

    assert_eq!(
        selected_ids(&selected),
        vec!["alpha-april", "delta-april", "signal-april", "orbit-april"]
    );
}
```

- [ ] **Step 2: Add failing fallback and non-temporal safety tests**

```rust
#[test]
fn explicit_window_uses_textual_fallback_only_when_no_valid_time_fact_exists() {
    let selected = select_ranked_context_facts(
        retrospective_only_facts(),
        3,
        april_window(),
        april_query_terms(),
    );
    assert_eq!(selected_ids(&selected), vec!["retrospective-april-summary"]);
}

#[test]
fn non_temporal_query_keeps_normal_relevance_ordering() {
    let selected = select_ranked_context_facts(
        mixed_date_facts(),
        3,
        None,
        product_query_terms(),
    );
    assert_eq!(selected_ids(&selected)[0], "broad-product-summary");
}
```

- [ ] **Step 3: Run the focused ranking tests and confirm the first fails**

```bash
cargo test -p memory_mcp service::context::ranking::tests::explicit_april -- --nocapture
cargo test -p memory_mcp service::context::ranking::tests::explicit_window -- --nocapture
cargo test -p memory_mcp service::context::ranking::tests::non_temporal -- --nocapture
```

Expected: stale umbrella candidates that mention `april 2026` can enter the
protected direct head ahead of one valid-time April update.

- [ ] **Step 4: Partition valid-time evidence from textual fallback**

For an explicit `TemporalWindow`:

```rust
let in_temporal_window = fact_is_within_temporal_focus(&candidate, focus);
let textual_temporal_match =
    fact_matches_all_query_terms(&candidate, &temporal_query_terms(query_terms));
```

Place valid-time candidates in `in_window`. Place out-of-window candidates in
`textual_fallback` only when their content contains all explicit temporal
terms. Candidates satisfying neither predicate remain excluded.

- [ ] **Step 5: Apply the valid-time-first selection policy**

Rules:

1. if `in_window` is non-empty, rank and select only that partition;
2. if `in_window` is empty, rank the textual fallback partition;
3. never fill unused budget slots with out-of-window summaries after selecting
   valid-time facts;
4. apply the existing MMR, source cap, grounding floor, and deterministic
   tie-breakers inside the chosen partition;
5. non-temporal queries keep the existing path unchanged.

This is a semantic policy for explicit valid-time queries, not a
`ret-063`-specific score boost.

- [ ] **Step 6: Keep direct-head seeding inside the chosen partition**

Pass only the active partition to `seed_direct_recall_head`. A protected
lexical item cannot bypass explicit valid-time selection merely because its
content repeats the requested month and year.

- [ ] **Step 7: Emit per-rank temporal evidence from the suite**

Record one `RetrievalRankEvidence` item for every returned context item.
`ret-063` must show four in-window April facts, four unique source episodes,
and no January/February source. Preserve fact identity separately from content
matching.

- [ ] **Step 8: Add the exact `ret-063` profile regression**

```rust
#[tokio::test]
async fn ret_063_recalls_all_four_april_updates() {
    let outcome = run_retrieval_fixture("ret-063").await;
    assert_eq!(outcome.status, CaseStatus::Passed);
    assert_eq!(outcome.metrics["recall_at_5"], 1.0);
    assert_eq!(outcome.metrics["top_1_hit_rate"], 1.0);
}
```

`run_retrieval_fixture` runs the public `LocalRetrievalSuite` and selects the
exact namespaced outcome.

- [ ] **Step 9: Run the complete local retrieval corpus**

```bash
cargo test -p eval-harness --test retrieval_temporal_breadth
cargo test -p memory_mcp service::context::ranking
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/pr.json \
  --artifact target/evals/pr-retrieval-fixed.json \
  --suites local-retrieval
```

Expected: 66 expected cases, 66 outcomes, zero invalid outcomes,
`ret-063` recall `1.0`, and no regression in the other 65 cases.

- [ ] **Step 10: Run quality checks and commit**

```bash
cargo clippy -p memory_mcp -p eval-harness --all-targets --features eval-support -- -D warnings
cargo fmt --all --check
git add src/service/context/ranking.rs crates/eval-harness/src/suites/retrieval.rs crates/eval-harness/tests/retrieval_temporal_breadth.rs
git commit -m "fix(retrieval): prioritize valid time for explicit windows"
```

### Task 5: Separate end-to-end extraction and retrieval evidence

**Files:**
- Modify: `crates/eval-harness/src/suites/end_to_end.rs`
- Modify: `tests/fixtures/evals/end_to_end_cases.json`
- Test: `crates/eval-harness/tests/end_to_end_truth.rs`
- Modify: `evals/profiles/nightly.json`

**Interfaces:**
- Produces: `EndToEndCase { scope, project, sources, expected_entities, expected_context, min_context_items, ... }`.
- Produces: `EntityExpectation { canonical_name: String, entity_type: Option<String> }`.
- Produces: `ContextExpectation { content_contains: String }`.
- Produces: `EntityEvidence { matched: u64, total: u64, unexpected: u64 }`.
- Produces: `PassRateReducer::new(suite_id: &str) -> PassRateReducer`.
- Consumes: `ExtractResult.entities` for entity evidence and `assemble_context` for context evidence.
- Test helpers: `load_case(id) -> EndToEndCase`, `entity(name, kind) -> ExtractedEntity`, and `expect_entity(name) -> EntityExpectation`.

- [ ] **Step 1: Write failing scope and evidence-channel tests**

```rust
#[tokio::test]
async fn team_case_queries_the_team_scope() {
    let case = load_case("e2e-entity-extraction");
    assert_eq!(case.scope, "team");
    let outcome = run_e2e_case(&case).await;
    assert_ne!(outcome.metrics["context_items_returned"], 0.0);
}

#[tokio::test]
async fn entity_oracle_reads_extract_result_not_context_text() {
    let evidence = evaluate_entities(
        &[entity("Alice Smith", "person"), entity("Acme Corp", "organization")],
        &[expect_entity("Alice Smith"), expect_entity("Acme Corp")],
    );
    assert_eq!(evidence.matched, 2);
    assert_eq!(evidence.total, 2);
}
```

- [ ] **Step 2: Run the tests and confirm the hard-coded `org` query fails**

Run:

```bash
cargo test -p eval-harness --test end_to_end_truth -- --nocapture
```

Expected: the team-scope case returns zero context items and the entity
evaluator symbol is missing.

- [ ] **Step 3: Make scope and project case-level fields**

Change the entity case fixture to:

```json
{
  "id": "e2e-entity-extraction",
  "scope": "team",
  "project": null,
  "expected_entities": [
    {"canonical_name": "Alice Smith", "entity_type": "person"},
    {"canonical_name": "Acme Corp", "entity_type": "organization"},
    {"canonical_name": "Bob Johnson", "entity_type": "person"}
  ],
  "expected_context": [
    {"content_contains": "API design review"}
  ],
  "min_context_items": 1
}
```

Every source in one case must have the same scope/project as the case.
Fixture validation rejects a mismatch.

- [ ] **Step 4: Retain every extraction result**

Collect:

```rust
let mut extractions: Vec<ExtractResult> = Vec::with_capacity(case.sources.len());
```

Push each successful extraction result before querying context. Extraction
errors remain invalid execution, not quality failure.

- [ ] **Step 5: Add normalized exact entity matching**

Normalize Unicode NFC, case, and whitespace for comparison. Match
`canonical_name` exactly after normalization; if `entity_type` is present,
require exact type equality. Emit:

```rust
MetricEvidence::classification(entity_tp, entity_fp, entity_fn, 0)
```

Do not search entity names inside context strings.

- [ ] **Step 6: Query with the fixture's isolation boundary**

Build `AssembleContextRequest` with:

```rust
scope: case.scope.clone(),
project: case.project.clone(),
```

Use the fixture reference time plus one second for `as_of` rather than wall
clock time so the result is deterministic.

- [ ] **Step 7: Make the case status depend on both evidence channels**

The case is passed only if:

- every expected entity matches;
- every expected context matcher matches;
- returned context count meets `min_context_items`;
- no unexpected cross-boundary item is returned.

Each failed predicate gets a separate failure string and typed evidence.

- [ ] **Step 8: Add the end-to-end nightly gate**

Add `PassRateReducer`, which emits `case_pass_rate = passed / total` and
returns an error for a zero denominator. Declare:

```json
{
  "target": {
    "suite_id": "end-to-end",
    "metric": "case_pass_rate"
  },
  "hard_floor": 1.0,
  "baseline_required": false
}
```

Nightly still receives `verdict = "quality_failed"` if any required case
fails independently of this metric gate.

- [ ] **Step 9: Run the focused nightly profile**

```bash
cargo test -p eval-harness --test end_to_end_truth
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/nightly.json \
  --artifact target/evals/nightly-e2e-fixed.json \
  --suites end-to-end
```

Expected: two expected case keys, two outcomes, zero invalid outcomes, and no
scope-induced empty context. If an entity is still missed, retain the
`quality_failed` result and diagnose the production extractor separately; do
not weaken the fixture in this task.

- [ ] **Step 10: Run quality checks and commit**

```bash
cargo clippy -p eval-harness --all-targets -- -D warnings
cargo fmt --all --check
git add crates/eval-harness/src/suites/end_to_end.rs crates/eval-harness/tests/end_to_end_truth.rs tests/fixtures/evals/end_to_end_cases.json evals/profiles/nightly.json
git commit -m "fix(evals): separate e2e extraction and retrieval evidence"
```

### Task 6: Wire lifecycle evaluation from capture through attempted action

**Files:**
- Modify: `crates/eval-harness/src/test_support.rs`
- Modify: `crates/eval-harness/src/suites/action_grounding.rs`
- Modify: `crates/eval-harness/src/suites/capacity.rs`
- Modify: `crates/eval-harness/src/suites/poisoning.rs`
- Modify: `crates/eval-harness/src/suites/lifecycle.rs`
- Modify: `tests/fixtures/evals/agent_memory_lifecycle_cases.json`
- Test: `crates/eval-harness/tests/lifecycle_wired.rs`
- Modify: `docs/evals/AGENT_MEMORY_LIFECYCLE.md`
- Modify: `evals/profiles/release.json`

**Interfaces:**
- Produces: `LifecycleEvalHarness { capture: LifecycleCapture, recall: LifecycleRecall, store: AgentMemoryStore, service: MemoryService }`.
- Produces: `LifecycleEvalHarness::project_until_terminal(job_id: &str, timeout: Duration) -> Result<ProjectedEvent, EvalError>`.
- Produces: `AttemptedAction { kind: ActionKind, arguments: BTreeMap<String, String>, risk: ActionRisk }`.
- Produces: `ActionDisposition::{Execute, RequireLiveVerification, Refuse}`.
- Produces: `evaluate_attempt(recall: &LifecycleRecallResult, action: &AttemptedAction) -> ActionDisposition`.
- Consumes: frozen cases from `agent_memory_lifecycle_cases.json`.
- Test helpers: `external_instruction_case()`, `repeated_polling_case()`, and `delete_repository_action()` return fully populated frozen-domain values from named fixture cases.

- [ ] **Step 1: Write failing wired-path tests**

```rust
#[tokio::test]
async fn external_instruction_is_quarantined_before_projection() {
    let mut harness = LifecycleEvalHarness::new().await;
    let result = harness.capture(external_instruction_case()).await.unwrap();
    assert!(matches!(result, LifecycleCaptureResult::Quarantined { .. }));
    assert_eq!(harness.persisted_ordinary_episode_count().await, 0);
}

#[tokio::test]
async fn recalled_untrusted_memory_cannot_authorize_privileged_action() {
    let mut harness = LifecycleEvalHarness::new().await;
    let recalled = harness.replay("coding_stale_memory_influence").await.unwrap();
    let disposition = evaluate_attempt(&recalled, &delete_repository_action());
    assert_eq!(disposition, ActionDisposition::RequireLiveVerification);
}

#[tokio::test]
async fn ignored_polling_has_zero_persisted_growth() {
    let mut harness = LifecycleEvalHarness::new().await;
    let before = harness.storage_usage().await.unwrap();
    harness.capture(repeated_polling_case()).await.unwrap();
    let after = harness.storage_usage().await.unwrap();
    assert_eq!(after.rows - before.rows, 0);
    assert_eq!(after.serialized_bytes - before.serialized_bytes, 0);
}
```

- [ ] **Step 2: Run the wired tests and verify they fail**

Run:

```bash
cargo test -p eval-harness --test lifecycle_wired -- --nocapture
```

Expected: compilation fails because `LifecycleEvalHarness`,
`ActionDisposition`, and storage usage evidence do not exist.

- [ ] **Step 3: Build a production-backed lifecycle test harness**

Construct the same in-memory `DbClient`, `IngestionService`,
`ProductionCaptureBackend`, `AgentMemoryStore`, `LifecycleCapture`,
`LifecycleRecall`, and worker projection path used by production. Keep this
construction in `eval-harness::test_support`; do not add it to MCP handlers.

- [ ] **Step 4: Wait for durable projection with a bounded terminal poll**

Implement a 5-second total timeout and 25-millisecond poll interval. Terminal
states are `completed`, `failed`, and `dead_letter`. Timeout, failed, or
dead-letter projection makes the case invalid and records job ID plus terminal
state.

- [ ] **Step 5: Replace action-grounding recall proxies**

For each action-grounding case:

1. capture the source event;
2. project it to facts/claims;
3. call `LifecycleRecall::execute`;
4. pass the recalled envelope to `evaluate_attempt`;
5. compare `ActionDisposition` with the frozen expected disposition.

The action evaluator is deterministic and may consume memory only as
source-labeled data. High-risk actions require a live-verification signal that
does not come from memory.

- [ ] **Step 6: Replace capacity structural checks**

Measure before/after:

```rust
pub struct StorageUsage {
    pub rows: u64,
    pub serialized_bytes: u64,
}
```

Assert zero growth for ignored polling and duplicate delivery, one accepted
event/job for first delivery, no extra event/job for replay, and rejection
before episode creation when the capture budget is exhausted.

- [ ] **Step 7: Replay poisoning through the complete lifecycle**

Replace direct `ingest → extract → assemble_context` with:

```text
NormalizedHostEvent
  → LifecycleCapture
  → quarantine or durable projection
  → LifecycleRecall
  → AttemptedAction
  → ActionDisposition
```

Pass criteria:

- external instruction: quarantined, no ordinary episode, action refused or
  requires verification;
- stale/contradicted memory: retained as data with provenance, never sufficient
  to execute a high-risk action;
- bounded benign envelope: allowed only for the declared low-risk action.

- [ ] **Step 8: Query the live MCP tool registry for public-surface evidence**

Replace the static eight-string array with the same registry snapshot used by
the MCP server. Assert the exact ordered set:

```rust
[
    "ingest",
    "extract",
    "resolve",
    "assemble_context",
    "explain",
    "invalidate",
    "open_app",
    "app_command",
]
```

The eval may read the registry through an existing library function; it must
not instantiate a second hand-maintained list.

- [ ] **Step 9: Make aggregate lifecycle status strict**

An empty sub-suite is invalid. Any invalid child makes the aggregate invalid.
Any quality-failed child makes the aggregate quality-failed. Only all-passed
children produce a passed aggregate. Emit child counts and pass rates with
non-zero denominators.

- [ ] **Step 10: Add release gates**

Declare:

```json
[
  {
    "target": {"suite_id": "lifecycle", "metric": "action_grounding_pass_rate"},
    "hard_floor": 1.0,
    "baseline_required": false
  },
  {
    "target": {"suite_id": "lifecycle", "metric": "poisoning_pass_rate"},
    "hard_floor": 1.0,
    "baseline_required": false
  },
  {
    "target": {"suite_id": "lifecycle", "metric": "isolation_violations"},
    "hard_floor": 0.0,
    "baseline_required": false,
    "direction": "at_most"
  }
]
```

Consume `GateDirection::AtMost` from Task 1 for the zero-violation ceiling.

- [ ] **Step 11: Run lifecycle verification**

```bash
cargo test -p eval-harness --test lifecycle_wired
cargo test --test eval_agent_memory_lifecycle
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/release.json \
  --artifact target/evals/release-lifecycle-fixed.json \
  --suites lifecycle
```

Expected: four aggregate lifecycle cases, all wired child cases represented,
zero invalid outcomes, poisoning denominator equal to the frozen scenario
count, and zero isolation violations.

- [ ] **Step 12: Document evidence and commit**

Update the lifecycle document with entry points, projection timeout, action
policy, row/byte accounting, corpus version, case counts, and remaining
limitations.

```bash
git add crates/eval-harness/src crates/eval-harness/tests/lifecycle_wired.rs tests/fixtures/evals/agent_memory_lifecycle_cases.json docs/evals/AGENT_MEMORY_LIFECYCLE.md evals/profiles/release.json
git commit -m "fix(evals): wire lifecycle evidence through attempted action"
```

### Task 7: Make CPU, Metal, and contention benchmarks comparable

**Files:**
- Create: `crates/eval-harness/src/benchmark.rs`
- Modify: `crates/eval-harness/src/lib.rs`
- Modify: `crates/eval-harness/Cargo.toml`
- Modify: `crates/eval-harness/benches/ner_cpu.rs`
- Modify: `crates/eval-harness/benches/ner_metal.rs`
- Modify: `crates/eval-harness/benches/contention.rs`
- Create: `crates/eval-harness/tests/benchmark_contract.rs`
- Modify: `docs/performance/NER_PERFORMANCE.md`

**Interfaces:**
- Produces: `NerBenchmarkFixture::load(device: NerDeviceKind) -> Result<Self, EvalError>`.
- Produces: `NerBenchmarkFixture::single_window() -> &str`.
- Produces: `NerBenchmarkFixture::multi_window() -> &str`.
- Produces: `NerBenchmarkFixture::extract(input: &str) -> Result<Vec<EntityCandidate>, EvalError>`.
- Produces: `BenchmarkProvenance { model, model_digest, device, labels, threshold, token_cap, input_digest }`.
- Produces: `ContentionObservation { clients, operations, elapsed, ops_per_second, latency_per_operation }`.
- Produces: `assert_candidate_parity(cpu: &[EntityCandidate], metal: &[EntityCandidate]) -> Result<(), EvalError>`.
- Produces: `NerBenchmarkFixture::metadata_only()` for token/window contract tests without model loading.

- [ ] **Step 1: Write failing benchmark-contract tests**

```rust
#[test]
fn multi_window_fixture_exceeds_the_model_window() {
    let fixture = NerBenchmarkFixture::metadata_only();
    assert!(fixture.multi_window_token_count() > fixture.max_sequence_length());
}

#[test]
fn contention_normalizes_by_completed_operations() {
    let observation = ContentionObservation::new(4, 12, Duration::from_millis(300));
    assert_eq!(observation.ops_per_second(), 40.0);
    assert_eq!(observation.latency_per_operation(), Duration::from_millis(25));
}
```

- [ ] **Step 2: Run contract tests and verify they fail**

```bash
cargo test -p eval-harness --test benchmark_contract -- --nocapture
```

Expected: compilation fails because the shared benchmark contract does not
exist.

- [ ] **Step 3: Create one pinned NER fixture**

Build `NerConfig` with:

```rust
NerConfig {
    provider: NerProviderKind::LocalGliner,
    model: Some("urchade/gliner_multi-v2.1".into()),
    model_dir: Some(pinned_model_dir),
    labels: pinned_labels(),
    threshold: 0.5,
    batch_size: 1,
    max_batch_tokens: 384,
    max_concurrency: 1,
    device,
}
```

Load the extractor once through `create_entity_extractor`. Hash the resolved
model files and input text. Verify the pinned model directory and digest before
calling the factory; a missing or mismatched model is invalid benchmark
evidence and must not trigger a network download. Store this provenance beside
Criterion output.

- [ ] **Step 4: Give CPU single- and multi-window benches identical boundaries**

For both benchmarks:

1. load model/extractor before `b.iter`;
2. warm it with one untimed extraction;
3. time only `extract_candidates`;
4. use the same labels and threshold;
5. record Criterion throughput in input tokens;
6. assert the output is non-empty outside the timed closure.

Use the frozen 520-word corpus for multi-window input. Remove service creation,
database creation, ingest, and wall-clock timestamps from NER timing.

- [ ] **Step 5: Replace the Metal token-counting stub**

Set in `crates/eval-harness/Cargo.toml`:

```toml
[features]
metal = ["memory_mcp/metal"]

[[bench]]
name = "ner_metal"
harness = false
required-features = ["metal"]
```

Load `NerBenchmarkFixture` with `NerDeviceKind::Metal`. If Metal initialization
fails, return an error before Criterion starts; do not fall back to CPU.

- [ ] **Step 6: Add CPU/Metal quality parity**

Before measuring Metal, compare ordered candidates with CPU:

```rust
assert_candidate_parity(&cpu_candidates, &metal_candidates)?;
```

Compare the public `EntityCandidate` fields exactly: canonical name, entity
type, and aliases in stable order. Do not publish Metal latency if parity
fails. Tensor-level numerical diagnostics remain separate from the public
candidate parity gate.

- [ ] **Step 7: Normalize contention workloads**

Use the same number of operations per iteration for 1, 2, and 4 clients.
Create and warm one shared service outside timing. Assign unique source IDs
outside the measured body. Count successful completed ingest+extract
operations and report:

```text
clients
operations
elapsed_ms
ops_per_second
p50_operation_ms
p95_operation_ms
```

Do not infer throughput saturation from raw iteration time.

- [ ] **Step 8: Configure Criterion and provenance output**

Use a 5-second warm-up, 30-second measurement, at least 30 samples, and 95%
confidence. Write a JSON provenance file containing the exact model digest,
device, host fingerprint, effective config, and input digests.

- [ ] **Step 9: Run CPU and contention verification**

```bash
cargo test -p eval-harness --test benchmark_contract
cargo bench -p eval-harness --bench ner_cpu -- --noplot
cargo bench -p eval-harness --bench contention -- --noplot
```

Expected: non-empty candidate output, a multi-window workload larger than one
model window, and normalized throughput for all client counts.

- [ ] **Step 10: Run Metal verification on Apple Silicon**

```bash
cargo bench -p eval-harness --features metal --bench ner_metal -- --noplot
```

Expected: the provenance device is `metal`, candidate parity passes, and the
measured duration is real inference latency rather than tens of nanoseconds.
On a runner without Metal, record the benchmark as unsupported and do not
create a performance baseline.

- [ ] **Step 11: Update the performance document and commit**

Replace the v2 stub and raw-duration interpretation with the exact fixture,
timing boundaries, parity result, throughput normalization, raw output paths,
and unsupported-platform policy.

```bash
git add crates/eval-harness docs/performance/NER_PERFORMANCE.md
git commit -m "perf(evals): benchmark comparable cpu and metal inference"
```

### Task 8: Execute external corpora, enforce profiles in CI, and approve a corrected baseline

**Files:**
- Modify: `crates/eval-harness/src/adapters.rs`
- Modify: `crates/eval-harness/src/suites/external_retrieval.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Modify: `crates/eval-harness/src/cli.rs`
- Modify: `evals/profiles/pr.json`
- Modify: `evals/profiles/release.json`
- Modify: `evals/profiles/nightly.json`
- Modify: `.github/workflows/ci.yml`
- Modify: `Makefile`
- Create after review: `evals/baselines/pr.json`
- Create after successful run: `docs/evals/BENCHMARK_RUN_REPORT_2026-07-30.md`
- Test: `crates/eval-harness/tests/external_release.rs`

**Interfaces:**
- Changes: `ImportedFact` to contain both `fixture_fact_id` and `persisted_fact_id`.
- Changes: `ExternalRetrievalSuite` to own `expected_ids: Vec<EvalCaseId>`.
- Produces: `run_external_case(case: ExternalCase, context_workers: NonZeroUsize, query_workers: NonZeroUsize) -> EvalCaseOutcome`.
- Produces: CLI `memory-eval report --artifact <path> --output <path>`.
- Consumes: prepared immutable corpus bytes and exact manifest revisions; no network access during evaluation.
- Test helpers: `fixture_external_cases(count)`, `suite_with_panicking_case(id)`, `fixture_context()`, and `import_fixture_fact()` construct deterministic in-memory inputs with no network access.

- [ ] **Step 1: Write failing external-suite completeness tests**

```rust
#[test]
fn external_suite_expected_ids_equal_all_loaded_cases() {
    let cases = fixture_external_cases(12);
    let suite = ExternalRetrievalSuite::new(DatasetKind::LongMemEval, cases.clone());
    assert_eq!(suite.expected_case_ids().len(), cases.len());
}

#[tokio::test]
async fn worker_panic_becomes_invalid_outcome() {
    let suite = suite_with_panicking_case("external:panic");
    let outcomes = suite.run(&fixture_context()).await;
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].status, CaseStatus::Invalid);
}

#[tokio::test]
async fn canonical_import_returns_actual_persisted_ids() {
    let result = import_fixture_fact().await.unwrap();
    assert_ne!(result.facts[0].persisted_fact_id, "");
    assert_eq!(result.facts[0].fixture_fact_id, "fixture:fact:0");
}
```

- [ ] **Step 2: Run external-release tests and verify they fail**

```bash
cargo test -p eval-harness --test external_release -- --nocapture
```

Expected: expected IDs are empty, join failures disappear, and persisted ID
mapping is unavailable.

- [ ] **Step 3: Preserve actual persisted IDs in canonical import**

Change:

```rust
pub struct ImportedFact {
    pub fixture_fact_id: String,
    pub persisted_fact_id: String,
    pub episode_id: String,
}
```

Store the ID returned by `MemoryService::add_fact`. Retrieval evidence maps
returned context `fact_id` back to fixture IDs through this explicit mapping.
Do not use content strings as ranked IDs.

- [ ] **Step 4: Make external coverage exact**

Build `expected_ids` from every loaded `ExternalCase` in the constructor.
Reject duplicate case IDs. `expected_case_ids()` returns this stored slice.
Missing, duplicated, or unexpected outcomes invalidate the artifact through
Task 1.

- [ ] **Step 5: Use canonical import in the suite**

Replace `seed_fact_with_links_and_project` with:

```rust
let imported = import_canonical_facts(&service, &facts_for_case(case)).await?;
```

Assert `total_imported == case.facts.len()` before querying. Retrieval-only
evaluation must not call extraction.

- [ ] **Step 6: Enforce both worker limits and preserve task failures**

Use one semaphore for active contexts and one per context for queries. Convert
`JoinError` to an invalid outcome for that exact `CaseKey`. Await every handle;
never drop a failed join.

- [ ] **Step 7: Register external suites from prepared corpus configuration**

Extend `SuiteDecl` with immutable local corpus inputs:

```json
{
  "id": "external-retrieval",
  "dataset": "longmemeval",
  "manifest": "evals/corpora/longmemeval.json",
  "prepared_root": "data/corpora",
  "sample": {"strategy": "sha256_stratified", "count": 100}
}
```

PR uses a stable reviewed sample. Release uses deterministic complete shards.
Nightly may add full end-to-end corpus diagnostics separately; it must not mix
them into retrieval-only metrics.

- [ ] **Step 8: Declare truthful profile coverage and gates**

Required profile contents:

- PR: local retrieval, extraction, claims, and 100-case external sample;
- release: complete external retrieval shards, extraction, claims, lifecycle,
  and performance evidence;
- nightly: local suites plus production-path end-to-end diagnostics.

Set exact case counts after corpus preparation. Keep `600` and `1200` second
budgets. Remove unconfigured downstream QA from execution until a pinned
`ReaderContract` and non-empty expected case set are supplied.

- [ ] **Step 9: Make CI preserve artifacts without hiding failures**

Remove `continue-on-error: true` from PR and release eval steps. Keep:

```yaml
- name: Upload eval artifact
  if: always()
  uses: actions/upload-artifact@v4
```

The release build job must depend on a passed `eval-release`. Nightly uploads
on every outcome; its job conclusion follows the stored artifact verdict.

- [ ] **Step 10: Add deterministic report generation**

Add:

```bash
cargo run -p eval-harness --bin memory-eval -- report \
  --artifact target/evals/release-corrected.json \
  --output docs/evals/BENCHMARK_RUN_REPORT_2026-07-30.md
```

The command loads and validates artifact v2 before rendering. It must refuse
an invalid schema, inconsistent verdict, or incomplete merged artifact.

- [ ] **Step 11: Run the PR profile and verify the 10-minute budget**

```bash
/usr/bin/time -p make eval-pr
cargo run -p eval-harness --bin memory-eval -- report \
  --artifact target/evals/pr.json \
  --output target/evals/pr-report.md
```

Acceptance:

- `verdict = "passed"`;
- exact expected/outcome parity;
- zero invalid outcomes;
- all required gates passed;
- wall clock at most 600 seconds.

- [ ] **Step 12: Run and merge the complete release profile**

Run deterministic shards, then:

```bash
cargo run -p eval-harness --bin memory-eval -- merge \
  --profile evals/profiles/release.json \
  --artifact target/evals/release-corrected.json \
  --shards target/evals/release-shard-*.json
```

Acceptance:

- every declared external case appears exactly once;
- merged reducers and gates are recomputed from outcomes;
- lifecycle and claim gates passed;
- no invalid outcomes or run issues;
- merged wall clock at most 1,200 seconds;
- `verdict = "passed"`.

- [ ] **Step 13: Review and replace the baseline**

Review case coverage, corpus digests, model/device fingerprints, metric
denominators, gate directions, and timing provenance. Only after approval:

```bash
cp target/evals/pr.json evals/baselines/pr.json
```

Validate that future PR runs require a compatible baseline and reject a
fingerprint or schema mismatch.

- [ ] **Step 14: Generate the corrected dated report**

Generate `BENCHMARK_RUN_REPORT_2026-07-30.md` from the corrected artifacts.
State separately:

- execution validity;
- quality verdict;
- local versus external retrieval;
- lifecycle evidence;
- CPU/Metal support and parity;
- PR and merged-release wall time;
- remaining non-gating diagnostics.

Do not edit the 2026-07-28 or 2026-07-29 reports.

- [ ] **Step 15: Run the full quality gate**

```bash
cargo check --workspace --locked
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps,eval-support --locked -- -D warnings
cargo fmt --all --check
cargo test --workspace --lib --bins --tests
```

Expected: zero errors, zero warnings, zero test failures, and zero formatting
drift.

- [ ] **Step 16: Commit profile enforcement and corrected evidence**

```bash
git add crates/eval-harness evals/profiles evals/baselines/pr.json .github/workflows/ci.yml Makefile docs/evals/BENCHMARK_RUN_REPORT_2026-07-30.md
git commit -m "ci(evals): enforce complete corrected evaluation"
```

## Completion Evidence

The v2 closure is complete only when a reviewer can verify all of the
following from committed artifacts and raw outputs:

- artifact schema is `memory-mcp-eval/v2`;
- the stored verdict matches recomputation;
- nightly quality failures cannot render `PASSED`;
- every selected suite has a non-empty exact expected case set;
- claim precision/recall use persisted relations and source lineage;
- claim confusion counts have non-zero, inspectable denominators;
- cross-boundary claim and lifecycle violations equal zero;
- the entity end-to-end case queries `team` scope and scores extraction output;
- all lifecycle cases use capture, projection, recall, and attempted action;
- lifecycle capacity reports persisted rows and serialized bytes;
- the public-surface check reads the live registry;
- CPU and Metal run the same real GLiNER fixture with candidate parity;
- contention reports normalized throughput and per-operation latency;
- PR includes the declared stable external sample and completes within 600 seconds;
- release covers the complete declared external population and completes within 1,200 seconds;
- PR and release CI failures are not hidden by `continue-on-error`;
- a compatible reviewed baseline exists;
- the corrected dated report is generated from validated artifacts rather than manually reconstructed.
