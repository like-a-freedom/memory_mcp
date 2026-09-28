# Retire the migration residue, then deepen the seams it left behind

**Branch:** `ddd-refactorings` · **Baseline:** `f310a90` · **Status:** plan for approval (v2 — corrected)
**Scope:** the 5 candidates from the architecture review, in priority order, each independently shippable.

> **v2 corrections.** Three errors in v1 were found by re-verification and are fixed here:
> 1. `docs/architecture` was **deleted** in `6bf227a`, not renamed. The auditor fix is removal, not an
>    existence assertion (Phase 1c).
> 2. The deletion-recovery cleanup SQL runs against the **tenant namespace**, not the control database.
>    Phase 2's dependency is therefore tenant-runtime-scoped, not `operations → provisioning` (Phase 2b).
> 3. The Phase 3 store-ownership table was **wrong for 4 of 8 stores**. Three of them split across
>    contexts, and a 9th store trait (`LocalAdminStore`) exists outside `RegistryStore` entirely
>    (Phase 3a).

---

## Context

A DDD modular-monolith migration (ADR-0058) was declared complete in `6bf227a` / `f310a90`. Those commits
retired the migration's source guard (`tests/module_boundaries.rs`), its 385-file per-file manifest,
`docs/architecture/`, and `docs/superpowers/plans/`.

The migration moved code into bounded contexts. It did not delete the old copies. **137 files / 54,502 lines
— 29.6% of `crates/memory-mcp/src` — are never passed to rustc.** They are undeclared in their parent module
file, so no compiler warning, clippy lint, or test can see them. Several are byte-identical to the live
implementation that replaced them.

This matters beyond tidiness. `CONTEXT.md:66` still describes `src/service/` as "remaining legacy bridges
pending expiry, tracked per file in the migration manifest" — the manifest no longer exists, and most of that
tree was never a bridge. Anyone auditing ownership, writing a new capability, or reviewing a diff reads a tree
where 30% of the files are not real.

---

## Constraints inherited from the project

Non-negotiable, not re-litigated here:

- **No behaviour change.** The 8-tool MCP surface, HTTP payloads/status envelopes, cookie and OIDC contracts,
  bi-temporal semantics, config defaults and persisted schema are untouched.
- **No schema, migration, or data-copy change.** Phase 3 changes Rust trait structure only; SQL text must
  stay byte-identical or `registry_query_shape` fails.
- **Namespace invariant.** One Active Namespace per stdio process, one Tenant Namespace per SaaS Tenant
  Runtime, selected only by authenticated server-side resolution. A control-plane module writing directly
  into a tenant namespace is exactly what this forbids (Phase 2b addresses the one live instance).
- **Historical ADRs 0001–0057 and prior specs stay unchanged.** New ADRs append at 0061+ (0060 is the
  current maximum; 0061 and 0062 are free).
- **Twelve-Factor** as recorded: typed config at startup, no env reads in use cases, stdout reserved for MCP
  framing, disposability preserved.
- **DRY/KISS/YAGNI/SOLID** as applied in the DDD spec: one owner per business rule and canonical write path
  (SRP/DRY), ports follow use-case needs not a universal CRUD repository (ISP/DIP).

---

## Phase 0 — Pre-flight (no code change)

1. Re-derive the orphan set with the **full** feature matrix, not the default profile:
   ```
   rm -f target/debug/deps/*.d
   cargo check -p memory_mcp --all-targets \
     --features streamable-http,mcp-apps,fs-watch,eval-support,test-fixtures,prometheus
   ```
   Union every `.rs` named in the resulting dep-info files; diff against every `.rs` on disk.
2. **Verify the matrix covers every `mod` cfg.** `metal`, `accelerate`, `mimalloc` gate dependencies, not
   modules — confirm by reading their feature definitions, do not assume. A file gated behind an off-matrix
   feature is *not* an orphan.
3. Assert zero `include_str!` / `include!` / `#[path]` / build-script references from any compiled or repo
   file into the candidate set. (Verified: 0.)
4. Assert `crates/eval-harness` imports no candidate path. (Verified: it imports only live
   `service::MemoryService`, `service::memory_container_shims::*`, `storage::{DbClient, SurrealDbClient}`,
   `config`, `logging`, `models`, `eval_support`; its Cargo.toml enables only `eval-support`, so it cannot
   reach `http/`.)

