# ADR-0038: Use One Active Namespace per Server

## Status

Accepted — 2026-08-12. Implemented incrementally under the
[one-active-namespace implementation plan](../superpowers/plans/2026-08-12-one-active-namespace.md).
The compatibility contract and completion checklist remain the source of truth
for any unfinished hard-break surface.

## Context

Memory MCP currently exposes `scope` and `project` across capture, recall,
lifecycle, CLI, and derived-state pipelines, then maps scope values onto a
configured list of SurrealDB namespaces. This gives a personal server a
multi-tenant-style routing model: users and agents must understand overlapping
partition concepts, every operation carries isolation inputs, and the first
configured namespace can become an implicit default.

The current product is personal and pre-stable. Its owner accepts mixing
personal, corporate, family, and project memories in exchange for a smaller,
more truthful model. Native SurrealDB namespaces remain useful as a coarse
storage and administration boundary, but simultaneous routing among them is not
a demonstrated product requirement.

The simplification is a compatibility break, not permission to weaken evidence,
provenance, trust, or access-policy behavior. It must also account for existing
SCHEMAFULL records, deterministic identities, durable jobs, and databases that
were created under the previous zero-config namespace `org`.

## Decision

### One process, one storage context

One running `memory_mcp` process has exactly one immutable **Active Namespace**,
selected during startup. All ordinary MCP, CLI, lifecycle, app, maintenance, and
worker operations use it implicitly.

Namespace is storage configuration. It is not a per-memory domain field and is
never selected by an ordinary data-plane request.

- With no storage configuration, Memory MCP uses embedded SurrealDB, Active
  Namespace `main`, and database `memory`.
- `SURREALDB_NAMESPACE` may select one namespace for the whole process. An
  absent variable selects `main`; an explicitly empty/whitespace-only value or
  a value containing a comma is a configuration error.
- The removed plural variable `SURREALDB_NAMESPACES` is a hard configuration
  error even when it contains one value. The error tells the operator to choose
  one value for `SURREALDB_NAMESPACE`.
- Memory MCP trims surrounding whitespace and otherwise delegates namespace-name
  validity to SurrealDB instead of defining a second grammar.
- Changing `SURREALDB_NAMESPACE` takes effect only after process restart. It
  switches storage context and never automatically moves, merges, copies, or
  deletes data.
- Startup reports backend, Active Namespace, and database. It neither enumerates
  other namespaces nor claims that data was migrated or that a namespace was
  newly created.

### Initialization and readiness

Startup must establish one fully selected namespace/database context before any
data-plane work or background worker starts.

| Deployment | Required behavior |
|---|---|
| Embedded | Open the configured data directory, select or create the Active Namespace and database, apply pending migrations, and verify schema postconditions. |
| Remote; namespace/database already accessible | Select and probe them without requiring global namespace-list permission, then apply migrations if the authenticated identity has the required database permissions. |
| Remote; namespace/database absent and identity may create them | Use idempotent native SurrealDB definition operations, select/probe the result, then apply migrations. |
| Remote; missing resource or migration with insufficient permission | Fail startup with the resource, operation, and required operator action; do not serve partially initialized storage. |

The implementation must verify these paths against the pinned SurrealDB SDK and
server behavior. `use_ns`/`use_db` selection alone is not treated as proof that a
remote resource was safely created.

The ordinary production seam is `BoundDbClient` plus namespace-bound narrow
stores. The low-level `DbClient` trait remains namespace-parameterized only for
startup/migration infrastructure and explicit compatibility fixtures. That
parameter is not exposed to capabilities, MCP/CLI requests, or ordinary domain
methods, and it must not be treated as a second routing model.

Schema migration and schema-readiness failures are blocking. Embedding readiness
may explicitly degrade semantic retrieval according to its existing policy, but
that state must be singular and namespace-local. Durable derived-work scheduling
must either succeed before workers start or produce a persisted, observable
degraded state; a warning-only lost schedule is not readiness.

