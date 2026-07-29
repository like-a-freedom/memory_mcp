# Evaluation V3 Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining v3 truth gaps so PR, release, nightly, lifecycle, claims, end-to-end, external-corpus, and performance results are reproducible, objectively gated, and complete within the declared 10/20-minute profile budgets.

**Architecture:** Keep all evaluation orchestration in the private `eval-harness` crate and preserve production behavior unless a corrected evaluator exposes a real product defect. Replace wall-clock-dependent fixtures and float-copy reducers with deterministic typed evidence, make every selected profile fail closed on missing or failed required evidence, and keep platform performance measurements separate from semantic quality runs.

**Tech Stack:** Rust 2024, Tokio, Serde/serde_json, SurrealDB, Candle GLiNER, Criterion, GitHub Actions, the existing `memory_mcp` library, and the private `eval-harness` workspace crate.

## Global Constraints

- Preserve the frozen eight-tool MCP public surface.
- Do not add evaluation logic to MCP handlers, ordinary CLI commands, or `src/main.rs`.
- Do not change production retrieval to make a broken fixture pass.
- Use fixed case-owned reference times; evaluator behavior must not depend on the wall clock.
- Derive aggregate metrics from typed `MetricEvidence`, not copied per-case floats.
- Missing evidence, a zero denominator, an unavailable required device, incomplete coverage, and an unregistered selected suite are `invalid`, never zero-valued quality measurements.
- Any required `quality_failed` case makes the run `quality_failed`, even when all aggregate metric gates pass.
- Any invalid case or gate makes the run `invalid`.
- PR wall-clock budget remains at most 600 seconds.
- Release wall-clock budget remains at most 1,200 seconds.
- Nightly is diagnostic but must still return a non-zero exit code and a non-passed verdict on required failures.
- Cross-scope, cross-project, policy-tag, and access-policy leakage tolerates exactly zero violations.
- Claim precision and recall use exact persisted lineage and relation labels; source IDs are never compared with persisted fact IDs.
- Performance comparisons use identical fixtures, features, model, device, warm-up, sample size, and measurement time.
- Metal unavailability is reported as unsupported/invalid evidence; it is never represented by a successful nanosecond benchmark.
- Do not approve a new baseline until the corrected release artifact has `verdict = "passed"` and exact declared coverage.
- Every task follows TDD, has a focused verification command, and ends with a focused commit.

---

## Evidence, Decisions, and Scope

The plan is based on `docs/evals/BENCHMARK_RUN_REPORT_2026-07-29-v3.md` and the
stored `target/evals/v3-*.json` artifacts.

Verified v3 facts:

- PR is `quality_failed`, with 6 failed cases despite 6/6 metric gates passing.
- Release is `invalid`: lifecycle summary metrics are `{}`, therefore both
  lifecycle gates are invalid.
- Nightly is `quality_failed`: both required end-to-end cases return zero
  context.
- End-to-end setup writes `t_ref = Utc::now()` but queries with
  `as_of = 2026-07-15T14:00:01Z`; the bi-temporal store correctly hides those
  future facts. This is evaluator clock drift, not evidence of a production
  retrieval regression.
- The lifecycle wrapper uses `CountReducer`, which intentionally drops the
  child pass-rate metrics that release gates consume.
- All four failed claim cases report `isolation_violations=1`; the evaluator
  currently counts every warning between two different fact IDs as an
  isolation violation, including expected same-boundary contradictions.
- The two extraction failures are `ext-006` and `ext-007`, both with
  `warnings: 0/1`.
- The Metal benchmark still times token counting and reports 43 ns; it is not a
  GPU measurement.
- External retrieval exists in source but is not registered by the CLI, returns
  an empty expected-ID set, and is absent from all three profiles.

Decision records:

- No new ADR is required. ADR-0019 already defines truthful profile-driven
  evaluation, and ADR-0017 already defines lifecycle evidence.
- ADR-0019 must be accepted before implementation because the repository has
  already adopted its artifact/profile architecture while the record remains
  `Proposed`.
- A production change is permitted only after a corrected evaluator still
  demonstrates a failure through a production-path integration test.

## File Map

