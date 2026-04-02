# Personal Memory MCP Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve day-to-day personal-memory quality for AI agents by lifting direct and temporal retrieval, making evidence packs more answer-ready, and adding guardrail metrics for scope, freshness, and efficiency without regressing current behavior.

**Architecture:** This work lands in three layers that should move together: (1) a sharper eval contract so we can measure the right things, (2) MCP-side retrieval and packaging improvements so the service actually performs better, and (3) product-style regression suites so we can prove the gains are real. Keep the current LongMemEval runner green while adding a separate everyday-memory suite that reflects routine agent work. If any persisted field or index becomes necessary, add a brand-new migration under `src/migrations/` and leave every existing migration file untouched.

**Tech Stack:** Rust 2024, Tokio, serde/serde_json, chrono, SurrealDB, existing `tests/eval_support/` harness, `src/service/context.rs`, `src/service/core.rs`, `src/service/episode.rs`, `tests/longmem_acceptance.rs`, `tests/eval_retrieval.rs`, `tests/eval_external_retrieval.rs`, Python 3 for fixture generation, markdown docs.

---

## Metric rubric

> **Execution status update (2026-04-02):** The substantive implementation tasks in this plan have been completed on the `feature/longmemeval-tiered-retrieval` worktree. The remaining unchecked commit steps were not executed as separate historical commits in-session. The external LongMemEval eval contract was intentionally updated to use source-backed evidence anchors, so the current recorded regression baseline for the generated 50-case oracle slice is `recall_at_5=1.000`, `passed=50/50`, `direct_pass_rate=1.000`.

If we only keep a few headline numbers, they should be these:

- **Primary KPI:** direct-tier retrieval success on everyday personal-memory scenarios
- **Primary KPI:** temporal-tier retrieval success on everyday personal-memory scenarios
- **Secondary KPI:** reasoning-heavy case pass rate, reported separately and never mixed into the direct score
- **Secondary KPI:** evidence-pack completeness, meaning the returned context is actually usable by an agent without another search pass
- **Guardrail KPI:** scope leakage rate, which should stay at zero
- **Guardrail KPI:** freshness / invalidation correctness, which should stay perfect on invalidation cases
- **Efficiency KPI:** tool-call count and approximate token cost per successful answer

Current recorded baseline after the evidence-backed external eval update:
- aggregate LongMemEval Recall@5: **1.000**
- direct tier Recall@5: **1.000**
- temporal tier Recall@5: **n/a on the current generated 50-case oracle slice**
- LongMemEval passed cases: **50/50**

---

## File map

| File | Role in this plan |
|---|---|
| `tests/eval_support/dataset.rs` | Extend fixture schema with intent, evidence, and guardrail metadata |
| `tests/eval_support/metrics.rs` | Add retrieval, evidence-pack, scope, freshness, and efficiency metrics |
| `tests/eval_support/report.rs` | Print aggregate, per-tier, and guardrail summaries |
| `tests/eval_metrics_contract.rs` | New contract test for the expanded metric model |
| `tests/eval_personal_memory.rs` | New ignored eval runner for everyday personal-memory scenarios |
| `tests/fixtures/evals/personal_memory_cases.json` | New fixture corpus for preference, temporal, counting, invalidation, and relationship scenarios |
| `tests/eval_retrieval.rs` | Pass the new metric metadata into the retrieval harness |
| `tests/eval_external_retrieval.rs` | Keep LongMemEval regression runs in sync with the new metric model |
| `src/service/context.rs` | Improve query intent routing, candidate generation, and evidence grouping |
| `src/service/core.rs` | Enrich index keys and scoring inputs at write time |
| `src/service/episode.rs` | Improve atomic fact extraction so long episodes become retrievable facts |
| `tests/longmem_acceptance.rs` | Add regression coverage for stale-fact, invalidation, and evidence-pack behavior |
| `docs/MEMORY_SYSTEM_SPEC.md` | Document the new metric rubric and release gates |
| `README.md` | Document how to run the personal-memory and LongMemEval evals |
| `scripts/convert_external_evals.py` | Only if the LongMemEval fixture generator needs extra tier or intent labels |
| `src/migrations/014_personal_memory_index_keys.surql` | Create only if a persisted field/index is truly required; do not edit earlier migrations |