Inactive namespaces are not opened, inspected, migrated, or modified. When a
previously inactive namespace is selected in a later process, it receives all
pending append-only migrations before use.

### Scope-free active model

`scope`, partitioning `project`, request-level `namespace`, and
`visibility_scope` leave the active data model and ordinary public interfaces.
Legacy arguments are rejected rather than accepted and ignored.

Existing stored `scope`, `project`, and `visibility_scope` values are **legacy
operational metadata**, not source evidence and not active isolation. They may be
read for upgrade compatibility and retained for audit, but new writes, filters,
routing, deterministic identities, claim slots, and policy decisions must not
depend on them. No destructive bulk rewrite is performed merely to erase these
fields.

Public responses also stop presenting scope/project as an active boundary.
Legacy values remain internal compatibility metadata unless a future,
operator-only audit design explicitly exposes them under a clearly legacy name.

No replacement partition alias such as collection, basket, vault, tenant,
domain, or context is introduced.

### Security and trust classification

One server process represents one authorization domain. This design is not
multi-tenant remote authorization and must not be documented as isolating
personal, corporate, family, or project data inside the Active Namespace.
Operators who require those as separate authorization domains must run separate
process configurations and accept the embedded-store locking constraint, or wait
for a separately designed multi-namespace capability.

Removed as legacy routing/isolation concepts:

- `MemoryScope`;
- `allowed_scopes`, `cross_scope_allow`, and `AccessScopeAllow`;
- request/record scope and partitioning project;
- visibility scope when it only mirrors request scope;
- scope-to-namespace mapping and cross-scope routing policy.

Preserved because they have independent security or provenance meaning:

- invocation origin and trust class;
- quarantine/restricted lifecycle dispositions;
- `policy_tags` and `allowed_tags` filtering;
- caller identity and rate limiting;
- transport, session, content-type, and native event metadata;
- source lineage and explicit source-authority domains;
- source content, immutable facts, bi-temporal validity, and provenance.

Removing routing fields must never broaden source authority, promote quarantined
content, or bypass tag policy.

### Compatibility and identity

The change uses append-only schema evolution and read-old/write-new compatibility.
Historical migration files are immutable. A new expand migration must make
legacy-only required fields optional before scope-free writers are enabled and
must preserve old values.

Derived identities and durable job payloads whose formulas include
scope/project are versioned deliberately. The implementation must provide a
checked-in compatibility matrix and fixtures proving that:

- re-ingesting a pre-upgrade source does not duplicate its episode or facts;
- re-projecting claims does not create semantic v1/v2 copies;
- procedure evidence is not silently split or merged;
- malformed or ambiguous legacy durable work becomes observable rather than
  disappearing;
- namespace-local cursors and failed-ID lists never cross into another namespace.

Source episodes and facts are never rewritten merely to remove legacy fields.
Derived operational metadata may receive an additive/versioned compatibility
projection when needed for safe lookup; old values and provenance remain
available.

Migration execution must inspect every SurrealDB statement result. Because
SurrealDB runs statements as separate transactions by default, implementation
must first prove against the pinned SDK/server whether required DDL and migration
ledger writes can share one supported transaction. If not, every migration step
must be independently idempotent and use explicit applying/applied state plus
schema postconditions. Crash/restart and concurrent remote migrator behavior are
part of the migration contract.

### Pre-stable `org` to `main` break

The previously implemented zero-config path selected namespace `org`. This ADR
intentionally changes fresh unset startup to `main` without namespace discovery,
fallback, or automatic transfer.

Existing `org` data remains intact. To access it after upgrade, the operator must
set:

```dotenv
SURREALDB_NAMESPACE=org
```

Startup must not claim that `org` data moved to `main`. Upgrade fixtures must
prove that explicitly selecting `org` still reads the old data and that selecting
`main` does not copy or delete it.