**Gate:** the derived set must reproduce the 137-file list in Phase 1 before anything is deleted.

---

## Phase 1 — Delete the uncompiled residue, and make it impossible to hide again

*Candidates 1 + 4. One body of work: you cannot correct docs about residue without deleting it.*

### 1a. Delete 137 files

Exactly this set, grouped by origin with the live owner for spot-checking:

| Group | Files | Live owner |
|---|---|---|
| `service/context{.rs,/}` | 20 | `memory/retrieval/` |
| `service/entity_extraction{.rs,/}` | 13 | `knowledge/entity_extraction/` |
| `service/claims{.rs,/}` | 9 | `knowledge/claims_policy/` |
| `service/model_artifacts{.rs,/}` | 6 | `embedding/model_artifacts/` |
| `service/episode{.rs,/}` | 9 | `memory/episode/` |
| `service/content_extraction{.rs,/}` | 6 | `memory/content_extraction/` |
| `service/capabilities{.rs,/}` | 9 | `memory/capabilities/` |
| `service/lifecycle{.rs,/}` | 4 | `memory/lifecycle_workers/` |
| `service/procedures{.rs,/}` | 3 | `memory/procedures_service/` |
| `service/embedding{.rs,/}` | 4 | `embedding/providers/` |
| `service/util{.rs,/}` | 4 | `shared/{ids,validation}`, `platform/rate_limiter` |
| `service/query/{lexical,search}.rs` | 2 | `shared/search{,_lexical}.rs` |
| `service/cache{.rs,/}` | 3 | `memory/context_cache.rs`, `platform/context_cache_key.rs` |
| `service/agent_memory/{capture,policy,recall}.rs` | 3 | `memory/agent_memory/` |
| `service/apps/{diff,ingestion_review,lifecycle,types}.rs` | 4 | `knowledge/{diff,diff_types}.rs`, `memory/{ingestion_review,lifecycle,lifecycle_types}.rs`, `service/memory_container_shims/memory_lifecycle.rs` |
| `service/{community,conflict_resolver,entity,entity_resolution,explanation,fact,ingestion,value_helpers,triple_extractor,durable_work,embedding_runtime,embedding_service,model_loader,model_runtime}.rs` | 14 | `knowledge/`, `memory/`, `shared/`, `platform/`, `storage/value_helpers.rs` |
| `service/core/helpers.rs` | 1 | `service/core/builder.rs` |
| `storage/{agent_memory,claims,close,embedding_backfill_store,embedding_state_store,entity_store,episode_context_store,episode_store,fact_access_store,fact_store,inbox_revision_store,knowledge_graph_store,knowledge_store,procedures,reembed_store,triple_store}.rs` | 16 | `knowledge/`, `memory/`, `embedding/` stores |
| `http/fault_injection.rs` | 1 | `platform/fault_injection.rs` |
| `http/registry/models.rs` | 1 | `models/registry.rs` |
| `http/subscriptions/outbox.rs` | 1 | `platform/persistence/outbox.rs` |
| `cli/commands/{lifecycle,lifecycle_capture,lifecycle_recall}.rs` | 3 | `service/cli/lifecycle*.rs` |

**Three hazards that could bite:**

- `src/service/model_artifacts.rs` sat at a path `src/service.rs:36` already re-points to
  `crate::embedding::model_artifacts` via a live `pub use`. Deleting it removes a live path-shadowing
  hazard. Integration tests importing `memory_mcp::service::model_artifacts` resolve to the embedding
  module and were unaffected.
- `src/storage/entity_store.rs` and `src/storage/inbox_revision_store.rs` referenced
  `crate::service::value_helpers`, which does not exist. They compiled only because they were themselves
  undeclared. Do **not** "fix" them by re-declaring `value_helpers` in `src/service.rs` — that would create a
  second row-unwrapper competing with the live `storage/value_helpers.rs`.
- `src/service/apps/lifecycle.rs` contained `use super::types::{…}` — a reference to an undeclared sibling
  that would have been a hard compile error if the file were built. Independent proof these four were
  unreachable.

