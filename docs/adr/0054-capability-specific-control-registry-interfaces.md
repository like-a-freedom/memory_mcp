# ADR-0054: Capability-specific control Registry interfaces

## Status

Accepted — 2026-09-04, architecture audit remediation.

## Context

`RegistryStore` is a single `async_trait` that exposes every method the
control-plane and provisioning layer needs. Consumers that handle a
single concern (auth, account read, API-key write, OIDC state, session
refresh, deletion challenge) all hold the same `Arc<dyn RegistryStore>`
and therefore all know about every other concern. The surface also
encodes the dependency direction in the wrong place: a future caller
that only needs `AccountIdentityStore` cannot ask for it, and a test
that wants a `NoopRegistryStore` cannot satisfy the trait without
implementing 40+ methods.

ADR-0044 already requires that stores expose named methods; ADR-0052
refines the HTTP profile's internal dependency shape. Both are
undermined by the omnibus trait because the trait *is* the seam; the
underlying store is fine, but its interface is the public shape that
every consumer agrees on.

## Decision

1. Split `RegistryStore` into eight crate-private capability traits:
   - `RegistryHealth` (`ping`);
   - `AccountIdentityStore` (account read, account+tenant bundle write,
     OIDC identity read/write);
   - `CredentialStore` (API key CRUD and the atomic
     `create_api_key_if_below_limit`);
   - `TenantProvisioningStore` (tenant write, fenced CAS, lease claim
     and heartbeat, ready/deleting/due/reconcile lists,
     `append_provisioning_event`);
   - `PlanUsageStore` (plan load, plan ensure, usage load,
     `reserve_ingest_usage`, `reconcile_usage`);
   - `OidcRequestStore` (gated on `control-plane`; sealed payload store
     and atomic take);
   - `ControlSessionStore` (gated on `control-plane`; session store,
     find, touch, delete);
   - `AccountDeletionStore` (gated on `control-plane`; deletion
     challenge create/consume, fenced
     `begin_account_deletion` / `begin_operator_deletion` /
     `finalize_account_deletion`).
2. Required operations have no default implementation. The four
   atomic operations (`create_account_bundle`,
   `begin_account_deletion`, `begin_operator_deletion`,
   `finalize_account_deletion`) keep their single-method atomicity
   on the capability trait; they are not reconstructed at the
   application layer.
3. The capability traits are `pub(crate)` and live next to
   `RegistryStore`. Consumers that want one capability ask for one
   capability.
4. `RegistryStores` aggregates one clone of `Arc<SurrealRegistryStore>`
   (or `Arc<InMemoryStore>`) into eight `Arc<dyn ...>` fields.
   `RegistryHandle` exposes narrow accessors; the aggregation struct
   itself stays crate-private.
5. **Stable Rust constraint (RFC 3324)**: trait upcasting from
   `Arc<dyn RegistryStore>` to `Arc<dyn Capability>` is not yet
   stable. The aggregator cannot be built by re-typing an
   existing `Arc<dyn RegistryStore>`; the conversion has to
   happen at the construction site where the concrete
   `Arc<SurrealRegistryStore>` (or `Arc<InMemoryStore>`) is
   first wrapped. This is consistent with the plan's
   "Construct all fields from clones of one `Arc<...>`" — the
   `Arc<...>` is the concrete adapter, not the
   `Arc<dyn RegistryStore>`. The `RegistryHandle::store_clone`
   accessor therefore stays on `Arc<dyn RegistryStore>` until
   upstream trait upcasting lands; the capability views are
   produced by the per-adapter constructors and held alongside
   the existing store reference.
6. The aggregator is therefore **deferred** in this milestone
   to the per-adapter path. The capability traits, the
   per-adapter `impl`s, and the compile-time assertions are
   the durable deliverable; the aggregator lands in a
   follow-up that targets stable upcasting (or a manual
   `coerce_unsized`-equivalent helper).
5. `ping` remains part of `RegistryHealth` for now so readiness
   semantics are unchanged while the interface splits. The
   `Unavailable` default helper is removed once the only default
   implementations that depended on it have moved to the capability
   impls.
6. Compile-time assertions (`assert_registry_capabilities`) live in
   the storage test module. They prove that both production
   adapters implement every enabled capability, so a future
   method that lands in `RegistryStore` cannot silently fall
   through to a stale default in one adapter but not the other.
7. `RegistryStore` itself stays in this milestone. Removing it is
   a consumer-only change (Task 10) once the capability impls
   are wired and the aggregator is in place.

## Consequences

- A consumer that only needs account identity, plan usage, or
  provisioning fences can be parameterized over the matching
  trait instead of the omnibus store. This makes the test
  surface honest: a unit test for plan usage does not have to
  invent an account API.
- The eight traits are a soft contract; the hard contract is the
  implementation. The compile-time assertions are the bridge.
- One concrete allocation still backs all eight views, so there is
  no extra storage cost and no second source of truth.
- Crate-private visibility keeps the capability split an
  internal-seam refinement, not a public-API expansion.
- Atomic operations remain owned by the storage implementation;
  callers cannot accidentally re-implement a multi-row write as
  sequential single-row writes.

## Relationships

This decision refines ADR-0044 (named methods only) and ADR-0052
(Streamable HTTP SaaS profile internal dependency shape). It does
not change identity, tenancy, provisioning, quota, session, or
deletion semantics; it changes the shape of the interface that
exposes them.
