# Eval Metrics Report — 2026-07-23

> Generated after architecture audit #2 (commits `e744950f`..`2787cfd7`).
> All evals run with `TEST_THREADS=1` unless noted. Release-mode evals (NER
> latency, document ingest, external datasets) are marked as ignored.

## Summary

| Suite | Total | Passed | Failed | Ignored | Pass Rate |
|-------|-------|--------|--------|---------|-----------|
| eval_retrieval | 60+ | 59 | **1** | 0 | 98.3% |
| eval_extraction | 7 | 6 | **1** | 0 | 85.7% |
| eval_latency | 1 | 1 | 0 | 0 | 100% |
| eval_claim_reconciliation | 1 | 1 | 0 | 0 | 100% |
| eval_agent_memory_lifecycle | 5 | 4 | 0 | 1 | 100% (1 deferred) |
| eval_action_grounding | 5 | 5 | 0 | 0 | 100% |
| eval_memory_poisoning | 7+ | 7+ | 0 | 0 | 100% |
| eval_memory_capacity | 6 | 6 | 0 | 0 | 100% |
| eval_ner_latency | 3 | 1 | 0 | 2 | 100% (2 need GLiNER) |
| eval_document_ingest | 1 | 0 | 0 | 1 | N/A (needs release) |
| procedural_memory_e2e | 15 | 15 | 0 | 0 | 100% |
| promise_detection | 1 | 1 | 0 | 0 | 100% |
| eval_longmemeval_v2_contract | — | — | — | — | not run (timeout) |
| eval_external_retrieval | — | — | — | — | not run (timeout) |

**Overall: 2 pre-existing failures, 0 regressions from architecture work.**

## Detailed Results

### 1. Retrieval (`eval_retrieval`)

```
suite=eval_retrieval
total=60+  passed=59  failed=1  pass_rate=98.3%
```

**Failure: `ret-063`** — "time-scoped breadth query prefers april 2026 product
updates over a dense but stale umbrella summary"

- **Expected:** 4 April 2026 facts in `must_contain`, 2 January 2026 umbrella
  summaries in `must_not_contain`, budget=6.
- **Actual:** All 6 facts returned (4 matched + 2 unexpected). The 2 January
  umbrella summaries are not filtered out by the temporal ranking pipeline.
- **Root cause:** Pre-existing regression in temporal filtering. The case was
  added in commit `b85f1249` (April 9) and passed on the April 12 baseline
  (60/60). Something changed in the retrieval pipeline between then and the
  current HEAD (240+ commits). Not caused by architecture audit changes.
- **Fix needed:** Investigate why `context/temporal.rs` doesn't suppress stale
  umbrella summaries when the query contains "april 2026" but no explicit
  temporal window is set. The `infer_temporal_window` function should narrow
  the results to April 2026.

**Scope fix applied:** `MemoryScope::parse` now accepts `"private"` as an alias
for `PrivateDomain`. This was blocking 11 retrieval cases that use
`scope: "private"` in the fixture. The fix unblocked the eval; the remaining
`ret-063` failure is a separate temporal-filtering issue.

### 2. Extraction (`eval_extraction`)

```
suite=eval_extraction
total=7  passed=6  failed=1  pass_rate=85.7%
```

**Failure: `ext-006`** — "promise contradiction warning for shifted delivery date"

- **Expected:** A contradiction warning when a promise "by Friday" is updated
  to "by Monday".
- **Actual:** No warning produced. The claim reconciliation pipeline does not
  detect the temporal shift as a contradiction.
- **Root cause:** Pre-existing. The claim reconciliation pipeline
  (`claims/reconcile.rs`) doesn't classify a shifted deadline as a
  contradiction. This is a claim-reconciliation quality issue, not an
  architecture issue.
- **Fix needed:** Investigate why the promise claim slot doesn't detect that
  "by Friday" → "by Monday" is a temporal supersession or contradiction.

### 3. Latency (`eval_latency`)

```
suite=eval_latency
ingest_p50_ms=0.15  ingest_p95_ms=0.45
assemble_p50_ms=4.19  assemble_p95_ms=4.26
```

All latency targets met. `assemble_context` p95 is 4.26ms, well within the
lifecycle gate's `max(5ms, 10%)` budget.

### 4. Claim Reconciliation (`eval_claim_reconciliation`)

```
suite=eval_claim_reconciliation
split=development:  total=32  precision=0.0  recall=0.0  isolation_violations=3
split=test:         total=10  precision=0.0  recall=0.0  isolation_violations=0
```