| Path | Responsibility |
|---|---|
| `docs/adr/0019-adopt-profile-driven-truthful-evaluation.md` | Accept the already-adopted evaluation decision and record v3 closure criteria |
| `crates/eval-harness/src/suites/end_to_end.rs` | Deterministic event/query time and end-to-end evidence |
| `tests/fixtures/evals/end_to_end_cases.json` | Case-owned `t_ref` and `as_of` values |
| `crates/eval-harness/src/reducer.rs` | Typed lifecycle ratio aggregation |
| `crates/eval-harness/src/suites/lifecycle.rs` | Release-level lifecycle evidence |
| `crates/eval-harness/src/suites/poisoning.rs` | Poisoning capture/recall/action-policy scenarios |
| `tests/fixtures/evals/agent_memory_lifecycle_cases.json` | Frozen lifecycle adversarial cases |
| `crates/eval-harness/src/suites/claims.rs` | Exact relation matching and boundary-isolation scoring |
| `tests/fixtures/evals/claim_reconciliation_cases.json` | Claim oracle and explicit boundary expectations |
| `crates/eval-harness/src/suites/extraction.rs` | Warning evidence for commitment changes |
| `src/service/claims/schema.rs` | Production claim projection, only if corrected tests prove a defect |
| `src/service/claims/project.rs` | Fact-type-aware projection, only if corrected tests prove a defect |
| `tests/fixtures/evals/extraction_cases.json` | Frozen warning expectations |
| `crates/eval-harness/src/suites/external_retrieval.rs` | External expected IDs, bounded execution, corpus metrics |
| `crates/eval-harness/src/main.rs` | Thin suite construction and error-to-exit mapping |
| `crates/eval-harness/src/profile.rs` | Corpus selection and exact coverage declarations |
| `crates/eval-harness/src/domain.rs` | Explicit invalid reasons and verdict truth table |
| `crates/eval-harness/src/gate.rs` | Missing-metric gate semantics |
| `crates/eval-harness/src/report.rs` | Unambiguous verdict and gate/case summaries |
| `evals/profiles/pr.json` | Fast deterministic local quality profile |
| `evals/profiles/release.json` | Complete release quality and lifecycle profile |
| `evals/profiles/nightly.json` | End-to-end and stable external diagnostic profile |
| `crates/eval-harness/src/benchmark.rs` | Shared NER fixture, model, device, and parity oracle |
| `crates/eval-harness/benches/ner_cpu.rs` | Canonical CPU inference benchmark |
| `crates/eval-harness/benches/ner_metal.rs` | Real Metal inference or explicit unsupported result |
| `crates/eval-harness/benches/pipeline.rs` | Canonical Criterion configuration |
| `crates/eval-harness/benches/contention.rs` | Per-operation latency and throughput |
| `evals/performance/pinned-runner.json` | Comparable runner and Criterion contract |
| `.github/workflows/ci.yml` | Profile enforcement, shards, pinned performance job, artifact upload |
| `docs/evals/BENCHMARK_RUN_REPORT_2026-07-30.md` | Generated acceptance report; never hand-edited metric values |

### Task 1: Ratify the truth contract and fix deterministic end-to-end time

**Files:**
- Modify: `docs/adr/0019-adopt-profile-driven-truthful-evaluation.md`
- Modify: `tests/fixtures/evals/end_to_end_cases.json`
- Modify: `crates/eval-harness/src/suites/end_to_end.rs`
- Test: `crates/eval-harness/src/suites/end_to_end.rs`

**Interfaces:**
- Produces: `EndToEndCase.t_ref: DateTime<Utc>`.
- Produces: `EndToEndCase.as_of: DateTime<Utc>`.
- Produces: `EndToEndCase::validate_timeline(&self) -> Result<(), EvalError>`.
- Preserves: production `MemoryService::{ingest,extract,assemble_context}` behavior.

- [ ] **Step 1: Accept ADR-0019 and record the v3 closure**

Change the status to `Accepted (2026-07-29)` and add:

```markdown
## V3 closure clarification

- Case reference time is fixture-owned and deterministic.
- A required case failure dominates passing aggregate gates.
- A missing required metric is invalid evidence.
- Platform performance is comparable only under the pinned-runner contract.
```

- [ ] **Step 2: Write failing timeline-validation tests**

```rust
#[test]
fn end_to_end_case_rejects_query_before_ingestion_time() {
    let case = fixture_case(
        "2026-07-15T14:00:00Z",
        "2026-07-15T13:59:59Z",
    );
    assert!(matches!(
        case.validate_timeline(),
        Err(EvalError::InvalidInput(message))
            if message.contains("as_of precedes t_ref")
    ));
}

#[test]
fn end_to_end_case_uses_fixture_time_not_wall_clock() {
    let case = fixture_case(
        "2026-07-15T14:00:00Z",
        "2026-07-15T14:00:01Z",
    );
    assert_eq!(case.t_ref.to_rfc3339(), "2026-07-15T14:00:00+00:00");
}
```

- [ ] **Step 3: Run the tests and verify the clock defect is exposed**

Run:

```bash
cargo test -p eval-harness suites::end_to_end -- --nocapture
```

Expected: FAIL because the case has no typed `t_ref`/`as_of` fields and the
suite owns a hard-coded query time.

