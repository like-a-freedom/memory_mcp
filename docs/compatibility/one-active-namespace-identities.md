# One-Active-Namespace Identity Compatibility

> **Status:** implemented compatibility contract; the checked-in tests and
> [2026-08-12 implementation plan](../superpowers/plans/2026-08-12-one-active-namespace.md)
> track any remaining release-gate work. The ADR records the accepted product
> decision and the hard-break boundaries.
>
> This document is the compatibility slice of ADR-0038. It defines how
> pre-change identity inputs are looked up after the server becomes scope-free.
> It does not authorize source rewriting, data transfer, or destructive
> migration.

## 1. Terms and invariants

### Active Namespace is a storage boundary

A running server has one immutable **Active Namespace**, selected at startup.
The namespace is implicit in the bound database context; it is not a
per-record partition field and is never selected by an ordinary data-plane
request.

Consequently:

- a v2 identity is evaluated only inside the selected namespace/database;
- an identity lookup never searches, lists, or joins inactive namespaces;
- changing the configured namespace requires a clean process restart;
- switching storage context does not imply that any record was moved, copied,
  merged, or deleted.

The namespace boundary must not be confused with source authority, trust,
quarantine, policy tags, caller identity, or bi-temporal validity. Removing
legacy scope/project inputs must not weaken any of those controls.

### Compatibility outcomes

Every legacy durable row that is read by a compatibility seam has one explicit
outcome:

| Outcome | Meaning | Mutation allowed? |
|---|---|---|
| `native_v2` | The row already has the v2 identity/version and is structurally valid. | Normal idempotent reuse/update allowed. |
| `legacy_unambiguous` | Exactly one valid v1 row can be identified without removing a boundary. | Reuse the existing ID and append compatible derived metadata if required. |
| `legacy_ambiguous` | More than one valid v1 row matches the scope-free lookup, or candidates conflict. | No automatic merge, split, or new replacement row. Surface the ambiguity. |
| `malformed` | Required identity or durable-work fields are absent, invalid, or inconsistent. | Do not reinterpret as an empty/default row; make the failure observable. |

Ambiguity and malformed data are non-mutating compatibility failures. An error
must include the record IDs, a stable reason code, and operator guidance, but
must not include source contents. A source row remains retrievable even when a
derived row cannot be upgraded automatically.

## 2. Episode identity: v1 to v2

### Identity formulas

| Version | Identity inputs | Namespace behavior |
|---|---|---|
| v1 | `source_type` + `source_id` + `t_ref` + legacy `scope` | The legacy scope was part of the episode identity/routing boundary. |
| v2 | `source_type` + `source_id` + `t_ref` | The Active Namespace is implicit in the bound database; scope/project/visibility are not identity inputs. |

The tuple is the stable source identity. Compatibility code must compare the
canonical identity inputs at the owning storage seam; it must not treat an
unlabelled v1/v2 hash string as proof of equality.

### Lookup before create

When ingesting a source under v2:

1. Check for a valid native v2 episode in the Active Namespace.
2. If no native v2 episode exists, look up legacy episodes using the stable
   source tuple, without requiring a caller to provide a legacy scope.
3. If exactly one valid legacy episode matches, classify it as
   `legacy_unambiguous` and reuse its existing episode ID.
4. If several legacy scopes produce matching episodes, classify the result as
   `legacy_ambiguous`, return an actionable non-mutating error, and create
   nothing.
5. If no valid episode matches, create one v2 episode in the Active Namespace.
6. If a candidate is malformed, retain it for audit/diagnostics and do not
   silently turn it into a new empty episode.

A conflicting native-v2/legacy duplicate is an integrity condition, not a
reason to silently select, merge, or delete a candidate. The compatibility
seam must make the conflict observable and preserve the existing records.

### Episode and fact preservation

Reusing a unique legacy episode is the compatibility path that preserves its
facts and source lineage. It must not create a second semantic episode merely
because the v2 formula omits a legacy boundary.