**Verification of the `service/apps/*` group specifically** (the one that looked least certain):

| File | Verdict | Evidence |
|---|---|---|
| `apps/diff.rs` | moved | `build_diff` + private helpers `facts_at`/`matches_target`/`visible_at` → `knowledge/diff.rs:19,116,138,152`; types → `knowledge/diff_types.rs:11`. It declares **zero** types — it imports all six `Diff*` from `crate::service`, which routes to `knowledge::diff_types`. Live caller `mcp/handlers/apps.rs:372`. |
| `apps/ingestion_review.rs` | moved | 4 items → `memory/ingestion_review.rs:30,111,147,170`. Live callers `mcp/handlers/apps.rs:412-413`, `service/apps/dispatch.rs:334,369,406,443`. |
| `apps/lifecycle.rs` | moved | `execute_lifecycle_command` → `service/memory_container_shims/memory_lifecycle.rs:13`; 6 `impl MemoryService` methods → `memory/lifecycle.rs:11,27,70,103,131,158` (now `impl LifecycleHandles`). |
| `src/service/apps/types.rs` | moved, split | 24 items → `src/memory/lifecycle_types.rs` (ingestion-review + lifecycle types) and `src/knowledge/diff_types.rs` (6 `Diff*`), re-exported from `lifecycle_types.rs` and surfaced via `src/service/apps.rs` / `src/service.rs`. |

**No file in the set contains `include_str!` of a migration.** Migration SQL is untouched.

### 1b. Add the guard (the part that outlives the deletion)

New `scripts/ci/audit_undeclared_sources.py` + `scripts/ci/test_audit_undeclared_sources.py`, following the
established pattern of `scripts/ci/audit_doc_claims.py` (same discovery discipline, same self-test shape).

- Run the Phase 0 cargo invocation; parse every `target/debug/deps/*.d`.
- Compute: every `.rs` under `crates/memory-mcp/src` **not** named by any dep-file.
- **Fail** if that set is non-empty, printing the offending paths and the matrix used.
- **Fail if the matrix does not compile**, so the guard cannot pass by silently checking nothing.

Wire into `.github/workflows/ci.yml` beside the doc-claims step.

This is disjoint from the guard retired in `f310a90`: that one policed *cross-context imports*, this one
catches *files the compiler never reads*. ADR-0061 states this so ADR-0058's revised "Rust privacy is what
the architecture rests on" is not contradicted.

### 1c. Correct the docs and the auditor

- `CONTEXT.md:66-67` — replace "remaining legacy bridges pending expiry, tracked per file in the migration
  manifest" with what `src/service/` now is: the container (`service/core.rs`), its adapters
  (`service/capability_deps.rs`, `service/memory_container_shims/`, `service/retrieval_deps_from_container.rs`),
  the app/CLI/worker/local-admin layers, and the compatibility re-exports. State that undeclared files are
  forbidden and the new audit enforces it.
- `CONTEXT.md:56-62` — delete the "Phase 5 is where that narrows" sentence. There is no Phase 5; the
  migration record was retired in `6bf227a`.
- `CONTEXT.md:63-65` — `src/service/embedding_service.rs` is deleted here. Repoint to `src/embedding/service.rs`.
- `CONTEXT.md:54-55` — `src/service/agent_memory/` keeps only `projection.rs` and `worker.rs`. Say the
  policy/recall/capture logic lives in `memory/agent_memory/`.
- **`scripts/ci/audit_doc_claims.py:31-35`** — `discover_docs()` iterates four trees:
  `docs/architecture` (deleted in `6bf227a`, 3 files), `docs/superpowers/plans` (deleted, 1 file),
  `docs/operations`, `docs/adr`. It currently reports *"all cited files, line numbers and identifiers
  resolve"* over two-thirds of nothing. **Fix: drop the two deleted trees from the tuple and have the
  function fail loudly if any configured tree is missing** — so the next tree that disappears is caught
  rather than silently narrowing the audit's scope.
- Restore `docs/superpowers/plans/` and commit this plan there, so the tree exists and carries current status.
- `AGENTS.md:132` — the module-seam description remains accurate. Verify; do not assume.

### 1d. New ADR

