# Architecture Audit Follow-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the five findings of the 2026-10-03 architecture audit — Tenant activation that survives request cancellation, canonical vector writes that cannot clobber unrelated fact fields, initialization proposals that reach protocol negotiation, architecture guards that reject their own counterexamples, and a no-default test profile that CI actually executes.

**Architecture:** Three of the five are repairs of existing decisions (ADR-0042 vectors, ADR-0071 negotiation, ADR-0061/0065 guards); only Tenant activation needs a new decision, recorded as ADR-0072: the HTTP runtime pool becomes the sole owner of runtime residency, single-flight activation, timeout, backoff, capacity and eviction, with request-owned RAII cancellation, while `tenancy::api` keeps trusted resolution, status policy, binding identity and the factory port.

**Tech Stack:** Rust 1.97.1, Tokio (watch/semaphore), Axum 0.8.9, rmcp 3.5.0, SurrealDB 3.3.0, `async_trait`, `thiserror`, `serde_json`.

**Spec:** `docs/superpowers/specs/2026-10-03-architecture-audit-follow-up.md` — the decision record is `docs/adr/0072-single-owner-for-tenant-runtime-activation.md`. Executors read all three.

## Execution Status

Recorded 2026-10-03, after two implementation/review passes. Checkboxes below are
the original step plan; this table is what actually happened.

| Task | State | Evidence |
|---|---|---|
| 1 Injected factory seam | Done | `Pool::with_factory`; commit `2e0c3e1` |
| 2 Cancellation counterexamples | Done | The three cancellation tests fail with the attempt guard's cleanup removed and pass with it |
| 3 Tenancy loses the cache | Done | `Tenancy`/`RuntimeLease` deleted; identity projection added |
| 4 One state machine + RAII | Done | `SlotState`, `ActivationAttempt`, generation fencing |
| 5 Reservations and capacity | Done | `SlotReservation`; cancelled reservations recover capacity; pin release wakes every drain/capacity waiter |
| 6 Identity and revision | Done | `binding_mismatch_is_rejected_*`, `plan_change_replaces_the_runtime` |
| 7 Shutdown wiring | Done | `Pool` holds the instance `ShutdownState`; existing drain tests pass |
| 8 HTTP-level deadline test | Done | `http_deadline_cancels_activation_and_the_next_request_recovers` uses the real Axum deadline layer around pool acquisition |
| 9 Vector conditional writes | Done | `replace_stale_preserves_concurrent_fact_access` fails against the previous write and passes with the fix; metadata comes from `VectorIdentity`; commit `b3fd349` |
| 10 Access writer | Done | One atomic saturating update; concurrent increments, concurrent vector and forged-id tests pass |
| 11 Initialize negotiation | Done | `unsupported_initialize_proposal_negotiates_legacy_revision`; commit `6881141` |
| 12 Rooted source guard | Done | `source_tree_integrity` discovers Cargo lib/bin roots, walks external and inline modules, and rejects cycles, missing/ambiguous files, and path attrs |
| 13 Public-surface scope | Done | Split `MemoryService` impls inventoried; grouped exports parse via shared lexer; nested grouped globs fail closed; trait declarations match their named body and caller scans ignore comments/literals |
| 14 No-default CI row | Done | `2 passed` under `--no-default-features`; commit `afee2b8` |
| 15 Integrated verification | Done | Workspace (103 targets), full-feature (2440 passed), no-default (2 passed), observability, fmt and required Clippy all pass |

### Second pass, 2026-10-03

Commit `d453a6f` closed the reproduced grouped-export bypass in
`tests/public_surface_audit.rs`: `pub use crate::types::{Fact, ids};` used to
enumerate no names at all, so the live ratchet could not fire on it however a
name regrew. Proven end to end by temporarily adding
`pub use crate::shared::ids::{hash_prefix};` to `src/service.rs`, which now fails
`no_cut_name_is_re_exported_again` naming `hash_prefix`; the line was removed
again before the commit.

The source guard was then replaced with a rooted module graph starting from
Cargo's lib/bin targets. It uses the bounded token reader in
`tests/support/rust_source.rs`; the same reader now powers the public use-tree
and lexical caller guards. The guard unions cfg-gated declarations, follows
inline/external modules with Rust's directory rules, and fails closed on
unresolved/ambiguous modules and production path attributes. Fixtures cover an
unrooted cycle, incorrect sibling fallback, inline search dirs, Cargo roots,
feature-gated declarations, comments/strings/macros, path attributes and
ambiguity.

The pool now drains old runtime pins before replacing a revision, extracts
runtime destruction outside the pool lock, wakes capacity waiters on guard
release, and linearizes shutdown against runtime publication. Tests cover each
path, including the HTTP deadline middleware around a blocked activation.

The second review of the implementation found all of those lifecycle gaps before
they were closed, plus a metadata source mismatch and the unrooted guard/parser
gaps. Vector metadata now comes only from the validated identity. The source
walker starts at Cargo production roots; the public export and trait declaration/
caller guards share the bounded lexer, fail closed on unsupported forms, and
exclude comment/string lookalikes.
The Axum deadline middleware now has a direct request-level regression: the first
request times out while activation is blocked; the next request for that tenant
activates successfully.

The final independent review caught two additional bypasses: a pin-release
notification could wake a capacity waiter but leave a revision-drain waiter
asleep, and a nested grouped glob could enumerate no export names. Pin release
now wakes all pool waiters to re-check their predicates; `public_surface_audit`
rejects glob leaves anywhere in a grouped use tree. Regressions cover both cases.

Gates actually run: `cargo fmt --all --check` clean; the repo's required Clippy
command clean; `cargo test --workspace --lib --bins --tests --locked` green
across 103 test targets with 0 failures; `cargo test -p memory_mcp --lib --bins
--tests --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked`
green (2440 passed, 4 ignored); `cargo test -p memory_mcp --no-default-features
--test fs_watch_process_disabled --locked` runs 2 tests; `cargo run --locked -p
xtask -- check-observability` passes all three checkers. The focused guards pass:
doc_claims (3), source_tree_integrity (13), public_surface_audit (16),
trait_methods_are_called (7), no_duplicate_implementations (4).
`http_control_plane`, `http_load_concurrency`, `http_crash_recovery`,
`http_isolation`, `http_proto_conformance`, `tenancy_runtime`,
`tenancy_resolution`, `tenant_lifecycle`, `embedding_vector_policies`,
`embedding_canonical_vectors` and `memory_recall` all pass;
`--no-default-features --test fs_watch_process_disabled` runs 2 tests.

### Additional finding, not in the plan

`cargo clippy -p memory_mcp --all-targets --features
fs-watch,mcp-apps,streamable-http,test-fixtures` reports 4 pre-existing
`needless_borrow` warnings in `tests/http_proto_conformance.rs`. The repo's
required Clippy row does not enable `test-fixtures`, so those files are never
linted, while CI does execute them in the `Optional feature tests` row. Not
fixed here: it is unrelated to this plan and fixing it would hide that the lint
row has a hole.

## Global Constraints

