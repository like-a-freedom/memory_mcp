# ADR-0066: Business policy lives in the owning context, not the store adapter

- Status: accepted
- Date: 2026-10-01
- Related: ADR-0054, ADR-0058, ADR-0059, ADR-0042

## Context

Four business policies sat in `src/http/` — the transport adapter — rather
than in the bounded context that owns the concept:

  * the ingest quota, in `http/registry/plan.rs`, deciding whether a tenant
    may ingest and naming the reason when it may not;
  * the Tenant status transition table, in `http/registry/provisioning.rs`;
  * the app-session concurrency ceiling, in
    `http/middleware/acquire_runtime.rs`;
  * the byte-drift threshold the usage reconciler applies, also in
    `plan.rs`.

None of them is about HTTP. `plan.rs` exists in `http/registry/` because that
is where the store adapters were when the quota was first written, and it was
never moved, because nothing requires a policy to sit next to its caller.

Two things followed from the placement.

**The rule was unreachable from anywhere else.** A use case that wanted to
admit an ingest had to import a module under `http/`, which is the direction
ADR-0058 forbids for a bounded context. So the policy was available to exactly
one set of callers and to no other set, and the second set that needed it
would either import across the seam or reimplement it.

**The same rule was expressed twice, in two languages.** `surreal_store.rs`
enforces the quota in a SQL `WHERE` clause, and calls `enforce_ingest`
afterwards on a discarded local copy to produce the denial reason. The SQL is
the admission gate; the Rust function names the refusal. Two expressions of
one rule, in two languages, with no check that they agree — and a tenant that
gets refused for a reason other than the one that stopped it is a `retry_after_secs`
that lies.

## Decision

Policy lives in the owning context's `api.rs`, as pure functions over state
the caller supplies. The store adapter supplies state and performs the write.

Specifically:

  * `operations::quota::enforce_ingest` and `operations::quota::usage_drift_report`
    own the quota. `http/registry/plan.rs` keeps `scheduler_job` and
    `reconcile_all`, which are HTTP scheduler wiring rather than policy.
  * `provisioning::api::can_transition` owns the Tenant transition table.
  * The quota predicate remains in SQL in `surreal_store.rs`, and that is not
    a contradiction of this record — see Consequences.

## Consequences

- The store adapters can no longer be self-contained. `InMemoryStore` calls
  `enforce_ingest` while holding its lock, and `SurrealRegistryStore` calls it
  on a discarded copy. Both are one line longer than before and neither owns
  the rule any more.
- **The quota predicate now exists in two places on purpose**, and the two
  must agree. The SQL `WHERE` at `surreal_store.rs` is the admission gate
  because it has to be: two requests racing past a Rust-level check both
  rely on it, and a check-then-write would let both through. The Rust function
  exists to produce a typed refusal reason, which SQL cannot. This record
  requires a test that pins the two together, and a drift between them is a
  denial with the wrong `retry_after_secs` rather than a wrong admission —
  the failure is a wrong error message, not a wrong answer, which is why the
  split is defensible at all.
- `reconcile_usage` is renamed `usage_drift_report`. The `UsageStore` trait
  method already owns the bare name, and two things in one module called
  `reconcile_usage` is a trap: a reader importing both would assume they are
  the same operation, and they are not — one reads two counts and reports
  drift, the other writes a repaired counter.
- `Plan` is renamed `QuotaPlan`. It collides with `models::registry::Plan`, and
  the collision forces a `From` import at every call site that constructs one.
- A third reason string is now reachable from the context function and not
  from the durable store, which the pinning test reports rather than hides.

## Alternatives considered

1. **Keep the policy in the store trait**, so each adapter applies its own.
   Rejected. It makes every new adapter re-implement the rule, and a rule
   re-implemented per adapter is a rule that eventually differs per adapter —
   which is the failure this record exists to close.

2. **Express the whole admission check in Rust** and drop the SQL predicate.
   Rejected. The counter increment must be conditional on the check, and in
   two statements that is a race. The durable gate has to be in the statement.

3. **Generate the SQL from the Rust policy.** Tempting, and rejected as a
   larger change than the problem warrants. It would remove the duplication
   rather than pin it, but it means building query text from a policy
   description, and the generated statement is no longer readable in a
   profiler. Worth revisiting if the two ever drift in a way a test cannot
   express — which is a real possibility and is why the pinning test exists.