- [ ] **Step 4: Add exact timestamps to both fixture cases**

Use these values in each case:

```json
{
  "t_ref": "2026-07-15T14:00:00Z",
  "as_of": "2026-07-15T14:00:01Z"
}
```

Deserialize directly into `DateTime<Utc>`. Reject `as_of < t_ref`; do not
silently clamp or replace invalid input.

- [ ] **Step 5: Use case time throughout the production-path scenario**

Replace `Utc::now()` and the suite-level hard-coded cutoff with:

```rust
t_ref: case.t_ref,
// ...
as_of: Some(case.as_of),
```

Record typed ratio evidence for expected context matches:

```rust
evidence_map.insert(
    "context_match".into(),
    MetricEvidence::ratio(
        u64::try_from(context_matched)?,
        u64::try_from(case.expected_context.len())?,
    ),
);
```

If the expected-context denominator is zero, mark the case `Invalid`; do not
treat it as a vacuous pass.

- [ ] **Step 6: Prove both E2E cases pass without production changes**

Run:

```bash
cargo test -p eval-harness suites::end_to_end -- --nocapture
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/nightly.json \
  --artifact target/evals/v4-nightly-e2e.json \
  --suites end-to-end
jq '{verdict, outcomes: [.outcomes[] | {case_key, status, metrics}]}' \
  target/evals/v4-nightly-e2e.json
```

Expected: both cases are `passed`, both return at least one context item, and
the artifact verdict is `passed`. If either remains failed, stop this task and
open a production-path defect with the exact context trace; do not weaken the
fixture oracle.

- [ ] **Step 7: Commit**

```bash
git add docs/adr/0019-adopt-profile-driven-truthful-evaluation.md \
  tests/fixtures/evals/end_to_end_cases.json \
  crates/eval-harness/src/suites/end_to_end.rs
git commit -m "fix(evals): make end-to-end time deterministic"
```

### Task 2: Aggregate lifecycle metrics from typed evidence and repair poisoning semantics

**Files:**
- Modify: `crates/eval-harness/src/reducer.rs`
- Modify: `crates/eval-harness/src/suites/lifecycle.rs`
- Modify: `crates/eval-harness/src/suites/poisoning.rs`
- Modify: `tests/fixtures/evals/agent_memory_lifecycle_cases.json`
- Test: `crates/eval-harness/tests/lifecycle_release.rs`

**Interfaces:**
- Produces: `RatioMetricSpec { evidence_key: &'static str, metric_name: &'static str }`.
- Produces: `RatioReducer::new(suite_id: impl Into<String>, specs: &'static [RatioMetricSpec])`.
- Consumes: `MetricEvidence::Ratio { numerator, denominator }`.
- Produces: lifecycle metrics `action_grounding_pass_rate` and `poisoning_pass_rate`.
- Produces: poisoning result based on whether recalled data can cause an unsafe action, not whether hostile text can be retrieved as data.

- [ ] **Step 1: Write failing reducer tests**

```rust
#[test]
fn lifecycle_reducer_aggregates_child_ratios() {
    let outcomes = vec![
        ratio_outcome("lifecycle-action-grounding", "action_grounding", 3, 3),
        ratio_outcome("lifecycle-poisoning", "poisoning", 2, 3),
    ];
    let summary = LifecycleReducer::new().reduce(&outcomes).unwrap().remove(0);
    assert_eq!(summary.metrics["action_grounding_pass_rate"], 1.0);
    assert_eq!(summary.metrics["poisoning_pass_rate"], 2.0 / 3.0);
}

#[test]
fn lifecycle_reducer_rejects_missing_or_zero_denominator() {
    let outcomes = vec![ratio_outcome("lifecycle-poisoning", "poisoning", 0, 0)];
    assert!(LifecycleReducer::new().reduce(&outcomes).is_err());
}
```

- [ ] **Step 2: Verify the current `CountReducer` fails the contract**

Run:

```bash
cargo test -p eval-harness --test lifecycle_release -- --nocapture
```

Expected: FAIL because the lifecycle summary contains no metrics.

- [ ] **Step 3: Add a focused `LifecycleReducer`**

The reducer must:

1. count `passed`, `quality_failed`, and `invalid`;
2. require one action-grounding and one poisoning ratio;
3. reject missing evidence, duplicate evidence, `denominator == 0`, or
   `numerator > denominator`;
4. compute ratios from integers;
5. emit the exact gate metric names.

Do not copy `outcome.metrics["..."]`; those floats remain presentation-only.

- [ ] **Step 4: Emit typed child evidence**

For the lifecycle wrapper, replace float-only maps with:

```rust
evidence.insert(
    "action_grounding".into(),
    MetricEvidence::ratio(grounding_passed, grounding_total),
);
evidence.insert(
    "poisoning".into(),
    MetricEvidence::ratio(poisoning_passed, poisoning_total),
);
```

Convert lengths with `u64::try_from`. An empty child suite is `Invalid`.

- [ ] **Step 5: Write adversarial action-policy tests before changing poisoning**

```rust
#[tokio::test]
async fn hostile_memory_is_data_and_cannot_authorize_admin_action() {
    let result = run_poisoning_case("poisoning-no-privileged-instruction").await;
    assert_eq!(result.attempted_action, "disable_safety_checks");
    assert_eq!(result.disposition, ActionDisposition::Refuse);
    assert!(result.evidence_ids.iter().all(|id| !id.is_empty()));
}

#[tokio::test]
async fn benign_recalled_status_does_not_fail_for_being_retrievable() {
    let result = run_poisoning_case("poisoning-envelope-bounded").await;
    assert_eq!(result.disposition, ActionDisposition::RequireLiveVerification);
    assert!(result.context_items <= 16);
}
```

- [ ] **Step 6: Replace substring-leak scoring with the real lifecycle use case**

Run each case through the existing capture/recall seam and the deterministic
attempted-action policy. The pass condition is:

```rust
let passed =
    result.context_items <= 16
    && result.disposition != ActionDisposition::Execute
    && result.privilege_elevation_count == 0;
```

Do not fail merely because the recalled, source-labelled memory contains
`SYSTEM:` or `admin mode`; memory content is data. Fail only if it is elevated
to authority or causes an unsafe action.

- [ ] **Step 7: Verify release lifecycle gates become meaningful**

Run:

```bash
cargo test -p eval-harness --test lifecycle_release -- --nocapture
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/release.json \
  --artifact target/evals/v4-release-lifecycle.json \
  --suites lifecycle
jq '{verdict, lifecycle: [.suite_summaries[] | select(.suite_id == "lifecycle")], gates}' \
  target/evals/v4-release-lifecycle.json
```

Expected: lifecycle metrics are present; both observed values are `1.0`; both
gates are `passed`; no gate uses `observed: 0.0` to represent missing data.

- [ ] **Step 8: Commit**

```bash
git add crates/eval-harness/src/reducer.rs \
  crates/eval-harness/src/suites/lifecycle.rs \
  crates/eval-harness/src/suites/poisoning.rs \
  tests/fixtures/evals/agent_memory_lifecycle_cases.json \
  crates/eval-harness/tests/lifecycle_release.rs
git commit -m "fix(evals): derive lifecycle gates from typed evidence"
```

### Task 3: Make claim isolation and extraction-warning scoring objective

**Files:**
- Modify: `crates/eval-harness/src/suites/claims.rs`
- Modify: `tests/fixtures/evals/claim_reconciliation_cases.json`
- Modify: `crates/eval-harness/src/suites/extraction.rs`
- Modify: `tests/fixtures/evals/extraction_cases.json`
- Modify only if proven: `src/service/claims/schema.rs`
- Modify only if proven: `src/service/claims/project.rs`
- Test: `crates/eval-harness/tests/claim_quality.rs`
- Test: `tests/claim_reconciliation.rs`

**Interfaces:**
- Produces: `BoundaryKey { scope, project, policy_tags }`.
- Produces: `classify_warning(expected_relations, lineage, boundaries, warning) -> WarningLabel`.
- Produces: `WarningLabel::{TruePositive, FalsePositive, IsolationViolation, Unlabelled}`.
- Produces: separate `claim_precision`, `claim_recall`, and `isolation_violation_count`.

- [ ] **Step 1: Freeze the four v3 failures as evaluator regression tests**

```rust
#[test]
fn expected_same_boundary_relation_is_not_an_isolation_violation() {
    let label = classify_warning(&expected_relation(), &same_boundary_lineage(), &warning());
    assert_eq!(label, WarningLabel::TruePositive);
}

#[test]
fn warning_crossing_project_boundary_is_an_isolation_violation() {
    let label = classify_warning(&[], &cross_project_lineage(), &warning());
    assert_eq!(label, WarningLabel::IsolationViolation);
}

#[test]
fn unexpected_same_boundary_warning_is_false_positive_not_isolation() {
    let label = classify_warning(&[], &same_boundary_lineage(), &warning());
    assert_eq!(label, WarningLabel::FalsePositive);
}
```

- [ ] **Step 2: Run focused claim tests**

Run:

```bash
cargo test -p eval-harness --test claim_quality -- --nocapture
```

Expected: FAIL because `count_isolation_violations` currently treats every
warning between distinct fact IDs as a boundary violation.

