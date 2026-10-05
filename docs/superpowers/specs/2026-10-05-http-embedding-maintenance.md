# HTTP Embedding Maintenance — Design Spec

**Date:** 2026-10-05
**Status:** Approved for implementation
**Supersedes:** nothing. Amends the embedding state of ADR-0042 and the provider
wiring of §13 of `docs/superpowers/specs/2026-08-27-streamable-http-saas.md`.

## 1. Problem

The Streamable HTTP profile has no way to bring a tenant's stored vectors to the
deployment's current provider. Three operator-visible situations all end in the
same place — degraded, permanent, and with a reason string that names a command
this binary does not have:

1. **Provider/model/dimension change.** Stored vectors carry a foreign
   `embedding_signature`. `decide_embedding_startup` returns `DisableSemantic`
   ("configured embedding signature differs from persisted state"). The tenant
   serves lexical retrieval indefinitely.
2. **Provider outage.** Facts ingested while the provider was unreachable have
   `embedding IS NONE`. The stdio profile recovers these via
   `EmbeddingRecoveryRuntime`; the HTTP profile never spawns it, so they stay
   unembedded.
3. **Arbitrary enable/disable cycles.** A tenant written while embeddings were
   off, then re-enabled, has facts with no vectors and no stored state. Before
   this spec's first change, the empty vector sample was classified as "legacy
   embeddings require reembed" — naming `reembed` as the exit for a namespace
   that has nothing to re-embed *from*.

Verified absence: `reembed` has zero call sites under `src/http/`. No route, no
MCP tool, no scheduler job, no env var, no `ForceEnabledForReembed`, no
`EmbeddingRecoveryRuntime`. `memory_mcp reembed` exists only on the stdio
binary's CLI.

Verified present: the reembed mechanism itself works against a tenant namespace.
A force-enabled service over a degraded tenant namespace rewrote every fact and
recreated the HNSW index at the target dimension, returning
`ReembedOutcome::Completed`. Only a trigger is missing.

## 2. The distinction this spec rests on

Two operations that look alike and are not:

| | **Class A — backfill** | **Class B — reembed** |
|---|---|---|
| Selects | `embedding IS NONE` | `embedding_signature != target` |
| Touches existing vectors | never | all of them |
| HNSW index | re-declared **only** when the namespace stores no vector; never under vectors | dropped, recreated at target dimension |
| Idempotence | additive, naturally | convergent but not idempotent mid-pass |
| Reversible | yes (writes only what was absent) | **no** |
| Cost | bounded by the missing count | bounded by the total fact count |
| Failure mode | leaves a gap | leaves a partially-rewritten namespace |

**Rule 1. Class A is automatic. Class B requires an explicit operator decision.**

This is the central design commitment. Backfill writes only absent values, so
its worst outcome is wasted provider spend. Reembed overwrites the whole
namespace destructively and irreversibly: a reembed pointed at the wrong
provider destroys the previous vectors with no way back. Therefore no signal —
including a changed env var — may trigger Class B without a human asking.

**Corollary (Rule 2).** `decide_embedding_startup` must never name `reembed` as
a remedy in the HTTP profile. Its current "legacy embeddings … require reembed"
string is a dangling reference in a binary that has no reembed. Where reembed
becomes reachable (Task 6) the string becomes true; until then, the empty-vector
case must not be described as a migration.

## 3. Scenario matrix

Every reachable (provider state, namespace state) pair, with the decision, the
acting mechanism, and the trigger. `V` = stored vectors, `S` = `embedding_state`
row, `Sig` = target signature.

