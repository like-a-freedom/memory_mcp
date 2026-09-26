# Decision: typed record accessors and the cross-owner provenance read

Status: accepted. The typed accessors and the `invalidate` guard are
implemented (see "Status of implementation" below); the cross-owner
`select_episodes_via_entity` read is deliberately deferred.

## Context

`AppStoreClient` is being split by canonical data owner. Three
methods remain that are not owner-correct:

- `select_record` / `find_record_by_id` — table-agnostic reads whose
  target table is parsed out of the record-id string.
- `ContextStoreClient::select_episodes_via_entity` — a single query
  joining `episode`, `fact` and `edge`.

While checking the first of these we found a defect that is not a
refactoring concern.

## Finding: `invalidate` can close a record that is not a fact

`InvalidateParams.fact_id` is caller-supplied. The tool documents the
canonical `fact:<id>` form, but nothing enforces it. The call path is:

    tools::invalidate
      -> InvalidateCapability::invalidate
        -> memory::api::invalidate_fact
          -> InvalidationPort::find_record   (generic: table from id string)
          -> InvalidationPort::close_record  (generic: table from id string)

Both the existence check and the close derive their table from the
caller's string, so an id naming any other existing record passes the
existence check and is then written to by the close. The typed-phrase
and rate-limit guards authorize the operation, not its target, so they
do not prevent this.

Severity is bounded but real. `build_close_query` emits
`SET t_invalid, t_invalid_ingested, invalidation_reason` against
whatever table the id names. Only `fact`, `edge` and `triple` define
`t_invalid`; `episode`, `entity`, `community` and the rest do not. So
the outcome is a schema violation on those tables rather than a silent
corruption, and the caller receives a storage error. For `fact`-like
targets it is a real write.

Consequence: the documented contract of the tool ("invalidate a fact")
is not the enforced contract. In a system where facts are never
deleted and every close is an audit-trail event, a close primitive
whose target is a caller-supplied string is the wrong shape.

This predates the current work. It is not introduced by it, and it is
found while splitting the store — which is the last moment at which
the generic accessor is still load-bearing and therefore still worth
removing.

## Decisions

### 1. Typed accessors replace the generic accessor; the generic accessor is deleted

Add, on the owning stores:

- `EpisodeStoreClient::select_episode(id)` — refuses a non-`episode:`
  id before any query.
- `FactStoreClient::select_fact(id)` — refuses a non-`fact:` id before
  any query.

`ServiceContext::find_episode_record` and `find_fact_record` become thin
delegations to those. `AppStoreClient::select_record` and
`find_record_by_id` are deleted.

The generic accessor is **not** moved to a platform-private home. A
private generic accessor is a god-store with the door unmarked: the
next contributor reopens it precisely because it is right there. Table
derivation from an id string is a primitive that already has a correct
home — `BoundDbClient` — and it stays there. Nothing above platform
should be able to name a table through a record id.

### 2. `invalidate` validates its target at the application boundary

The memory use case checks the prefix before the port is touched, so
the refusal happens before the existence lookup and is independent of
which store answers it. This is defense in depth with decision 1, not
a substitute: even with a correct store, the use case should state
which record kind it is willing to invalidate.

The error is a `Validation` error naming the id and the expected kind.
The existing "fact_id not found" message is unchanged for well-formed
ids, so current callers see no difference.

### 3. `select_episodes_via_entity` is deferred, and is not a fourth store

It joins three owners' tables, so it is a read model rather than any
one context's store. Two candidate shapes:

- **(a)** A composition-layer read that calls the three owner-approved
  reads and joins in memory.
- **(b)** A dedicated read-model store holding the one query.

Choose (a) as the target shape; it keeps every table behind its owner
and adds no cross-owner store. But (a) risks an N+1 on a hot
provenance path, and ADR-0058 / the spec require benchmarking query
decomposition before splitting a cross-owner optimized read. So:

- Defer. It is the one item in this store split that is not a
  capability reduction but a performance trade, and it must be decided
  with measurements rather than in the same pass as a security fix.
- Record it in the manifest as an open Phase 6 item with the
  measurement it requires, so it is not lost.

## Rationale against the principles

- **DADP / 12-factor**: configuration and credentials are resolved at
  composition time; the store that a use case talks to is wiring, not
  business policy. Decision 1 keeps record-kind policy in the
  application layer (a use-case invariant) and leaves only mechanism in
  platform.