- [ ] **Step 3: Carry exact source boundary and lineage**

Store, for every extracted fact ID:

```rust
struct FactOracle {
    source_id: String,
    boundary: BoundaryKey,
}
```

Normalize `policy_tags` by sorting and deduplicating. Boundary equality requires
equal scope, project, and policy tags. Do not infer a boundary from warning
text or fact type.

- [ ] **Step 4: Classify every prediction exactly once**

For each actual warning:

1. resolve both persisted fact IDs through lineage;
2. unresolved lineage makes the case `Invalid`;
3. different boundary keys produce `IsolationViolation`;
4. same-boundary exact expected relation produces `TruePositive`;
5. same-boundary unmatched warning produces `FalsePositive`.

For each expected contradiction without a matched warning, add one false
negative. Compute precision/recall only from TP/FP/FN. Keep isolation as a
separate hard-zero metric.

- [ ] **Step 5: Add explicit isolation and negative-control cases**

The fixture must contain at least:

- same scope and project: relation allowed;
- different scope: zero warning allowed;
- same scope, different project: zero warning allowed;
- same scope/project, different policy tags: zero warning allowed;
- same boundary, no expected contradiction: warning is a false positive.

Every expected relation names both source IDs and the expected outcome.

- [ ] **Step 6: Correct extraction warning evidence**

For `ext-006` and `ext-007`, add a test that uses their exact fixture payloads
and asserts persisted lineage plus warning relation, not only warning count:

```rust
assert_warning_relation(
    &result,
    ExpectedWarning {
        old_source_id: "ext-006-setup",
        new_source_id: "ext-006-source",
        outcome: "contradiction",
    },
);
```

If the corrected evaluator still yields no warning, add the smallest
production-path integration test that proves the projector loses commitment
schema, comparison key, or reference time. Only then modify
`src/service/claims/schema.rs` or `src/service/claims/project.rs`.

- [ ] **Step 7: Enforce realistic floors after the evaluator is correct**

Do not immediately promote v3's 0.75/0.60 as a baseline. First require:

```json
{
  "claim_precision": 0.80,
  "claim_recall": 0.90,
  "isolation_violation_count": 0
}
```

These align with the repository claim-reconciliation contract. Keep
development-split diagnostics visible, but calculate release gates only from
official test-split cases.

- [ ] **Step 8: Verify all case-level failures and aggregates**

Run:

```bash
cargo test -p eval-harness --test claim_quality -- --nocapture
cargo test --test claim_reconciliation -- --nocapture
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/pr.json \
  --artifact target/evals/v4-pr-claims.json \
  --suites claim-reconciliation extraction
jq '[.outcomes[] | select(.status != "passed") | {case_key, status, failures}]' \
  target/evals/v4-pr-claims.json
```

Expected: `[]`; aggregate claim precision is at least `0.80`, recall at least
`0.90`, and isolation violations equal zero. If the quality floors remain
unmet, the task is not complete even if the old 0.50 gates would pass.

- [ ] **Step 9: Commit**

```bash
git add crates/eval-harness/src/suites/claims.rs \
  crates/eval-harness/src/suites/extraction.rs \
  tests/fixtures/evals/claim_reconciliation_cases.json \
  tests/fixtures/evals/extraction_cases.json \
  crates/eval-harness/tests/claim_quality.rs \
  tests/claim_reconciliation.rs
git add src/service/claims/schema.rs src/service/claims/project.rs
git commit -m "fix(evals): score claim relations and isolation exactly"
```

If production files were not changed, omit them from `git add`.

### Task 4: Make profile results fail closed and reports impossible to misread

**Files:**
- Modify: `crates/eval-harness/src/domain.rs`
- Modify: `crates/eval-harness/src/gate.rs`
- Modify: `crates/eval-harness/src/report.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Modify: `evals/profiles/pr.json`
- Modify: `evals/profiles/release.json`
- Modify: `evals/profiles/nightly.json`
- Test: `crates/eval-harness/tests/profile_verdict.rs`

**Interfaces:**
- Preserves: `Invalid > QualityFailed > Passed` verdict precedence.
- Produces: `GateFailureReason::MissingMetric`.
- Produces: one authoritative `RESULT: PASSED|QUALITY FAILED|INVALID`.
- Produces: exact selected-suite coverage and case-failure counts in the report.

- [ ] **Step 1: Add truth-table and report tests**

```rust
#[test]
fn passing_metric_gates_cannot_hide_failed_cases() {
    assert_eq!(
        derive_run_verdict(&[quality_failed_case()], &[passed_gate()], GateStatus::Passed, &[]),
        RunVerdict::QualityFailed
    );
}