| # | Deployment | Namespace | Decision | Mechanism | Trigger |
|---|---|---|---|---|---|
| 1 | off | empty | lexical, no policy | none | — |
| 2 | off | any `V`, any `S` | lexical, no policy; **vectors preserved** | none | — |
| 3 | on | no facts | `BootstrapReadyNamespace` | none | startup |
| 4 | on | `V=∅`, no `S` | `BootstrapReadyNamespace` | backfill fills `IS NONE` | startup + scheduler |
| 5 | on | `S.ready`, `Sig` match, some `IS NONE` | `ResumePendingBackfill` | backfill | startup + scheduler |
| 6 | on | `S.ready`, `Sig` match, none missing | `UseConfiguredProvider` | none | startup |
| 7 | on | `S.ready`, `Sig` differs | `DisableSemantic` (Class B) | reembed | **operator** |
| 8 | on | `V` at foreign width, no `S` | `DisableSemantic` (Class B) | reembed | **operator** |
| 9 | on | `S` rebuilding/failed | `DisableSemantic` | reembed retry | **operator** |
| 10 | on | provider unreachable at preflight | no policy, startup fails | — | operator fixes endpoint |
| 11 | on, after outage, `S` unchanged | `IS NONE` rows exist | row 5 | backfill | scheduler |
| 12 | provider A → B → A | `S` holds B's `Sig` | row 5 if A==B else row 7 | backfill / reembed | scheduler / operator |
| 13 | `EMBEDDINGS_ENABLED` toggles off→on | any | re-evaluated from scratch | rows 4/5 or 7 | startup |

Rows 1–6, 10–13 need no new code beyond the Task 1 decision fix. Rows 7–9 are
the reembed gap. Row 11 is the backfill gap.

**Scenario 3 of the operator's brief (arbitrary enable/disable cycles) is fully
covered by rows 1–6, 12, 13.** It is not a third feature; it is the same
A/B distinction applied to a sequence. Cycles that return to the same provider
(12-A) self-heal via backfill; cycles that end on a different provider (12-B)
require one operator reembed, after which further cycles are again symmetric.

## 4. Design

### 4.1 The decision fix (already implemented in this branch)

`decide_embedding_startup` (`src/service/startup.rs`) gained an arm ahead of the
"legacy" case:

```rust
None if sample_dimensions.is_empty() => BootstrapReadyNamespace { .. }
```

`sample_stored_embedding_dimensions` filters facts through
`embedding_from_value` before `take(16)`, so an empty sample is a proof, not an
absence of evidence: **no stored fact carries a vector**. That is Class A work.

Pinned by `decide_embedding_startup_bootstraps_a_namespace_whose_facts_have_no_vectors`.
The adjacent test was corrected rather than deleted —
`decide_embedding_startup_does_not_bootstrap_when_legacy_dimensions_are_unknown`
now uses `[768]` (vectors present, wrong width) because `[&[]]` is no longer the
legacy case, it is the bootstrap case.

### 4.2 Index ownership: the boundary this change must not cross

ADR-0042 §58: reembed owns HNSW index replacement. The activation-path
reconcile added in the provider-wiring work (`reconcile_tenant_index_dimension`)
currently re-defines the index on any dimension mismatch. That is correct in
one case and destructive in the other:

- **Index wrong, no vectors** (Class A): reconcile is *required*. Without it the
  first vector the provider writes is rejected by the stale index, and backfill
  fails forever.
- **Index wrong, vectors present** (Class B): reconcile is *harmful*. It declares
  the index at the new width while it still holds old-width vectors — precisely
  the state reembed exists to repair — and reembed then cannot complete.

**Rule 3.** Reconcile the activation-path index only when the namespace holds no
vectors. Every other mismatch is Class B and belongs to the operator.

### 4.3 Class A — per-tenant backfill (Task 2)

A scheduler job, modelled directly on `http::tasks::scheduler`:
`list_ready_tenants(None, 100)`, then per tenant bind and work, errors logged
never aborting the pass, `record_job_metric` at the end.

Rejected — per-tenant `EmbeddingRecoveryRuntime` as in stdio:

- the runtime pool is bounded and evicts on idle TTL, so a worker would die
  with the runtime it is attached to, while the need outlives it;
- a tenant nobody has called has no runtime, so its accumulated unembedded facts
  would never be scanned — the common case in multi-tenant, not an edge case;
- it would create N×M workers over time.

The stdio worker stays as it is. `run_backfill` is reused, not reimplemented —
but note what that reuse costs in visibility: it takes `&RecoveryHandles`, which
`RecoveryHandles::from(&MemoryService)` derives from a service's own
`db_client`, `active_namespace`, `logger` and `context_cache`. The HTTP tick must
therefore build a `MemoryService` per tenant, not a bare client, and it cannot
be driven from an out-of-crate test because `RecoveryHandles` is `pub(crate)`.
That is a constraint on the test shape, not a reason to reimplement the loop:
`run_backfill` already does the parts that are easy to get wrong — it writes
through `embedding::api::generate_and_update` with `VectorWritePolicy::FillMissing`
so a concurrent pass's vector is never clobbered, and it routes generation
through `ProviderGeneration` rather than `provider.embed()` so the 8,000-character
input limit and the disabled-provider check are not bypassed.

