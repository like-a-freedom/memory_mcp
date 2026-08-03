# Architecture Hardening Plan — 2026-08-01 (Round 2)

> Audit: `target/arch-review/architecture-review-2026-08-01.html`
> (rendered copy; sources/evidence in `target/arch-review/2026-08-01-SOURCES.md`).
> Quality baseline: benchmark v5 (2026-07-29) — 17/17 gates green across
> PR/Release/Nightly, confirmed stable through the 2026-07-30 hardening
> round at HEAD `88fbf61b`.
> **Eval non-regression: every card below preserves those gates.**

This plan absorbs: (a) the seven candidates from the 2026-08-01 audit;
(b) Cards 1, 3, 4 from `2026-07-30-architecture-hardening.md`, still open
with ADRs 0023/0025 Accepted and Card 4's dependency (Card 1) now
prioritized first. Card 6 was partially executed (builders moved); its
remaining half is the deferred corpora move — kept deferred here.

## New ADRs

- **ADR-0026** — Adopt durable-work mechanics over per-worker constants
  (candidate 6).
- **ADR-0027** — Finish ADR-0024: deepen the storage seam (candidate 1).
  Explicitly amends ADR-0024; verification restated with concrete greps.

ADR-0023 (app-command descriptors) closes candidate 2; ADR-0025 (metric
formula home) closes candidate 5; no new ADR needed for cards 4 or 7.

## Cards, order, and dependencies

| # | Candidate | ADR | Gates checked | Depends on |
|---|-----------|-----|---------------|------------|
| 1 | Execute ADR-0023: typed `COMMAND_TABLE` in `service/apps/dispatch.rs`; 713-line `app_command` match → ~50-line dispatch; 15 `unreachable!()` zeroed; auth duplication with `workflow.rs::parse` collapsed | 0023 | lifecycle release gates (`action_grounding_pass_rate`, `poisoning_pass_rate`), `public_surface_snapshot` frozen 8-tool surface | — |
| 2 | Execute ADR-0027: move SQL into owning stores, shrink `DbClient` to 7 record ops, remove stub `Ok(vec![])`/`Ok(0)` defaults, shrink `MockDbClient`, migrate 11 hand-rolled test stubs onto stores/in-memory engine | 0027 | PR + Release eval gates at v5 values; `cargo test --workspace --all-targets` | — |
| 3 | Sweep dangling clusters: wire `durable_work` into agent_memory workers (ADR-0026), wire `claims::telemetry` dark metrics (duration/candidate/relations/lag) into reconcile+backfill workers, delete str-helper duplicates so `capture.rs` uses one home | 0026 + wire/delete | PR profile smoke; prometheus feature-on build check | — |
| 4 | Execute ADR-0025 for real: suites stop constructing `metric_map.insert("<string>", float)`; per-case values rendered through `metrics.rs` from evidence; reducer remains sole formula home | 0025 | PR + Release + Nightly profile values must match v5 **exactly** (byte-identical headlines) | — |
| 5 | Wire-or-delete the rest: remaining stale `#[allow(dead_code)]` items (ClaimStore trait field, `from_raw`, `RetrievalTier`, `filter_facts_by_policy`, model-loader API, gliner `run_forward`, `select_*` non-advanced variants), mutex-poison panics in `recall.rs`+`logging.rs` (lock poisoning → `(MemoryError::Internal)`, no panic on request path) | house-keeping | full test suite + clippy `-D warnings` | 1–4 (touches most files; sequencing avoids conflicts) |
| 6 | Card 4 from 2026-07-30, **rescoped**: `MemoryService` keeps only construction + worker lifecycle; delegates removed; `find_intro_chain` shim resolved into `service/apps/graph.rs`; `core.rs` test module split per-capability; `tools/` call capabilities directly; ~30 test/eval call sites migrated onto capability structs (the consumer migration is the real work, per audit finding B2) | completes capability-seam-completion plan | PR + Release + Nightly gates; pipeline benches within v5 noise band | 1, 2 |
| 7 | Candidate 5 (pipeline.rs depth): extract `assemble_default_context` tier-fallback decisions into per-tier strategy objects with unit tests; redistribute `context.rs` 2.6k test block into per-tier files | — | retrieval suite metric values unchanged (recall_at_5 etc. byte-identical at v5) | 2 (touches context collectors) |

**Run order:** 1 and 2 first (both unblock 6; 2 unblocks 7) → 3, 4, 5 in
parallel (disjoint write-sets: agent_memory+claims telemetry vs
eval-harness vs mixed housekeeping) → 6 → 7 last.

## Stage gates (each card)

- `cargo build` — green
- `cargo test -p <affected-crate>` — green
- `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` — zero warnings
- `cargo fmt --all --check` — zero diff
- Benchmark non-regression per card's Gates column above; artifact diff reviewed for headline metrics before merge.

## Card details