### Amendments to earlier ADRs

This ADR makes the following narrow amendments; all unrelated decisions remain
in force:

- **ADR-0008:** “domain scope” means the authority domain for a claim schema, not
  removed `MemoryScope`. Removing routing fields never broadens source authority.
- **ADR-0011:** startup applies pending migrations only to the Active Namespace.
  Inactive namespaces are upgraded when later selected. The pre-stable
  `org`→`main` selection break requires explicit `SURREALDB_NAMESPACE=org`; it
  does not imply data loss or automatic transfer.
- **ADR-0012:** backfill progress remains namespace-local, but one process advances
  only its Active Namespace. Fairness across simultaneously configured
  namespaces no longer applies.
- **ADR-0014:** claim-slot identity no longer includes scope or project. Within
  the Active Namespace it consists of access-policy fingerprint, canonical
  subject, compatible claim schema, and comparison key. Qualifier hashes remain
  excluded from slot identity and are evaluated inside reconciliation.
- **ADR-0016 AD-2:** this ADR authorizes exactly this breaking revision of MCP,
  ordinary/hidden CLI, lifecycle, and app parameter schemas while preserving the
  eight MCP tool names and existing command set. AD-4 scope enforcement is
  removed; trust remains channel-derived. AD-8 project daily budgets become a
  process/Active-Namespace daily budget with equivalent bounded-growth intent.

## Consequences

### Positive

- Zero-config startup has one obvious memory space and no prerequisite isolation
  vocabulary.
- Agents cannot create or select arbitrary storage partitions per request.
- The storage seam can bind namespace/database once, making dynamic routing
  physically unavailable to ordinary domain code.
- Native SurrealDB namespace isolation remains available between process
  configurations without inventing a Memory MCP-specific abstraction.
- Keeping the low-level `DbClient` namespace argument at the startup/migration
  boundary avoids a broad, high-risk trait rewrite without restoring request-level
  routing; the bound stores are the production domain seam.
- Adding multi-namespace routing later remains possible through a separate ADR
  and explicit product evidence; it does not require replacing the storage
  engine.

### Negative

- Personal, corporate, family, and project memories are mixed inside one Active
  Namespace.
- Accessing another namespace requires a restart and a configuration change.
- Existing clients, host prompts, hooks, distributed skills, and configurations
  using legacy fields break and must be released compatibly with the server.
- Existing zero-config `org` data is not visible from fresh default `main` until
  the operator explicitly selects `org`.
- Schema and deterministic-identity compatibility make the implementation larger
  than the final public model. This complexity is confined to migration and
  compatibility modules rather than exposed to every caller.
- Selected namespaces receive append-only migrations. “Switching back” preserves
  data but does not roll schema versions backward.

## Alternatives Considered

### Multiple namespaces selected per operation

Rejected for now because it puts routing, validation, guidance, and negative
tests into every data path and lets an AI agent repeatedly choose an isolation
boundary. It may be reconsidered after a demonstrated simultaneous-isolation use
case.

### Preserve scope and project

Rejected because these application concepts duplicate or obscure the native
namespace boundary and retain multi-tenant complexity in a personal-memory
product.

### Rename namespace to collection or basket

Rejected because the adapter already uses native SurrealDB namespaces and no
second storage implementation establishes a useful abstraction. Renaming would
increase vocabulary without reducing coupling.

### Accept and ignore legacy arguments

Rejected because silent compatibility would falsely imply that isolation still
applies. An actionable breaking error is safer.

### Discover `org` or automatically migrate data

Rejected because startup discovery needs broader permissions, makes behavior
non-local, and can conceal where data lives. A configuration edit must not trigger
an implicit bulk copy, merge, deletion, or provenance-changing operation.

### Remove namespaces entirely

Rejected because one native namespace is a useful coarse storage and
administration boundary at negligible zero-config cost. The simplification
removes dynamic routing, not the database's native hierarchy.