Fact identity remains tied to the source episode and fact payload. Therefore:

- reusing the legacy episode preserves its existing fact IDs;
- extraction against an ambiguous episode is blocked;
- a repeat extraction must not create a second semantic fact for the same
  source episode/payload;
- source episodes and facts are not rewritten merely to remove stored
  `scope`, `project`, or `visibility_scope` metadata.

The legacy metadata may remain available through an internal compatibility or
audit view. It is not active isolation and is not a new public domain field.

## 3. Claim identity and reconciliation compatibility

### Claim ID

The claim ID formula is unchanged:

```text
claim schema + extractor + source fact ID + canonical claim payload
```

A reused fact ID therefore preserves the corresponding claim ID. Claim
projection must check the source fact, extractor, and canonical payload before
inserting. A v2 projection must not create a semantic v1/v2 duplicate merely
because surrounding policy or slot formulas changed.

### Policy fingerprint

Policy fingerprints are versioned because the legacy routing fields are being
removed:

| Version | Formula | Use |
|---|---|---|
| v1 | `scope` + `project` + sorted policy tags | Legacy lookup/audit only. |
| v2 | Sorted policy tags only | New reconciliation and active policy decisions. |

Store v1 and v2 fingerprints separately when both are needed for compatibility.
Never map legacy `scope` or `project` into synthetic policy tags. A v1 hash and
a v2 hash must not be compared as equal merely because their byte strings look
similar.

### Claim slot identity

| Version | Identity inputs |
|---|---|
| v1 | namespace + scope + project + schema version + subject + comparison key + policy fingerprint |
| v2 | compatible schema + subject + comparison key + v2 tag-policy fingerprint; Active Namespace is implicit and is not hashed into the slot fingerprint |

Qualifier hashes are excluded from both v1 and v2 candidate-slot identity;
qualifiers are evaluated during reconciliation. Legacy claims with colliding
scope/project-free identity may coexist. They are not automatically
invalidated, merged, or assigned to a guessed v2 slot.

A compatibility reader may lazily project a v2 slot for a legacy claim only
when the mapping is unambiguous. If the legacy policy context cannot be
reconstructed safely, retain the legacy claim and make the compatibility state
observable rather than manufacturing a policy boundary.

### Relations and jobs

Claim relations retain their unordered claim-ID-pair identity. If the policy
context changes, append/version the reconciliation decision or evaluator
context; do not fork a parallel semantic relation solely because the slot
formula changed.

Claim and backfill work is namespace-local. A legacy job may be converted or
resumed only when its source/claim identity and evaluator/extractor context are
unambiguous. Duplicate pending v1/v2 work is collapsed by source plus work kind
plus evaluator/extractor fingerprint, not by deleting evidence. Malformed or
ambiguous work is failed or dead-lettered with a reason and operator guidance;
it is never silently skipped.

## 4. Procedure identity: v1 to v2

### Candidate formulas

| Version | Identity inputs |
|---|---|
| v1 | namespace + scope + project + task fingerprint |
| v2 | `procedure_candidate:v2:` + SHA-256 of a canonical tuple `(identity_version=2, task_fingerprint, trust_floor, policy_fingerprint_v2)`; Active Namespace is implicit |

For v2, encode each tuple field as a `u32` big-endian byte length followed by
its exact UTF-8 bytes, in the declared order. Compute
`policy_fingerprint_v2` from policy tags after the existing boundary
validation/trim, byte-exact stable sort, and deduplication. Do not lowercase
tags. Encode no tags as a zero-count canonical list, not as an omitted field or
delimiter-concatenated value.

The v2 formula is explicitly versioned. An unlabelled hash change is not a
compatibility mapping.

### Legacy procedure lookup

A legacy procedure candidate may be reused as v2 work only when its original
policy tags can be reconstructed unambiguously from persisted accepted
evidence. In that case, resume the one existing counter set and retain the
legacy candidate's provenance.

