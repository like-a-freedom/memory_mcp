# Evaluation Suite Realism Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace misleading local, claim, end-to-end, and lifecycle proxies with evaluators that observe the intended production behavior and fail for the correct reasons.

**Architecture:** Each suite produces typed case evidence for the Truth Layer reducers. Retrieval matches expected evidence explicitly, extraction aggregates entity/fact/warning confusion counts, and claim reconciliation resolves fixture source IDs to persisted fact/claim/relation IDs. Lifecycle suites execute `LifecycleCapture` and `LifecycleRecall`, inspect real storage growth, and evaluate a deterministic consequential action.

**Tech Stack:** Rust 2024, Tokio, existing `MemoryService`, `LifecycleCapture`, `LifecycleRecall`, `AgentMemoryStore`, `DbClient`, Serde fixtures.

## Global Constraints

- Start only after Evaluation Truth Layer Remediation is complete.
- Do not change frozen labels or thresholds in the same change as evaluator logic.
- A failed expected behavior is `quality_failed`; inability to measure it is `invalid`.
- Exact IDs are resolved through recorded provenance; substring identity matching is prohibited.
- End-to-end mode uses production ingest, extract, claim projection, and `assemble_context`.
- Lifecycle release evidence uses wired lifecycle entry points, not ordinary ingest/extract proxies.
- Action grounding requires an observed action outcome, not a recall hit.
- Capacity uses persisted rows and serialized bytes.
- Poisoning follows capture, projection, recall, and attempted action.
- Public-surface evidence queries the live registry.

---

## File Map

| Path | Responsibility |
|---|---|
| `crates/eval-harness/src/suites/retrieval.rs` | Local retrieval evidence and failures |
| `crates/eval-harness/src/suites/extraction.rs` | Entity/fact/warning confusion evidence |
| `crates/eval-harness/src/suites/claims.rs` | Persisted claim/relation evaluation |
| `crates/eval-harness/src/suites/end_to_end.rs` | Stable production-path nightly cases |
| `crates/eval-harness/src/suites/action_grounding.rs` | Wired recall and deterministic action |
| `crates/eval-harness/src/suites/capacity.rs` | Wired capture and persistence deltas |
| `crates/eval-harness/src/suites/poisoning.rs` | Capture-to-action adversarial replay |
| `crates/eval-harness/src/suites/lifecycle.rs` | ADR-0017 aggregate release gate |
| `crates/eval-harness/src/test_support.rs` | Narrow service/store fixtures |
| `tests/fixtures/evals/*.json` | Frozen case inputs and expected evidence |
| `docs/evals/CLAIM_RECONCILIATION.md` | Exact claim evaluator semantics |
| `docs/evals/AGENT_MEMORY_LIFECYCLE.md` | Wired lifecycle evidence |

### Task 1: Correct local retrieval evidence and investigate three real failures

**Files:**
- Modify: `crates/eval-harness/src/suites/retrieval.rs`
- Modify: `crates/eval-harness/src/metrics.rs`
- Test: `crates/eval-harness/tests/retrieval_truth.rs`
- Read-only fixture during diagnosis: `tests/fixtures/evals/retrieval_cases.json`

**Interfaces:**
- Produces: `RetrievalEvidence { expected_items, matched_items_at_k, first_relevant_rank, unexpected_items }`.
- Consumes: normalized expected matchers and returned context items.

- [ ] **Step 1: Write failing matcher tests**

```rust
#[test]
fn expected_snippet_matches_content_without_becoming_a_ranked_id() {
    let expected = ExpectedEvidence::snippet("Orbital Labs");
    let item = observed_item("fact:1", "Alice joined Orbital Labs in 2025.");
    assert!(expected.matches(&item));
    assert_eq!(item.identity(), "fact:1");
}
```

- [ ] **Step 2: Separate identity, matching, and ranking**

Use fact/source IDs when fixtures provide them. Use a versioned normalized
snippet matcher only for reviewed text labels. Record one boolean hit per
expected item within the first five returned items; calculate rank from the
first matched returned item.