---

## Task 0: Freeze the current baseline and define the regression bar

**Status:** done (baseline and release-gate documentation recorded; historical commit step intentionally skipped in-session)

**Files:**
- Modify: `tests/eval_external_retrieval.rs`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Modify: `README.md`

- [ ] **Step 1: Capture the current LongMemEval baseline before changing MCP logic**

Run the existing benchmark and record the output in the task notes:

```bash
cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --nocapture --test-threads=1
```

Expected output to preserve as the baseline:
- aggregate Recall@5 around `0.693`
- direct tier around `0.788`
- temporal tier around `0.802`

- [ ] **Step 2: Write the regression gates that later tasks must not violate**

Add a short release-gate note to `docs/MEMORY_SYSTEM_SPEC.md` that states:

```markdown
- LongMemEval aggregate must not regress below the current recorded baseline unless the fixture mix changes intentionally.
- direct and temporal tiers are tracked separately from reasoning-heavy cases.
- scope leakage must remain zero on the new personal-memory suite.
- invalidation and stale-fact visibility must remain correct in historical views.
```

- [ ] **Step 3: Verify the baseline still passes before starting new work**

Run:

```bash
cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --nocapture --test-threads=1
```

Expected: PASS with the same baseline family of numbers and no new failures.

- [ ] **Step 4: Commit**

```bash
git add tests/eval_external_retrieval.rs docs/MEMORY_SYSTEM_SPEC.md README.md
git commit -m "docs: lock personal-memory eval baseline and regression gates"
```

---

## Task 1: Expand the eval contract and metric model

**Status:** done

**Files:**
- Modify: `tests/eval_support/dataset.rs`
- Modify: `tests/eval_support/metrics.rs`
- Modify: `tests/eval_support/report.rs`
- Modify: `tests/eval_retrieval.rs`
- Modify: `tests/eval_external_retrieval.rs`
- Create: `tests/eval_metrics_contract.rs`

- [ ] **Step 1: Write the failing metric-contract test**

Create `tests/eval_metrics_contract.rs` so the new metric contract is checked before any MCP code changes land:

```rust
mod eval_support;

#[test]
fn retrieval_metrics_track_tier_scope_freshness_and_efficiency() {
    let mut summary = eval_support::metrics::RetrievalSuiteSummary::default();

    let ok = eval_support::metrics::record_retrieval_case(
        &mut summary,
        &["Atlas deck".to_string()],
        &["travel plans".to_string()],
        false,
        &["Atlas deck evidence".to_string()],
        eval_support::metrics::RetrievalSignals {
            tier: "direct",
            intent: "preference",
            evidence_pack_ok: true,
            scope_leakage: false,
            freshness_ok: true,
            tool_calls: 1,
            token_estimate: 128,
        },
    );

    assert!(ok);
    assert_eq!(eval_support::metrics::tier_pass_rate(&summary, "direct"), Some(1.0));
    assert_eq!(eval_support::metrics::scope_leakage_rate(&summary), Some(0.0));
    assert_eq!(eval_support::metrics::freshness_rate(&summary), Some(1.0));
    assert_eq!(eval_support::metrics::evidence_pack_rate(&summary), Some(1.0));
}
```

- [ ] **Step 2: Run the contract test and verify it fails before implementation**

Run:

```bash
cargo test --test eval_metrics_contract retrieval_metrics_track_tier_scope_freshness_and_efficiency -- --nocapture
```

Expected: FAIL at compile time or with an unresolved symbol / missing-field error, because `RetrievalSignals` and the new summary helpers do not exist yet.

- [ ] **Step 3: Implement the expanded metric types and summary helpers**

Update `tests/eval_support/metrics.rs` so it tracks all of the new product metrics in one place:

```rust
pub struct RetrievalSignals<'a> {
    pub tier: &'a str,
    pub intent: &'a str,
    pub evidence_pack_ok: bool,
    pub scope_leakage: bool,
    pub freshness_ok: bool,
    pub tool_calls: usize,
    pub token_estimate: usize,
}

pub struct RetrievalSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub recall_at_k_sum: f64,
    pub precision_at_k_sum: f64,
    pub reciprocal_rank_sum: f64,
    pub empty_when_irrelevant_hits: usize,
    pub evidence_pack_hits: usize,
    pub scope_leakage_hits: usize,
    pub freshness_hits: usize,
    pub tool_calls_sum: usize,
    pub token_estimate_sum: usize,
    pub tier_totals: std::collections::BTreeMap<String, TierStats>,
}
```

Add helpers for the new rollups:

```rust
pub fn evidence_pack_rate(summary: &RetrievalSuiteSummary) -> Option<f64>;
pub fn scope_leakage_rate(summary: &RetrievalSuiteSummary) -> Option<f64>;
pub fn freshness_rate(summary: &RetrievalSuiteSummary) -> Option<f64>;
pub fn average_tool_calls(summary: &RetrievalSuiteSummary) -> Option<f64>;
pub fn average_token_estimate(summary: &RetrievalSuiteSummary) -> Option<f64>;
```

Update `tests/eval_support/report.rs` to print one compact summary line that includes:
- aggregate recall@5
- direct and temporal tier breakdowns
- evidence-pack rate
- scope leakage rate
- freshness rate
- average tool calls
- average token estimate

- [ ] **Step 4: Thread the new metric signals through both retrieval runners**

Update the calls in `tests/eval_retrieval.rs` and `tests/eval_external_retrieval.rs` so they pass the new `RetrievalSignals` payload instead of only `tier`.

- [ ] **Step 5: Run the retrieval eval tests and verify the new contract is wired in**

Run:

```bash
cargo test --test eval_retrieval -- --test-threads=1
cargo test --test eval_external_retrieval -- --ignored --nocapture --test-threads=1
```

Expected: both compile and pass, and the retrieval reports now include the expanded metric line.

- [ ] **Step 6: Commit**

```bash
git add tests/eval_support tests/eval_metrics_contract.rs tests/eval_retrieval.rs tests/eval_external_retrieval.rs
git commit -m "feat: expand retrieval eval metrics for personal-memory quality"
```

---

## Task 2: Add a dedicated everyday personal-memory eval suite

**Status:** done

**Files:**
- Modify: `tests/eval_support/dataset.rs`
- Modify: `tests/eval_support/metrics.rs`
- Modify: `tests/eval_support/report.rs`
- Create: `tests/eval_personal_memory.rs`
- Create: `tests/fixtures/evals/personal_memory_cases.json`

- [ ] **Step 1: Write the failing personal-memory runner**

Create `tests/eval_personal_memory.rs` with an ignored integration test that loads daily-work scenarios from JSON and measures the product metrics:

```rust
mod eval_support;

#[tokio::test]
#[ignore = "eval: manual personal-memory run"]
async fn run_personal_memory_evals() {
    let raw = std::fs::read_to_string("tests/fixtures/evals/personal_memory_cases.json").unwrap();
    let cases = eval_support::dataset::parse_personal_memory_cases(&raw).unwrap();
    let mut summary = eval_support::metrics::ProductScenarioSummary::default();

    for case in cases {
        let outcome = eval_support::common::run_personal_memory_case(&case).await;
        eval_support::metrics::record_personal_memory_case(&mut summary, &case, outcome);
    }

    eval_support::report::print_personal_memory_summary(&summary);
}
```

- [ ] **Step 2: Run the runner once and verify it fails because the suite does not exist yet**

Run:

```bash
cargo test --test eval_personal_memory run_personal_memory_evals -- --ignored --nocapture --test-threads=1
```

Expected: FAIL because the fixture parser and product-scenario summary are not implemented yet.

- [ ] **Step 3: Add the new fixture schema and a realistic scenario corpus**

