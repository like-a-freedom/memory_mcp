# ADR-0039: One Owner for the Bi-Temporal Close Protocol

## Status

Accepted — 2026-08-19. Implemented under task T2 of the
[architecture deepening round-2 plan](../superpowers/plans/2026-08-19-architecture-deepening.md).

## Context

Bi-temporal close — setting the valid-time end (`t_invalid`) together with the
transaction-time end (`t_invalid_ingested`) — is the single operation that
removes a record from active truth while preserving audit. Before this ADR the
codebase spelled it five different ways in three syntaxes:

1. `capabilities/invalidate.rs` — Rust-side `normalize_dt(now())`, fact table,
   both fields, store-bound full-record write-back; `request.reason` dropped
   (never persisted to `invalidation_reason`).
2. `lifecycle/decay.rs` — Rust-side now, fact table, both fields, raw
   `DbClient::update` with an explicit namespace (an ADR-0024 residual).
3. `conflict_resolver.rs` — server-side `time::now()` twice, triple table,
   SQL supplied by the caller through an `EntityService` pass-through.
4. `storage/claims.rs::retract_fact_and_claims` — server-side `time::now()`,
   fact table: sets `t_invalid` + `invalidation_reason` but **not**
   `t_invalid_ingested` (a latent invariant violation); claim and
   claim_relation rows get only `t_invalid_ingested`, guarded by
   `IS NONE OR IS NULL`.
5. `episode/edges.rs` — edge supersession closes old edges with the **new
   edge's** `t_valid`/`t_ingested`, not with now.

Consequences of the spread: the invariant "whenever `t_invalid` is closed,
`t_invalid_ingested` is closed too" was inexpressible and already violated at
site 4; the invalidation reason was silently lost at site 1; and any change to
the protocol (e.g. a new audit field) required touching five files in three
syntaxes.

## Decision

### One close operation, one owner

The storage layer owns exactly one bi-temporal close recipe per table family.
Service and capability code expresses intent ("close this fact", "retract this
fact and its claims", "supersede this edge") and never composes close SQL or
close field sets itself.

The close operation:

- **Defaults timestamps to server-side `time::now()`** so SurrealDB stores a
  native datetime, not a string that must survive `option<datetime>` coercion
  (the failure mode documented at site 4).
- **Accepts optional caller-supplied timestamps.** Edge supersession closes
  the old edge with the new edge's `t_valid`/`t_ingested` so the audit trail
  records the superseding version's times; this shape must remain expressible.
- **Always closes both fields of the pair.** For fact/edge/triple: `t_invalid`
  and `t_invalid_ingested` together, no exceptions. For claim/claim_relation
  (transaction-time-only tables): `t_invalid_ingested`, guarded so an
  already-closed row is never re-closed.
- **Persists the close reason** where the table carries
  `invalidation_reason` (fact, migration 029). Callers that have a reason
  (manual invalidation, confidence decay, source retraction) pass it; the
  close owner writes it.

### Behavior fixes riding the consolidation

- `retract_fact_and_claims` also sets `t_invalid_ingested` on the fact. This
  is a deliberate behavior fix of the site-4 invariant violation, covered by
  an embedded test.
- `InvalidateCapability` persists `request.reason` into `invalidation_reason`
  instead of dropping it.

### What this does not change

- Retraction, supersession, correction, contradiction, and deprecation remain
  separate operations with separate semantics (CONTEXT.md constraints). This
  ADR unifies only the mechanical close step they share.
- Bi-temporal visibility filters (`BI_TEMPORAL_WHERE`,
  `build_fact_visibility_clause`) are unchanged.
- No schema migration is required: all close fields already exist
  (`__Initial.surql` + migration 006 for fact/edge, 024 for triple, 029 for
  `invalidation_reason`).

## Consequences

### Positive

- The close invariant is expressible and testable in one place.
- A protocol change (new audit field, new timestamp source) touches one
  module.
- Invalidation reasons stop being silently dropped.
- Raw `DbClient` + explicit namespace disappears from `decay.rs`, removing an
  ADR-0024 residual.

### Negative

- Callers with unusual close semantics (edge supersession) must use the
  optional-timestamp parameter instead of free-form SQL — a small loss of
  flexibility in exchange for the invariant.

## Alternatives Considered

### Keep the five spellings, add tests per site

Rejected: tests would pin the divergence instead of removing it, and the
invariant would still have no single owner.

### Rust-side timestamps everywhere

Rejected: server-side `time::now()` avoids string-to-datetime coercion
failures on `option<datetime>` fields and keeps close atomic with the write.
Caller-supplied timestamps remain available for supersession semantics.

### Close via full-record write-back

Rejected: reading, mutating, and writing back the whole record (site 1)
widens the write surface and risks clobbering concurrent field updates. The
close owner issues a targeted `SET` of exactly the close fields.