Gate: `EMBEDDINGS_AUTO_RECOVERY`, the same variable stdio reads, so one mental
model covers both profiles.

Readiness is deliberately untouched. Spec §16 keeps readiness true under a
degraded provider; backfill improves data in the background and must not make a
deployment unhealthy because one tenant has many facts.

Per-tenant opt-out is out of scope: §13 makes provider policy deployment-level
and excludes per-tenant opt-out from v1. Backfill finishes what ingest started;
it is not a separately metered feature.

### 4.4 Class B — operator reembed (Tasks 3–6)

**Trigger.** `POST /api/v1/operator/tenants/{id}/reembed`.

This is the path the spec already prescribes. §45: "`memory_mcp_http` has no
ingest/extract/reembed/admin CLI commands. Operator automation uses the protected
control-plane API. A future admin CLI may be a thin HTTP client, never a direct
production-DB client." A control-plane route is therefore not a concession to
the freeze — it is the specified mechanism. Authorization is inherited by
placement inside the existing `operator` router block: session → operator
allowlist → CSRF → `require_recent_auth` (600 s).

Rejected — an MCP tool. ADR-0016 freezes the tool surface at eight; `AGENTS.md`
requires an ADR and an evidence gate for a ninth. Independently of the freeze,
the authorization model is wrong: tenant credentials must not be able to trigger
a whole-fleet vector rewrite.

Rejected — automatic reembed on signature change. A mistyped
`EMBEDDINGS_MODEL` would silently and irreversibly rewrite every tenant's
vectors, at N tenants × M facts provider spend, with no cap, no opt-out and no
rollback. Rule 1 forbids it.

**Substrate.** Extend `tenant_task` with a `kind` column, not a second
mechanism. Purge already demonstrates a workable pattern on a separate deletion
lease, but two durable-task substrates in one codebase is a choice trap that
`AGENTS.md` already flags. Task 3 adds the discriminator; existing rows without
it read as `extract` and the extract path is unchanged.

**Dispatch.** `execute_one_task` currently hardcodes
`from_value::<ExtractParams>(record.params)`; a reembed payload would fail into
`invalid durable extract parameters` and be marked `Failed`. Task 4 turns it into
a match on `kind`.

**Lease.** `claim_next_due` sets `now + 60s` with no heartbeat. A 50k-fact
reembed far exceeds that and would lose its fence at `complete_fenced`. This is
a pre-existing latent hazard, not reembed-specific; `provision_one` already solves
it with `run_heartbeated`. Task 5 applies the same shape to the reembed branch.

**Force-enabled service.** `prepare_reembed_pass` refuses a tenant whose runtime
carries `signature: None` or a disabled provider — correct for serving, fatal for
rewriting. The reembed executor must therefore build its own service that ignores
the namespace decision and forces the deployment's provider, exactly as
`bootstrap/stdio.rs:106-118` does under `ForceEnabledForReembed`. Task 6 extracts
that provider-resolution into one shared function called from both places, rather
than copying it — a third copy would eventually diverge.

`MemoryService::new_with_embedding_provider` is `pub(crate)`, so the HTTP
executor is in-crate and can call it directly.

**Not new:** `embedding_job:fact_reembed` already provides a durable resume
cursor keyed by `last_completed_fact_id`. Task 5's heartbeat makes progress real
rather than thrashing; duplicating the cursor would be a second substrate.

## 5. Non-goals

- No per-tenant provider credentials or opt-out (§13).
- No ninth MCP tool (ADR-0016).
- No change to readiness semantics (§16).
- No lifecycle background workers per tenant. The config is copied onto the
  tenant service; the workers are not spawned. Resource growth across N tenants
  is a separate question.
- No `embedding_job` reimplementation.
- No stdio behaviour change beyond the §4.1 decision fix, which is shared.

## 6. Documentation deliverables

- ADR recording: control-plane reembed is not a §45 violation; ADR-0042 index
  ownership boundary; the A/B rule.