- **SOLID / ISP**: `AppStoreClient` currently violates the interface
  segregation principle — consumers of the knowledge reads are forced
  to depend on a surface that also reaches `episode` and `event_log`.
  Narrow stores are the fix, and this is the last big offender.
- **DRY**: `select_edge_neighbors` exists on three stores today
  (`AppStoreClient`, `ContextStoreClient`, `EpisodeStoreClient`). The
  split is the moment to collapse those, but that is a mechanical
  follow-up to decision 1, not part of it.
- **YAGNI**: no new abstraction is introduced. Two named methods
  replace one generic one; the interface surface shrinks.
- **DDD**: the aggregate boundary is the record kind. A repository that
  can load and mutate any aggregate is not a repository.

## Consequences

- The `invalidate` behaviour change is a **behaviour change outside the
  freeze** for a caller who was relying on invalidating a non-fact
  record. That reliance is undocumented and unsupported; the
  migration note in the changelog must say so explicitly.
- The `find_record_by_id` validation tests in `service_context.rs` and
  `service/core.rs` assert on the generic accessor's validation, so they
  move to the typed accessors' tests. Their *observable* assertions —
  bare hex rejected, empty id rejected, well-formed accepted — are
  preserved.
- `invalidate` with a well-formed `fact:<id>` that does not exist still
  returns "fact_id not found".

## Status of implementation

- [x] `invalidate` target-kind and record-id validation, in
  `memory::api::invalidate_fact`. Pinned by
  `tests/invalidation_target_kind.rs` (4 tests): a well-formed
  `fact:` id still closes exactly once; `episode:`, `edge:`,
  `triple:`, `entity:`, `community:`, `event_log:` and `query_log:`
  ids are refused before the close owner is reached; a bare id is
  refused as malformed.
- [x] Typed accessors `EpisodeStoreClient::select_episode` and
  `FactStoreClient::select_fact`, both routed through
  `storage::helpers::require_record_kind`, and deletion of
  `AppStoreClient::select_record` / `find_record_by_id` and
  `ServiceContext::find_record_by_id`. Pinned by
  `tests/typed_record_accessors.rs` (4 tests).

  Two consumers relied on the generic accessor tolerating a
  non-matching id, and were corrected rather than the accessor
  being weakened:

  - `ExplanationService` used to read any record id as an
    episode/fact. It now treats a wrong-kind id as "no
    provenance for this item", which is what it already did for an
    id that no longer exists. The classification is the named
    `provenance_lookup_error`, and
    `explanation::tests::a_wrong_kind_lookup_is_absorbed_but_a_storage_failure_is_not`
    pins that a `Storage` or `ConfigInvalid` failure still
    propagates, so the absorption cannot widen into a silent
    degradation. End-to-end: `tools_e2e::test_mcp_explain_mixed_array`.
  - `assemble_context` access tracking was calling
    `record_fact_access` for synthesised view items
    (`episode_fallback:`, `facet:`, `map:`), which produced a
    validation refusal logged on every such request. It now
    tracks only `fact:` ids, decided by the named predicate
    `context::is_fact_access_trackable`. Pinned by
    `context::tests::only_real_fact_ids_are_access_trackable`, which
    fails if the predicate is inverted, and by
    `service_integration::test_service_assemble_context_does_not_track_access_for_synthetic_view_ids`.

    **Correction.** This checklist previously cited
    `test_service_assemble_context_records_fact_access_heat*` as the
    pin for this change. That citation was false: those tests only
    exercise the pass-through arm, and deleting the guard failed
    nothing in the suite. The predicate test above was added
    specifically to close that gap.
- [ ] `select_episodes_via_entity` — deferred, see decision 3.

## Behaviour change to record on release

`invalidate` with a record id that is not `fact:<id>` now returns a
validation error instead of attempting a close. Callers relying on
invalidation of `edge:` or `triple:` records are affected; that path
was undocumented and unsupported — the tool's documented contract is
"invalidate a fact" — but the release note must say so, because
`edge` and `triple` do carry `t_invalid` and the old behaviour was a
successful write, not an error.

## Verification

- RED test first: a public `invalidate` seam that passes a
  non-`fact:` id and asserts the port is never reached, and one that
  asserts a well-formed id still closes exactly once.
- Existing suites that pin the old behaviour:
  `service::core` and `service_context` record-id validation,
  `tools_invalidate_validates_t_invalid`, `tools_shared`,
  `service_acceptance::test_invalidate_and_explain`,
  `service_integration::test_service_fact_invalidation`.
- Full local gate: fmt, clippy `-D warnings`, both workspace suites,
  boundary harness, packaging.