**ADR-0061 — The compiler's dependency graph, not a hand-maintained manifest, defines the source tree.**

### Gate for Phase 1

```
cargo check -p memory_mcp --all-targets --features streamable-http,mcp-apps,fs-watch,eval-support,test-fixtures,prometheus
cargo test  -p memory_mcp --features streamable-http,mcp-apps,fs-watch,test-fixtures,prometheus
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo fmt --all --check
python3 scripts/ci/audit_doc_claims.py
python3 scripts/ci/audit_undeclared_sources.py     # must exit 0 with an empty report
```

**Already proven on a scratch worktree at this exact baseline:** 137 files deleted → `cargo check` clean
across the full matrix (2 `unused import: RegistryStore` warnings, unchanged in count from baseline) →
**2,176 tests passed, 0 failed**.

**Verified on this baseline:** the canonical `cargo clippy` command from `AGENTS.md` (feature set
`fs-watch,mcp-apps,streamable-http`) emits **0 warnings and 0 errors** today. The 2 `unused import` warnings
appear only under the test feature set (`test-fixtures`, via `registry_query_shape` and `http_local_admin`),
not under the canonical set. Phase 3 removes both by deleting the import they name.

---

## Phase 2 — Make `operations` own the deletion-recovery workflow

*Candidate 2. Sequential; nothing else starts until this lands.*

`operations/api.rs` is 124 lines: two free functions (a 14-line loop; 22 lines of two guards plus a
delegate) and data shapes. The recovery state machine — tenant lookup, provisioning-lease claim with
heartbeat, two cleanup scripts, tombstone finalization, fault injection, purge detection, lease release,
~75 lines in what was then `src/bootstrap/integration/legacy_registry_operations.rs` — still lived in
`LegacyDeletionRecoveryAdapter`. The module fails the deletion test: delete it and the guards move, nothing
concentrates.

### 2a. Move the sequence, not the SQL

`operations` owns *the order of steps and the policy between them*. SQL stays in the adapter; `operations`
holds no SQL, per the spec's layer table.

### 2b. Route the tenant-namespace cleanup through a provisioning capability

**This is the correction.** `APP_SESSION_CLEANUP_SQL` and `TASK_CLEANUP_SQL` are executed via
`client.execute_migration_script(SQL, &namespace)` at `legacy_registry_operations.rs:121,129`, where
`namespace = tenant.namespace_binding.namespace` and `client` comes from
`registry.tenant_engine()?`. They run **in the tenant namespace against the tenant's own SurrealDB** —
`app_session` and `tenant_task` are tenant-namespace tables, not control-plane tables.

That makes the defect sharper than "wrong owner". A control-plane module is acquiring a tenant engine,
binds the tenant namespace, and writes two tables directly. That is precisely the namespace invariant:
*"one immutable Tenant Namespace per SaaS Tenant Runtime, selected through authenticated server-side
resolution"* (spec §Compatibility). The namespace is being selected from a recovery record rather than
through the tenant runtime the resolution pipeline produces.

Fix: a narrow **tenant-scoped** provisioning capability that receives an already-resolved tenant runtime and
owns both cleanup statements in provisioning's infra. Its name is a proposal, not a claim — a
purge-retained-tenant-work operation — and Phase 2 lands it. `operations` depends on it through a
consumer-owned port. The namespace then enters only where the resolution pipeline put it.

This is more work than moving two SQL constants up a level. It is the correct owner, it is the first and
only use of the `operations → provisioning` edge (spec §Directed dependencies), and leaving the writes where
they are preserves a live namespace-invariant violation.

### 2c. Reshape the port

`DeletionRecoveryPort` currently exposes `list_deleting_tenants` + `recover_tenant` — the workflow hides
inside the adapter. Split it so the workflow is in the module and the port is a persistence port. The
method set, in the order `recover_tenant` calls it, with the proposal noted where the name is new:

| # | Current member | Proposed port method |
|---|---|---|
| 1 | `list_deleting_tenants` | unchanged |
| 2 | `find_tenant_by_id` + the `Purged` short-circuit | a recovery-scoped read, so the port does not expose a general tenant lookup it has no use for |
| 3 | `claim_provisioning` | a deletion-scoped claim |
| 4 | `Lease::run_with_heartbeat` | a deletion-scoped heartbeat |
| 5 | `finalize_account_deletion` | a deletion-scoped tombstone write |
| 6 | the module-private purge-detection helper | promoted onto the port |
| 7 | the lease release call | a deletion-scoped release |

