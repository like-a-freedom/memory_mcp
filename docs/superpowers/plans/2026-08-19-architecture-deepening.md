# Architecture Deepening — Round 2 — 2026-08-19

> Status: In progress
> Parent: `/improve-codebase-architecture` round-2 report (11 candidates) +
> `/grill-with-docs` planning round
> Branch: `deepening-round-2` (from `73a60aad`)
> Report: `$TMPDIR/architecture-review-20260819-101359.html` (not in repo)

## Context

Round 1 (2026-08-17) fixed wiring gaps and false suppressions. Round 2 is about
**deepening modules**, not wiring: turning shallow modules into deep ones so
that complexity is concentrated behind small interfaces. The audit found 11
candidates (9 Strong, 2 Worth exploring). All fact-finding was completed before
grilling; every number below is verified against the tree at `73a60aad`.

Vocabulary follows `CONTEXT.md` (domain) and the codebase-design skill
(architecture): module, interface, implementation, depth, seam, adapter,
leverage, locality; deletion test.

## Grilling decisions (binding)

The user answered Q1 explicitly ("yes") and delegated Q2–Q8 ("decide yourself
based on best engineering practices and product use cases"). The decisions
below are therefore agent-made and recorded here as binding.

- **Q1 — Scope & sequence: ALL 11 candidates**, in this order:
  - Storage track: **#4 → #5 → #6 → #11**
  - Retrieval track (parallel where disjoint): **#1 + #2**
  - Standalone: **#3, #7, #8, #9**
  - Any time: **#10**
- **Q2 — ADR allocation.** ADRs written for decisions that future architecture
  reviews must not re-litigate or that change an established contract:
  - **ADR-0039** — one owner for the bi-temporal close protocol (#6). An
    invariant with 5 historical spellings; must be pinned.
  - **ADR-0040** — narrow retrieval infrastructure context (#1). Changes the
    established `ServiceContext` shape that round 1 introduced.
  - **ADR-0041** — NER backends declare scheduling requirements (#9). Extends
    the ADR-0029/0036 registry contract.
  - Plan-level only (no ADR): #4 (recipe concentration — mechanical DRY),
    #5 (store consolidation — applies ADR-0024/0027 pattern), #10 (bug fix),
    #11 (test-strategy shift, gradual — revisit ADR if it proves load-bearing),
    #2, #3, #7, #8 (internal reorganizations).
- **Q3 — #6 close-op semantics.**
  1. Default timestamps come from server-side `time::now()` (native datetime;
     no string coercion — the `storage/claims.rs` comment is authoritative).
  2. The close operation accepts **optional caller-supplied timestamps**
     (`episode/edges.rs` supersession closes old edges with the new edge's
     `t_valid`/`t_ingested`, not now).
  3. **Fix the latent invariant violation at site 4**: `retract_fact_and_claims`
     sets `t_invalid` + `invalidation_reason` on the fact but not
     `t_invalid_ingested`. The fix is a tested behavior change (embedded test).
- **Q4 — #3 delegator fates.**
  - **Keep** `MemoryService::add_fact` — ~100 test call sites make it a fixture
    API; deleting churns the suite for zero depth gain (deletion test: it
    concentrates test ergonomics, not complexity).
  - **Delete** `generate_embedding` / `build_fact_embedding_input` delegators;
    the single production caller (`reembed.rs:855`) calls the services directly.
  - **Move** `episode_count` into `EpisodeStoreClient` (0 production callers,
    2 test callers; raw SQL + explicit namespace is an ADR-0024 residual).
  - **One record-lookup owner** in the storage-adjacent layer; delete the
    triplicated `find_record_by_id` copies in `MemoryService` and
    `ExplanationService`; `ServiceContext` keeps its thin delegating wrapper.
- **Q5 — #10 rate-limit owner.** The service layer owns enforcement (the
  lifecycle capture path legitimately bypasses the capability check). Delete
  the capability-level double debit; test wiring shares one limiter; assert
  N ingests = N tokens.
- **Q6 — #7 typed graph state.** Introduce typed `GraphSessionState` in
  `apps/graph.rs`; serialize to the **identical JSON shape** at the session
  edge (the payload is embedded in app HTML at `mcp/resources.rs:154`);
  dispatch handlers become operations on the typed state. A round-trip test
  pins JSON compatibility.
- **Q7 — #11 ambition.** Gradual. After #4–#6 land, migrate SQL-prefix
  consumers to the embedded in-memory adapter (pattern proven in 9+ files);
  delete `expect_query` / `expect_edge_neighbors` / the `FROM edge` special
  case; keep the simple canned `select_one`/`create`/`update` builders.
- **Q8 — Logistics.** One plan doc (this file), one branch
  (`deepening-round-2`), strict TDD, commit per task, full verify gate + push
  when green.

## Findings and resolutions

| ID | Strength | Finding | Resolution |
|----|----------|---------|------------|
| #4 | Strong | Row-query recipe (missing-table → empty) hand-copied ~26× across 5 store files; policy changes require touching every site | `BoundDbClient::query_rows` / `query_first` concentrate the recipe; all sites migrate |
| #6 | Strong | Bi-temporal close written 5 ways (invalidate.rs, decay.rs, conflict_resolver.rs, claims.rs, edges.rs) in 3 syntaxes; site 4 omits `t_invalid_ingested` on fact retraction (latent invariant violation) | Single close owner in storage (ADR-0039); server-side `time::now()` default + optional caller timestamps; site-4 bug fixed with embedded test |
| #5 | Strong | `EntityService` is a second shallow entity store; 3 SQL-taking pass-throughs (`query_triples`, `invalidate_triple_by_id`, `execute_query`) exist only so `conflict_resolver.rs` / `triples.rs` can inject SQL | Triple persistence moves behind the store seam; `conflict_resolver` expresses intent (find-conflicting / close), not SQL |
| #2 | Strong | `matched_query_terms_*` lexical-relevance helpers duplicated across 5 files; `is_four_digit_year` ×2 | One home in `service/query/lexical.rs`; all callers import |
| #1 | Strong | `ServiceContext` carries 18 fields; the retrieval pipeline uses 5; `context_store()` rebuilt 19× in 9 files; 12 hand-written `DbClient` fakes exist because the seam is too wide | Narrow retrieval context (ADR-0040): pipeline depends on the narrow interface it actually uses |
| #3 | Strong | Residual `MemoryService` surface: triplicated `find_record_by_id`, embedding delegators, 11-arg `EmbeddingService` rebuilt per `build_context()` | Collapse per Q4 decisions |
| #7 | Worth exploring | Graph app session state is JSON surgery in `dispatch.rs`; invariants inexpressible | Typed `GraphSessionState` with identical JSON serialization (Q6) |
| #8 | Strong | Community logic spread over 3 modules with 2 write paths; `is_entity_id` ×3; convergence invariant inexpressible | One community module, one write path, shared predicates |
| #9 | Strong | NER scheduling decided by `matches!` string routing in `entity_extraction.rs`; `anno_onnx` hand-rolls idle-unload without an `InferenceGate` | Backends declare scheduling via the registry contract (ADR-0041) |
| #10 | Worth exploring | Ingest rate-limit double debit: tool path takes 2 tokens, capture path 1 | Service owns enforcement; single debit (Q5) |
| #11 | Strong | `MockDbClient` SQL-prefix scripting (13 builders; `expect_update_with` missing; zero consumers inject `Err`) | Gradual retirement per Q7 after #4–#6 |

## Task list (priority order, strict TDD)

Sequencing respects file-overlap constraints: #4 before #6 (both touch
`storage/claims.rs`); #6 before #5 (both touch `conflict_resolver.rs`); #3
before #1 (both touch `service_context.rs`). Parallel batches only where write
sets are disjoint.

1. **T1 — #4 row-query recipe** (foundation stone).
   - Red: `BoundDbClient::query_rows` returns rows, empty on missing table,
     propagates other errors; `query_first` returns first row / None.
   - Green: implement on `BoundDbClient` (storage/client.rs) using
     `helpers::is_missing_table_error` + `extract_records` semantics.
   - Refactor: migrate all ~26 sites (episode_store, app_store, context_store,
     reembed_store, claims, entity) to the recipe methods; delete local copies
     of the match arm. Behavior-identical.
2. **T2 — #6 bi-temporal close owner** (ADR-0039).
   - Red: embedded test — `retract_fact_and_claims` sets `t_invalid_ingested`
     on the fact (site-4 bug fix); unit tests for the close-op SQL builder
     (default `time::now()` vs caller-supplied timestamps; fact/edge/triple
     field sets).
   - Green: one close module in storage; migrate the 5 sites; fix site 4.
   - `invalidate.rs` keeps writing `request.reason` into `invalidation_reason`
     (currently dropped — wire it through the close op).
3. **T3 — #2 lexical primitive home.**
   - Red: tests for `matched_query_terms_*` behavior at the new location.
   - Green: `service/query/lexical.rs`; migrate 5 files; dedupe
     `is_four_digit_year`.
4. **T4 — #5 entity/triple store.**
   - Red: store-level tests for find-conflicting-triples + close-triple.
   - Green: triple persistence behind `EpisodeStoreClient` (or a new
     `EntityStoreClient` if the seam is cleaner); `conflict_resolver.rs` and
     `episode/triples.rs` stop supplying SQL; delete the 3 pass-throughs.
5. **T5 — #10 rate-limit single debit.**
   - Red: test asserting N ingests = N tokens on the tool path.
   - Green: delete capability-level debit; service owns enforcement.
6. **T6 — #7 typed graph session state.**
   - Red: round-trip test pinning the JSON shape consumed by app HTML.
   - Green: `GraphSessionState` typed; dispatch operates on it; serialization
     identical.
7. **T7 — #9 NER scheduling declaration** (ADR-0041).
   - Red: registry test — every backend declares `scheduling()`; routing uses
     the declaration, not `matches!` on names; `anno_onnx` declares
     blocking-pool + gate.
   - Green: trait method + per-backend impls; `entity_extraction.rs` routing
     consumes the declaration; `anno_onnx` gains an `InferenceGate`.
8. **T8 — #8 community module.**
   - Red: convergence-invariant test expressible against one module.
   - Green: merge the 3 modules; one write path; `is_entity_id` once.
9. **T9 — #3 MemoryService collapse** (Q4).
   - Red: tests pinning record-lookup owner behavior; reembed path via
     services directly.
   - Green: delete embedding delegators; move `episode_count`; delete
     duplicate `find_record_by_id` copies; keep `add_fact`.
10. **T10 — #1 narrow retrieval context** (ADR-0040).
    - Red: retrieval pipeline tests against the narrow seam; fakes shrink.
    - Green: pipeline depends on the narrow interface; `context_store()` built
      once; hand-written `DbClient` fakes deleted where the narrow seam
      replaces them.
11. **T11 — #11 MockDbClient gradual retirement** (Q7).
    - Migrate SQL-prefix consumers (entity_resolution ×13, resolve,
      apps/graph, ingestion) to embedded in-memory adapter; delete
      `expect_query`, `expect_edge_neighbors`, `FROM edge` special case; keep
      canned builders.
12. **Verification:** `cargo fmt --all`, canonical clippy command,
    `cargo test --workspace --all-targets --features cli-watch,mcp-apps`.
    Code-review pass per `code-review.prompt.md`; merge to `master`; push.

## Constraints honored

- 8-tool MCP surface frozen; no new MCP tools.
- Append-only migrations only (next = 039); migration files never edited.
- ADRs 0001–0038 not re-litigated; new ADRs start at 0039.
- No `unwrap()`/`expect()`/`panic!` introduced in production code.
- Business logic stays in `src/service/`; storage owns SQL (ADR-0024/0027).
- One Active Namespace (ADR-0038); no request-level partitioning.
- Errors via `MemoryError`; feature flags additive; verify under
  `--features cli-watch,mcp-apps --locked`.
- Bi-temporal model: never delete, only invalidate; `t_invalid_ingested` MUST
  be set whenever `t_invalid` is closed (ADR-0039).
