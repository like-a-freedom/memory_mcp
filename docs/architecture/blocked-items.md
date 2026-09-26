# Phase 3/4 blockers: two items require approval the plan does not grant

Recorded 2026-09-26 on `ddd-refactorings` at `7b3ad18`, while
executing the remaining Phase 3/4 items. These are not omissions:
each is blocked by a constraint the plan and spec place above the
item itself, and the plan's own execution contract requires the
blocker to be resolved before the item, not worked around.

## 1. The `http/registry` split is deferred by ADR-0054

Plan Phase 3 asks to "Split registry by policy/table ownership". The
plan also states, at line 73, an ordering constraint that governs
this item:

> Keep existing transaction boundaries; design narrow atomic ports
> **before** replacing the broad registry or splitting any store.

**ADR-0054** (`docs/adr/0054-capability-specific-control-registry-interfaces.md`)
is *superseded* — but its superseding decision kept `RegistryStore`
and set an explicit reconsideration condition:

> Reconsider a narrower split only when at least two production
> consumers need materially different subsets of Registry
> operations, **or** when a concrete test cannot be written without
> implementing unrelated methods.

Neither condition currently holds, and both are false today:

- Every consumer that holds a registry reference holds the same
  omnibus `Arc<dyn RegistryStore>`: `control/account_api.rs`,
  `control/oidc/handlers.rs`, `control/application/oidc_signup.rs`,
  `http/principal/auth.rs`, `http/leases.rs`,
  `http/leases/migration.rs`, `http/registry/provisioning.rs`,
  `bootstrap/integration/{legacy_registry_operations,
  legacy_registry_identity, control_sessions, provisioning,
  auth_method_policy}.rs`, and `cli/commands/admin.rs`.
- No test is blocked by the trait's size: `InMemoryStore` and
  `SurrealRegistryStore` both implement it in full, and the suite
  passes.

The eight capability traits ADR-0054 originally proposed were
**deleted by that ADR's own superseding decision** because "they
had no production consumers or adapter implementations, so they
increased surface area without changing runtime boundaries".
Restoring them now would re-introduce exactly the speculative
abstraction the ADR removed, with no new consumer to justify it.

There is also a hard technical blocker, recorded in ADR-0054
point 5: trait upcasting from `Arc<dyn RegistryStore>` to
`Arc<dyn Capability>` is **not stable Rust** (RFC 3324). The
aggregator that would make the split useful cannot be built from
the existing `Arc<dyn RegistryStore>` that every consumer holds.

### What is needed to unblock

Any one of:

1. The repository records at least two production consumers that
   need materially different subsets, satisfying ADR-0054's own
   reconsideration condition; or
2. Approval to land the capability traits **and** the per-adapter
   aggregator at the construction site, accepting that the
   omnibus `Arc<dyn RegistryStore>` must remain for the consumers
   that still hold it, per ADR-0054 point 7.

The narrow ports the plan asks for *first* are already in place and
wired: `identity::api::{IdentityLinkTransactions,
InvitationSessionPort}`, `operations::api::AccountDeletionPort`, and
`provisioning::api::ApiKeyIssuancePort`, each constructed in
`bootstrap/integration/` and each delegating to the atomic
registry method. That is the ordering the plan required, and it is
done. Replacing the broad registry behind them is the step ADR-0054
defers.

## 2. The outbox move needs the repository's explicit approval

Plan Phase 4 asks to "Separate durable outbox transaction mechanics
from HTTP subscription streaming"; the spec places
`outbox.rs` in `platform/persistence`.

The plan's execution contract says:

> No migration SQL/schema changes, new MCP tools or dependency
> additions are implied. **Obtain the specific repository-required
> approval** if a later implementation step needs any of them;
> documentation changes do not do so.

The spec adds a narrower rule for this file: "moving
generated/migration files still requires the repository's explicit
approval." `http/subscriptions/outbox.rs` is transaction
mechanics over the durable event tables, and moving it into
`platform/persistence` is a repository-level change to a
persistence boundary rather than a module-internal refactor.

This item is otherwise ready to execute; it is blocked only on the
approval, not on a design question.

## What was completed instead

The four items that are *not* blocked were completed and verified:

- `ServiceContext` deleted (`3ccf3b0`), replaced by per-capability
  dependency structs; the tool layer depends on `&MemoryService`
  rather than on an internal container.
- `context_store.rs` split by canonical owner into
  `knowledge_store.rs`, `episode_context_store.rs` and
  `access_log_store.rs` (`2f666e7`), with the method bodies moved
  verbatim.
- `src/shared/` (pure kernel) and `src/platform/` (technical
  mechanisms) established (`7b3ad18`), with database error
  classification and the retry wrapper placed by what they
  actually are rather than by convenience.

The remaining unblocked work is the identity/tenancy implementation
move and the `app_store` / `service/apps` owner split, both of
which are within the already-approved scope.