Only names 2-7 are new. The right-hand column is a proposal, not a citation: none of those names exist yet,
and Phase 2 introduces them. The behaviour each must preserve is the current behaviour exactly.

`recover_tenant` moves into `operations` as a function over that port, keeping the existing first-error-wins
loop and the `RecoveryOutcome::{Finalized, Purged}` mapping. The `missing_table(error, "app_session")`
tolerance (`:125,133`) must survive: absent-table is not an error, because a tenant may never have had App
Sessions.

### 2d. Rename the adapter

The workflow is gone, so `LegacyDeletionRecoveryAdapter` is no longer legacy → `RegistryDeletionRecoveryAdapter`.
`RegistryAccountDeletionAdapter` is a pure pass-through (`:29-41`); the file's own test comment at `:172-176`
admits the *type* carries the guarantee rather than the code. Either give it a real body or merge it into its
sibling.

### 2e. Preserve Twelve-Factor disposability

`DELETION_LEASE_TTL_SECS = 60` and `run_with_heartbeat` must still expire on abrupt termination so another
replica can reclaim. Do not convert the loop into a long-lived background task; keep `run_deletion_recovery`
and its batch bound of 64.

### 2f. No ADR

ADR-0058 and the spec already assign this to `operations`; this phase is spec conformance. The one new fact
— that app-session/task cleanup is provisioning's tenant-namespace data reached through a resolved runtime —
is a direct reading of the existing ownership table and namespace invariant.

### Gate for Phase 2

All Phase 1 gates, plus: recovery tests cross the new port, not the adapter. Add a negative test injecting a
fault at each step and asserting the lease stays reclaimable by another replica.

---

## Phase 3 — Decompose the omnibus `RegistryStore`

*Candidate 3. The largest and riskiest item; its own dedicated phase and ADR.*

`RegistryStore` (`src/http/registry/storage.rs:602`) is a supertrait of 8 store traits plus `ping`. A caller
taking `Arc<dyn RegistryStore>` must learn **51 methods** (50 inherited + `ping`; 37 without
`control-plane`). This is the only seam in the crate with two real adapters — `InMemoryStore` at `:2196` and
`SurrealRegistryStore` at `surreal_store.rs:3142` — so the interface's width is a real cost, not a
hypothetical one.

The trait's own docstring (`:590-600`) already concedes it: *"It exists so the two store implementations and
the existing call sites are unaffected by the split, not because the omnibus capability is wanted."*

### 3a. Target shape — the ownership table, corrected

**4 of the 8 stores do not belong to a single context.** v1 of this plan assigned them wholesale; that was
wrong and would have shipped cross-owner writes under a clean-looking name.

| Store | Owner | Methods | Evidence |
|---|---|---|---|
| `IdentityStore` | **identity** | 4 | `external_identity` + `audit_event` only. Already ported via `IdentityLinkTx` (`platform/persistence/control.rs:50`). |
| `BrowserPolicyStore` | **identity** | 2 | `browser_auth_policy` + `local_admin_audit`. Spec:37 "auth-method policy". Already ported via `AuthMethodPolicyPort`. |
| `SessionStore` (6 methods) | **identity** | 6 | `control_plane_session`, `oidc_request`. Spec:37 "browser-session lifecycle"; spec:100 `oidc_request` epoch-guarded. |
| `SessionStore` (2 methods) | **operations** | 2 | `create/consume_deletion_challenge` — `deletion_challenge`; the precondition half of `begin_account_deletion` (spec:102). |
| `AccountStore` (4 methods) | **identity** | 4 | `account`-row CRUD/CAS. |
| `AccountStore` (2 methods) | **provisioning** | 2 | `create_account_bundle` / `create_oidc_account_bundle` — writes `account`+`tenant`+`external_identity` atomically. Spec:98 names this `provisioning`. Already isolated as `AccountBundleTx`. |
| `AccountStore` (1 method) | **operations** | 1 | `begin_account_deletion` — 6 tables in one transaction. Already isolated as `AccountDeletionTx`. |
| `TenantStore` (4 methods) | **tenancy** | 4 | `find_tenant_by_account`, `find_tenant_by_id`, `list_tenants`, `list_ready_tenants`. |
| `TenantStore` (3 methods) | **provisioning** | 3 | `write_tenant`, `update_tenant_state_fenced`, `update_tenant_schema_version_fenced` — "provisioning leases" (spec:39). |
| `TenantStore` (4 methods) | **operations** | 4 | `update_tenant_state` (operator suspend/resume), `begin_operator_deletion`, `finalize_account_deletion`, `list_deleting_tenants`. |
| `ApiKeyStore` | **provisioning** | 7 | `api_key` only. Spec:39 "API-key issuance/revocation administration". |
| `ProvisioningStore` | **provisioning** | 5 | `tenant` lease + `provisioning_event`. Spec:39. |
| `UsageStore` → `PlanStore` | **provisioning** | 4 | `plan` table; `plan::enforce_ingest` is pure policy in `plan.rs:44,214`. |
| `UsageStore` → `UsageStore` | **memory** | 2 | `usage` table; `plan.rs:264` reconciles by `SELECT count(), sum(len(content)) FROM episode` — a direct read of **memory's** canonical table. |