### Card 1 — typed dispatch (ADR-0023)

ADR-0023's table + outcome contract land as written; two audit refinements:
the *parse-half* already exists in `service/apps/workflow.rs` (per-app
`AppCommand::parse` with `require_app`), so dispatch trusts it and the ~9
redundant `if app != "lifecycle"` arm checks are deleted, not moved.
`AppCommandDescriptor` gains no authorization field — authorization stays
in parse. Confirmation policy sits in the descriptor row.

- `grep -c "unreachable!" crates/memory-mcp/src/mcp/handlers.rs` = 0
- `handlers.rs` `app_command` body ≤ ~60 lines
- `COMMAND_TABLE` in `service/apps/dispatch.rs`; adding a command = adding a row

### Card 2 — finish storage seam (ADR-0027)

Execute per ADR-0027's six steps, migration order `context → app → fact →
episode`. Watchpoint from the audit: `SurrealClaimStore` bypassed
`queries.rs` because `queries.rs` builders were shaped for `DbClient`
consumers; when the SQL moves into stores, keep one SQL home per store
(file-local `mod queries` inside each store file if that reads better than a
shared crate module — decide at implementation, record choice in the PR).

### Card 3 — agent_memory workers + telemetry (ADR-0026 + wire)

Wire, never re-key:
- `agent_memory/{worker,projection}.rs` replace local consts with
  `durable_work::{DEFAULT_EMPTY_POLL_SECS, …}` and helpers; delete
  `#[allow(dead_code)]` from `durable_work.rs`.
- Reconcile worker calls `record_pipeline_duration` per page;
  `record_candidate_count`; `set_active_relations` after relation commit.
  Backfill worker calls the lag gauge (`METRIC_BACKFILL_LAG`).
- Delete duplicated enum→str mappers — one home in `storage/agent_memory.rs`,
  used by `capture.rs`.
- Verify metric names/labels remain bounded enums per CONTEXT.md constraint.

### Card 4 — single formula home (ADR-0025)

Suites stop constructing `metric_map.insert("recall_at_5", …)` /
`metrics.insert("<string>", …)` (sites: `retrieval.rs:311-313`,
`external_retrieval.rs:129-145`, `claims.rs:520-559`, `action_grounding.rs`,
`capacity.rs`, `poisoning.rs`, `end_to_end.rs`, `downstream_qa.rs`,
`response_size.rs` — per-case diagnostic maps only where they mirror
evidence; rendered from `MetricEvidence` through `metrics.rs`). Wire-format
keys unchanged; values come from exactly one code path.

### Card 5 — wire-or-delete remainder + panic hygiene

Wire/delete table per item, from audit evidence:
- `ClaimStore` trait allow → remove (field is used by project.rs)
- `ClaimProjectionSource`, `PersistProjectionRequest`, `lifecycle_mutation`
  field → wire or delete (check `project.rs`/`backfill.rs` usage)
- 6 stub-default methods removed by Card 2; confirm zero remain
- `models/ids.rs from_raw` — remove the stale allow where used by tests
- `RetrievalTier` (ranking.rs:98) — wire or delete per rescue/budget/read
  path review
- `filter_facts_by_policy` — wire into selective-recall or delete
- model-loader API + gliner `run_forward` — wire into ensure-cached flow
  for tests, or delete
- `agent_memory/recall.rs` ×4 + `logging.rs` mutex `.expect(...)` → map
  poison to `MemoryError::Internal` + return `Err`, request path never
  panics (CONTEXT.md constraint upheld literally, not just in spirit)

### Card 6 — MemoryService composition root (rescoped)

Real work per audit finding B2: ~30 test/eval call sites in
`tests/*.rs` + `crates/eval-harness/src/{suites,benches}/*.rs` +
`service/agent_memory/projection.rs:248-250` migrate from
`service.extract(...)`-style delegates to capability structs.
`find_intro_chain` ("interface while consumers migrate" shim) moves into
`service/apps/graph.rs`. `core.rs` after: construction, worker lifecycle,
`ProductionRecallPipeline`, logging shapers — and the 1,189-line test
module splits per capability. Delete the `eval-harness/lib.rs` 13-commit
churn risk by landing this *before* trait-bearing shims regrow.

### Card 7 — context pipeline depth

`pipeline.rs` (549 prod lines, 0 tests) gets per-tier unit tests by
extracting tier-fallback into strategy objects; the sideways collector
coupling (ranking⇄lexical, ranking⇄scoring, filtering used by 7 siblings)
is reduced by moving `RetrievalTier` + shared candidate types into
`context/params.rs` or a new `context/types.rs`, making collectors depend
on types, not each other. Test blocks in `context.rs` (2,577 lines)
redistribute to per-tier files. This is design-it-twice territory — run
the interface options through the grilling loop before locking the shape.

## Card 5 from 2026-07-30 confirms closed

