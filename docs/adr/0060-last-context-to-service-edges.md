# ADR-0060: The last context-to-service edges, and what replaced them

- Status: accepted
- Date: 2026-09-26
- Supersedes: nothing
- Related: ADR-0058, ADR-0038

## Context

The dependency-boundary guard forbids a bounded context from reaching
`crate::service`, `crate::control` or `crate::http`. For most of the
refactor the fix was mechanical: move the file into its owning context
and inject the narrow port it needed. A handful of edges resisted that,
because the thing on the other side was not adapter code at all — it was
a kernel primitive that had been filed under `service` by convenience.

The graph read paths were the hard case. `service/apps/graph.rs` held the
two map-view reads, three write paths, and four shared types in one
1,417-line file. Two scripted splits failed because the shared types were
cut in half, leaving both sides uncompilable. Splitting it needed a
per-symbol classification of which side used what, then a hand edit.

## Decision

Classify by ownership, not by location, and let the awkward cases name
themselves.

1. **`GraphContext` and `HubEntity` and the hub-scan ceilings** moved to
   `memory/retrieval/graph_reads.rs` with the reads that use them. The
   trait is a read contract naming no adapter, so it is the port the
   memory context implements.

2. **`GraphCommunity`, `edge_identity` and `graph_community_from_value`**
   are pure parsers over knowledge's own rows. They moved to the reads,
   and the write paths import them back. Placing them by "who calls
   them" would have been wrong: they parse rows, and the rows are
   knowledge's.

3. **`GraphTraversalBudget`** is used by both halves and names no context,
   so it went to `platform/traversal_budget.rs`. Neither the read nor the
   write side owns a budget that bounds both.

4. **`CacheKey` and `CacheView`** moved to `platform/context_cache_key.rs`.
   The embedding context keys its query-embedding cache on the same
   shape, so the key could not live in the memory context.

5. **`InvalidateContextCache`** is an extension trait over the cache
   handle, defined in the platform layer. The embedding service writes a
   vector and then clears the cache; expressing that as a call into
   `memory::context_cache` made the dependency point the wrong way. The
   operation belongs to the cache, not to the context that fills it.

6. **`EmbeddingService` and its runtime** moved wholesale to
   `src/embedding/`, which the manifest had already recorded as the
   destination.

## The guard rule that was wrong

The rule banning a context module from importing its own `infra` fired on
`embedding/service.rs`, which constructs `FactVectorAdapter` and hands it
to `api::update_canonical_vector`. That is the intended shape: a context's
composition root wires its own adapter, exactly as `service/reembed.rs`
and `service/embedding_recovery.rs` already did. The rule now exempts
`{context}/service.rs`, because the distinguishing fact is that a
composition root is not a domain or application module.

## Consequences

- No bounded context contains a legacy row; the violation class is closed.
- 13/13 boundary guards pass, 2337 tests green, clippy and fmt clean
  across the feature matrix including `--no-default-features`.
- 114 manifest rows remain `legacy:`, all of them adapter *layout*
  work in `cli`, `http`, `control`, `bootstrap` and `service` — the
  direction those files point is legal, so they are refactoring
  candidates, not violations.
