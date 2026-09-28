# ADR-0059: Adopt the capability-specific control Registry split

## Status

Accepted — 2026-09-28.

## Supersedes

[ADR-0054](0054-capability-specific-control-registry-interfaces.md), which
recorded the opposite decision (keep the omnibus `RegistryStore` seam) in
2026-09-04.

## Context

ADR-0054 rejected a proposed eight-trait split on the grounds that the
traits had no production consumers or adapter implementations, and set a
reconsideration condition: at least two production consumers need
materially different subsets of `RegistryStore` operations, or a concrete
test cannot be written without implementing unrelated methods.

The modular-monolith migration (see
[ADR-0058](0058-bounded-contexts-modular-monolith.md) and
`docs/superpowers/plans/2026-09-23-ddd-modular-monolith.md`) is what
satisfied that condition, and it did so for the first reason, not the
second. The bounded contexts `tenancy`, `provisioning`, `identity` and
`operations` each own a different canonical table set, and each now
takes a narrow port rather than the omnibus trait. A consumer that
resolves tenancy must not be handed the browser-auth policy writer.

Concretely, `RegistryStore` had grown to 51 methods, and every consumer
holding an `Arc<dyn RegistryStore>` knew about all 51. `AccountBundleTx`
(the one genuine cross-owner atomic write) is exactly the case where a
single transaction spans tables, so it became its own explicit port
rather than a reason to keep the omnibus trait.

ADR-0054 also asked for a split *by capability* — auth, account read,
API-key write, and so on — driven by caller need. The implemented split
is by **canonical table owner** instead. That is a deliberate
divergence: the eight owners map onto the conceptual domains, so a
port's name states whose data it touches rather than which call site
wanted it. The effect ADR-0054 was after — no consumer is handed
operations for a concern it does not own — is unchanged.

## Decision

`RegistryStore` is a bundle supertrait carrying only `ping`, and the 51
methods are split across eight owner traits, each `#[async_trait]`:

| Trait | Methods |
|-------|---------|
| `AccountStore` | 7 |
| `TenantStore` | 11 |
| `SessionStore` | 8 |
| `ApiKeyStore` | 7 |
| `UsageStore` | 6 |
| `ProvisioningStore` | 5 |
| `IdentityStore` | 4 |
| `BrowserPolicyStore` | 2 |

Cross-owner atomicity that does not fit a single owner is expressed by
the separate transactional ports in `platform/persistence/registry/`
(`AccountDeletionTx`, `IdentityLinkTx`, `AccountBundleTx`,
`IdentityLookup`), which compose private SQL fragments under one
transaction rather than widening a read port.

`#[async_trait]` on the owner traits is load-bearing, not decorative.
Dropping it from a supertrait still compiles the declarations and fails
only where a trait object is constructed, so `every_owner_trait_is_dyn_compatible`
and `both_stores_satisfy_every_owner_trait_and_the_bundle` in
`http/registry/storage.rs` pin the invariant. Removing the attribute from
`AccountStore` alone produces 606 errors citing `not dyn compatible`,
which is what the guard was written to catch.

## Consequences

- A context can no longer be handed a port that writes another owner's
  tables. The graph becomes checkable per context rather than per crate.
- `InMemoryStore` and `SurrealStore` are each eight owner impls plus the
  bundle, so adding a method to one owner no longer forces every
  unrelated test double to implement it.
- ADR-0054's text is left in place as the historical record of the
  decision that was reversed, including the reconsideration condition
  this ADR answers.
- The `legacy:` dispositions that existed only because the registry split was
  deferred are gone: the split is implemented, and the migration manifest that
  recorded them has been retired.