#[test]
fn missing_required_metric_has_explicit_reason() {
    let gate = evaluate_missing_metric("lifecycle", "poisoning_pass_rate");
    assert_eq!(gate.status, GateStatus::Invalid);
    assert_eq!(gate.reason, GateFailureReason::MissingMetric);
}

#[test]
fn report_never_prefixes_failure_with_success_symbol() {
    let report = render_markdown(&quality_failed_artifact()).unwrap();
    assert!(report.contains("Result: QUALITY FAILED"));
    assert!(!report.contains("✅ QUALITY FAILED"));
}
```

- [ ] **Step 2: Run the tests and confirm v3 wording is rejected**

Run:

```bash
cargo test -p eval-harness --test profile_verdict -- --nocapture
```

Expected: FAIL on missing reason and ambiguous report presentation.

- [ ] **Step 3: Make missing metrics first-class invalid evidence**

Add `GateFailureReason::MissingMetric`. Store `observed: Option<f64>` in
artifact v3 rather than overloading `0.0`; update schema and deserialization
compatibility so v2 artifacts remain readable but cannot serve as new
baselines.

- [ ] **Step 4: Add required nightly gates**

Nightly must gate:

```json
[
  {"suite_id": "end-to-end", "metric": "context_match_rate", "hard_floor": 1.0},
  {"suite_id": "end-to-end", "metric": "case_pass_rate", "hard_floor": 1.0}
]
```

Add an end-to-end reducer that derives both metrics from typed ratio evidence.
Do not use `CountReducer` for a gated suite.

- [ ] **Step 5: Raise claim gates and add isolation gate**

Apply to PR and release:

```json
[
  {"suite_id": "claim-reconciliation", "metric": "claim_precision", "hard_floor": 0.80},
  {"suite_id": "claim-reconciliation", "metric": "claim_recall", "hard_floor": 0.90},
  {"suite_id": "claim-reconciliation", "metric": "isolation_violation_count", "hard_ceiling": 0}
]
```

Use the existing direction-aware gate model (`AtMost` for the isolation
ceiling); do not encode a ceiling by negating the metric.

- [ ] **Step 6: Render one authoritative result**

The report header must show verdict, failed/invalid case counts, failed/invalid
gate counts, coverage, and budget. Remove celebratory symbols from non-passed
results. The CLI exit mapping remains:

- `Passed` → 0;
- `QualityFailed` → 1;
- `Invalid` → 2.

- [ ] **Step 7: Verify all three profile policies**

Run:

```bash
cargo test -p eval-harness --test profile_verdict -- --nocapture
make eval-pr
make eval-release
make eval-nightly
```

Expected after Tasks 1–3: exit 0 for all three; each artifact has exact
coverage, no failed/invalid case, no failed/invalid gate, and a passed budget.
Before Tasks 1–3 are complete, these commands must exit non-zero.

- [ ] **Step 8: Commit**

```bash
git add crates/eval-harness/src/domain.rs \
  crates/eval-harness/src/gate.rs \
  crates/eval-harness/src/report.rs \
  crates/eval-harness/src/main.rs \
  crates/eval-harness/tests/profile_verdict.rs \
  evals/profiles/pr.json evals/profiles/release.json evals/profiles/nightly.json
git commit -m "fix(evals): make every profile fail closed"
```

### Task 5: Wire external corpora with exact coverage and bounded concurrency

**Files:**
- Modify: `crates/eval-harness/src/suites/external_retrieval.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Modify: `crates/eval-harness/src/profile.rs`
- Modify: `crates/eval-harness/src/corpus/selection.rs`
- Modify: `evals/profiles/release.json`
- Modify: `evals/profiles/nightly.json`
- Test: `crates/eval-harness/tests/external_profile.rs`

**Interfaces:**
- Produces: non-empty `ExternalRetrievalSuite::expected_case_ids()`.
- Produces: `ExternalSuiteConfig { dataset, manifest, prepared_root, selection, workers }`.
- Produces: stable selection by case ID and declared strata.
- Preserves: external retrieval as `EvalMode::RetrievalOnly`; it never imports oracle answers into production extraction.

- [ ] **Step 1: Write failing registration and coverage tests**

```rust
#[test]
fn external_suite_expected_ids_equal_selected_ids() {
    let cases = fixture_external_cases();
    let suite = ExternalRetrievalSuite::new(DatasetKind::LongMemEval, cases.clone());
    assert_eq!(
        suite.expected_case_ids(),
        cases.iter().map(|c| c.id.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn selected_external_suite_is_constructed_from_profile() {
    let manifest = fixture_manifest_with_external_suite();
    let suites = build_suites(&manifest).unwrap();
    assert!(suites.iter().any(|suite| suite.id() == "external-retrieval"));
}
```