**The two splits that matter most:**

- **`AccountStore` is not identity's.** `create_account_bundle` writes `tenant` rows. Identity has no outgoing
  business-context edges (spec §Directed dependencies), so giving it that write would make identity a writer
  of a tenant it does not own — the same class of hazard as "a credential is a namespace selector" (spec:37).
  The code already agrees: `platform/persistence/control.rs` defines `AccountBundleTx` and `AccountDeletionTx`
  as narrow separate traits, and its header states the intent — *"a caller that links identities should not be
  handed account creation, and a test that exercises deletion should not implement forty methods to do it."*
- **`UsageStore` is two owners on one trait.** `plan` gates key/session caps (provisioning) while `usage`
  meters episode ingestion (memory). The coupling is the bug: `plan.rs:264` aggregating over memory's
  `episode` table is a cross-canonical-table read. Split it; publish the aggregate as a memory query.

### 3b. The 9th store: `LocalAdminStore`

`LocalAdminStore` (`service/local_admin/contracts.rs:580`) is **not** in `RegistryStore` but is the largest
source of un-declared cross-owner writes, and a manifest built from the 8 would miss it:

| Table(s) | Line | Other owner |
|---|---|---|
| `account` + `tenant` | `local_admin.rs:1318,1323` | identity, tenancy |
| `account` + `tenant` | `:1844,1845` | identity, tenancy |
| `api_key` | `:1600,1726` | provisioning (duplicates `ApiKeyStore`'s writes) |
| `plan` (read) | `:1594` | provisioning |
| 9 `local_admin*` tables | throughout | identity |

It also re-implements the `browser_auth_policy` epoch guard ~10 times outside `BrowserPolicyStore` (`:308,444,
570,637,749,839,901,995,1111`) — the same table and the same invariant enforced a second time (DRY violation,
and a drift hazard). Give it a Phase 3 sub-step: route those writes through the owning contexts' capabilities
and delete the duplicated guards.

### 3c. Replace `RegistryHandle`

`RegistryHandle` (`src/http/registry.rs:209`) is `Arc<dyn RegistryStore>` plus `PrivilegedEngine`. Replace
with a `ControlPlaneStores` struct holding the per-context handles. `PrivilegedEngine` — the one genuinely
polymorphic engine (Remote/Local/LocalMem) — stays as-is.

### 3d. Delete `RegistryStore` and `RegistryHandle` once unreferenced

The 8 store traits and both adapters survive; only the omnibus supertrait and the handle go. Both already
implement the individual traits.

### 3e. Constraints

- **SQL must not change.** `registry_query_shape` pins query text; any diff there is a failure.
- The physical control database stays shared. **Logical write ownership is what changes.**
- `platform/persistence/control/` remains the sanctioned cross-owner transaction seam. It must not become a
  universal registry.
- The 13 consumer ports in `identity`/`tenancy`/`provisioning`/`operations` keep working unchanged; they are
  the adapters that receive the narrower stores. Phase 4 judges them.

### 3f. New ADR

**ADR-0062 — The control-plane persistence seam is per-context stores composed at bootstrap, not one omnibus
registry trait.** Must record the two splits (`AccountStore`, `UsageStore`) and the `LocalAdminStore`
re-home, because each is a judgement a future reader would otherwise re-litigate.

### Gate for Phase 3

All prior gates, plus: a test asserting a context cannot reach a store outside its 3a row (type-level, not a
source scan); `registry_query_shape` unchanged; the 2 `unused import: RegistryStore` warnings resolved.

---

## Phase 4 — Collapse the 13 one-adapter context ports

*Candidate 5. Speculative by design — it depends on Phase 3 having settled where the seams are.*

All 13 traits (identity 5, tenancy 2, provisioning 4, operations 2) have exactly **one** production impl
each; a recording fake is the only other. By "two adapters make a real seam" they are hypothetical.

But they are **not** worthless. `legacy_registry_identity.rs:26-28` states the intent: *"The adapter itself is
typed against the port only and cannot reach the wider trait — that is the point."* Each port is a deliberate
capability restriction, and each is what the existing fakes test through. So: a per-port judgement, not a
blanket removal.

### Rule

For each of the 13, apply the deletion test:

> Delete the port and have callers depend on the corresponding store trait (Phase 3) or a concrete function
> type. If the recording fake is replaced by an equally capable in-memory adapter and the same behaviour is
> still reachable through the seam, **delete the port**. If the port restricts access to something the store
> trait exposes, or a test genuinely needs a narrower fake, **keep it** — and record the reason in the port's
> doc comment.

Default to delete (YAGNI/ISP). Do not keep a trait that adds no restriction over a store trait.

### Explicitly not in scope

- **The modules themselves.** the single-flight, eviction and immutable-binding state machine in `src/tenancy/api.rs`,
  identity's recent-auth window and last-link guard, and provisioning's idempotency fingerprint and redacting
  `CreatedApiKey` Debug are genuine policy. Deleting any concentrates complexity. They are *interface-only*,
  which is the honest hexagonal shape the DDD spec describes.
- **Their 3-line root files.** The spec's `mod.rs` + `api.rs` split was never written; these are 3-line
  forwarders to a single `api.rs`. Restructuring them to match the spec's lettering is churn with no
  behavioural or testability gain. If the spec is the contract, amend the spec's layout paragraph instead —
  otherwise the plan and the spec are silently at odds.

### No ADR

Port granularity is a judgement recorded in code, not a decision a future reader would puzzle over.

### Gate for Phase 4

All prior gates. Each retained port carries a one-line justification naming the restriction it preserves.

---

## Sequencing

```
Phase 1  delete residue + guard + docs
   │        (shrinks the tree 30%; makes later work measurable)
   ▼
Phase 2  operations owns deletion recovery (tenant-scoped cleanup)
   │        (operations becomes a real module before its ports are judged)
   ▼
Phase 3  RegistryStore decomposed + LocalAdminStore re-homed
   │        (the 13 ports now sit on per-context stores)
   ▼
Phase 4  port collapse, per-port deletion test
```

Strictly ordered. Each phase is independently shippable and green on exit; stopping after any loses nothing
already landed. Do **not** parallelise 2 and 3 — Phase 2 moves code Phase 3 re-homes, so doing them together
means building the same adapter twice.

---

## Files that will change

| File | Change |
|---|---|
| 137 orphan `.rs` files | **deleted** (Phase 1) |
| `crates/memory-mcp/src/operations/api.rs` | port reshaped; `recover_tenant` moved in (Phase 2) |
| `crates/memory-mcp/src/bootstrap/integration/legacy_registry_operations.rs` | workflow removed; renamed (Phase 2) |
| `crates/memory-mcp/src/provisioning/api.rs` + infra | tenant-scoped retained-work purge (Phase 2) |
| `crates/memory-mcp/src/http/registry/storage.rs` | `RegistryStore` supertrait removed; `AccountStore`/`TenantStore`/`SessionStore`/`UsageStore` split (Phase 3) |
| `crates/memory-mcp/src/http/registry.rs` | `RegistryHandle` → `ControlPlaneStores` (Phase 3) |
| `crates/memory-mcp/src/http/registry/surreal_store/local_admin.rs` | cross-owner writes routed through owners; duplicate epoch guards removed (Phase 3b) |
| `crates/memory-mcp/src/{identity,tenancy,provisioning,operations}/api.rs` | per-port collapse (Phase 4) |
| `scripts/ci/audit_undeclared_sources.py` (+ self-test) | **new** (Phase 1) |
| `scripts/ci/audit_doc_claims.py` | drop 2 deleted trees; assert configured trees exist (Phase 1c) |
| `.github/workflows/ci.yml` | new guard step (Phase 1) |
| `CONTEXT.md` | 4 corrections (Phase 1c) |
| `AGENTS.md` | verified, unchanged (Phase 1c) |
| `docs/adr/0061-*.md` | **new** |
| `docs/adr/0062-*.md` | **new** (Phase 3) |
| `docs/superpowers/plans/2026-09-28-retire-migration-residue.md` | **new** — this plan (Phase 1) |

---

## Self-review: completeness and contradiction

**Completeness — every review candidate assigned:**

| Candidate | Where it lands |
|---|---|
| 1 · uncompiled residue | Phase 1a (+ 1b so it cannot recur) |
| 2 · `operations` recovery | Phase 2 |
| 3 · `RegistryStore` split | Phase 3 (+ 3b for the 9th store) |
| 4 · docs + auditor | Phase 1c |
| 5 · 13 port collapse | Phase 4 |

**Contradictions checked against the existing contract:**

- *No schema/migration change* — Phase 1 touches no `.sql`; Phase 3 changes trait structure only, gated by
  `registry_query_shape`.
- *Historical ADRs unchanged* — only 0061/0062 appended; 0060 confirmed as the current maximum.
- *ADR-0058 not contradicted* — the retired guard policed cross-context imports; the new audit catches unread
  files. Disjoint mechanisms, stated in ADR-0061.
- *Namespace invariant* — Phase 2b removes the one live violation (operations binding a tenant namespace
  directly). Phase 3 does not introduce a new one: control-plane stores read/write control-plane tables.
- *Spec's `mod.rs`/`api.rs` layout vs the 3-line context roots* — flagged out of scope in Phase 4 so plan and
  spec are not silently at odds.
- *Twelve-Factor* — disposability preserved (2e); XI (stdout reserved) untouched; III (typed config) untouched.
- *No new MCP tools* — the 8-tool surface is untouched throughout.
- *DRY* — Phase 3b deletes ~10 duplicated `browser_auth_policy` epoch guards; Phase 1 removes 54,502 lines of
  duplicate implementation.

**Risks carried, with mitigations:**

| Risk | Mitigation |
|---|---|
| A file gated behind an off-matrix feature is deleted wrongly | Phase 0 step 2 verifies the matrix covers every `mod` cfg; the guard fails if the matrix does not compile |
| Phase 2b breaks tenant purge for a tenant that never had App Sessions | Preserve the `missing_table` tolerance; add a regression test for the absent-table path |
| Phase 3 breaks the shared control-DB transaction contract | `platform/persistence/control/` stays the sole cross-owner seam; atomicity tests run as a Phase 3 gate |
| Phase 3b's `LocalAdminStore` re-home is the largest blast radius | Isolate as its own sub-step, after the `RegistryStore` split is green |
| Phase 4 removes a port a test depends on | Per-port deletion test; keep-with-justification default flipped to delete |

**Not covered / accepted:** the 2 `unused import: RegistryStore` warnings appear only under the *test*
feature set, not the canonical `AGENTS.md` clippy set, which is clean today. Phase 3 resolves them; the
Phase 1 gate asserts the canonical set stays at zero rather than asserting the whole matrix is warning-free.

**Domain model:** no new glossary term required. `operations`, `Deletion Recovery`, `App Session` and
`Tenant Task` already exist in `CONTEXT.md`. Phase 1 edits only factual claims about where code lives;
Phase 2 introduces no new domain concept; Phase 3's store splits are ownership, not vocabulary.
`CONTEXT.md` is corrected in place, not restructured, matching its current dual shape (public surface +
module seams).