- `README.md` operator route table row: the verb list becomes "provisioning
  retry, suspend, resume, purge, reembed, recovery".
- README: HTTP `EMBEDDINGS_*` behaviour — backfill is automatic, reembed is
  operator-triggered, and `EMBEDDINGS_AUTO_RECOVERY` gates the former.
- `docs/operations/LIMITATIONS.md`: reembed is not available on the stdio build
  for HTTP tenants; a degraded tenant stays degraded until an operator reembeds.

## 7. Principles audit

Checked against SOLID, DRY, KISS, YAGNI and 12-factor. Findings and resolutions:

**SOLID**
- *SRP violation found in already-merged code.* `SurrealDbClient` — the storage
  adapter — learned the `fact_embedding_hnsw` index name, the `embedding` field
  and the `DIMENSION` clause during provider wiring, and gained
  `set_embedding_index_dimension`, which re-implements
  `ReembedStoreClient::remove_embedding_index` + `define_embedding_index`. Task 2
  moves index introspection to `ReembedStoreClient`, the established owner of
  that DDL vocabulary, and deletes the duplication. `SurrealDbClient` retains
  only `fact_embedding_dimension`, which it legitimately owns because it renders
  the migration template.
- *SRP* for the backfill tick: `backfill_scheduler` holds scheduling policy
  (which tenants, when, error handling) and delegates the batch loop. One reason
  to change.
- *OCP* is the reason for the `kind` match rather than a second task table: a
  third task kind is a new match arm, not a new subsystem.
- *DIP* holds — both jobs depend on the `provisioning::SchedulerJob` type, not
  on a concrete scheduler.
- *LSP/ISP* are not engaged; no new trait is introduced anywhere in this plan.

**DRY**
- `run_backfill` is called, not reimplemented. The first draft of the backfill
  task spelled out its own batch loop and would have silently lost
  `VectorWritePolicy::FillMissing` and the `ProviderGeneration` input limit.
- The provider resolution under `ForceEnabledForReembed` is extracted into one
  function shared with stdio (Task 6) rather than copied into a third caller.
- The index DDL has exactly one definition of the name and the
  missing-index tolerance after Task 2.
- *One genuine tension, left visible rather than resolved by fiat:* the
  heartbeat interval arithmetic exists in `run_heartbeated` and is needed again
  for the task lease, which `run_heartbeated` cannot serve because it is bound to
  `ProvisioningLease`. DRY says extract; YAGNI says fifteen lines of interval
  setup is not an abstraction. Task 5 Step 4 states both options and asks the
  reviewer to choose rather than pre-empting the call.

**KISS**
- No new abstraction layer, no plugin registry, no per-tenant configuration
  object. The `kind` column is one `option<string>`.
- The reconcile predicate is one comparison (`stored vector count > 0`).

**YAGNI**
- No per-tenant provider opt-out, no credential plumbing, no ninth MCP tool, no
  per-tenant lifecycle workers, no reimplementation of the `embedding_job`
  resume cursor. Each is a thing that could be built and that this spec refuses.

**12-factor**
- *III, config in the environment:* env is read exactly once, in
  `resolve_deployment_policy`. `DeploymentPolicy` gains
  `auto_recovery: bool` so the scheduler tick never calls `env::var`; this also
  keeps the tick testable without process-global mutation.
- *V, build/release/run separation:* the provider is deployment-level policy in
  both profiles; no build-time switch distinguishes them.
- *VII, port binding:* the reembed operator route binds the same configured
  address as every other control-plane route.
- *XI, logs as event streams:* every new decision is a structured
  `op=` event on the existing `StdoutLogger`, including the new
  `http.tenant_embedding_index_reconcile_skipped`.
- *IX, disposable resources:* the backfill job holds no state between ticks; a
  restarted replica re-derives everything from the namespace.

## 8. Audit trail — corrections made to the plan during self-review

Recorded because each was a wrong belief about the code that a reader would
otherwise inherit.