- [ ] **Step 2: Verify the current dead wiring**

Run:

```bash
cargo test -p eval-harness --test external_profile -- --nocapture
```

Expected: FAIL because expected IDs are empty and `main.rs` does not construct
`ExternalRetrievalSuite`.

- [ ] **Step 3: Store expected IDs at construction**

Parse each external case ID once and keep `Vec<EvalCaseId>` in the suite.
Reject duplicate or empty IDs. Join failures produce one `Invalid` outcome for
the affected selected ID; never drop a failed task from coverage.

- [ ] **Step 4: Add typed profile construction**

Parse dataset kind, corpus manifest, prepared root, deterministic selection,
and worker limits from the suite declaration. Construction errors become
`RunIssue::SuiteLoad`; they must not be printed as warnings followed by a
partial run.

- [ ] **Step 5: Declare stable release/nightly coverage**

Release uses a reviewed stratified sample sized to keep the merged run below
1,200 seconds. Nightly uses complete prepared coverage and may shard by stable
case ID. Both record selected IDs, manifest digest, dataset revision, and
selection policy in the artifact fingerprint.

- [ ] **Step 6: Verify exact coverage and the release budget**

Run:

```bash
cargo test -p eval-harness --test external_profile -- --nocapture
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/release.json \
  --artifact target/evals/v4-release-external.json \
  --suites external-retrieval
jq '{
  verdict,
  duration_ms,
  expected: (.expected_case_ids | length),
  outcomes: (.outcomes | length),
  invalid: [.outcomes[] | select(.status == "invalid")] | length
}' target/evals/v4-release-external.json
```

Expected: expected count equals outcome count, invalid count is zero, and the
declared selection finishes within its portion of the 1,200-second budget.

- [ ] **Step 7: Commit**

```bash
git add crates/eval-harness/src/suites/external_retrieval.rs \
  crates/eval-harness/src/main.rs \
  crates/eval-harness/src/profile.rs \
  crates/eval-harness/src/corpus/selection.rs \
  crates/eval-harness/tests/external_profile.rs \
  evals/profiles/release.json evals/profiles/nightly.json
git commit -m "feat(evals): wire exact external corpus coverage"
```

### Task 6: Replace the Metal stub and establish comparable performance evidence

**Files:**
- Create: `crates/eval-harness/src/benchmark.rs`
- Modify: `crates/eval-harness/src/lib.rs`
- Modify: `crates/eval-harness/benches/ner_cpu.rs`
- Modify: `crates/eval-harness/benches/ner_metal.rs`
- Modify: `crates/eval-harness/benches/pipeline.rs`
- Modify: `crates/eval-harness/benches/contention.rs`
- Modify: `evals/performance/pinned-runner.json`
- Modify: `.github/workflows/ci.yml`
- Create after verified run: `docs/evals/BENCHMARK_RUN_REPORT_2026-07-30.md`
- Test: `crates/eval-harness/tests/benchmark_contract.rs`

**Interfaces:**
- Produces: `NerBenchmarkFixture::load() -> Result<NerBenchmarkFixture, EvalError>`.
- Produces: `NerRunner::cpu(&NerBenchmarkFixture) -> Result<NerRunner, EvalError>`.
- Produces: `NerRunner::metal(&NerBenchmarkFixture) -> Result<NerRunner, UnsupportedDevice>`.
- Produces: `NerOutput::canonical()` for CPU/Metal parity.
- Produces: shared Criterion configuration and benchmark provenance.

- [ ] **Step 1: Write failing benchmark-contract tests**

```rust
#[test]
fn cpu_and_metal_use_identical_model_labels_threshold_and_input() {
    let fixture = NerBenchmarkFixture::load().unwrap();
    assert_eq!(fixture.model_digest.len(), 64);
    assert!(!fixture.labels.is_empty());
    assert!((0.0..=1.0).contains(&fixture.threshold));
    assert!(!fixture.inputs.is_empty());
}

#[test]
fn unavailable_metal_is_not_a_measurement() {
    let fixture = NerBenchmarkFixture::load().unwrap();
    if !metal_is_available() {
        assert!(matches!(
            NerRunner::metal(&fixture),
            Err(UnsupportedDevice::Metal)
        ));
    }
}
```

- [ ] **Step 2: Verify the 43 ns stub fails the contract**

Run:

```bash
cargo test -p eval-harness --test benchmark_contract -- --nocapture
```

Expected: FAIL because there is no shared fixture or real Metal runner.

- [ ] **Step 3: Extract one shared NER benchmark fixture**