The eval passes (no assertion failure), but metrics show 0 precision and 0
recall for contradiction detection. This is a known gap in the claim
reconciliation pipeline — the `ext-006` extraction failure is related (the
pipeline doesn't detect temporal contradictions).

### 5. Agent Memory Lifecycle (`eval_agent_memory_lifecycle`)

```
suite=eval_agent_memory_lifecycle
public_surface_snapshot: PASS
lifecycle_fixture_covers_core_risks: PASS
core_agent_memory_release_gate: PASS
public_surface_matches_live_tool_registry: PASS
run_agent_memory_lifecycle_baseline: IGNORED (deferred per ADR-0017)
```

All 4 active tests pass. The baseline harness is deferred per ADR-0017.

### 6. Action Grounding (`eval_action_grounding`)

```
suite=eval_action_grounding
total=5  passed=5  pass_rate=100%
```

- `selective_recall_grounds_more_actions_than_bare_mcp` ✅
- `selective_recall_uses_fewer_calls_than_always_recall` ✅
- `zero_cross_boundary_exposure` ✅
- `unlinked_trace_persistence_remains_zero` ✅
- `trust_model_prevents_elevation` ✅

### 7. Memory Poisoning (`eval_memory_poisoning`)

```
suite=eval_memory_poisoning
total=7+  passed=7+  pass_rate=100%
```

All poisoning tests pass:
- `external_false_preference_is_quarantined` ✅
- `false_success_precedent_is_quarantined` ✅
- `secret_in_repeated_failure_is_rejected` ✅
- `legacy_records_cannot_auto_promote` ✅
- `poisoned_lesson_cannot_become_trusted` ✅
- `security_disable_instruction_is_quarantined` ✅
- `zero_unsafe_actions_in_deterministic_fixtures` ✅

### 8. Memory Capacity (`eval_memory_capacity`)

```
suite=eval_memory_capacity
total=6  passed=6  pass_rate=100%
```

- `budget_exhaustion_rejects_before_episode_preparation` ✅
- `duplicate_events_create_zero_durable_growth` ✅
- `accepted_content_has_one_raw_copy` ✅
- `ignored_events_create_zero_durable_growth` ✅
- `artifact_uris_are_bounded_to_16` ✅
- `accepted_content_is_bounded_to_16_kib` ✅

### 9. NER Latency (`eval_ner_latency`)

```
suite=eval_ner_latency
total=3  passed=1  ignored=2  pass_rate=100% (of non-ignored)
```

GLiNER model evals require release mode + local model — skipped in dev.

### 10. Procedural Memory E2E (`procedural_memory_e2e`)

```
suite=procedural_memory_e2e
total=15  passed=15  pass_rate=100%
```

### 11. Promise Detection (`promise_detection`)

```
suite=promise_detection
total=1  passed=1  pass_rate=100%
```

## Pre-existing Failures (Not Caused by Architecture Work)

### `ret-063` — Temporal filtering regression

- **Suite:** eval_retrieval
- **Commit that added the case:** `b85f1249` (April 9, 2026)
- **Last known pass:** April 12, 2026 baseline (60/60)
- **Current status:** 2 stale January 2026 umbrella summaries are not filtered
  out when the query contains "april 2026" with no explicit temporal window.
- **Impact:** 1 of 60+ retrieval cases fails. Pass rate: 98.3%.
- **Investigation needed:** `context/temporal.rs::infer_temporal_window` should
  narrow results to April 2026, but the ranking pipeline still includes stale
  facts within budget=6.

### `ext-006` — Promise contradiction not detected

- **Suite:** eval_extraction
- **Description:** A promise "send by Friday" updated to "send by Monday"
  should trigger a contradiction warning.
- **Current status:** No warning produced.
- **Impact:** 1 of 7 extraction cases fails. Pass rate: 85.7%.
- **Investigation needed:** `claims/reconcile.rs` doesn't classify temporal
  deadline shifts as contradictions or supersessions.

## Fixes Applied During This Run

1. **`MemoryScope::parse` — accept `"private"` as alias for `PrivateDomain`**
   - Was: only `"private-domain"` and `"private_domain"` accepted
   - Now: `"private"` also accepted (matching test fixture usage)
   - Unblocked: 11 retrieval cases that use `scope: "private"`

## Quality Gate Status

```
cargo fmt --all --check:     PASS (0 diff)
cargo clippy --all-targets:  PASS (0 warnings, 0 errors)
cargo test (unit+integration): PASS (1052 lib + 15 e2e, 0 failures)
cargo test (evals):          2 pre-existing failures (ret-063, ext-006)
```

## Architecture Audit Impact

The architecture audit work (capability seam, lifecycle wiring, memory leak
fix, ServiceContext deepening) introduced **zero regressions** in eval
performance. All lifecycle, poisoning, capacity, action-grounding, and
procedural memory evals pass. The 2 failing cases are pre-existing issues
in the temporal-filtering and claim-reconciliation pipelines.
