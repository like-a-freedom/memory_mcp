# ADR-0027: Finish ADR-0024 — Deepen the Storage Seam

> Status: Accepted (2026-08-01)
> Amends ADR-0024: the accepted narrowing shipped the *shape* (concrete store
> structs) but left the *substance* behind — the universal `DbClient` still
> owns the queries. Verification criteria of ADR-0024 are unmet at HEAD.

## Context

ADR-0024 (Accepted 2026-07-30) mandated:

> Shrink `DbClient` to connection machinery plus core record operations
> (`create` / `update` / `select_one` / `select_table` / `query` /
> `apply_migrations`). … `MockDbClient` shrinks to the record-op surface
> only. … Remove `#[allow(dead_code)]` from `ClaimStore`. …
> `grep -c "forwarding" crates/memory-mcp/src/storage/client.rs` is 0 —
> no capability impl is a one-line re-delegate.

Measured at HEAD (`88fbf61b`, "review fixes: close remaining Card 2 seams"):

- `DbClient` still declares ~28 methods in `storage/client.rs:48-345`,
  including `select_facts_filtered`, `select_facts_ann`,
  `select_edges_filtered`, `select_entity_lookup`, `select_active_facts`,
  `relate_edge`, `count_facts_needing_reembed`, and friends.
- Six of those trait methods carry `Ok(vec![])` / `Ok(0)` *stub defaults*
  (`select_facts_by_triple`, `select_entities_by_ids`,
  `select_edges_for_triple`, `count_facts_needing_reembed`,
  `select_facts_needing_reembed`, `select_episodes_by_content`) — silently
  returning empty for any implementor that forgets them. That is a failure
  mode masquerading as a default.
- The concrete stores are pass-throughs:
  `ContextStoreClient::select_facts_filtered_advanced(ctx, q)` executes
  `self.db.select_facts_filtered_advanced(q)` — the SQL still lives in the
  ~900-line `impl DbClient for SurrealDbClient` block
  (`client.rs:1049-1952`). Two layers of indirection over one seam, zero
  added depth.
- `service/mock_db.rs` is 685 lines and implements ~26 methods by hand;
  43 of the 79 non-test `.unwrap()`/`.expect()` calls in the crate are in
  that file. At least 11 additional test files hand-write full-trait stubs
  (`core.rs` `StartupMigrationDbClient`, `context.rs`
  `FallbackTierDbClient`, etc.), each paying the interface-width tax.
- `claims.rs` (`SurrealClaimStore`) is the reference shape — it owns its
  SQL — and to do so it bypassed `storage/queries.rs` entirely (See 4.1 in
  the 2026-08-01 audit sources). The one store that did it right had to
  fork the SQL layer.

Consequence: every new query still edits ~5 places; every test double still
re-implements the fat interface; the mocks are the largest source of
production-adjacent unwraps; and the trait-with-`Ok(vec![])`-defaults
pattern means a missing storage capability can fail *silently* in
production rather than failing to compile.

## Decision

Move each capability's queries out of `DbClient`/`queries.rs` and into the
owning store struct, then shrink `DbClient` to the ADR-0024 record ops —
completing the migration order steps 2-6 from ADR-0024 that were planned
but not carried through.

1. For each domain in `storage/{context_store,app_store,fact_store,episode_store}.rs`:
   move the SQL construction from `queries.rs` + the execution plumbing from
   `client.rs`'s `impl DbClient for SurrealDbClient` into a `pub(crate)`
   struct method on the store. Delete the corresponding `DbClient` trait
   method and its default variants.
2. Collapse `*_advanced` / non-advanced pairs: pick one signature, migrate
   callers, delete the other and any `build_*_query` re-delegates in
   `queries.rs`.
3. Forbid silent capability defaults: no `Ok(vec![])` / `Ok(0)` bodies on
   the remaining trait surface. If a default is legitimately the core op
   behavior in the absence of a backend feature, the trait method moves to
   the owning store where the absence is visible.
4. Shrink `service/mock_db.rs` to the record-op surface (per ADR-0024 §4);
   per-capability test doubles construct a store over a real in-memory
   engine (SurrealDB memory backend, already used by capability tests via
   `make_context_base`).
5. Remove the remaining `#[allow(dead_code)]` from `storage/claims.rs`
   (trait + `ClaimProjectionSource`/`PersistProjectionRequest` + field) and
   `storage/agent_memory.rs` (`*_str` helpers) — wire or delete per item;
   see the 2026-08-01 hardening plan for the wire/delete table.
6. Move `ContextFactQuery` from `storage/client.rs` next to
   `storage/context_store.rs` (locality).

Order: execute per-domain `context → app → fact → episode/claims-followup`,
as ADR-0024's migration order prescribed, each stage PR-gated.

## Consequences

- A new query lives in exactly one place — the owning store — not five.
- `MockDbClient` implements ~7 methods; capability tests stop paying the
  interface-width tax; the 43-unwrap cluster in `mock_db.rs` disappears
  with the hand-coded response maps.
- Trait-level `Ok(vec![])` defaults cannot recur: capability absence becomes
  a compile error in the owning store.
- Benchmarks: query plans are unchanged (same SQL, moved). Verified via PR
  + Release profile gates at v5 observed values before merge of each step,
  protecting against accidental indexing/plan regressions during the SQL
  relocation.

## Alternatives Considered

### Revert ADR-0024 instead — collapse the stores back onto `DbClient`

Rejected — ADR-0001 established the direction andnothing has changed; the
friction list above is the cost of the *half*-migration. Reverting keeps
the mock-width tax and the five-places-per-query problem forever.

### Keep `DbClient` fat but delete the defaults

Rejected — insufficient: the mock-width and double-indirection costs
remain, and capability tests still pay for the full trait.

## Verification

- Every ADR-0024 Verification item now true:
  - `cargo test --workspace --all-targets --features cli-watch,mcp-apps` passes.
  - PR + Release eval profiles preserve all gates at v5 observed values
    (recall_at_5 = 1.0000, mrr = 0.9924, top_1_hit_rate = 0.9848,
    entity_f1 = 0.75, claim_precision = claim_recall = 1.0000).
  - `wc -l crates/memory-mcp/src/service/mock_db.rs` ≤ 350 and
    `impl DbClient for MockDbClient` matches `DbClient`'s record-op
    surface (target: the core 7 ops).
  - `grep -c 'allow(dead_code)' crates/memory-mcp/src/storage/claims.rs` = 0.
  - No `Ok(vec![])` / `Ok(0)` default method bodies remain on the
    `DbClient` trait (grep for `=> Ok(vec![])` inside the trait block in
    `storage/client.rs` = 0).