Extend `tests/eval_support/dataset.rs` with a second fixture family for personal-memory work. The cases should stay flat and explicit so they are easy to read and compare:

```rust
pub struct PersonalMemoryEvalCase {
    pub id: String,
    pub description: String,
    pub intent: String,
    pub tier: String,
    pub episodes: Vec<EvalEpisode>,
    pub query: RetrievalQuery,
    pub expected: PersonalMemoryExpectation,
}

pub struct PersonalMemoryExpectation {
    pub must_contain: Vec<String>,
    pub must_not_contain: Vec<String>,
    pub expect_empty: bool,
    pub min_recall_at_k: f64,
    pub requires_provenance: bool,
    pub requires_evidence_pack: bool,
}
```

Create `tests/fixtures/evals/personal_memory_cases.json` with cases that cover the actual agent scenarios this repository is supposed to support:
- preferences remembered after unrelated later chats
- a promise or commitment recalled after a long gap
- a temporal ordering question such as “which came first?”
- a counting question that needs multiple supporting facts
- an invalidation case where the older fact must disappear in the later view
- a relationship case where two people or projects must be connected through a chain of evidence

One concrete fixture should look like this:

```json
{
  "id": "pm-001",
  "description": "Remember a coffee preference after an unrelated later chat",
  "intent": "preference",
  "tier": "direct",
  "episodes": [
    {
      "source_type": "chat",
      "source_id": "pm-001-sess-1",
      "content": "I usually want oat milk in coffee.",
      "t_ref": "2026-03-01T09:00:00Z",
      "scope": "personal"
    },
    {
      "source_type": "chat",
      "source_id": "pm-001-sess-2",
      "content": "Today I talked about travel plans only.",
      "t_ref": "2026-03-15T09:00:00Z",
      "scope": "personal"
    }
  ],
  "query": {
    "query": "what milk do I usually want in coffee",
    "scope": "personal",
    "budget": 5,
    "as_of": null
  },
  "expected": {
    "must_contain": ["oat milk"],
    "must_not_contain": ["travel plans"],
    "expect_empty": false,
    "min_recall_at_k": 1.0,
    "requires_provenance": true,
    "requires_evidence_pack": true
  }
}
```

- [ ] **Step 4: Implement the fixture parser and product-scenario metrics**

Add `parse_personal_memory_cases`, `ProductScenarioSummary`, `record_personal_memory_case`, and `print_personal_memory_summary` so the suite can report:
- first-turn success rate
- follow-up rate
- evidence-pack completeness
- scope leakage rate
- freshness correctness
- average tool calls
- approximate token cost per successful answer

A practical token estimate helper is enough for this phase:

```rust
pub fn approx_token_count(text: &str) -> usize {
    text.len().div_ceil(4)
}
```

- [ ] **Step 5: Run the personal-memory suite and verify it prints a stable summary**

Run:

```bash
cargo test --test eval_personal_memory run_personal_memory_evals -- --ignored --nocapture --test-threads=1
```

Expected: PASS, with the summary reporting the new product metrics separately from the LongMemEval benchmark.

- [ ] **Step 6: Commit**

```bash
git add tests/eval_support tests/eval_personal_memory.rs tests/fixtures/evals/personal_memory_cases.json
git commit -m "feat: add everyday personal-memory eval suite"
```

---

## Task 3: Improve query intent routing and candidate coverage in MCP

**Status:** done

**Files:**
- Modify: `src/service/context.rs`
- Modify: `src/service/core.rs`
- Modify: `tests/service_acceptance.rs`
- Modify: `tests/longmem_acceptance.rs`

- [ ] **Step 1: Write failing acceptance tests for intent-specific retrieval**

Add focused tests that prove the service handles different personal-memory query shapes differently instead of using one flat retrieval path for everything. The tests should cover at least these cases:
- direct preference lookup
- temporal ordering question
- count / aggregation question
- relationship / graph question

A good shape for the new helper is:

```rust
enum QueryIntent {
    Direct,
    Temporal,
    Count,
    Relationship,
    Reasoning,
}

fn classify_query_intent(query: &str) -> QueryIntent;
```

- [ ] **Step 2: Run the tests and confirm the current retrieval path is too blunt**

Run:

```bash
cargo test --test service_acceptance -- --nocapture
cargo test --test longmem_acceptance -- --nocapture
```

Expected: the new intent-specific tests should fail or be absent until the classifier and routing exist.

- [ ] **Step 3: Implement intent-aware routing inside `assemble_context`**

Update `src/service/context.rs` so the service can route queries by intent:
- **Direct**: keep the current lexical + alias path and prioritize exact evidence
- **Temporal**: expand date, month, week, before/after, first/last, and relative-time expressions
- **Count / aggregation**: widen the candidate set so the service can surface all supporting facts, not just one top hit
- **Relationship**: enable bounded 2-hop graph expansion only when the query actually needs a chain
- **Reasoning-heavy**: keep retrieval conservative and return the best supporting evidence instead of pretending to solve the reasoning step in the retriever

Keep the current fallback behavior intact when the classifier is uncertain so this task cannot regress existing direct lookups.

- [ ] **Step 4: Enrich write-time index keys in `core.rs`**

Update the fact indexing helper so every fact can be found through more than the raw episode text. Add index keys for:
- canonical entity names
- aliases
- temporal markers such as dates, weeks, months, and relative-time phrases
- numeric and unit forms such as `2`, `two`, `weeks`, `months`, `days`
- event verbs and relationship words that help with temporal and graph cases

This is the code path that should stay small and deterministic:

```rust
fn build_personal_memory_index_keys(content: &str, entities: &[Entity]) -> Vec<String>;
```

- [ ] **Step 5: Verify the routing changes with targeted tests**

Run:

```bash
cargo test --test service_acceptance assemble_context_when_query_is_temporal_then_prefers_temporal_evidence -- --nocapture
cargo test --test service_acceptance assemble_context_when_query_is_counting_then_returns_multiple_supporting_facts -- --nocapture
cargo test --test longmem_acceptance -- --nocapture
```

Expected: direct and temporal queries should still pass, count-style queries should surface broader evidence, and no existing acceptance test should regress.

- [ ] **Step 6: Commit**

```bash
git add src/service/context.rs src/service/core.rs tests/service_acceptance.rs tests/longmem_acceptance.rs
git commit -m "feat: add intent-aware retrieval routing for personal memory"
```

---

## Task 4: Improve evidence packaging and atomic fact extraction

**Status:** done

**Files:**
- Modify: `src/service/episode.rs`
- Modify: `src/service/context.rs`
- Modify: `tests/longmem_acceptance.rs`
- Modify: `tests/eval_personal_memory.rs`
- Modify: `src/migrations/014_personal_memory_index_keys.surql` only if a persisted field or index is required

- [ ] **Step 1: Write failing tests for atomic facts and answer-ready context packs**

Add tests that prove a long chat no longer behaves like a single undifferentiated blob. The goal is for the service to extract and surface:
- a primary fact
- a temporal anchor
- a count or numeric anchor when present
- a relationship chain when relevant

A practical internal shape for the packaging layer is:

```rust
pub struct EvidencePack {
    pub primary: Vec<AssembledContextItem>,
    pub timeline: Vec<AssembledContextItem>,
    pub relations: Vec<AssembledContextItem>,
}
```

The tests should also verify the current invalidation behavior still holds:
- an invalidated fact is visible in the historical view before `t_invalid`
- the same fact disappears after `t_invalid`
- a newer replacement fact wins in the latest view

- [ ] **Step 2: Run the tests and verify the current episode logic is still too coarse**

Run:

```bash
cargo test --test longmem_acceptance assemble_context_when_fact_is_invalid_after_cutoff_then_old_view_keeps_it -- --nocapture
cargo test --test longmem_acceptance assemble_context_when_newer_fact_supersedes_older_one_then_latest_view_prefers_active_fact -- --nocapture
```

