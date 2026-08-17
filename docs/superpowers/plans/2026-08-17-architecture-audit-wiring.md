# Architecture Audit — Wiring & Suppression Sweep — 2026-08-17

> Status: In progress
> Parent: `/improve-codebase-architecture` audit + `/grill-with-docs` planning round
> Branch: `housekeeping-baby` (from `53435300`)

## Context

Audit of `memory_mcp` against four goals: (1) maintainability and testability,
(2) best engineering/architectural practices and system design, (3) DRY, KISS,
YAGNI, DDD, and (4) no non-wired or dangling functionality (checked against
`docs/superpowers/plans/`). This round resolves the remaining wiring gaps and
lint-suppression debt found after the 2026-08-11 audit follow-up.

## Grilling decisions (binding)

The grilling round produced two answers. The exact question text was lost to
context compaction; the mapping below is the inferred reading of the evidence
and is recorded here explicitly rather than re-asking the user.

- **q1 — "let's defer and document decision"** → interpreted as: **defer the
  claim-level eval gate**. The eval fixture `claim_reconciliation_cases.json`
  carries `expected.claims` (schema, value, qualifiers, validity,
  `source_span`) that no eval suite asserts yet. Building a full
  claim-assertion gate is a roadmap item, not audit remediation. Deferred and
  documented here (precedent: ADR-0017 deferral, 2026-08-11 `try_parse_json_scalars`
  documented deferral). No ADR needed: this is a plan-level deferral of a
  feature gate, not a hard-to-reverse design decision.
- **q2 — "must be wired"** → interpreted as: **persist `source_span` through
  the claim pipeline**. `ClaimDraftCandidate.source_span` is computed by all
  four schemas but dropped in `projection.rs` when building `ClaimDraft` →
  `Claim`; the code comment literally says "persisted through
  `Claim.source_span` in a later step". This is the strongest "built but never
  wired" finding. Wired in this round.

## Findings and resolutions

| ID | Severity | Finding | Resolution |
|----|----------|---------|------------|
| W1 | Major | `ClaimDraftCandidate.source_span` computed by all four schemas, dropped in `projection.rs`; `Claim` model has no `source_span` field; eval fixtures carry span expectations no suite asserts (q2) | Wire `source_span: Option<(usize, usize)>` through `ClaimDraft` → `build_claim` → `Claim`; new append-only migration 038 (`claim` table is SCHEMAFULL); remove the false `#[allow(dead_code)]` in `schema.rs`; assert spans in the fixture-shape test |
| W2 | Minor | False `#[allow(dead_code)]` / `#[cfg_attr(not(test), allow(dead_code))]` on wired items: `GraphCandidate.trace` (consumed in `ranking.rs` production path), `QueryFlags::max_graph_hops` (called from `pipeline.rs`), module-level allows in `claims/worker.rs`, `claims/structural.rs`, `claims/backfill.rs` (all wired via `start_claim_workers` / `projection.rs` / `builder.rs`) | Remove the suppressions; compile + test is the evidence. Items that surface as genuinely dead after removal are deleted (YAGNI) |
| W3 | Minor | DRY: three copies of the rate-limit check — `MemoryService::enforce_rate_limit` (test-only via `cfg_attr`), `ServiceContext::enforce_rate_limit` (production), `IngestionService::rate_limiter_check` | Delete the `MemoryService` copy; relocate its 6 tests to `RateLimiter`/`ServiceContext`; collapse `IngestionService::rate_limiter_check` onto a shared `RateLimiter::check_access` helper |
| W4 | Minor | `compare_ranked_context_facts` / `sort_ranked_context_facts` (`ranking.rs`) are test-only helpers gated by `cfg_attr(not(test), allow(dead_code))` | Keep, but reclassify honestly: they are test-only by design (deterministic no-focus ordering for ranking assertions). Replace the misleading `cfg_attr` with a `#[cfg(test)]` visibility or a documented test-helper note, keeping production `compare_ranked_context_facts_with_focus` as the single production comparator |
| W5 | Nit | `scope.rs` deleted; `LifecyclePolicy` moved into `service/lifecycle.rs`; false `#[allow(dead_code)]` on `test_support::db_client`; three plan docs still marked pending though executed; `gen_anno_onnx_parity.py` reads `/tmp` config | Already done in the working tree (pre-audit housekeeping); committed as the first commit of this round |
| W6 | — | Kept as documented/justified (no action): telemetry reserved variants `ClaimMetricStage::Backfill` + `ClaimMatchMode::Alias`; `dispatch.rs` `app` descriptor field; `gliner.rs` `max_concurrency` recipe field + test-only `run_forward`; feature-gated `mcp-apps` allows; `run_agent_memory_lifecycle_baseline` (ADR-0017); `try_parse_json_scalars` stub (2026-08-11 documented deferral); `lfm2_gliner` module-level allows (dormant span-decoding contract surface, verified wired via `lfm2_gliner::build` for the parts that are live) | No action — documented |

## Task list (priority order, strict TDD)

1. **Commit pre-audit housekeeping** (W5) — already in the working tree.
2. **W1 — wire `source_span`** (top priority):
   - Red: `build_claim` carries `source_span` from draft (models test);
     projection integration test asserting the persisted claim has the span;
     migration-038 registration test updated.
   - Green: add `source_span: Option<(usize, usize)>` with `#[serde(default)]`
     to `Claim` and `ClaimDraft`; thread through `build_claim` and
     `projection.rs::after_fact_persisted`; migration
     `038_claim_source_span.surql` (`DEFINE FIELD source_span ON claim TYPE
     option<array>` — SCHEMAFULL table requires the field definition);
     remove `#[allow(dead_code)]` in `schema.rs`.
   - Eval: remove `#[allow(dead_code)]` from `ExpectedClaim.source_span` in
     `tests/eval_claim_reconciliation.rs` by asserting span format in the
     fixture-shape test (minimal wiring; full suite gate = q1 deferral).
   - **Identity constraint:** `source_span` must NOT enter `claim_id`
     computation (ADR-0013 deterministic identity: schema_ref + extractor
     fingerprint + fact_id + `CanonicalPayloadHash` of value + qualifiers).
3. **W2 — remove false suppressions** — compile + test is the evidence; no
   behavior change. Verify under the canonical feature set
   `--features cli-watch,mcp-apps`.
4. **W3 — dedupe rate-limit enforcement** — tests green before and after;
   delete `MemoryService::enforce_rate_limit`, relocate tests, fold
   `IngestionService::rate_limiter_check` onto `RateLimiter::check_access`.
5. **W4 — reclassify ranking test-only sort helpers** — honest `#[cfg(test)]`
   or documented test-helper status; no production behavior change.
6. **q1 deferral documentation** — this plan doc is the record (see Grilling
   decisions). No ADR.
7. **Verification:** `cargo fmt --all`, canonical clippy command,
   `cargo test --workspace --all-targets --features cli-watch,mcp-apps`.

## Constraints honored

- Claim identity unchanged (ADR-0013); `source_span` excluded from payload hash.
- Append-only migrations only; migration 029 never edited (ADR-0011).
- No new MCP tools (8-tool surface frozen); no new dependencies.
- No `unwrap()`/`expect()` introduced in production code.
- Business logic stays in `src/service/`; errors via `MemoryError`.