- `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` produces zero warnings.
- `cargo fmt --all --check` produces zero diff.
- `cargo test --workspace --lib --bins --tests --locked` passes.
- Production code returns `MemoryError`/`Result`. No `unwrap`, `expect`, `panic`, `todo!`, or new `#[allow(...)]` outside `#[cfg(test)]` (existing sites in `tests/common/` and `#[cfg(test)]` modules are unchanged).
- No lock guard is held across `.await`. In this plan the pool's bookkeeping lock is `std::sync::Mutex` and is never held across an await.
- No new dependency, no Cargo.toml change, no migration, no new MCP tool. The eight-tool surface is frozen.
- `MemoryService` may be named only at the composition edge (`service/memory_container_shims/`, `service/capability_deps.rs`, `service/retrieval_deps_from_container.rs`, `service/core/`, `service/cli/`, `runner.rs`, `mcp/`, `cli/`, `tools/`, `http/runtime/`). No bounded context names it.
- Business policy lives in the owning context's `api.rs` (ADR-0066); the HTTP layer wires and enforces, it does not own policy.
- Feature flags are additive; `default = ["fs-watch"]` and `streamable-http` stays the single SaaS switch.
- Environment variable names and defaults are unchanged; a new configuration knob is out of scope.
- Documentation claims are executed: adding a spec file requires a row in `expected_spec_statuses()` in `crates/memory-mcp/tests/doc_claims.rs`.

## Review Focus

The five input classes or conditions the spec implies that no current test pins. Each line names the owning task's test.

1. **A request cancelled while it is the activation leader** (`http/middleware/deadline.rs` drops `next.run`) — the tenant must still be activatable afterwards and followers must not wait forever. Test: `cancelled_activation_leader_allows_retry` (Task 2).
2. **A fact read by the vector writer, then mutated by another writer before the vector write lands** — the unrelated field must survive. Test: `replace_stale_preserves_concurrent_fact_access` (Task 9).
3. **A legacy `initialize` whose `protocolVersion` is not a known revision** — negotiation must answer with a supported legacy revision, not a pre-auth 400. Test: `unsupported_initialize_proposal_negotiates_legacy_revision` (Task 11).
4. **A source file that no Cargo root reaches, and a grouped `pub use` of a cut name** — both must fail their guard. Tests: `unrooted_mod_cycle_is_reported_unreachable` and `grouped_use_of_a_cut_name_is_rejected` (Tasks 12–13).
5. **A binary built without `fs-watch` while `MEMORY_INGESTION_INBOX` is set** — the process must exit with the actionable error, verified in CI rather than only compiled. Test: `configured_inbox_fails_with_actionable_error_without_feature` (Task 14).

---

## File Structure

| File | Responsibility after this plan |
|---|---|
| `crates/memory-mcp/src/tenancy/api.rs` | Tenant spec/identity/resolution policy and the `TenantRuntimeFactory` port. No runtime cache, no activation registry, no capacity or TTL. |
| `crates/memory-mcp/src/http/runtime/lifecycle.rs` | The single slot state machine (`SlotState`), generation counter, pins, concurrency and last-used. |
| `crates/memory-mcp/src/http/runtime/pool.rs` | Sole owner of activation, residency, capacity, eviction, backoff and activation timeout; the RAII attempt guard; the injected factory seam. |
| `crates/memory-mcp/src/http/runtime/guard.rs` | Operation pin and response lease; takes a pre-reserved pin instead of incrementing its own. |
| `crates/memory-mcp/src/http.rs` | Wires the instance `ShutdownState` and the registry-backed factory into the pool. |
| `crates/memory-mcp/src/embedding/infra.rs`, `api.rs`, `backfill_store.rs` | Canonical vector persistence: embedding-only conditional SQL and truthful outcomes. |
| `crates/memory-mcp/src/memory/fact_access_store.rs` | Access-heat writes limited to access fields. |
| `crates/memory-mcp/src/http/middleware/preflight.rs` | Request-shape and mirror validation; no membership check on an initialization proposal. |
| `crates/memory-mcp/tests/support/rust_source.rs` (new) | Test-only tokenizer and module-walk mechanics shared by the two source guards. |
| `crates/memory-mcp/tests/source_tree_integrity.rs` | Rooted declared-module reachability from Cargo roots. |
| `crates/memory-mcp/tests/public_surface_audit.rs` | Use-tree-aware export inventory and production-caller scope. |
| `.github/workflows/ci.yml` | One explicit no-default behavior-test row. |

---

### Task 1: Injected factory seam and a deterministic blocking factory

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/pool.rs` (add a `pub(crate)` constructor and a test factory next to the existing `#[cfg(test)] mod tests`)
- Test: `crates/memory-mcp/src/http/runtime/pool.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::tenancy::api::TenantRuntimeFactory` (existing trait: `async fn activate(&self, spec: TenantRuntimeSpec) -> Result<Self::Runtime, RuntimeFactoryError>`), `crate::http::runtime::storage::{TenantRuntime, RuntimeOptions, build_runtime_with_options}`, `crate::http::shutdown::ShutdownState`.
- Produces:
  - `pub(crate) fn with_factory(cap: usize, idle_ttl: Duration, capacity_wait: Duration, activation_timeout: Duration, per_tenant_concurrency: u32, factory: Arc<dyn crate::tenancy::api::TenantRuntimeFactory<Runtime = TenantRuntime>>, shutdown: ShutdownState) -> Self`
  - Test-only `struct BlockingFactory` in `pool.rs` with `fn entered(&self) -> &tokio::sync::Notify`, `fn release(&self)`, `fn calls(&self) -> u64`, and `impl TenantRuntimeFactory for BlockingFactory { type Runtime = TenantRuntime; }` that signals `entered`, awaits `release`, then calls `build_runtime_with_options`.

- [ ] **Step 1: Add the injected constructor**

`Pool::new`, `Pool::with_defaults` and `Pool::from_http_config` keep their signatures and build the registry-backed factory, then delegate to `with_factory`. One field, `factory: Arc<dyn TenantRuntimeFactory<Runtime = TenantRuntime>>`, replaces the concrete `tenancy` field. `RuntimeOptions` is constructed once and handed to the factory at construction; delete the `pool.runtime_options` field and the `factory().set_options(...)` call in `from_http_config`.

- [ ] **Step 2: Add the blocking factory used by Tasks 2 and 5–7**

```rust
struct BlockingFactory {
    inner: RegistryTenantRuntimeFactory,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    calls: std::sync::atomic::AtomicU64,
}

#[async_trait::async_trait]
impl TenantRuntimeFactory for BlockingFactory {
    type Runtime = TenantRuntime;
    async fn activate(
        &self,
        spec: TenantRuntimeSpec,
    ) -> Result<TenantRuntime, RuntimeFactoryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_waiters();
        self.release
            .acquire()
            .await
            .map_err(|_| RuntimeFactoryError::Storage(MemoryError::Unavailable("released".into())))?
            .forget();
        self.inner.activate(spec).await
    }
}
```

`inner` is the registry-backed factory the pool builds today; a test constructs it with an in-memory `RegistryHandle` and `RuntimeOptions::default()`. The runtime is built only after release, so a cancelled attempt proves the abandoned work stops.

- [ ] **Step 3: Run the existing pool unit tests unchanged**

Run: `cargo test -p memory_mcp --lib http::runtime::pool --features streamable-http,test-fixtures --locked`
Expected: PASS. Existing `#[cfg(test)]` tests in `pool.rs` are the interface contract for every later task in this lane.

- [ ] **Step 4: Commit**

```bash
git add crates/memory-mcp/src/http/runtime/pool.rs
git commit -m "refactor(http): inject the runtime factory into the pool"
```

---

### Task 2: Pin the cancellation, capacity and pinning counterexamples

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/pool.rs` (`#[cfg(test)] mod tests`)
- Modify: `crates/memory-mcp/src/http/runtime/lifecycle.rs` (`#[cfg(test)] mod tests`, only for assertions about slot state that survive Task 3/4)
- Test: same two modules