`claims/normalize.rs` deleted, response-size suite + profile landed,
ADR-0022 Accepted on Phase B data. Verified present in tree at HEAD.

## Deferred (deliberate, with reason)

- Eval corpora relocation (`2026-07-30` Card 6 second half): ADR-0020 pins
  digests on the current path; re-pinning across all profiles for a pure
  path move is churn with no behavior gain. Revisit when a second consumer
  of the corpora appears.
- Trait-object strategy for `EmbeddingService::new` per-call in
  `build_context()` (audit B4): cost unmeasured; decide after profiling
  under the Nightly performance mode if it shows up.

## Execution status

_Landed 2026-08-01 → 2026-08-02 in the order: Card 1 → Card 3 → Card 2 stage 1 → Card 4 (+ Card 5 panic hygiene)._

| Card | Status | Notes |
|------|--------|-------|
| 1 | ✅ complete | ADR-0023 landed — `service/apps/dispatch.rs`, 14 descriptors, 0 `unreachable!()`; `Redundant auth checks deleted 9x`; 3 unit tests for descriptor↔parse alignment |
| 2 | ✅ complete | ADR-0027 landed: stage 1 (6 stub defaults removed, 19 test doubles explicit) + stage 2 (all capability SQL moved into narrow stores; `DbClient` shrunk to the core 6 ops `select_one`/`select_table`/`create`/`update`/`query`/`apply_migrations`; MockDbClient shrunk to match with `query`-op dispatch for entity-lookup/edge-neighbors; behavior doubles route through `query` overrides; 6 behavior tests migrated to real in-memory SurrealDB; `build_select_edges_filtered_query` deleted; PR eval at v5 byte-parity) |
| 3 | ✅ complete | ADR-0026 landed — `durable_work` wired into `agent_memory/{worker,projection}.rs`; `claims::telemetry` 4 dark metrics wired into `reconcile_page_with_owning` + `backfill`; `METRIC_BACKFILL_LAG` deleted |
| 4 | ✅ complete | ADR-0025 landed — `render_case_metrics` + `CaseMetricNames` added to `metrics.rs`; five suites (retrieval, external_retrieval, claims, extraction, end_to_end) emit typed evidence and render per-case maps through it; pattern-scan regression tests added in `suites.rs`; PR/Release/Nightly at v5 byte-parity. Remaining string-key writes are schemaless diagnostics ADR-0025 allows (`query_ms`, `rows_*`). |
| 5 | ✅ complete | Panic hygiene (Mutex poison → Err everywhere), mock_db .unwrap → poison-recovery, shared env_lock, `CommitRelationRequest`/`commit_relation` removed; stage 2: `select_facts_filtered`/`_advanced` + `select_episodes_by_content`/`_advanced` collapsed into single DB-side-filtered signatures, pure-delegate builders + client-side `filter_records_by_*` helpers deleted, 3 dangling `ClaimStore` methods (`load_projection_source`, `select_source_evidence`, `upsert_compiled_policies`) + Surreal impls + Noop stubs deleted; `grep _advanced` = 0; PR eval at v5 byte-parity |
| 6 | ✅ complete | `capabilities` exposed as `pub` (API addition; frozen 8-tool MCP surface untouched); 6 thin `MemoryService` delegates (`ingest`/`explain`/`extract`/`resolve`/`invalidate`/`assemble_context`) deleted from `core.rs` — core.rs is now construction + worker lifecycle + `ProductionRecallPipeline` + logging; `resolve_entity`/`relate` relocated to `service/apps/graph.rs` as inherent methods alongside `find_intro_chain` (ADR-0024 step 1); dead `invalidate_metric_if_superseded` deleted; ~200 consumer sites migrated onto capability structs (eval-harness suites + benches + adapters + test_support, `projection.rs`, `content_extraction/watcher.rs`, `apps/ingestion_review.rs`, `mcp/handlers` tests, 15 integration-test files); `core.rs` 1,189-line test module split per owning module (find_intro_chain/resolve_entity/relate → `apps/graph.rs`, indexed resolve → `capabilities/resolve.rs`; verified zero test loss); 1502 tests green, PR/Release/Nightly at v5 byte-parity |
| 7 | ✅ complete | New `context/types.rs` homes `RetrievalTier` + `RankedContextFact` so collectors depend on types not on the ranker (budget/lexical/views/rescue import from types; ranking re-exports for compat); `pipeline.rs` (was 0 tests) gains `EpisodeFallbackStrategy` + `FallbackDecision` with 4 unit tests, wired into `assemble_default_context` with identical behavior. Test redistribution: already complete per the 07-23 relocation plan — tier-local tests live in their tier files and only cross-tier `assemble_context` integration tests remain in `context.rs` (the 07-23 plan's stated end-state). Full suite 1506 green; PR/Release/Nightly at v5 byte-parity |