- [ ] **Step 3: Add case-level diagnostic snapshots**

For `first-person-rescue-profile`, `graph-anchor-alice-neighbor`, and `ret-063`,
record expected evidence, returned fact/source IDs, tiers, scores, and exclusion
reason. Do not change gold labels during diagnosis.

- [ ] **Step 4: Classify each failure**

Add a checked diagnosis table to the plan execution notes:

- evaluator mismatch → fix matcher and retain production behavior;
- stale/incorrect reviewed label → change fixture in a separate reviewed commit;
- production retrieval defect → add a failing integration test in
  `tests/context/` and fix production only in a separately scoped task.

- [ ] **Step 5: Run and commit evaluator corrections**

Run:

```bash
cargo test -p eval-harness retrieval --test retrieval_truth
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --suite local-retrieval --artifact target/evals/retrieval-corrected.json
```

Commit:

```bash
git add crates/eval-harness/src/suites/retrieval.rs crates/eval-harness/src/metrics.rs crates/eval-harness/tests/retrieval_truth.rs
git commit -m "fix(evals): measure retrieval evidence explicitly"
```

### Task 2: Correct extraction confusion counts and warning outcomes

**Files:**
- Modify: `crates/eval-harness/src/suites/extraction.rs`
- Test: `crates/eval-harness/tests/extraction_truth.rs`
- Read-only fixture during diagnosis: `tests/fixtures/evals/extraction_cases.json`

**Interfaces:**
- Produces: entity, fact-type, and warning `ClassificationCounts`.
- Produces: one case status based on all declared expectations.

- [ ] **Step 1: Write a failing aggregate-evidence test**

```rust
#[tokio::test]
async fn no_expected_entities_does_not_create_entity_f1_zero() {
    let case = extraction_case_without_entity_labels();
    let outcome = evaluate_case(&fake_result_with_no_entities(), &case).unwrap();
    assert!(!outcome.evidence.contains_key("entity"));
}
```

- [ ] **Step 2: Normalize entity identity**

Compare `(entity_type, normalized canonical name)` and documented aliases.
Count true positives, false positives, and false negatives explicitly. Cases
without entity labels do not contribute to entity denominators.

- [ ] **Step 3: Evaluate every declared dimension**

A case passes only when entity, fact-type, and warning requirements all pass.
Exact warning comparison uses fact type plus mapped source/fact identity and
normalized contents. Missing expected warnings in `ext-006` and `ext-007`
remain quality failures unless separate evidence proves their labels wrong.

- [ ] **Step 4: Add diagnostic output for the two warning failures**

Record extracted facts, actual warning identities/content, expected warnings,
and claim projection state. This distinguishes missing production warnings from
evaluator mismatch without changing the result.

- [ ] **Step 5: Run and commit**

Run:

```bash
cargo test -p eval-harness extraction --test extraction_truth
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --suite extraction --artifact target/evals/extraction-corrected.json
```

Commit:

```bash
git add crates/eval-harness/src/suites/extraction.rs crates/eval-harness/tests/extraction_truth.rs
git commit -m "fix(evals): aggregate extraction confusion evidence"
```

### Task 3: Evaluate persisted claims and relations through provenance

**Files:**
- Modify: `crates/eval-harness/src/suites/claims.rs`
- Modify: `crates/eval-harness/src/test_support.rs`
- Test: `crates/eval-harness/tests/claim_truth.rs`
- Modify: `docs/evals/CLAIM_RECONCILIATION.md`

**Interfaces:**
- Produces: `SourceLineageMap { source_id -> episode_id -> fact_ids -> claim_ids }`.
- Produces: `ObservedRelation { predecessor, successor, outcome, reason_code, slot }`.
- Consumes: persisted `fact_claim` and `claim_relation` records through `DbClient`.

- [ ] **Step 1: Write failing identity-mapping tests**