If the policy context cannot be reconstructed, or multiple legacy candidates
would map to one v2 candidate:

- retain the legacy candidate;
- keep separate v2 work as an explicit compatibility-review state when such
  work is needed;
- report the counter split and the reason;
- never merge or split candidates silently.

Procedure evidence must not be silently divided between candidates or combined
into a candidate with a different trust/policy boundary.

## 5. Namespace switching and the pre-stable `org` data

### Startup contract

| Configuration | Behavior |
|---|---|
| No `SURREALDB_*` variables | Embedded storage, Active Namespace `main`, database `memory`. |
| `SURREALDB_NAMESPACE=work` | Trim and bind `work` for the whole process; use `memory` or the configured database name. |
| `SURREALDB_NAMESPACE=org` | Bind the legacy namespace explicitly so existing zero-config `org` data can be read. |
| Empty/whitespace-only singular value | Fail before serving or connecting with actionable configuration guidance. |
| Comma-separated value or present `SURREALDB_NAMESPACES` | Hard configuration error; select exactly one value with `SURREALDB_NAMESPACE`. |

Startup must select and verify one namespace/database context before data-plane
work or workers begin. Only that namespace receives pending append-only schema
migrations. An inactive namespace is not opened, listed, inspected, migrated,
or modified by the process.

### Restart is the switch

Changing `SURREALDB_NAMESPACE` takes effect only after a clean restart. A
process never holds multiple active namespace sessions, and ordinary requests
cannot select another namespace or perform a cross-namespace search.

Switching from `main` to `org` means that the restarted process reads the
existing `org` storage context. Switching back means that it reads `main`
again. Applying an append-only migration while a namespace is selected does
not roll that namespace's schema backward when the process later switches
away and back.

Startup observability may report the backend, Active Namespace, database,
migration readiness, and explicit degraded capabilities. It must not enumerate
other namespaces or claim that data moved or that a namespace was newly
created.

## 6. Explicit non-behaviors

ADR-0038 and this compatibility contract do **not** perform any of the
following when configuration changes or a v2 lookup runs:

- copy data from `org` to `main`, or between any other namespaces;
- move, merge, export, import, or synchronize records across namespaces;
- delete an inactive namespace, legacy episode, fact, claim, procedure
  candidate, or durable job;
- discover or fall back to another namespace when the selected one is empty;
- rewrite source episodes or facts to erase legacy fields;
- synthesize `scope`, `project`, or `visibility_scope` placeholders for v2
  writes;
- silently accept and ignore legacy request fields;
- automatically merge ambiguous v1 identities into a v2 identity;
- silently skip malformed or ambiguous durable work.

Schema migrations are append-only and apply only to the Active Namespace.
Compatibility projections are additive/versioned operational metadata, not a
bulk source-data rewrite. Any intentional copy, export/import, deletion, or
manual reconciliation is an explicit operator procedure outside this ADR and
must preserve provenance and auditability.

## 7. Required compatibility evidence

The implementation must cover these fixture classes before the hard break is
released:

- unique v1 episode lookup reuses the episode and fact IDs;
- two legacy scopes matching one scope-free source identity fail without
  creating a v2 episode;
- v1 and v2 claim projection creates no semantic duplicate;
- claim relation history is versioned rather than forked;
- unique procedure evidence resumes one counter set;
- colliding or non-reconstructable procedure candidates remain separate and
  observable;
- malformed identity and durable-work rows are not accepted as defaults;
- selecting `org` reads old data, selecting `main` does not copy or delete it;
- namespace-local cursors, failed-ID lists, claims, and procedure counters do
  not cross into another namespace.

The checked-in tests and fixtures are the executable source of implementation
status. This document must be updated if an approved identity formula or
fixture classification changes; historical migration files and source
provenance remain immutable.