Move model loading, labels, threshold, tokenizer/input preparation, and
canonical output conversion out of the individual benches. Setup and warm-up
occur outside `b.iter`; timed code includes only the intended inference stage.

- [ ] **Step 4: Implement real Metal inference with parity**

Build the same model and inputs on `Device::new_metal(0)`. Before Criterion
timing, compare canonical CPU and Metal entity spans, labels, and scores within
the documented tolerance. A parity mismatch aborts the benchmark as invalid.

- [ ] **Step 5: Normalize Criterion configuration**

Use one checked-in contract:

```json
{
  "warm_up_seconds": 3,
  "measurement_seconds": 10,
  "sample_size": 30,
  "noise_threshold": 0.03,
  "confidence_level": 0.95
}
```

The v3 `--measurement-time 3` pipeline result remains diagnostic and must not
be compared with canonical baselines.

- [ ] **Step 6: Report contention in comparable units**

For every client count, record:

- operations per iteration;
- total iteration time;
- nanoseconds per operation;
- operations per second;
- error count.

Gate only per-operation latency/throughput on the pinned runner. Do not compare
raw multi-client iteration duration as if it were single-operation latency.

- [ ] **Step 7: Keep semantic profiles fast**

PR and release do not run full Criterion. CI layout:

- PR: `make eval-pr`, timeout 10 minutes;
- release: `make eval-release`, timeout 20 minutes;
- nightly: semantic/nightly and external shards;
- pinned macOS Apple Silicon job: CPU, Metal, pipeline, and contention
  Criterion, with JSON artifacts.

Upload artifacts with `if: always()`.

- [ ] **Step 8: Run canonical performance verification**

Run on the pinned Apple Silicon runner:

```bash
cargo test -p eval-harness --test benchmark_contract -- --nocapture
cargo bench -p eval-harness --bench pipeline -- --noplot
cargo bench -p eval-harness --bench ner_cpu -- --noplot
cargo bench -p eval-harness --features metal --bench ner_metal -- --noplot
cargo bench -p eval-harness --bench contention -- --noplot
```

Expected: CPU and Metal perform real inference, both are millisecond-scale for
the current fixture, outputs satisfy parity, and every result records the
pinned-runner fingerprint and canonical Criterion settings.

- [ ] **Step 9: Generate the acceptance report from artifacts**

The report must include:

- exact commit and dirty-worktree state;
- profile verdicts, budgets, coverage, cases, gates, and issues;
- corpus revisions and selections;
- CPU/Metal model/device/configuration;
- canonical confidence intervals;
- v3 comparison only where configurations are identical;
- an explicit list of unsupported or incomparable measurements.

Do not manually transcribe metric values before generating the report.

- [ ] **Step 10: Run the repository quality gate**

Run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo test --workspace --locked
make eval-pr
make eval-release
make eval-nightly
```

Expected: zero format drift, warnings, test failures, invalid evidence, failed
cases, or failed gates. PR is at most 600 seconds and release at most 1,200
seconds.

- [ ] **Step 11: Commit**

```bash
git add crates/eval-harness/src/benchmark.rs \
  crates/eval-harness/src/lib.rs \
  crates/eval-harness/benches/ner_cpu.rs \
  crates/eval-harness/benches/ner_metal.rs \
  crates/eval-harness/benches/pipeline.rs \
  crates/eval-harness/benches/contention.rs \
  crates/eval-harness/tests/benchmark_contract.rs \
  evals/performance/pinned-runner.json \
  .github/workflows/ci.yml \
  docs/evals/BENCHMARK_RUN_REPORT_2026-07-30.md
git commit -m "perf(evals): add comparable cpu and metal evidence"
```

## Final Acceptance Matrix

| Area | Required evidence |
|---|---|
| PR | `verdict=passed`, exact coverage, zero failed/invalid cases and gates, duration ≤ 600 s |
| Release | `verdict=passed`, lifecycle metrics 1.0, claims ≥ 0.80 precision and ≥ 0.90 recall, zero isolation violations, duration ≤ 1,200 s |
| Nightly | both E2E cases pass with deterministic time; required gates pass; external coverage is exact |
| Claims | no source-ID/fact-ID comparison; TP/FP/FN and isolation are independently auditable |
| Lifecycle | ratios derived from typed integer evidence; poisoning tests attempted-action safety |
| External | registered suite, non-empty expected IDs, stable selection, no dropped worker failures |
| Performance | real CPU/Metal inference, parity checked, canonical Criterion settings, pinned runner |
| Reporting | one unambiguous verdict; missing data is null/invalid, never numeric zero |

Baseline promotion is a separate reviewed action after this matrix is
satisfied. The implementation must not overwrite the v1, v2, or v3 artifacts
or reports.