```rust
#[tokio::test]
async fn fixture_source_ids_resolve_to_generated_fact_ids() {
    let observed = execute_claim_case(&fixture_case("cr-001")).await.unwrap();
    assert!(observed.lineage.fact_ids("source-old").len() >= 1);
    assert!(observed.lineage.fact_ids("source-new").len() >= 1);
    assert_ne!(
        observed.lineage.fact_ids("source-new")[0].as_str(),
        "source-new"
    );
}
```

- [ ] **Step 2: Drive projection to a deterministic terminal state**

After ingest/extract, wait for or explicitly invoke the production claim
projection/reconciliation worker with a bounded timeout. A timeout or dead
letter makes the case invalid.

- [ ] **Step 3: Load persisted claims and relations**

Use `DbClient::select_table` for the claim and relation tables in the case
namespace, map generated records back through episode/fact provenance, and
compare typed outcome, reason code, predecessor, successor, claim slot,
validity, scope, project, and policy fingerprint.

- [ ] **Step 4: Replace the invalid isolation heuristic**

Delete the rule that treats `new_fact_id != conflicting_fact_id` as an
isolation violation. A violation exists only when an actual persisted relation
crosses a fixture-declared boundary. Expected skip codes are true-negative
expectations.

- [ ] **Step 5: Make expected relations affect case status**

Missed expected contradiction, false-positive relation, wrong outcome/reason,
and isolation leak all produce `quality_failed`. Cases with zero expected
positives contribute true negatives but do not manufacture precision/recall
values of 1.0.

- [ ] **Step 6: Gate the frozen test split only**

Produce separate development/test confusion counts. Assert profile gates target
the test slice and official/reviewed labels only.

- [ ] **Step 7: Run and commit**

Run:

```bash
cargo test -p eval-harness claims --test claim_truth
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --suite claim-reconciliation --artifact target/evals/claims-corrected.json
```

Commit:

```bash
git add crates/eval-harness/src/suites/claims.rs crates/eval-harness/src/test_support.rs crates/eval-harness/tests/claim_truth.rs docs/evals/CLAIM_RECONCILIATION.md
git commit -m "fix(evals): verify persisted claim relations"
```

### Task 4: Replace the nightly smoke stub with stable end-to-end cases

**Files:**
- Modify: `crates/eval-harness/src/suites/end_to_end.rs`
- Create: `tests/fixtures/evals/end_to_end_cases.json`
- Modify: `evals/profiles/nightly.json`
- Test: `crates/eval-harness/tests/end_to_end_truth.rs`

**Interfaces:**
- Produces: `EndToEndSuite::new(cases: Vec<EndToEndCase>) -> Result<Self, EvalError>`.
- Produces: stable expected IDs before execution.

- [ ] **Step 1: Write the regression test for the reported crash**

```rust
#[test]
fn expected_ids_equal_all_possible_outcome_ids() {
    let suite = EndToEndSuite::from_fixture().unwrap();
    assert_eq!(
        suite.expected_case_ids(),
        &[EvalCaseId::parse("e2e-pipeline-completes").unwrap()]
    );
}
```

- [ ] **Step 2: Remove outcome-dependent IDs**

One selected case keeps the same `CaseKey` whether it passes, quality-fails, or
becomes invalid. Store failure stage in `invalid_reason`, never in case ID.

- [ ] **Step 3: Replace inline smoke data with frozen cases**

Each fixture case declares sources, timestamps, expected extracted evidence,
claim relations, query, expected retrieval evidence, and trust level. The suite
uses only production ingest, extract, claim projection, and context assembly.

- [ ] **Step 4: Remove unconfigured downstream QA from nightly**

Until a real `ReaderContract` is supplied, do not select `downstream-qa` in the
nightly profile. A deliberately configured diagnostic may be invalid without
failing unrelated end-to-end coverage, but an implicitly missing contract is a
profile configuration error.

- [ ] **Step 5: Run and commit**

Run:

```bash
cargo test -p eval-harness end_to_end --test end_to_end_truth
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/nightly.json --suite end-to-end --artifact target/evals/e2e-corrected.json
```

Commit:

```bash
git add crates/eval-harness/src/suites/end_to_end.rs crates/eval-harness/tests/end_to_end_truth.rs tests/fixtures/evals/end_to_end_cases.json evals/profiles/nightly.json
git commit -m "fix(evals): stabilize nightly end-to-end cases"
```

### Task 5: Fulfill ADR-0017 with wired lifecycle evidence

**Files:**
- Modify: `crates/eval-harness/src/suites/action_grounding.rs`
- Modify: `crates/eval-harness/src/suites/capacity.rs`
- Modify: `crates/eval-harness/src/suites/poisoning.rs`
- Modify: `crates/eval-harness/src/suites/lifecycle.rs`
- Modify: `crates/eval-harness/src/test_support.rs`
- Test: `crates/eval-harness/tests/lifecycle_truth.rs`
- Modify: `docs/evals/AGENT_MEMORY_LIFECYCLE.md`

**Interfaces:**
- Consumes: `LifecycleRecall::execute`, `LifecycleCapture::execute`, `AgentMemoryStore`, live `MemoryMcp` tool registry.
- Produces: `ActionOutcome`, `PersistenceSnapshot`, `PoisoningOutcome`.

- [ ] **Step 1: Write failing proxy-detection tests**

```rust
#[tokio::test]
async fn action_grounding_calls_lifecycle_recall() {
    let pipeline = RecordingRecallPipeline::new();
    run_grounding_case(&pipeline, AgentMode::SelectiveEnforced)
        .await
        .unwrap();
    assert_eq!(pipeline.lifecycle_execute_calls(), 1);
}

#[tokio::test]
async fn ignored_capture_has_zero_persisted_growth() {
    let result = run_capture_case(ignored_event()).await.unwrap();
    assert_eq!(result.after - result.before, PersistenceDelta::zero());
}
```

- [ ] **Step 2: Implement real action grounding**

Run `always_recall`, `selective_shadow`, and `selective_enforced` through
`LifecycleRecall::execute`. Feed the returned bounded envelope to a
deterministic task adapter and mark success only when the resulting
consequential action is correct and cites required evidence.

- [ ] **Step 3: Implement persisted capacity evidence**

Run accepted, ignored, duplicate, quarantined, rejected, and budget-exhausted
events through `LifecycleCapture::execute`. Snapshot event, job, audit, episode,
fact rows and deterministic serialized bytes before/after.

- [ ] **Step 4: Implement capture-to-action poisoning**

For each adversarial case, capture through lifecycle, drive projection, recall
through lifecycle, and evaluate attempted action. Gate zero privileged
promotion, zero unsafe actions, zero boundary leak, fixed preamble, and bounded
envelope.

- [ ] **Step 5: Query the live public registry**

Replace the static eight-string array with `MemoryMcp` registry enumeration and
compare exact tool names. Preserve the ordinary public-surface integration test.

- [ ] **Step 6: Propagate invalid sub-suite outcomes**

Lifecycle aggregate status is invalid when any required sub-case is invalid;
it is quality-failed when a measured invariant fails. Capacity must not treat
invalid as pass, and lifecycle summaries retain all underlying evidence rather
than only pass rates.

- [ ] **Step 7: Run and commit**

Run:

```bash
cargo test -p eval-harness lifecycle --test lifecycle_truth
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/release.json --suite lifecycle --artifact target/evals/lifecycle-corrected.json
```

Commit:

```bash
git add crates/eval-harness/src/suites crates/eval-harness/src/test_support.rs crates/eval-harness/tests/lifecycle_truth.rs docs/evals/AGENT_MEMORY_LIFECYCLE.md
git commit -m "fix(evals): exercise wired lifecycle behavior"
```

## Completion Evidence

- explain each of the three local retrieval failures using returned evidence;
- report extraction confusion totals and retain `ext-006`/`ext-007` honestly;
- report persisted claim/relation confusion totals and zero false isolation counts;
- complete nightly without unexpected IDs;
- show actual lifecycle capture/recall call evidence;
- show persisted row/byte deltas;
- show action outcomes for all recall modes and all poisoning cases;
- show live eight-tool registry evidence.

