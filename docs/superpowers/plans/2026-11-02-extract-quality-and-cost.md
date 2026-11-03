# Extract Quality and Cost Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkboxes for tracking.

**Goal:** Make extraction semantically reliable and measurably efficient for general sourced memory, including large episodes on the production Anno profile.

**Architecture:** Keep original episodes/facts distinct from claims eligible for reconciliation and from NER candidates eligible for automatic entity creation. Establish comparable-claim invariants before candidate lookup. Improve the measured hot path, including the warning path's repeated fact reads when material, without changing the eight-tool surface. Preserve historical records and defer historical repair until quality and retrieval are verified.

**Tech Stack:** Rust 1.99.0, SurrealDB, Anno (RegexNER + HeuristicNER), Prometheus/VictoriaMetrics via `gcx`, existing Rust integration and evaluation harnesses.

**Spec:** `docs/superpowers/specs/2026-11-02-extract-quality-and-cost.md`


**Local implementation status (2026-11-03):** Implemented and verified in
`codex/extract-quality-cost`. Checked steps include the following explicit
execution rulings; they do not assert a production rollout or full NER accuracy.

- The baseline report records exact queries and datasource identity; no durable
  Grafana panel permalink was established. Local stage deltas cover claim
  reconciliation, and a counting adapter isolates warning-body reads. NER and
  serialization were not individually timed; their production contribution is
  unknown. No candidate-query or memory improvement is claimed.
- Automatic candidate policy rejects demonstrated scalar false positives and
  supplements supported grammatical subjects with neutral concepts. Arbitrary
  Anno type errors remain possible; there is no validated universal classifier.
- The measured optimization is successful warning-content read reuse plus removal
  of unsupported comparison work. Existing inline/durable reconciliation budgets
  and the complete public response remain unchanged. Genuine-conflict scale is
  measured separately, without a response cap.
- Interdependent policy, canonical subject binding, retry-safe map reuse,
  regression fixtures and evidence ship in one atomic commit rather than the
  five provisional commit boundaries below. Splitting would leave intermediate
  revisions failing quality acceptance.
- Post-deployment production comparison and historical repair execution are
  separate operational actions. This implementation supplies the pinned baseline
  and recovery feasibility assessment; neither action was executed.

## Global Constraints

- No new MCP tool without an ADR; no dependency or migration edit without asking first.
- No business logic in `main.rs` or `src/service/`; knowledge rules live in `knowledge/api.rs` or knowledge-owned modules.
- Preserve bi-temporal and source lineage behavior; invalidate rather than delete.
- No invented confidence score or parallel belief state; Anno is the production baseline.
- Run the smallest relevant tests first, then workspace Clippy with `fs-watch,mcp-apps,streamable-http` and `-D warnings` before shipping.

## Review Focus

- A colon in a prose heading: retain the fact but do not invent a comparable claim (Task 2).
- Two unrelated records with the same heading: no contradiction or quadratic warning growth (Tasks 2 and 4).
- Two genuinely conflicting values for one subject and interval: retain a contradiction (Task 2).
- Anno calls a metric or workout label a person/company: preserve the mention without promoting unsupported identity (Task 3).
- A large set of genuine warning relations: do not fetch the same fact body repeatedly or silently omit warnings (Task 4).

---

### Task 1: Establish a reproducible quality and latency baseline

**Files:**
- Create: `docs/performance/EXTRACT_QUALITY_COST_BASELINE.md`
- Test: existing extraction and claim integration targets; add no production metric in this task

**Interfaces:** Consumes the current `gcx` operation, pipeline-stage, claim and result metrics. Produces a reproducible corpus manifest and table of request count, input bytes, output counts/bytes, stage time, candidate work and errors for small, medium, large and repeated extraction. Does not assume all fields already exist as metric labels; use a controlled profile for missing per-request dimensions.

- [x] Query `gcx` with a pinned UTC window and the production context/datasource from the spec; record exact PromQL, denominators, gaps and Grafana links in the baseline document. Include `memory_operation_duration_seconds`, `memory_pipeline_stage_duration_seconds`, `memory_claim_pipeline_duration_seconds`, `memory_claim_candidates_considered`, `memory_claim_pipeline_total` and `memory_operation_results_total`.
- [x] Reproduce the supplied false-contradiction shape from a minimal sourced fact, plus a comparable true contradiction, using the current build; record whether each failure occurs without relying on the truncated attachment.
- [x] Run controlled Anno extraction on small, medium and large inputs plus repeat extraction (four cases, not four sizes). Keep entity/fact mix and existing-data volume explicit. Measure NER, fact projection, claim candidate lookup/reconciliation, warning materialization and response serialization; use local tracing/profiling where `gcx` aggregates cannot distinguish them.
- [x] Record the leading bottleneck, alternative explanations, and a same-corpus acceptance comparison. Do not infer a CPU hotspot from an aggregate latency series or from warning count alone.
- [x] Commit the baseline as `docs: establish extract quality and cost baseline`.

### Task 2: Gate claim comparability at projection

**Files:**
- Modify: `crates/memory-mcp/src/knowledge/claims_policy/structural.rs`, `schema.rs`, `projection.rs`
- Test: owning module unit tests plus a claim/reconciliation integration scenario using the existing storage adapter