**Interfaces:**
- Consumes: `Pool::with_factory` and `BlockingFactory` from Task 1.
- Produces: the regression names the later tasks must keep green.

- [ ] **Step 1: Write the failing tests**

Two of them inline, the rest by name and assertion (each is a separate
`#[tokio::test]` in `pool.rs`'s `#[cfg(test)] mod tests`):

```rust
#[tokio::test]
async fn cancelled_activation_leader_allows_retry() {
    let (pool, factory, spec) = pool_with_blocking_factory().await;
    let leader = tokio::spawn({
        let pool = pool.clone();
        let spec = spec.clone();
        async move { pool.acquire_spec_with_limit(&spec, 1).await }
    });
    factory.entered.notified().await;
    leader.abort();
    let _ = leader.await;
    factory.release.add_permits(1);
    let retry = tokio::time::timeout(Duration::from_secs(5), pool.acquire_spec_with_limit(&spec, 1))
        .await
        .expect("retry must terminate rather than wait for a dead producer");
    assert!(retry.is_ok(), "a cancelled attempt must be retryable");
    assert_eq!(factory.calls(), 2, "the retry must start a new factory call");
}

#[tokio::test]
async fn cancelled_activation_notifies_existing_followers() {
    // Leader and follower both enter acquisition; the leader is aborted after
    // the factory signals `entered`. The follower must terminate with an error
    // within a test timeout instead of waiting on a producer that no longer exists.
}

#[tokio::test]
async fn cancelled_slots_do_not_exhaust_capacity() {
    // Capacity 1; the only activation is abandoned; a different tenant's
    // acquisition must succeed rather than return CapacityTimeout.
}
```

| Test | Assertion |
|---|---|
| `cancelling_a_follower_does_not_cancel_activation` | aborting a follower still lets the leader activate |
| `timed_out_activation_retries_after_backoff` | timeout sets one backoff window, then a retry succeeds |
| `factory_panic_does_not_poison_activation` | a panicking factory leaves the tenant retryable |
| `older_generation_cannot_clear_new_activation` | a late completion from generation *n* cannot clear *n+1* |
| `runtime_is_pinned_before_waiting_for_tenant_permit` | an eviction sweep cannot unload a runtime a waiter is about to use |
| `binding_mismatch_is_rejected_while_loading_and_ready` | same tenant id, different namespace/database, fails closed on both paths |

`pool_with_blocking_factory()`, `pool_with_capacity(...)` and `blocking_factory()` are test helpers that build `Pool::with_factory` over an in-memory `RegistryHandle` and return `(Arc<Pool>, Arc<BlockingFactory>, TenantRuntimeSpec)`.

- [ ] **Step 2: Run them and record exactly which fail how**

Run: `cargo test -p memory_mcp --lib http::runtime::pool --features streamable-http,test-fixtures --locked`
Expected: `cancelled_activation_leader_allows_retry`, `cancelled_activation_notifies_existing_followers`, `cancelled_slots_do_not_exhaust_capacity`, `factory_panic_does_not_poison_activation`, `older_generation_cannot_clear_new_activation`, `runtime_is_pinned_before_waiting_for_tenant_permit` and `binding_mismatch_is_rejected_while_loading_and_ready` FAIL (timeout, hang, or wrong outcome). `timed_out_activation_retries_after_backoff` may already pass; keep it as the guard that Tasks 3–5 must not break. Record the observed failure text in the commit message.

- [ ] **Step 3: Commit the red tests**

```bash
git add crates/memory-mcp/src/http/runtime/pool.rs crates/memory-mcp/src/http/runtime/lifecycle.rs
git commit -m "test(http): pin the activation cancellation counterexamples"
```

---

### Task 3: Tenancy keeps policy and loses the runtime cache

**Files:**
- Modify: `crates/memory-mcp/src/tenancy/api.rs`
- Modify: `crates/memory-mcp/tests/tenancy_runtime.rs`
- Modify: `crates/memory-mcp/src/bootstrap/integration/tenancy_runtime.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub struct TenantRuntimeIdentity { pub tenant_id: String, pub namespace: String, pub database: String }`
  - `impl TenantRuntimeSpec { pub fn identity(&self) -> TenantRuntimeIdentity }`
  - Unchanged and still public: `TenantRuntimeSpec`, `TenantLifecycleStatus`, `RuntimeFactoryError`, `TenantRuntimeFactory`, `ResolveTenantPort`, `TenantResolution`, `TenantBinding`, `resolve_tenant_runtime`.
  - Removed: `Tenancy<F>`, `RuntimeLease<R>`, `Entry<R>`, the `ActivationSender`/`ActivationResult` aliases, `Tenancy::factory`, `Tenancy::activate`, `Tenancy::evict_tenant`, and the cache fields `entries`, `activating`, `capacity`, `idle_ttl`, `activation_timeout`.

- [ ] **Step 1: Delete the cache and the lifecycle implementation from `tenancy/api.rs`**

Keep the module doc's explanation of *why* the two resolution paths differ; extend it to say the runtime lifecycle lives in the HTTP runtime pool, citing ADR-0072. Purge the now-stale paragraph that promises the pool must call `evict_tenant`.

- [ ] **Step 2: Add the identity projection**

```rust
/// The part of a runtime specification that must not change while a runtime
/// is resident: which tenant, and which storage it is bound to.
///
/// Plan, schema and concurrency are deliberately excluded. They describe the
/// runtime revision, and a revision change is a replacement rather than a
/// binding conflict; lifecycle status describes permission, which is resolved
/// per request and never cached. See ADR-0072.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantRuntimeIdentity {
    pub tenant_id: String,
    pub namespace: String,
    pub database: String,
}

impl TenantRuntimeSpec {
    pub fn identity(&self) -> TenantRuntimeIdentity { /* tenant_id, namespace, database */ }
}
```

- [ ] **Step 3: Rewrite `tests/tenancy_runtime.rs` against the surviving interface**

The file currently tests `Tenancy` activation directly. Delete the activation/lifecycle tests from it (Tasks 4–6 add their behavioural equivalents to `pool.rs`), keep anything that pins resolution or the factory port, and add `fn identity_ignores_status_plan_and_schema()` and `fn identity_separates_tenants_and_namespaces()`. The file must compile against the new public interface and must not reference `Tenancy` or `RuntimeLease`.

- [ ] **Step 4: Run the crate build and the surviving tests**

Run: `cargo test -p memory_mcp --lib tenancy --locked` and `cargo check -p memory_mcp --all-targets --features fs-watch,mcp-apps,streamable-http --locked`
Expected: PASS. This task alone will fail to compile `pool.rs` until Task 4 lands — that is expected; run both tasks as one commit range if the executor cannot stage a compiling intermediate state, and say so in the commit message.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/tenancy/api.rs crates/memory-mcp/tests/tenancy_runtime.rs crates/memory-mcp/src/bootstrap/integration/tenancy_runtime.rs
git commit -m "refactor(tenancy): the runtime cache moves to its HTTP owner"
```

---

### Task 4: One activation state machine with RAII cleanup

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/lifecycle.rs`
- Modify: `crates/memory-mcp/src/http/runtime/pool.rs`
- Modify: `crates/memory-mcp/src/http/runtime/guard.rs`
- Test: `crates/memory-mcp/src/http/runtime/pool.rs` (Task 2's tests) and `crates/memory-mcp/src/http/runtime/lifecycle.rs`

**Interfaces:**
- Consumes: `TenantRuntimeIdentity` (Task 3).
- Produces:
  - `pub enum SlotState { Absent, Loading, Ready { runtime: Arc<TenantRuntime> }, Failed { retry_at: Instant }, Draining { runtime: Arc<TenantRuntime> } }`
  - `pub struct TenantRuntimeSlot { pub identity: TenantRuntimeIdentity, pub revision: RuntimeRevision, pub generation: u64, pub state: SlotState, pub completion: Option<watch::Sender<Option<Result<(), MemoryError>>>>, pub pins: Arc<AtomicU32>, pub concurrency: Arc<Semaphore>, pub last_used: Instant }`
  - `pub struct RuntimeRevision { pub plan_version: u32, pub schema_version: u32, pub concurrency: u32 }`
  - `impl TenantRuntimeSlot { pub fn new(identity, revision); pub fn ready_runtime(&self); pub fn in_negative_backoff(&self, now); pub fn is_reclaimable(&self, now, idle_ttl); pub fn begin_loading(&mut self); pub fn subscribe(&self); pub fn finish_attempt(&mut self, generation, result, now); pub fn abandon(&mut self, generation) }`
  - `ActivationAttempt::complete` publishes under the shared shutdown gate; `Drop` abandons the generation if the producer future is cancelled or unwinds.
  - `OperationGuard::from_reservation(runtime, pins, release_notify, permit)` takes an already-counted pin. Dropping the guard decrements it and notifies pool waiters.

- [ ] **Step 1: Replace the duplicated state fields with `SlotState`**

Delete `RuntimePhase`, `ActivationSlot` and the split `phase`/`runtime`/`activation` fields. `TenantRuntimeSlot::new_with_limit(limit)` keeps its signature and starts `Absent`, `generation: 0`, `completion: None`. Update the `#[cfg(test)]` tests in `lifecycle.rs` that asserted on `phase`/`ActivationSlot.generation` to assert on `SlotState` and `generation`; keep every assertion's intent (fresh slot has no runtime, concurrency limit is honoured, backoff gates).

- [ ] **Step 2: Put the pool's bookkeeping under one short-held `std::sync::Mutex`**

Replace `map: tokio::sync::Mutex<LruCache<..>>` and the per-slot `tokio::sync::Mutex` with `state: std::sync::Mutex<PoolState>` where `PoolState { map: LruCache<String, TenantRuntimeSlot>, waiters: tokio::sync::Notify }`. Runtime values removed under the lock are dropped *after* it is released. The `eviction_scheduler_job` and `evict_idle` become synchronous-lock operations that return how many runtimes they unloaded; the job wraps that in `async move`.

- [ ] **Step 3: Make the attempt guard own cancellation**

`acquire_spec_with_limit` compares identity and revision under the pool lock. A Ready slot at the requested revision reserves its pin there, before waiting for a concurrency permit. A Loading slot is non-reclaimable and followers subscribe to its completion. The request that creates Loading runs the factory outside the lock under one activation timeout and owns an `ActivationAttempt`. `Drop` returns its still-current generation to `Absent`; `finish_attempt` sends a terminal watch value before dropping the sender. Cancellation has no failure backoff; factory error and timeout retain the 5-second backoff.

- [ ] **Step 4: Make followers observe completion without holding a slot lock**

A follower's `watch::Receiver<Option<Result<(), MemoryError>>>` is created while Loading; `wait_for` observes the saved completion value or channel closure. A closed channel means the attempt was abandoned → `Err(PoolError::ActivationFailed)`. A follower never calls `slot_for` again to re-find its slot.

- [ ] **Step 5: Run the Task 2 tests**

Run: `cargo test -p memory_mcp --lib http::runtime::pool --features streamable-http,test-fixtures --locked`
Expected: `cancelled_activation_leader_allows_retry`, `cancelled_activation_notifies_existing_followers`, `cancelling_a_follower_does_not_cancel_activation`, `factory_panic_does_not_poison_activation` and `older_generation_cannot_clear_new_activation` PASS. Capacity, pinning and binding tests may still fail until Tasks 5–6.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/http/runtime/lifecycle.rs crates/memory-mcp/src/http/runtime/pool.rs crates/memory-mcp/src/http/runtime/guard.rs
git commit -m "fix(http): one activation state machine that survives cancellation"

---

### Task 5: Reservations, capacity reclamation and bounded waits

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/pool.rs`
- Modify: `crates/memory-mcp/src/http/runtime/guard.rs`
- Test: `crates/memory-mcp/src/http/runtime/pool.rs`

**Interfaces:**
- Consumes: Task 4's `SlotState`, `TenantRuntimeSlot::pins`, and the pool's `Arc<Notify>` waiter.
- Produces:
  - `pub(crate) struct SlotReservation { pins: Arc<AtomicU32>, release_notify: Arc<Notify>, transferred: bool }` with `acquire(...)`, `into_guard(...)`, and `Drop` decrementing the pin and waking waiters.
  - `TenantRuntimeSlot::is_reclaimable(...)` is the single eligibility predicate used by idle eviction and capacity reclamation; map removals return a slot for destruction after the lock is released.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn runtime_is_pinned_before_waiting_for_tenant_permit() { /* Task 2's test, now expected to pass */ }

#[tokio::test]
async fn failed_slots_are_reclaimed_under_capacity_pressure() {
    // Capacity 1. Tenant A's factory fails. Tenant B must still acquire once
    // the backoff window has passed, rather than seeing CapacityTimeout.
}

#[tokio::test]
async fn capacity_wakeups_do_not_extend_the_deadline() {
    // Capacity 1 with the only slot Loading. Repeated eviction-tick wakeups
    // must not push the acquisition deadline out; the wait fails at
    // capacity_wait, not later.
}
```

- [ ] **Step 2: Reserve the pin while still holding the lock**

Order inside the lock: compare identity → (`Loading`: subscribe; `Ready` at same revision: reserve pin) → otherwise publish/revision state. Loading itself is non-reclaimable, so an activation leader does not need a pin before construction. The reservation is dropped by `SlotReservation::drop` on every permit error/cancellation path and transferred into `OperationGuard` on success without a second increment.

- [ ] **Step 3: Make one reclamation rule and use it everywhere**

Reclaimable: `Absent`, `Ready` with `pins == 0` and `last_used <= now - idle_ttl`, and `Failed` whose `retry_at` has passed. Never reclaimable: `Loading`, `Draining`, or any slot with `pins > 0`. `LruCache::put` must not implicitly evict a non-reclaimable entry — the decision returns the popped slot so it is dropped outside the lock before retrying insertion.

- [ ] **Step 4: Recheck the full predicate at a bounded deadline**

Replace the 10 ms sleep-poll with one `tokio::time::timeout(capacity_wait, ...)` around a loop that (a) registers on `PoolState::waiters` *before* checking, (b) re-checks eligibility under the lock, (c) `notified().await`. One absolute deadline covers the whole wait.

- [ ] **Step 5: Run the pool unit tests**

Run: `cargo test -p memory_mcp --lib http::runtime::pool --features streamable-http,test-fixtures --locked`
Expected: PASS, including the pre-existing `capacity_one_returns_capacity_timeout_when_pinned` and `idle_runtime_is_evicted_to_recover_capacity`.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/http/runtime/pool.rs crates/memory-mcp/src/http/runtime/guard.rs
git commit -m "fix(http): reserve the pin early and reclaim terminal slots"
```

---

### Task 6: Binding identity and runtime revision replacement

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/pool.rs`
- Test: `crates/memory-mcp/src/http/runtime/pool.rs`

**Interfaces:**
- Consumes: `TenantRuntimeIdentity` (Task 3), `SlotState::Draining`, `reclaim_for` (Task 5).
- Produces:
  - `fn check_binding(slot: &TenantRuntimeSlot, spec: &TenantRuntimeSpec) -> Result<(), PoolError>` — identity mismatch is `PoolError::ActivationFailed` for now; add `PoolError::BindingConflict` only if a caller needs to distinguish it (record the decision either way in the commit message).
  - `fn needs_replacement(slot: &TenantRuntimeSlot, revision: &RuntimeRevision) -> bool`.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn binding_mismatch_is_rejected_while_loading_and_ready() { /* Task 2's test, now expected to pass */ }

#[tokio::test]
async fn plan_change_replaces_the_runtime_after_pins_drain() {
    // Acquire with plan_version 1. While a guard is held, a second acquisition
    // with plan_version 2 must not return the plan-1 runtime, and must not
    // replace it while the pin is held. After the guard drops, the next
    // acquisition builds a runtime carrying plan 2.
}

#[tokio::test]
async fn status_change_is_not_a_binding_conflict() {
    // Ready tenant re-acquired with TenantLifecycleStatus::Deleting for
    // maintenance returns the resident runtime rather than a conflict.
}
```

- [ ] **Step 2: Compare identity, not the whole spec**

`spec.identity() == slot.identity` gates warm reuse, joining a `Loading` attempt and reusing a `Failed` slot. A mismatch never replaces the slot, never joins its channel and never sets its backoff.

- [ ] **Step 3: Replace, do not conflict, on a revision change**

When `Ready`/`Failed` and `needs_replacement(&slot, &revision)`: if `pins == 0`, unload to `Absent` and activate in the same call; if `pins > 0`, mark `Draining` and keep retrying within `capacity_wait` (the waiter itself must not pin the revision it is replacing), then activate. `Loading` with a stale revision waits for that attempt to reach a terminal state first.

- [ ] **Step 4: Run the pool unit tests**

Run: `cargo test -p memory_mcp --lib http::runtime::pool --features streamable-http,test-fixtures --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/runtime/pool.rs
git commit -m "fix(http): bind by identity and replace a stale runtime revision"
```

---

### Task 7: Shutdown terminates pending acquisition without detaching cleanup

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/pool.rs`
- Modify: `crates/memory-mcp/src/http.rs`
- Test: `crates/memory-mcp/src/http/runtime/pool.rs`, `crates/memory-mcp/tests/http_crash_recovery.rs`

**Interfaces:**
- Consumes: `crate::http::shutdown::ShutdownState` (existing: `begin()`, `is_shutting_down()` — use exactly the accessor that exists), Task 1's `with_factory`.
- Produces: `Pool::with_factory` takes the instance's `ShutdownState`; `Pool::new`/`with_defaults` build an uncancelled one.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn shutdown_terminates_all_pending_acquisition_stages() {
    // Loading: a blocked factory, then shutdown.begin(); the leader and its
    // followers must fail rather than wait for release.
    // Capacity: an acquisition waiting for a slot must fail.
    // Permit: an acquisition waiting for a tenant permit must fail.
}

#[tokio::test]
async fn shutdown_prevents_late_runtime_publication() {
    // The factory completes after shutdown.begin(); the attempt must publish
    // shutdown failure, not Ready.
}
```

- [ ] **Step 2: Observe shutdown at every wait site**

Check `is_shutting_down()` under the lock before publishing `Loading`, and in the completion path before publishing `Ready`. Pending `watch::changed()`, `Semaphore::acquire_owned` and capacity waits select on the shutdown signal. Cancelled-but-not-timed-out is the existing `PoolError::ShuttingDown`; do not invent a new variant.

- [ ] **Step 3: Wire the instance state in `http.rs`**

The pool is built from the same `ShutdownState` the router and signal watcher use. Already-returned guards and their response bodies keep working until the existing drain completes; verify `tests/http_load_concurrency.rs` and `tests/http_crash_recovery.rs` still pass.

- [ ] **Step 4: Run the HTTP tests**

Run: `cargo test -p memory_mcp --test http_crash_recovery --test http_load_concurrency --features streamable-http,mcp-apps,test-fixtures --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/runtime/pool.rs crates/memory-mcp/src/http.rs
git commit -m "fix(http): shutdown terminates pending acquisition"
```

---

### Task 8: The real deadline path, and the last of the duplicate lifecycle

**Files:**
- Modify: `crates/memory-mcp/tests/common/http_server.rs` (only if the fixture needs factory injection)
- Modify: `crates/memory-mcp/tests/http_proto_conformance.rs` or a new `crates/memory-mcp/tests/http_activation_cancellation.rs`
- Modify: `docs/adr/0072-single-owner-for-tenant-runtime-activation.md` (status only)

**Interfaces:**
- Consumes: everything from Tasks 1–7.
- Produces: the P1 closure evidence.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn http_deadline_during_activation_does_not_strand_tenant() {
    // Production middleware ordering (deadline before runtime acquisition) with a
    // blocked injected factory and a request deadline shorter than the activation
    // timeout. The first request must fail with the deadline response; a second
    // request for the same tenant must then activate and succeed.
}
```

- [ ] **Step 2: Run it and confirm it fails against the pre-fix behaviour**

Record the observed failure (never-terminating second request, or `ActivationFailed` after an attempted retry) before the fix is applied; if the fixture cannot express the ordering, record that limitation instead of weakening the test.

- [ ] **Step 3: Prove the removal is complete**

Run: `cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,test-fixtures --locked` and confirm nothing references `Tenancy`, `RuntimeLease`, `RuntimePhase` or `ActivationSlot`. Record the caller/adapter/test inventory for `Pool::with_factory`, `SlotReservation`, `ActivationAttempt` and `TenantRuntimeIdentity` in the commit message.

- [ ] **Step 4: Run the lane's full gate**

Run:
```sh
cargo test -p memory_mcp --lib http::runtime --features streamable-http,test-fixtures --locked
cargo test -p memory_mcp --test tenancy_runtime --test tenancy_resolution --test tenant_lifecycle --test http_load_concurrency --test http_crash_recovery --test http_proxy_streaming --features streamable-http,mcp-apps,test-fixtures --locked
cargo fmt --all --check
```
Expected: PASS.

- [ ] **Step 5: Mark ADR-0072 implemented and commit**

Change the ADR status line to record that the decision is implemented, naming this plan. Commit:
```bash
git add crates/memory-mcp/tests crates/memory-mcp/src docs/adr/0072-single-owner-for-tenant-runtime-activation.md
git commit -m "test(http): prove the deadline path recovers, closing the activation audit"
```

---

## Lane B — canonical vector persistence (P2)

### Task 9: Truthful vector outcomes and embedding-only conditional writes

**Files:**
- Modify: `crates/memory-mcp/src/embedding/api.rs`, `crates/memory-mcp/src/embedding/infra.rs`, `crates/memory-mcp/src/embedding/backfill_store.rs`
- Modify: `crates/memory-mcp/src/embedding/service.rs`, `crates/memory-mcp/src/service/reembed.rs`, `crates/memory-mcp/src/service/embedding_recovery.rs` (call sites only)
- Test: `crates/memory-mcp/tests/embedding_vector_policies.rs`, `crates/memory-mcp/tests/embedding_canonical_vectors.rs`, `crates/memory-mcp/src/embedding/backfill_store.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `DbClient::query`, `DbClient::select_one`, `BoundDbClient`, `crate::knowledge::queries`.
- Produces:
  - `pub trait CanonicalVectorPort { async fn stored_fact_vector(&self, fact_id: &str) -> Result<StoredVector, MemoryError>; async fn apply_fact_vector(&self, fact_id: &str, vector: Vec<f64>, identity: VectorIdentity, at: DateTime<Utc>, policy: VectorWritePolicy) -> Result<VectorApplication, MemoryError>; }`
  - `pub async fn update_canonical_vector(...) -> Result<VectorApplication, MemoryError>` now returns the adapter's result instead of manufacturing `Applied`.
  - `EmbeddingBackfillStoreClient::update_embedding_fields(&self, fact_id: &str, fields: Value) -> Result<bool, MemoryError>` — `true` when a row was written.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn replace_stale_preserves_concurrent_fact_access() {
    // Read the fact through the port, mutate access_count behind the port's
    // back, then apply the vector with ReplaceStale. The stored access_count
    // must be the concurrent value, not the snapshot's.
}

#[tokio::test]
async fn fill_missing_loser_reports_already_current() {
    // Two writers both see a missing vector; the first wins, the second must
    // report AlreadyCurrent rather than Applied, and the winner's vector stays.
}

#[tokio::test]
async fn canonical_write_missing_fact_is_not_found() { /* NotFound, not Applied */ }

#[tokio::test]
async fn canonical_write_rejects_malformed_storage_result() { /* error, not Applied */ }

#[tokio::test]
async fn canonical_write_preserves_non_embedding_fields() { /* content, provenance, index_keys, scope */ }

#[tokio::test]
async fn vector_write_rejects_a_forged_record_target() { /* '⟩' in the id must not reach another record */ }
```

- [ ] **Step 2: Run them and confirm the lost-update test fails**

Run: `cargo test -p memory_mcp --test embedding_vector_policies --test embedding_canonical_vectors --locked`
Expected: `replace_stale_preserves_concurrent_fact_access` and `fill_missing_loser_reports_already_current` FAIL against the current whole-record `db.update` path. Record the observed failure.

- [ ] **Step 3: Make both policies one conditional embedding-only UPDATE**

Build a fresh payload of embedding fields only. Target the record with `type::record('fact', $id)` and bind `$id` as the suffix after the validated `fact:` prefix — the pattern already used in `http/tasks/worker.rs` and `knowledge/entity_store.rs`. Both policies share one SQL shape whose `WHERE` differs:

```sql
UPDATE type::record('fact', $id) SET embedding = $embedding, embedding_provider = $embedding_provider,
  embedding_dimension = $embedding_dimension, embedding_signature = $embedding_signature,
  embedding_updated_at = type::datetime($embedding_updated_at), embedding_model = $embedding_model
WHERE embedding IS NONE                       -- FillMissing
-- or
WHERE embedding IS NONE OR embedding_signature IS NONE OR embedding_signature != $embedding_signature
                                              -- ReplaceStale
```

Prove the `IS NONE`/`!=` behaviour of the pinned engine with an embedded-database test; do not infer it from Rust string comparison. Model is set explicitly (including `NONE`) so a provider change cannot leave stale metadata.

- [ ] **Step 4: Interpret the result strictly**

Use `DbClient::query` (not `query_rows`, which degrades malformed rows to empty). Exactly one returned fact means a write happened; an empty result is diagnosed with a narrow existence read: absent record → `NotFound`, present record → `AlreadyCurrent`; malformed or multiple rows → `MemoryError::Storage`. Update `embedding/service.rs`'s retry path and `reembed.rs` to consume the outcome; a `Skipped` reason stays a generation result, never a storage result.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p memory_mcp --test embedding_vector_policies --test embedding_canonical_vectors --locked && cargo test -p memory_mcp --lib embedding --locked`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/embedding crates/memory-mcp/src/service crates/memory-mcp/tests/embedding_vector_policies.rs crates/memory-mcp/tests/embedding_canonical_vectors.rs
git commit -m "fix(embedding): persist only vector fields and report the stored outcome"
```

---

### Task 10: The reciprocal access-heat writer

**Files:**
- Modify: `crates/memory-mcp/src/memory/fact_access_store.rs`
- Test: `crates/memory-mcp/src/memory/fact_access_store.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `DbClient::query`, `crate::shared::temporal`.
- Produces: `FactAccessStore::record_fact_access(&self, fact_id: &str, boost: i64) -> Result<(), MemoryError>` — unchanged signature; the count increment and timestamp update are one atomic access-fields-only statement.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn fact_access_preserves_concurrent_vector_write() {
    // Store a vector, then record access; the vector and its signature must
    // still be there. Against the current full-row writer this fails if the
    // snapshot predates the vector.
}

#[tokio::test]
async fn concurrent_access_updates_do_not_lose_increments() { /* simultaneous increments all persist */ }

#[tokio::test]
async fn record_fact_access_leaves_absent_records_alone() { /* still a no-op, no error */ }

#[tokio::test]
async fn record_fact_access_binds_forged_record_ids_as_values() { /* delimiter text is never SQL */ }

#[tokio::test]
async fn record_fact_access_saturates_at_the_lower_bound() { /* i64::MIN - 1 remains i64::MIN */ }
```

- [ ] **Step 2: Run them and confirm the vector-preservation test fails**

Run: `cargo test -p memory_mcp --lib memory::fact_access_store --locked`

- [ ] **Step 3: Update only the access fields**

Use one parameterized `UPDATE type::record('fact', $id)` statement that sets only `access_count` and `last_accessed`. The count expression handles a missing count as zero and clamps at both i64 bounds before adding; the single statement prevents concurrent retrievals from losing increments. Preserve the absent-record no-op (an update of a missing record affects no rows) and bind the suffix of the validated `fact:` id rather than interpolating it into SQL.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p memory_mcp --lib memory::fact_access_store --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/memory/fact_access_store.rs
git commit -m "fix(memory): the access writer stops rewriting whole facts"
```

---

## Lane C — initialization negotiation (P2)

### Task 11: An initialization proposal reaches negotiation

**Files:**
- Modify: `crates/memory-mcp/src/http/middleware/preflight.rs`
- Test: `crates/memory-mcp/tests/http_proto_conformance.rs`

**Interfaces:**
- Consumes: `is_legacy_revision`, `bad_request`, the existing `HttpServerFixture`.
- Produces: no new production symbol. The declared-revision membership check applies only when `body_method != "initialize"`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn unsupported_initialize_proposal_negotiates_legacy_revision() {
    // initialize with params.protocolVersion = "2099-01-01", no _meta, no
    // MCP-Protocol-Version header, valid Bearer token. Expect HTTP 200 and a
    // JSON-RPC result whose protocolVersion is a supported legacy revision,
    // with no Mcp-Session-Id.
}
```

Decode the JSON-RPC response; do not assert on a substring of the body.

- [ ] **Step 2: Run it to confirm the 400**

Run: `cargo test -p memory_mcp --test http_proto_conformance unsupported_initialize_proposal --features streamable-http,test-fixtures --locked`
Expected: FAIL with status 400 and body `HeaderMismatch: protocol version`.

- [ ] **Step 3: Scope the membership check**

In the legacy branch of preflight, skip the `is_legacy_revision(declared)` check when `body_method == "initialize"`. Keep every other check untouched: modern `_meta`/header agreement, the protocol header's own legacy membership, the declared-vs-header contradiction check, mirrored method/name agreement, host/origin and admission.

- [ ] **Step 4: Add the surrounding cases**

`known_initialize_revision_still_negotiates`, `malformed_initialize_proposal_is_rejected` (missing/null/number `protocolVersion`), `legacy_request_cannot_claim_a_modern_revision` (existing test, unchanged), `server_discover_advertises_every_known_revision` (existing, unchanged). Leave `docs/operations/HTTP_INTEROP_MATRIX.md` at `Not executed` — synthetic conformance is not a client connection.

- [ ] **Step 5: Run the file and commit**

Run: `cargo test -p memory_mcp --test http_proto_conformance --features streamable-http,test-fixtures --locked`

```bash
git add crates/memory-mcp/src/http/middleware/preflight.rs crates/memory-mcp/tests/http_proto_conformance.rs
git commit -m "fix(http): let an initialize proposal reach protocol negotiation"
```

---

## Lane D — architecture guard evidence (P2)

### Task 12: Rooted source-tree reachability

**Files:**
- Create: `crates/memory-mcp/tests/support/rust_source.rs`
- Modify: `crates/memory-mcp/tests/source_tree_integrity.rs`
- Test: both files

**Interfaces:**
- Produces: `tests/support/rust_source.rs`, included by each guard as `#[path = "support/rust_source.rs"] mod rust_source;`:
  - `Token { text: String }`, `tokenize(source: &str) -> Result<Vec<Token>, String>`, `matching_group(tokens, open)`, `is_identifier(name)`, and `is_open_group(token)`.
  - The reader removes comments and string/character/raw literals, tracks nested comments and delimiter groups, and errors on unterminated comments/literals. It is lexical support, not a Rust AST.
- Produces (local to `source_tree_integrity.rs`): `roots_from_metadata`, `rooted_module_graph`, Rust file-module search-directory handling, and explicit diagnostics for unsupported `#[path]`, unresolvable/ambiguous modules, and unsupported module-producing macros.
- Consumes: `serde_json` (already a direct dev dependency), `tempfile`, and `cargo metadata --no-deps`.

- [ ] **Step 1: Write the failing fixture tests**

```rust
#[test]
fn unrooted_mod_cycle_is_reported_unreachable() {
    // TempDir crate: src/lib.rs empty, src/a.rs `mod b;`, src/b.rs `mod a;`.
    // Neither a.rs nor b.rs is reachable from the lib root.
}

#[test]
fn ordinary_module_files_have_no_sibling_fallback() {
    // src/lib.rs `mod a;` with src/a.rs and src/b.rs: b.rs is unreachable,
    // and `a` must resolve to a.rs rather than to a sibling of a nonexistent a/.
}

#[test]
fn inline_modules_resolve_children_under_the_inline_directory() {
    // src/lib.rs `mod outer { mod inner; }` reaches src/outer/inner.rs.
}

#[test]
fn cargo_target_roots_are_not_identified_by_filename() { /* an explicit [[bin]] path is a root */ }

#[test]
fn ambiguous_module_locations_are_rejected() { /* child.rs and child/mod.rs both present */ }

#[test]
fn module_text_in_comments_strings_and_macros_is_not_a_declaration() { /* `// mod x;` never declares x */ }

#[test]
fn production_path_attributes_fail_closed() { /* #[path = "..."] is reported, not silently followed */ }
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p memory_mcp --test source_tree_integrity --locked`
Expected before the fix: the disconnected cycle is not reported as unreachable, or the inline module path fixture is reported missing.

- [ ] **Step 3: Implement the tokenizer and traversal**

Roots come from `cargo metadata --format-version 1 --no-deps --locked --offline`, parsed with `serde_json`, selecting only `lib`/`bin` targets for the exact memory-mcp manifest path. Traverse from those roots only, carrying the module search directory: crate-root children live beside the root; `foo.rs` children live in `foo/`; `mod.rs` children live beside it; an inline module changes the directory to `.../name/`. Union cfg-gated declarations (the graph is a declared-source union, not a proof every feature combination compiles). A module-generating macro invocation, `include!` outside `src/ui/assets.rs`, unresolved/ambiguous module or path attribute yields a diagnostic rather than an empty accepted result.

- [ ] **Step 4: Assert the live tree and report unreachable files**

Compare the disk inventory of `src/**/*.rs` against the reachable set. The assertion message must list every unreachable file, and the guard must remain silent about files it deliberately exempts.

- [ ] **Step 5: Run against the live tree**

Run: `cargo test -p memory_mcp --test source_tree_integrity --locked`
Expected: PASS with every current `src/**/*.rs` file reached from the Cargo lib/bin roots; the counterexample fixtures fail if the traversal regresses.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/tests/support/rust_source.rs crates/memory-mcp/tests/source_tree_integrity.rs
git commit -m "test(architecture): prove reachability from Cargo roots, not incoming edges"
```

---

### Task 13: Guarded exports, split impls and an honest caller claim

**Files:**
- Modify: `crates/memory-mcp/tests/public_surface_audit.rs`
- Modify: `crates/memory-mcp/tests/trait_methods_are_called.rs`
- Modify: `crates/memory-mcp/tests/support/rust_source.rs`
- Test: the same three files

**Interfaces:**
- Consumes: `rust_source::{tokenize, matching_group, is_identifier, is_open_group}` (Task 12); `use_tree_names` remains local to `public_surface_audit.rs`.
- Produces:
  - `reexported_names` enumerates exported leaves and aliases from public use trees, including `{"Fact", "ids"}` from `pub use crate::types::{Fact, ids};`; it expands the one local `constants::*` glob and rejects other public globs.
  - `collect_public_methods` token-scans every source file for inherent `impl MemoryService` blocks, including split and qualified impls, and records `(name, file)`.
  - The caller ratchet parses the named trait body, ignores cfg(test) items, comments and literals, and recognizes receiver, explicit `Trait::method`, and `<T as Trait>::method` syntax. It remains lexical, not receiver-type-resolved.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn grouped_use_of_a_cut_name_is_rejected() {
    // Parse `pub use crate::types::{Fact, ids};` and assert both names are seen
    // as re-exported, so the existing cut-name assertion fires.
}

#[test]
fn public_methods_are_found_after_moving_the_impl() {
    // A fixture with `impl MemoryService { pub async fn x(..) }` outside the
    // three currently hardcoded files is discovered.
}

#[test]
fn trait_table_checks_the_named_trait_not_same_named_functions() {
    // A free function `fn load()` must not satisfy a table row for
    // TaskStore::load; the declaration must come from the named trait body.
}

#[test]
fn lexical_calls_ignore_comments_and_string_literals() {
    // `.load(` inside a comment or a literal is not a call.
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p memory_mcp --test public_surface_audit --test trait_methods_are_called --locked`

- [ ] **Step 3: Fix the export parser and widen the impl scope**

Both use `rust_source` rather than the hand-rolled string splitting at `public_surface_audit.rs:98-109`. Widening the impl scope discovers `MemoryService::reembed_all_facts` in `src/service/reembed.rs`; confirm its caller exists (it has one) and record the baseline entry with its caller and reason rather than adding a blanket allowance. Keep the allowlist explicit and fail when an entry names no declaration.

- [ ] **Step 4: Bound the caller claim**

Detect calls from tokens, excluding declarations, comments and literals; recognize receiver calls and explicit `Trait::method`/`<T as Trait>::method`. Rename the assertion's doc comment to call it a *lexical usage ratchet*, and state in the file which certainty it does not provide (receiver type, aliasing, macros, blanket impls). Do not add `syn`.

- [ ] **Step 5: Run the guard suite**

Run: `cargo test -p memory_mcp --test public_surface_audit --test trait_methods_are_called --test source_tree_integrity --test doc_claims --locked`
Expected: PASS.

- [ ] **Step 6: Reconcile the ADR and commit**

Amend ADR-0065's assurance sentence so it describes a rooted *declared-module* graph with an explicit exemption list, rather than an unqualified claim, and note what the lexical guard does not prove.

```bash
git add crates/memory-mcp/tests docs/adr/0065-reinstate-the-source-tree-and-doc-claim-guards.md
git commit -m "test(architecture): parse grouped exports and split impls, bound the caller claim"
```

---

## Lane E — execute the filesystem-disabled profile (P2)

### Task 14: A bounded no-default behavior test row in CI

**Files:**
- Modify: `crates/memory-mcp/tests/fs_watch_process_disabled.rs`
- Modify: `.github/workflows/ci.yml`
- Test: `crates/memory-mcp/tests/fs_watch_process_disabled.rs`

**Interfaces:**
- Consumes: `CARGO_BIN_EXE_memory_mcp`, `tempfile`.
- Produces: `fn run_serve_with_inbox(inbox: Option<&Path>, deadline: Duration) -> Output` (was `run_serve_with_inbox(inbox: Option<&Path>)`), which kills and reaps the child on deadline expiry and reports the kill in its assertion message.

- [ ] **Step 1: Make the process tests bounded**

`Command::output()` and the `drop(stdin)` + `wait_with_output()` sequence can both block forever. Spawn with `Stdio::piped()`, poll `try_wait` to a hard deadline, `kill()` and `wait()` on expiry, and assert that the observed exit happened within the deadline. Keep both existing test names and their behavioral assertions.

- [ ] **Step 2: Run them locally with no defaults**

Run: `cargo test -p memory_mcp --no-default-features --test fs_watch_process_disabled --locked`
Expected: `2 passed`. Record the actual counts.

- [ ] **Step 3: Add the CI row**

```yaml
      - name: Filesystem-disabled process tests
        run: cargo test -p memory_mcp --no-default-features --test fs_watch_process_disabled --locked
```

Place it beside `Default workspace tests` and `Optional feature tests`. Do not use `--all-features` and do not touch the platform matrix or the no-default `cargo check` rows.

- [ ] **Step 4: Verify the row is inherited by release and that it is not vacuous**

Confirm `.github/workflows/release.yml` calls this workflow's `quality` job. Add a comment on the row naming the `cfg(not(feature = "fs-watch"))` gate it exercises, so a future reader does not delete it as redundant with the compile check.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/tests/fs_watch_process_disabled.rs .github/workflows/ci.yml
git commit -m "ci: execute the filesystem-disabled process behavior tests"
```

---

## Lane F — integrated verification

### Task 15: Whole-branch evidence and documentation reconciliation

**Files:**
- Modify: `docs/superpowers/specs/2026-10-03-architecture-audit-follow-up.md`, `docs/superpowers/plans/2026-10-03-architecture-audit-follow-up.md`, `GLOSSARY.md`

**Interfaces:**
- Consumes: every task above.
- Produces: the completed ledger.

- [ ] **Step 1: Run the whole gate**

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test --workspace --lib --bins --tests --locked
cargo test -p memory_mcp --lib --bins --tests --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked
cargo test -p memory_mcp --no-default-features --test fs_watch_process_disabled --locked
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate --locked
cargo run --locked -p xtask -- check-observability
```
Record the exact counts. A test that cannot run in this environment is reported as unrun, not as passing.

- [ ] **Step 2: Check the ledger against observed output**

For each of the five findings, write the command and its observed result into the plan's completion ledger. Nothing is marked complete on the strength of a successful compile.

- [ ] **Step 3: Reconcile the glossary and prior plan**

`GLOSSARY.md` keeps only domain terms (no file paths, no code); drop or rewrite anything that reads as an implementation note. The September plan gets a one-line follow-up link, and its unchecked review templates stay untouched.

- [ ] **Step 4: Verify the documentation guards still pass and commit**

Run: `cargo test -p memory_mcp --test doc_claims --locked`

```bash
git add docs GLOSSARY.md
git commit -m "docs: record the follow-up audit evidence"
```

---

## Self-Review

Run after writing the plan, against the spec with fresh eyes.

**1. Spec coverage.** P1 activation → Tasks 1–8 (seam, counterexamples, one owner,
state machine, reservations/capacity, binding/revision, shutdown, real deadline
path). P2 vectors → Tasks 9–10. P2 negotiation → Task 11. P2 guards → Tasks 12–13.
P2 CI → Task 14. Integrated evidence → Task 15. The spec's non-goals have no task,
which is the point; no spec requirement lacks a task.

**2. Step scan.** Every step names one action and its expected result. Deliberate
content decisions the implementer cannot derive: the identity projection's exact
field set, `Loading` carrying completion rather than the runtime, cancellation not
setting `Failed` backoff, `type::record('fact', $id)` instead of identifier
interpolation, strict `query` plus one existence read, and `body_method !=
"initialize"` as the only preflight relaxation. No step says "handle edge cases".

**3. Type consistency.** `TenantRuntimeIdentity` and `TenantRuntimeSpec::identity()`
are defined in Task 3 and used in Tasks 4 and 6. `SlotState`, `RuntimeRevision`,
`TenantRuntimeSlot::begin_loading`/`finish_attempt`, `ActivationAttempt`,
`SlotReservation` and `is_reclaimable` are defined once in Tasks 4–5 and referenced
by name afterwards. `CanonicalVectorPort::apply_fact_vector ->
VectorApplication` and `apply_embedding_fields -> bool` are defined in Task 9
and used by Task 10 and the call sites. `rust_source::{Token, tokenize,
matching_group}` are defined in Task 12 and consumed in Tasks 12–13.

**4. Review Focus.** Each of the five uncovered input classes has an owning task
with a named test: activation cancellation (Task 2), concurrent fact mutation under
a vector write (Task 9), an unsupported initialize proposal (Task 11), an
unreachable file and a grouped cut-name export (Tasks 12–13), and the no-default
process behavior (Task 14).

**5. Proportion.** The plan is longer than the spec, which is expected for a
mechanical Rust change, but the code blocks are limited to what pins a decision:
two counterexample tests, the factory double, the identity projection, the SQL
predicate, and the CI row. Bodies are otherwise described as signatures plus
assertions.

**Review resolution.**

An independent standards/spec review reported the revision drain, runtime drop
under lock, missing capacity wakeup, late shutdown publication, identity metadata
mismatch, unrooted-source, use-tree/caller lexer and stale-document claims. The
implementation now drains until pins reach zero, destroys runtimes outside the
lock, notifies on reservation/guard release, gates publication with shutdown,
derives vector metadata only from `VectorIdentity`, roots source inventory at
Cargo lib/bin targets, parses guarded syntax through the bounded lexer, and
records current implementation status in this ledger. Targeted tests cover each
reported behavior.

The review identified the completed implementation before these fixes; both its
standards and spec reports are retained in the thread. Remaining limits are
explicitly in the spec: the caller ratchet is lexical and does not resolve
receiver types; real-client interoperability and target-specific Metal tests are
not run in this environment.

## Execution Handoff

Plan complete. Reviewers should read this plan with the
[spec](../specs/2026-10-03-architecture-audit-follow-up.md) and
[ADR-0072](../../adr/0072-single-owner-for-tenant-runtime-activation.md).

Lane A is a single refactor with tightly coupled interfaces: Tasks 4–7 all rewrite
the same file and each task's tests are the next task's contract. Lane B, C, D and
E are independent of it and of each other.