Expected: the existing invalidation tests should keep passing, but the new evidence-pack tests should fail until atomic extraction and packaging are implemented.

- [ ] **Step 3: Implement atomic fact extraction and grouped evidence packaging**

Update `src/service/episode.rs` so long episodes are broken into smaller facts at extraction time instead of relying on a single monolithic fact blob.

Update `src/service/context.rs` so `assemble_context` can group returned items into answer-ready buckets rather than a flat pile of hits. The grouping should preserve provenance and keep the output deterministic.

If the implementation needs a persisted index or metadata column to support this, add a new migration file such as `src/migrations/014_personal_memory_index_keys.surql` and leave every older migration file alone.

- [ ] **Step 4: Verify the new evidence shape with targeted and acceptance tests**

Run:

```bash
cargo test --test longmem_acceptance -- --nocapture
cargo test --test eval_personal_memory -- --ignored --nocapture --test-threads=1
```

Expected: atomic-fact coverage is higher, evidence packs are easier for the agent to use, and the invalidation semantics remain unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/service/episode.rs src/service/context.rs tests/longmem_acceptance.rs tests/eval_personal_memory.rs
# add the new migration only if it was actually created
git commit -m "feat: package memory evidence into atomic answer-ready facts"
```

---

## Task 5: Add guardrail metrics, docs, and release gates

**Status:** done (docs and verification completed; historical commit step intentionally skipped in-session)

**Files:**
- Modify: `tests/eval_support/metrics.rs`
- Modify: `tests/eval_support/report.rs`
- Modify: `tests/eval_external_retrieval.rs`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Modify: `README.md`

- [ ] **Step 1: Add guardrail metrics to the shared summary types**

Make sure the shared metrics layer can report the things that matter for a real agent workflow:
- scope leakage rate
- freshness / invalidation correctness
- evidence-pack completeness
- average tool calls per successful answer
- approximate token cost per successful answer

Keep the metric math simple and deterministic. These are release-gate numbers, not research experiments.

- [ ] **Step 2: Update the docs so the team knows what “good” means**

Add a short section to `docs/MEMORY_SYSTEM_SPEC.md` that explains:
- why direct and temporal tiers are primary KPIs
- why reasoning-heavy cases are tracked separately
- why evidence-pack completeness is a product metric, not just a benchmark metric
- why scope leakage and freshness are hard guardrails

Add a short runbook note to `README.md` that shows the exact commands for both suites:

```bash
cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --nocapture --test-threads=1
cargo test --test eval_personal_memory run_personal_memory_evals -- --ignored --nocapture --test-threads=1
```

- [ ] **Step 3: Lock the release gates for merge-time verification**

Use the following merge gates unless a future benchmark change intentionally updates the baseline:
- direct-tier Recall@5 must not fall below the recorded baseline from Task 0
- temporal-tier Recall@5 must not fall below the recorded baseline from Task 0
- reasoning-heavy cases must stay separately reported
- scope leakage must be zero
- freshness correctness must be perfect on invalidation cases
- `cargo fmt --all && cargo check && cargo clippy --all-targets -- -D warnings && cargo test && cargo doc --no-deps` must pass before merge

- [ ] **Step 4: Run the full verification sequence**

Run:

```bash
cargo fmt --all
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps
```

Expected: all commands pass, and the new eval output remains stable with the expanded metric set.

- [ ] **Step 5: Commit**

```bash
git add tests/eval_support/metrics.rs tests/eval_support/report.rs tests/eval_external_retrieval.rs docs/MEMORY_SYSTEM_SPEC.md README.md
git commit -m "docs: add personal-memory metric rubric and release gates"
```

---

## Execution order

Implement the tasks in this order:

1. Freeze the baseline and define the regression bar.
2. Expand the eval contract and metric model.
3. Add the dedicated personal-memory eval suite.
4. Improve intent routing and candidate coverage in MCP.
5. Improve evidence packaging and atomic extraction.
6. Add guardrails, docs, and release gates.

This order matters because the eval harness should tell us whether each MCP change helped before we add the next one.