**Interfaces:** `parse_assertions(&str) -> Vec<StructuralAssertion>` supplies only structurally supported assertions; `ClaimService::after_fact_persisted(&FactPersistedParams<'_>) -> Result<FactProjectionSummary, MemoryError>` persists supported claims and reports skipped projections. Use existing `ClaimSkip` and claim types; no new persisted confidence field.

- [x] Add separate failing tests for a prose heading, unrelated same-heading records, genuine same-slot conflict, and valid structured fields. Use a real-storage integration test to assert the sourced fact survives when claim projection is skipped.
- [x] Define assertion eligibility in `structural.rs` and enforce it in `schema.rs`/`projection.rs`: generic colon-delimited prose or a JSON blob cannot establish a single-valued claim; unresolved subject cannot be represented by `""`. Preserve currently valid structured scalar claims and explicit correction/transition semantics.
- [x] Run the targeted tests and verify both false-positive removal and true contradiction retention; check duplicate/correction/supersession scenarios for regression.
- [x] Commit as `fix(knowledge): require comparable claim identity`.

### Task 3: Qualify automatic NER resolution

**Files:**
- Modify: `crates/memory-mcp/src/memory/episode/entity_extraction.rs` and a focused knowledge-owned candidate eligibility module if needed; leave `knowledge/entity_resolution.rs` and manual `resolve` behavior intact unless a separate failing test proves they are defective
- Test: Anno adapter cases and automatic extraction integration cases

**Interfaces:** `extract_entities(&ExtractDeps, &str, &str, Option<&[String]>) -> Result<Vec<ExtractedEntity>, MemoryError>` automatically resolves only eligible `EntityCandidate`s. The explicit `resolve` tool retains its existing contract. Episode/fact provenance and the extractor fingerprint remain available.

- [x] Capture actual Anno outputs for date/percentage tokens, domain terms and genuine names on a small multilingual corpus. Mark expected identity/link outcomes independently of the provider output; do not write a test that assumes a particular heuristic label unless observed.
- [x] Write failing automatic-extraction cases that assert invalid mentions create neither canonical entity nor `mentioned_in` edge, while supported names still link. Assert explicit manual `resolve` remains usable and the source episode remains retrievable.
- [x] Put deterministic, provider-agnostic eligibility checks before `resolve_and_dedupe` calls `resolve_or_create`; avoid a training-specific word list or invented probability. If reliable person/company promotion cannot be established from available evidence, keep uncertain candidates unlinked and report measured recall loss.
- [x] Run targeted automatic-resolution and retrieval tests; record precision/recall on the fixed corpus, including Russian and English examples.
- [x] Commit as `fix(knowledge): qualify ner candidates before linking`.

### Task 4: Bound measured extract work and response volume

**Files:**
- Modify if measured hot: `crates/memory-mcp/src/knowledge/claims_policy/worker.rs` and the owning claim store adapter
- Modify if warning materialization is material: `crates/memory-mcp/src/memory/episode/fact_extraction.rs` and the owning fact-read port/adapter
- Test: owning use-case tests and real-storage integration scenarios

**Interfaces:** Preserve the existing complete `ExtractResult` contract. Keep the current `ClaimConfig` inline budget/limit and durable reconciliation semantics; do not claim an existing public continuation token. Any proposed partial-result schema is a separate compatibility decision after measuring the need.

- [x] Add a failing scale test for the measured hot path. For warning materialization, use many genuine relations sharing counterpart facts and assert each distinct fact body is fetched once while every warning remains present. For candidate work, assert the bounded inline pass plus durable processing produces the same final relations as the small control.
- [x] Implement only the bottleneck established in Task 1. Prefer set-based, deduplicated fact reads for warning materialization; for candidate lookup, fix pagination/cursor behavior only if profiling shows it dominates. If another stage dominates, revise this task's exact file list and acceptance test before implementation.
- [x] Compare same-corpus latency, CPU/memory, query count and output bytes to Task 1; ensure no loss of genuine warnings, true contradictions, provenance or audit history. If response volume still requires a cap, design and review `ExtractResult` completeness semantics before changing the public response.
- [x] Commit as `perf(knowledge): bound extract reconciliation work` (or name the measured stage if different).

### Task 5: Verify product outcome and prepare historical recovery

**Files:**
- Modify: `docs/performance/EXTRACT_QUALITY_COST_BASELINE.md`
- Create: `docs/operations/EXTRACT_QUALITY_RECOVERY.md`
- Test: existing extraction, claim, entity and retrieval targets that cover changed paths

**Interfaces:** Produces a before/after report with identical corpus and production cohort definitions, plus a separately reviewable invalidation/re-projection procedure. Does not mutate production history.

- [x] Run focused tests, `rtk cargo fmt --all --check`, and `rtk cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings`.
- [x] Compare the fixed local corpus before and after. Query a fresh, comparable `gcx` production window only after deployment; the pinned pre-change window remains the baseline. Report latency distributions with sample counts, warning count per fact, candidate work, errors, entity quality and retrieval effects; label unavailable production input/output byte metrics as unavailable rather than inventing them.
- [x] Inventory how affected entity, edge, claim, relation and projection records are represented, then write a recovery procedure with affected-record selection, append-only/invalidation feasibility, dry-run counts, readback and retrieval checks. State explicitly where existing APIs cannot repair derived state. Do not execute production recovery under this plan.
- [x] Commit as `docs: record extract verification and recovery procedure`.
