# ADR-0024: Complete the DbClient Capability Narrowing

> Status: Accepted (2026-07-30)
> Completes ADR-0001's declared direction: *"storage capability seams can be narrowed further."*

## Context

ADR-0001 introduced capability stores (`ContextStore`, `ContextAccessLog`,
`AppStore`) to narrow the interface each consumer depends on. That structure
exists, but the adapters forward every call to the same concrete `DbClient`
trait (~40 methods in `storage/client.rs`), and the service layer still consumes
the universal `DbClient` directly for the bulk of its work.

Consequences visible today:

- `MockDbClient` in `service/mock_db.rs` implements all ~40 `DbClient`
  methods; 13 test files cross that seam per test.
- Adding one query to `DbClient` means editing five places (the trait,
  `SurrealDbClient`, `MockDbClient`, the narrow capability trait, and its
  forwarding impl).
- `storage/claims.rs` already demonstrates the intended end-state —
  `SurrealClaimStore` is a concrete struct owning its queries — but the
  pattern has not been generalized.
- `ClaimStore` carries `#[allow(dead_code)]` because the narrowing is
  incomplete.

## Decision

Migrate service-layer consumers onto narrow, owned capability stores and
shrink `DbClient` to connection machinery plus core record operations
(`create` / `update` / `select_one` / `select_table` / `query` /
`apply_migrations`).

1. Keep `ContextStore`, `ContextAccessLog`, `AppStore` as the *traits* callers
   depend on.
2. Replace their trait-over-trait forwarding with concrete structs that own
   their queries directly over `DbEngine` (the `SurrealClaimStore` pattern).
3. `DbClient` loses its capability-specific methods; those move onto the
   owning struct.
4. `MockDbClient` shrinks to the record-op surface only; capability tests
   construct the narrow store over an in-memory engine instead of
   re-implementing 40 trait methods.
5. Remove `#[allow(dead_code)]` from `ClaimStore`.

`FactService`, `EmbeddingService`, and lifecycle workers move onto their
narrow seam; the public tool interface is unchanged.

## End-state shape

The target after this refactor is:

- `storage/client.rs` keeps only connection setup + core record ops:
  `connect`, `connect_in_memory*`, `create`, `update`, `select_one`, `select_table`, `query`, `apply_migrations`, plus `DbEngine` and its helpers.
- `storage/claims.rs` (already correct), `storage/agent_memory.rs`, `storage/procedures.rs` — narrow struct stores wrapping `Arc<dyn DbClient>`; unchanged.
- New narrow structs in `storage/` for the remaining domains, each owning its queries:
  - `storage/context_store.rs` — `FactQueryStore`, `EpisodeQueryStore`, `CommunityQueryStore`, `EntityQueryStore`, `AccessLogStore`;
  - `storage/app_store.rs` — `AppSessionStore`, `IngestionReviewStore`;
  - `storage/fact_store.rs` — fact CRUD + reembed counters.
- `service/context/*.rs`, `service/apps/*.rs`, `service/fact.rs` consume their domain's narrow struct(s) — never `DbClient`.
- `service/mock_db.rs` shrinks from implementing ~40 trait methods to the core record ops only; per-capability test doubles sit beside their capability tests.
- `ClaimStore` in `storage/claims.rs` drops `#[allow(dead_code)]`.

## Consequences

- A new query lives in exactly one place (the owning struct), not ~5.
- Mocks shrink; test setup through the capability seam becomes honest.
- The capability modules gain real implementations instead of pass-throughs.
- `DbClient` stops being the universal interface every test must satisfy.
- This is the completion of the pre-existing ADR-0001 direction — not a
  change of direction.

## Alternatives Considered

### Keep `DbClient` as the universal interface, deprecate narrow stores

Rejected — ADR-0001 already rejected this; nothing has changed since that
decision to warrant reopening. The narrow stores exist; the work is to make
them real, not to delete them.

### Extract `DbClient` into separate runtime crates per capability

Rejected — premature module split; same crate, narrower traits, is sufficient
locality.

## Migration order (what to touch first)

1. Extract `core.rs`'s app-graph BFS (`find_intro_chain`, `bfs_path`, `build_intro_chain_from_start`) to `service/apps/graph.rs` next to existing graph expansion.
2. Split `service/mock_db.rs` down to a plain in-memory record store exposing only the core record ops.
3. Migrate `service/context/*.rs` off `ContextStore` onto a `storage/context_store.rs` of concrete structs; delete the forwarding trait impls in `storage/client.rs`.
4. Migrate `service/apps/*.rs` off `AppStore` onto a `storage/app_store.rs`; delete the forwarding impls.
5. Migrate `service/fact.rs` off direct `DbClient` onto a `storage/fact_store.rs`; delete the forwarding impls.
6. After all consumers are on narrow structs, drop `ContextStore`/`AppStore` traits and move`DbClient`'s capability methods into their owning structs. Re-export only the core record ops from `storage/client.rs`.

Each step is independently verifiable and PR-gated.

## Verification

- `cargo test --workspace --all-targets --features cli-watch,mcp-apps` passes.
- PR + Release eval profiles preserve all 16 gates at v5 observed values
  (recall_at_5 = 1.0000, mrr = 0.9924, top_1_hit_rate = 0.9848, entity_f1 =
  0.75, claim_precision = claim_recall = 1.0000).
- `MockDbClient` implements a surface no larger than `DbClient`'s core record
  ops.
- `grep -c "forwarding" crates/memory-mcp/src/storage/client.rs` is 0 —
  no capability impl is a one-line re-delegate.