1. **Apparent migration-number collision — investigated, and the plan is right.**
   The plan proposes `045_tenant_task_kind`, yet files `045`–`052` already exist,
   which looks like a collision. It is not. Three separate sequences live in that
   directory: `versioned_migrations()` in `storage::migrations` ends at `039`;
   `HTTP_MIGRATIONS` in `http::leases::migration` ends at `044` and drives
   `CURRENT_SCHEMA_VERSION`, currently `44`; and `045`–`052` are registry and
   local-admin scripts applied by `SurrealRegistryStore`, which is registered in
   neither list. `045` is therefore unique in the HTTP catalogue, and the bump to
   `CURRENT_SCHEMA_VERSION = 45` is what widens `REPLICA_SCHEMA_RANGE` to
   `44..=45`. The task's first step verifies this by asserting the name appears in
   neither list before writing.
2. **Batch loop reimplemented by mistake.** The first draft of the backfill task
   spelled out its own batch loop over
   `EmbeddingBackfillStoreClient::select_facts_missing_embeddings`. That
   discards `run_backfill`'s two safety properties — `VectorWritePolicy::FillMissing`
   and the `ProviderGeneration` path that enforces the input limit — and would have
   been a silent regression. The plan now calls `run_backfill`.
3. **`Arc<dyn DbClient>` from `&self`.** The draft instructed the implementer to
   build a bound client from a `SurrealDbClient` reference. `SurrealDbClient`
   holds its `DbEngine` by value and is not `Clone`; there is no such conversion.
   The correct route is `MemoryService::db_client` (already `Arc<dyn DbClient>`).
4. **Wrong module paths.** `LogProgressReporter` and `NoopProgressReporter` live
   in `service::reembed_progress`, not `embedding::`. `ReembedOutcome` is in
   `service::reembed_options` and is *not* re-exported from `service.rs`, unlike
   `ReembedSummary` — so it is named by full path rather than by adding a
   re-export for a single call site.
5. **The integration-test feature gates were missing, and the failure mode is
   invisible.** Every HTTP integration test that drives the task scheduler opens
   with `#![cfg(all(feature = "streamable-http", feature = "control-plane",
   feature = "test-fixtures"))]`, because `DurableTaskTestDriver` and
   `execute_one_task_for_test` are `#[cfg(any(test, feature = "test-fixtures"))]`.
   The plan originally ran the suite with `streamable-http` alone. That compiles
   the new test files to empty binaries: cargo reports success, the test count
   is zero, and nothing was verified. Every gate in the plan now carries the
   full feature set and requires a non-zero test count as part of "passed".
6. **The test seam had no provider parameter.** `execute_one_task_for_test`
   takes only an `ExtractorFn`, so a reembed tick — which needs an
   `EmbeddingProvider` — could not be driven from a `tests/` file at all. The
   plan adds a sibling `execute_reembed_task_for_test` rather than widening the
   existing function, because a parameter only one task kind needs does not
   belong on a function every caller shares.
7. **The type's module was named two ways.** `EmbeddingPolicy` is declared in
   `http/runtime/storage.rs`; `DeploymentPolicy` is declared in
   `http/runtime/bootstrap.rs` and imports it. The plan named both by the
   `bootstrap.rs` path at one point. Both are `pub` and both are reachable from a
   `tests/` file, so this was a documentation error rather than a visibility
   one — but a plan that names a type by the wrong module sends the implementer
   to the wrong file.

## 9. Verification gates

Every task's step ends in a command with an expected result. The branch gate,
run before merge:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test -p memory_mcp --lib --features streamable-http
cargo test -p memory_mcp --lib
cargo test -p memory_mcp --tests --features streamable-http,mcp-apps,test-fixtures
```

Zero warnings, zero failures, on both feature profiles. No task may merge with a
red gate; a red gate is a defect in the task, not an obstacle to route around.

**The integration line must carry `test-fixtures`.** Every HTTP integration
test that drives the durable scheduler or the control plane opens with
`#![cfg(all(feature = "streamable-http", feature = "control-plane",
feature = "test-fixtures"))]`. `streamable-http` already implies
`control-plane` (`crates/memory-mcp/Cargo.toml` lists it in that feature), so
`test-fixtures` is the only one of the three that must be added — and without
it those files compile to empty test binaries and report success. Verified
directly: `--features streamable-http` alone prints `running 0 tests` and
`ok`. A zero-count run is therefore part of "passed", not a detail of it.

`.github/workflows/ci.yml` runs the superset
(`--features fs-watch,mcp-apps,streamable-http,test-fixtures`), so CI covers
this; the gate above is the local equivalent.
