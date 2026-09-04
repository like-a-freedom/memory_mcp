# Streamable HTTP SaaS Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Status (2026-09-04):** Core implementation complete; release
> verification incomplete; not production-ready. The
> `memory_mcp_http` binary composes durable production
> storage and migration adapters, the test-fixtures
> feature does not select them, the control plane
> workflow, application services, registry capabilities,
> and HTTP release evidence are all in place. External
> gates (proxy, 500-tenant, interop, restore, credential
> rotation) are recorded in
> `docs/operations/HTTP_RELEASE_GATE.md` and remain
> `Not executed — release blocked` until they run
> against a supported environment. The
> architecture-audit-remediation plan
> (`docs/superpowers/plans/2026-09-02-architecture-audit-remediation.md`)
> tracks the deferred Task 10 (Registry consumer
> migration) which is blocked on stable trait
> upcasting (RFC 3324).

**Goal:** Довести feature-gated `memory_mcp_http` до production-ready multi-user Streamable HTTP SaaS профиля, сохранив существующий stdio-профиль без изменения его поведения.

**Architecture:** HTTP binary остаётся отдельным composition root: authenticated Bearer API key → Account → ready Tenant → immutable namespace-bound Tenant Runtime. Control Registry хранится в отдельной SurrealDB namespace/database, а данные Tenant — в его server-generated namespace; ни MCP arguments, ни URL, ни произвольные OAuth claims не участвуют в выборе namespace. Все correctness-sensitive фоновые операции выполняются tracked process-level scheduler jobs с durable CAS/fencing, а `rmcp` остаётся только протокольным адаптером поверх durable service/storage seams.

**Tech Stack:** Rust 1.97.1, edition 2024, `rmcp` 3.1.2-compatible API as resolved by `Cargo.lock`, Axum 0.8, Tokio, SurrealDB 3.2.4, existing `MemoryService`/`DbClient` seams, `serde`/`serde_json`, `hmac`/`sha2`/`chacha20poly1305`, `lru`, Prometheus metrics, separate Dioxus 0.7 web/WASM crate.

**Spec:** `docs/superpowers/specs/2026-08-27-streamable-http-saas.md`

**ADR:** `docs/adr/0052-streamable-http-saas-profile.md`

**Supersedes:** `docs/superpowers/plans/2026-08-27-streamable-http-saas.md` as the execution plan for unfinished work. The earlier plan and its already-landed transport scaffolding remain historical; do not revert its working-tree fixes.

## Global Constraints

- Protocol target is MCP `2026-07-28`; HTTP profile is modern-only and does not add legacy HTTP+SSE compatibility.
- `/mcp` is one `POST` route; `GET` and `DELETE` return `405`; no MCP protocol session, `Mcp-Session-Id`, `Last-Event-ID`, or resumable stream exists.
- `MCP-Protocol-Version`, `_meta.io.modelcontextprotocol/protocolVersion`, `Mcp-Method`, and method-specific `Mcp-Name` are validated before routing/auth decisions trust them.
- `memory_mcp` remains the existing stdio/CLI profile with one startup-bound Active Namespace; `memory_mcp_http` has no memory-operation CLI commands.
- Package defaults remain `[]`; `streamable-http`, `control-plane`, `control-plane-ui`, and `test-fixtures` are additive. Do not add MCP tools; the frozen eight-tool surface remains unchanged.
- Configuration is environment-only and follows 12-factor rules. The complete HTTP profile is validated before binding the listener.
- No mandatory nonstandard `Idempotency-Key` is introduced. Retry safety comes from domain fingerprints, unique constraints, CAS/versioning, and reconciliation.
- Raw OIDC subjects, API-key secrets, cookies, verifier fragments, email, memory content, namespaces, SQL, and provider credentials never appear in logs or external errors.
- Sensitive flow material uses keyed verifiers or approved AEAD; application-level encryption of memory content, entities, facts, embeddings, App payloads, and Task results is out of scope.
- The Tenant Registry is separate from Tenant namespaces; ordinary MCP tools cannot query the Registry and request input never contains a namespace selector.
- Account/Tenant/identity/credential/lease/provisioning/audit history is durable and non-reusable. Memory facts and audit-bearing records are invalidated/retained, never physically deleted. Only expired ephemeral Task/App Session rows may be physically removed.
- Embedded SurrealDB remains available only for development, demonstration, and single-process testing. It must emit a prominent non-production warning and must not be presented as HA or production storage.
- `/metrics` has no application authentication by decision; production restriction is a reverse-proxy/network responsibility.
- `subscriptions/listen` is long-lived request-scoped SSE. It is exempt from the ordinary 120-second body deadline, has separate bounded admission, does not indefinitely pin a full runtime, and rechecks authorization at least every 30 seconds and always within 60 seconds.
- `ingest` is always a synchronous durable commit. `extract` may return a durable Task only when the client advertises Tasks; otherwise it runs synchronously under a bounded policy or returns a preflight rejection.
- All correctness-sensitive scheduler jobs are tracked and joined during shutdown. No production no-op scheduler, placeholder adapter, ignored release gate, unconditional in-memory backend, or detached correctness worker is allowed.
- Production code uses `MemoryError`/`thiserror` errors and contains no `unwrap()`, `expect()`, `todo!()`, `unimplemented!()`, or silent warning helper. Test-only panics may remain in test fixtures where they identify a failed assertion.
- Any dependency or `Cargo.toml` change requires the existing dependency approval gate before implementation. Prefer current dependencies and standard library APIs.
- Every task below ends with a focused test command and a separate commit. Do not commit unrelated user changes or the existing review fixes.

---

## Current baseline and non-goals of this successor plan

The current branch already has passing transport/conformance scaffolding and several security fixes. The following are **not** accepted as evidence of completion:

- `test-fixtures` in-memory registry behavior is not production persistence.
- A declared trait, scheduler factory, or builder is not implementation until a production composition root invokes it.
- A `Result<(), _>` no-op, a false `ping()`, or a comment saying “later” is not a safe production fallback.
- A passing unit test over `Mem` is not evidence for remote SurrealDB connection, multi-replica fencing, crash recovery, or restart durability.
- The working tree contains uncommitted review fixes. Start by preserving them; do not reset or clean the tree.

## File map

### Control and tenant storage

- Modify `crates/memory-mcp/src/http/registry/models.rs` — canonical durable Registry DTOs, statuses, plan/usage/deletion records, and serialization validation.
- Modify `crates/memory-mcp/src/http/registry/storage.rs` — `RegistryStore` contract and test-only `InMemoryStore`; remove the production placeholder implementation.
- Create `crates/memory-mcp/src/http/registry/surreal_store.rs` — remote/embedded SurrealDB Registry implementation, query/result conversion, CAS helpers, and error mapping.
- Modify `crates/memory-mcp/src/http/registry/mod.rs` — production `RegistryHandle` constructors and privileged-engine ownership.
- Modify `crates/memory-mcp/src/http/registry/migrations.rs` — real control namespace migration catalog, ledger, checksums, postconditions, and recovery.
- Modify `crates/memory-mcp/migrations/001_registry.surql` — complete control schema and unique/invariant indexes.
- Create `crates/memory-mcp/migrations/044_task_artifacts.surql` — durable extraction artifact/reconciliation records.
- Create `crates/memory-mcp/migrations/045_deletion_and_usage_hardening.surql` — deletion challenges/tombstones, usage fields, and App Session counter/invariant schema if not folded into the earlier migration files.

### HTTP composition and runtime

- Modify `crates/memory-mcp/src/http/config.rs` — complete environment contract and fail-closed validation.
- Modify `crates/memory-mcp/src/http/mod.rs` — production control/tenant connections, dependency probes, runtime pool, and scheduler dependencies.
- Modify `crates/memory-mcp/src/http/router.rs` — route groups, control-plane middleware, API precedence, SPA fallback, and security layers.
- Modify `crates/memory-mcp/src/http/runtime/{mod.rs,pool.rs,lifecycle.rs,activation.rs,guard.rs,storage.rs}` — bounded eviction, per-Tenant concurrency, shutdown, and runtime-owned durable stores.
- Modify `crates/memory-mcp/src/http/leases/{mod.rs,migration.rs,scheduler.rs}` — production provisioning adapter, due queries, fencing, maintenance jobs, and scheduler lifecycle.
- Modify `crates/memory-mcp/src/http/health.rs`, `logging.rs`, `metrics.rs`, `server.rs`, `shutdown.rs` — truthful readiness, structured events, and graceful termination.
- Modify `crates/memory-mcp/src/bin/memory_mcp_http.rs` — thin production composition root that starts the real scheduler set.

### Durable capabilities and protocol adapters

- Modify `crates/memory-mcp/src/storage/client.rs` — bound transaction seam and embedded/remote connection support required by the HTTP profile.
- Modify `crates/memory-mcp/src/storage/migrations.rs` — expose the complete tenant migration catalog and reusable migration runner.
- Modify `crates/memory-mcp/src/http/app_sessions/{store.rs,scheduler.rs,mod.rs}` — atomic cap/version/tenant predicates and actual cleanup.
- Modify `crates/memory-mcp/src/http/tasks/{state.rs,worker.rs,scheduler.rs,mod.rs}` — durable worker execution, fenced transitions, artifact reconciliation, and retention.
- Modify `crates/memory-mcp/src/http/subscriptions/{outbox.rs,stream.rs,scheduler.rs,mod.rs}` — transactionally integrated outbox, filter enforcement, bounded streaming, and repair.
- Modify `crates/memory-mcp/src/mcp/handlers.rs` and `src/mcp/handlers/apps.rs` — HTTP durable backend dispatch while preserving stdio in-memory behavior.
- Modify relevant files under `crates/memory-mcp/src/service/` and `src/tools/` — quota and outbox hooks at canonical mutation boundaries, without putting business logic in `main.rs` or raw SQL in MCP handlers.

### Control plane and UI

- Modify `crates/memory-mcp/src/control/{oidc.rs,session.rs,csrf.rs,recent_auth.rs,account_api.rs,operator.rs,deletion.rs,error.rs,static_assets.rs}` — complete OIDC/account/operator/deletion behavior and safe response mapping.
- Create `crates/memory-mcp/build.rs` — deterministic compiled-Dioxus asset manifest generation; fail the UI build if a bundle is missing.
- Modify `crates/control-plane-ui/{Cargo.toml,src/**/*.rs}` and create `crates/control-plane-ui/assets/` — working Dioxus routes/pages, same-origin API client, CSRF flow, one-time key display, deletion disclosure, and no browser storage for secrets.
- Modify `docs/operations/RESTORE_DRILL.md`, `docs/operations/CREDENTIAL_ROTATION.md`, `docs/operations/CONFORMANCE.md`, and `README.md` — correct environment names and document the actual operational contract.
- Modify `docs/superpowers/specs/2026-08-27-streamable-http-saas.md` and `docs/adr/0052-streamable-http-saas-profile.md` only after implementation gates pass — mark implementation status accurately and record any approved operational caveat.

### Test suites and release evidence

- Create/modify `crates/memory-mcp/tests/http_registry_storage.rs` — durable control Registry integration tests.
- Create/modify `crates/memory-mcp/tests/http_app_sessions_optimistic.rs` — restart, cap race, tenant predicate, and CAS tests.
- Create/modify `crates/memory-mcp/tests/http_durable_tasks.rs` — worker execution, retry, cancellation, artifact reconciliation, and tenant authorization.
- Create/modify `crates/memory-mcp/tests/http_subscription_replica.rs` — filtering, bounded queue, auth expiry, cross-replica polling/repair.
- Create/modify `crates/memory-mcp/tests/http_control_plane.rs` — OIDC/session/API/deletion route integration.
- Create `crates/memory-mcp/tests/http_crash_recovery.rs` — deterministic crash/fault-injection convergence tests.
- Modify `crates/memory-mcp/tests/http_load_concurrency.rs` — executable 20-tenant and release-only 500-tenant load runs with recorded evidence, not placeholder ignored tests.
- Modify existing protocol/isolation/proxy tests only when their helper contract changes; keep their passing coverage.

---

## Phase A — Production Registry and migration foundation

### Task 1: Make the Registry contract complete and internally consistent

**Files:**
- Modify: `crates/memory-mcp/src/http/registry/models.rs`
- Modify: `crates/memory-mcp/src/http/registry/storage.rs`
- Modify `crates/memory-mcp/src/http/registry/account.rs`
- Modify `crates/memory-mcp/src/http/registry/plan.rs`
- Modify `crates/memory-mcp/src/lib.rs` only if feature-gating of the new HTTP/control modules needs correction
- Modify `crates/memory-mcp/src/http/health.rs` to consume the fallible Registry probe
- Test: inline unit tests in the modified modules and `crates/memory-mcp/tests/http_registry_storage.rs`

**Interfaces:**
- Consumes: existing `Account`, `Tenant`, `ApiKey`, `ExternalIdentity`, `ControlPlaneSession`, `LeaseFence`, `RegistryStore`, `AccountResolver`, and `Plan` call sites.
- Produces: one durable contract used by the HTTP pipeline:

```rust
#[async_trait::async_trait]
pub trait RegistryStore: Send + Sync + 'static {
    async fn ping(&self) -> Result<(), MemoryError>;
    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError>;
    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError>;
    async fn create_account_bundle(
        &self,
        account: &Account,
        tenant: &Tenant,
        identity: Option<&ExternalIdentity>,
    ) -> Result<(), MemoryError>; // None is valid for an operator-created invite account
    async fn find_tenant_by_account(&self, account_id: &str) -> Result<Option<Tenant>, MemoryError>;
    async fn find_tenant_by_id(&self, tenant_id: &str) -> Result<Option<Tenant>, MemoryError>;
    async fn find_external_identities(&self, account_id: &str) -> Result<Vec<ExternalIdentity>, MemoryError>;
    async fn link_external_identity(&self, identity: &ExternalIdentity) -> Result<(), MemoryError>;
    async fn unlink_external_identity(&self, account_id: &str, identity_id: &str) -> Result<(), MemoryError>;
    async fn find_api_key(&self, key_id: &str) -> Result<Option<ApiKey>, MemoryError>;
    async fn create_api_key_if_below_limit(&self, key: &ApiKey, max_active: u32) -> Result<(), MemoryError>;
    async fn list_api_keys(&self, account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError>;
    async fn revoke_api_key(&self, account_id: &str, key_id: &str) -> Result<(), MemoryError>;
    async fn revoke_all_api_keys(&self, account_id: &str) -> Result<u64, MemoryError>;
    async fn touch_api_key(&self, key_id: &str, used_at: DateTime<Utc>) -> Result<(), MemoryError>;
    async fn update_tenant_state(&self, tenant_id: &str, expected_version: u64, from: TenantStatus, to: TenantStatus) -> Result<u64, MemoryError>;
    async fn update_tenant_state_fenced(&self, tenant_id: &str, expected_version: u64, from: TenantStatus, to: TenantStatus, lease: &LeaseFence<'_>) -> Result<u64, MemoryError>;
    async fn update_tenant_schema_version_fenced(&self, tenant_id: &str, expected_version: u64, version: u32, lease: &LeaseFence<'_>) -> Result<u64, MemoryError>;
    async fn claim_provisioning(&self, tenant_id: &str, owner_id: &str, lease_id: &str, ttl_secs: i64) -> Result<Option<ProvisioningLease>, MemoryError>;
    async fn heartbeat_provisioning(&self, tenant_id: &str, owner_id: &str, lease_id: &str, generation: u64, heartbeat_at: DateTime<Utc>, expires_at: DateTime<Utc>) -> Result<(), MemoryError>;
    async fn release_provisioning_lease(&self, tenant_id: &str, owner_id: &str, lease_id: &str, generation: u64) -> Result<(), MemoryError>;
    async fn list_due_provisioning(&self, limit: usize, now: DateTime<Utc>) -> Result<Vec<Tenant>, MemoryError>;
    async fn list_ready_tenants(&self, cursor: Option<&str>, limit: usize) -> Result<Vec<Tenant>, MemoryError>;
    async fn list_deleting_tenants(&self, limit: usize, now: DateTime<Utc>) -> Result<Vec<Tenant>, MemoryError>;
    async fn append_provisioning_event(&self, tenant_id: &str, stage: &str) -> Result<(), MemoryError>;
    async fn load_plan(&self, version: u32) -> Result<Plan, MemoryError>;
    async fn load_usage(&self, tenant_id: &str) -> Result<UsageSnapshot, MemoryError>;
    async fn reserve_ingest_usage(&self, tenant_id: &str, source_bytes: u64, plan: &Plan, now: DateTime<Utc>) -> Result<QuotaDecision, MemoryError>;
    async fn reconcile_usage(&self, tenant_id: &str, expected: UsageSnapshot) -> Result<(), MemoryError>;
    async fn store_oidc_request(&self, state_hash: &str, sealed_payload: &[u8], nonce: &[u8; 12], expires_at: DateTime<Utc>) -> Result<(), MemoryError>;
    async fn take_oidc_request(&self, state_hash: &str, now: DateTime<Utc>) -> Result<Option<(Vec<u8>, [u8; 12])>, MemoryError>;
    async fn store_session(&self, session: &ControlPlaneSession) -> Result<(), MemoryError>;
    async fn find_session(&self, cookie_hash: &str, now: DateTime<Utc>) -> Result<Option<ControlPlaneSession>, MemoryError>;
    async fn touch_session(&self, session_id: &str, idle_expiry: DateTime<Utc>) -> Result<(), MemoryError>;
    async fn delete_session(&self, cookie_hash: &str) -> Result<(), MemoryError>;
    async fn delete_sessions_for_account(&self, account_id: &str) -> Result<u64, MemoryError>;
    async fn create_deletion_challenge(&self, challenge: &DeletionChallengeRecord) -> Result<(), MemoryError>;
    async fn consume_deletion_challenge(&self, verifier: &str, account_id: &str, session_id: &str, now: DateTime<Utc>) -> Result<(), MemoryError>;
    async fn transition_account_state(&self, account_id: &str, from: AccountStatus, to: AccountStatus) -> Result<(), MemoryError>;
}
```

The same producer task also defines these durable value types:

```rust
pub struct UsageSnapshot {
    pub ingested_bytes: u64,
    pub episode_count: u64,
    pub open_app_sessions: u32,
    pub active_api_keys: u32,
    pub ingest_window_start: DateTime<Utc>,
    pub ingest_current_minute: u32,
}

pub struct DeletionChallengeRecord {
    pub id: String,
    pub verifier: String,
    pub account_id: String,
    pub session_id: String,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

pub struct PlanLimits {
    pub max_ingested_bytes: u64,
    pub max_episode_count: u64,
    pub ingest_per_minute: u32,
    pub max_open_app_sessions: u32,
    pub max_active_api_keys: u32,
    pub per_tenant_request_concurrency: u32,
    pub extraction_concurrency: u32,
}
```

Control-plane-only methods in this trait retain `#[cfg(feature = "control-plane")]`, so a data-plane-only `streamable-http` build does not reference OIDC/session types. `AccountStatus` must include a terminal deleted/purged representation, while `TenantStatus::Purged` remains a non-reusable tombstone state.

- [ ] **Step 1: Add failing model tests.** Assert that no model contains a raw OIDC subject, account/tenant IDs remain independent, status serialization is snake_case, and `PlanLimits` includes byte/count/session/key/request/extraction limits.
- [ ] **Step 2: Run the focused model tests and verify the new assertions fail.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures registry::models
```

Expected: FAIL because the current model/trait has no complete account bundle, usage, deletion, identity CRUD, or due-maintenance contract.
- [ ] **Step 3: Implement the canonical DTOs and trait.** Remove duplicate/conflicting session/plan representations, add explicit `UsageSnapshot` and `DeletionChallengeRecord`, and preserve compatibility helpers only where existing stdio code uses them. Make `create_account_bundle` atomic for Account + Tenant and optionally ExternalIdentity (the identity is absent only for a deliberate invite account), and make API-key limit/state transitions the production mutation entry points.
- [ ] **Step 4: Strengthen `InMemoryStore` to obey the same invariants.** It must key identity lookup by `(issuer, subject_verifier)`, accept a missing identity only for invite-created Accounts, reject duplicate tenant ownership, enforce API-key active limits atomically under one lock, enforce tenant predicates, and implement all new methods. It remains compiled only under `test`/`test-fixtures`.
- [ ] **Step 5: Run focused tests and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures registry
cargo fmt --all --check
GIT_EDITOR=true git add crates/memory-mcp/src/http/registry crates/memory-mcp/tests/http_registry_storage.rs
GIT_EDITOR=true git commit -m "feat: complete tenant registry contract"
```

### Task 2: Implement the durable SurrealDB Registry store and production constructors

**Files:**
- Create: `crates/memory-mcp/src/http/registry/surreal_store.rs`
- Modify: `crates/memory-mcp/src/http/registry/storage.rs`
- Modify: `crates/memory-mcp/src/http/registry/mod.rs`
- Modify: `crates/memory-mcp/src/config/target.rs`
- Modify: `crates/memory-mcp/src/storage/client.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs`
- Test: `crates/memory-mcp/tests/http_registry_storage.rs`

**Interfaces:**
- Consumes: Task 1 `RegistryStore`, `SurrealTargetConfig`, existing `PrivilegedEngine`, and `BoundDbClient`/`SurrealDbClient` connection code.
- Produces: `SurrealRegistryStore::connect(&SurrealTargetConfig) -> Result<Self, MemoryError>`, `SurrealRegistryStore::connect_in_memory(...)` for fixtures, `RegistryHandle::connect(control_target, tenant_target)`, and a production `HttpState::build_registry` that never selects `InMemoryStore`.

```rust
pub enum RegistryDb {
    Remote(Arc<surrealdb::Surreal<surrealdb::engine::remote::ws::Client>>),
    Local(Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>),
}

pub struct SurrealRegistryStore {
    db: RegistryDb,
    namespace: String,
    database: String,
}

impl SurrealRegistryStore {
    pub async fn connect(target: &SurrealTargetConfig) -> Result<Self, MemoryError>;
    pub async fn connect_in_memory(namespace: &str, database: &str) -> Result<Self, MemoryError>;
}
```

- [ ] **Step 1: Add a failing production-startup smoke test.** Add a child-process/compile-matrix check that builds `memory_mcp_http` with `--no-default-features --features streamable-http` (without `test-fixtures`) and starts it against a disposable embedded target, asserting a reachable Registry and clean readiness instead of `ConfigInvalid("registry is not wired")`.
- [ ] **Step 2: Add storage integration tests for persistence and error mapping.** Use a single embedded test database to create/read/update an Account, Tenant, ExternalIdentity, API key, session, usage row, and OIDC request; run a second store instance against the same database and verify the records remain. Assert unique conflicts map to `MemoryError::Conflict`, missing rows map to `Ok(None)`/`NotFound`, and malformed rows map to `Storage` without leaking SQL.
- [ ] **Step 3: Run the new tests and verify they fail against the unit placeholder.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_registry_storage
```

Expected: FAIL because `SurrealRegistryStore` currently has no database handle and `HttpState::build_registry` rejects every production build.
- [ ] **Step 4: Implement the enum-dispatched SurrealDB store.** Parameterize every value; use explicit `RETURN` projections; parse Surreal record IDs/datetimes defensively; centralize error mapping; never interpolate user-controlled values. Use one control namespace/database binding for all registry queries.
- [ ] **Step 5: Implement remote and embedded target selection.** Keep `mem://` for single-process development/tests and add an explicit RocksDB/embedded data-directory representation for HTTP development. Remote uses WebSocket with configured credentials. Reject missing/ambiguous target configuration and reject control/tenant binding to the same namespace/database.
- [ ] **Step 6: Wire `RegistryHandle` and `HttpState::build_registry`.** Production startup connects control Registry and privileged tenant engine, applies control migrations before returning state, and returns a startup error on connection/migration failure. Test-fixture bootstrap remains behind its feature and is never reachable in a normal build.
- [ ] **Step 7: Run storage/startup tests and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_registry_storage
cargo check -p memory_mcp --features streamable-http,control-plane --locked
GIT_EDITOR=true git add crates/memory-mcp/src/http/registry crates/memory-mcp/src/config/target.rs crates/memory-mcp/src/storage/client.rs crates/memory-mcp/src/http/mod.rs crates/memory-mcp/tests/http_registry_storage.rs
GIT_EDITOR=true git commit -m "feat: wire durable surreal registry"
```

### Task 3: Make control and tenant migration runners real and versioned

**Files:**
- Modify: `crates/memory-mcp/src/http/registry/migrations.rs`
- Modify: `crates/memory-mcp/migrations/001_registry.surql`
- Modify: `crates/memory-mcp/src/storage/migrations.rs`
- Modify: `crates/memory-mcp/src/storage/client.rs`
- Modify: `crates/memory-mcp/src/http/leases/migration.rs`
- Create: `crates/memory-mcp/migrations/044_task_artifacts.surql`
- Create: `crates/memory-mcp/migrations/045_deletion_and_usage_hardening.surql`
- Test: `crates/memory-mcp/tests/http_registry_storage.rs`, `crates/memory-mcp/tests/http_crash_recovery.rs`

**Interfaces:**
- Consumes: Task 2 connected `SurrealRegistryStore` and `SurrealDbClient` execution helpers.
- Produces: `apply_registry_migrations(&SurrealRegistryStore)`, `tenant_versioned_migrations()`, and `run_migrations_for(client, namespace, catalog)` with durable ledger/checksum/postcondition behavior. The tenant catalog ends at `044_task_artifacts`; registry-only deletion/usage hardening is tracked separately as `045`, and the tenant `CURRENT_SCHEMA_VERSION` therefore remains `44`.

- [ ] **Step 1: Write failing schema tests.** Assert that `001_registry.surql` defines account, tenant, external identity, API key, control session, OIDC request, provisioning event, plan, usage, deletion challenge, tombstone, audit, and migration-ledger tables with unique constraints for identity, tenant ownership, namespace binding, cookie/state hashes, and API-key IDs. Assert the tenant catalog contains every script through `045` in order.
- [ ] **Step 2: Run migration tests and verify the catalog/schema assertions fail.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures storage::migrations
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::registry::migrations
```

Expected: FAIL because the registry catalog contains only `001_registry`, the ordinary catalog stops before `040`–`043`, and the registry runner only appends a fake event.
- [ ] **Step 3: Complete the migration SQL.** Make definitions idempotent, create singleton rows explicitly (for example the tenant-local change sequence and App Session counter), use schema permissions that prevent ordinary tenant credentials from accessing control records, and add required field assertions without storing raw OIDC subjects or secrets.
- [ ] **Step 4: Split reusable migration execution from the stdio catalog.** Keep stdio behavior unchanged while exposing an HTTP tenant catalog that includes the App Session, Task, outbox, artifact, and deletion/usage scripts. Record `file_name`, checksum, status, started/completed timestamps, and error information; recover `applying`/`failed` entries only when identity/checksum match.
- [ ] **Step 5: Implement real postconditions.** Verify required tables, fields, indexes, analyzer/index resources, and schema version after each run. A failed postcondition must prevent Tenant `Ready`; no first MCP request may execute migrations.
- [ ] **Step 6: Add crash injection around ledger transitions and test convergence.** A simulated process interruption before/after applying a script must restart idempotently, never accept a changed checksum, and converge to the same schema version.
- [ ] **Step 7: Run migration and crash tests and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::registry::migrations
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures storage::migrations
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_crash_recovery migration
GIT_EDITOR=true git add crates/memory-mcp/src/http/registry/migrations.rs crates/memory-mcp/src/storage/migrations.rs crates/memory-mcp/src/storage/client.rs crates/memory-mcp/src/http/leases/migration.rs crates/memory-mcp/migrations/001_registry.surql crates/memory-mcp/migrations/044_task_artifacts.surql crates/memory-mcp/migrations/045_deletion_and_usage_hardening.surql crates/memory-mcp/tests/http_crash_recovery.rs
GIT_EDITOR=true git commit -m "feat: add durable registry and tenant migrations"
```

---

## Phase B — Provisioning, runtime pool, and tracked maintenance

### Task 4: Connect the production provisioning adapter and separate maintenance queries

**Files:**
- Modify: `crates/memory-mcp/src/http/leases/migration.rs`
- Modify: `crates/memory-mcp/src/http/registry/storage.rs`
- Modify: `crates/memory-mcp/src/http/registry/surreal_store.rs`
- Modify: `crates/memory-mcp/src/http/registry/provisioning.rs`
- Modify: `crates/memory-mcp/src/http/leases/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs`
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs`
- Test: `crates/memory-mcp/tests/http_registry_storage.rs`, `crates/memory-mcp/tests/http_crash_recovery.rs`

**Interfaces:**
- Consumes: Task 3 migration runner and `PrivilegedEngine`.
- Produces: `SurrealApplyMigrations` implementing `ApplyMigrations`, production `run_due_provisioning`, and distinct registry methods for provisioning, ready maintenance, deletion, and cursor paging.

```rust
pub struct SurrealApplyMigrations {
    engine: Arc<PrivilegedEngine>,
    replica_id: String,
}

#[async_trait::async_trait]
impl ApplyMigrations for SurrealApplyMigrations {
    async fn ensure_namespace(&self, namespace: &str, database: &str) -> Result<(), MemoryError>;
    async fn apply_migrations(&self, namespace: &str, database: &str) -> Result<u32, MemoryError>;
}
```

- [ ] **Step 1: Add failing provisioning tests.** Exercise reserved → namespace_creating → migrating → ready against an embedded database; verify the real namespace/database exists and all tenant migrations are present. Add a test proving `Suspended` is not automatically selected by provisioning and `Ready` tenants are selected by maintenance queries instead.
- [ ] **Step 2: Run the tests and verify production path failure.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_registry_storage provisioning
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::leases::migration
```

Expected: the current production path returns `Unavailable` and the test scheduler invokes `NoopMigrations`.
- [ ] **Step 3: Implement remote/embedded `SurrealApplyMigrations`.** Bind a fresh tenant client exactly once, validate server-generated namespace/database identifiers against the immutable Tenant binding, execute the complete catalog, and return the verified schema version. Never route an ordinary tenant request through the privileged DDL handle.
- [ ] **Step 4: Make provisioning durable and fenced.** Use datastore-time lease claims, heartbeats, generation checks, conditional release, retry stage persistence, and failure transitions. A deletion state must preempt provisioning; a stale worker must fail before every state/schema/ledger commit.
- [ ] **Step 5: Implement actual reconciliation.** Query the privileged engine for `tns_` namespaces, compare them with non-reusable registry bindings, record missing/orphan reports and metrics, and do not silently delete or rebind any namespace.
- [ ] **Step 6: Replace `run_due_provisioning` production `Unavailable` branch and remove `NoopMigrations` from production call paths.** The scheduler must use a stable replica ID and process bounded pages, continuing after one Tenant failure while emitting structured error events.
- [ ] **Step 7: Add crash tests at namespace creation, after migration, before schema version write, before Ready, and before lease release.** Restart/reconciliation must converge to `Ready` or an explicit retryable `Failed`, never to an incorrectly admitted state.
- [ ] **Step 8: Run and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_registry_storage provisioning
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_crash_recovery provisioning
GIT_EDITOR=true git add crates/memory-mcp/src/http/leases crates/memory-mcp/src/http/registry crates/memory-mcp/src/http/mod.rs crates/memory-mcp/src/bin/memory_mcp_http.rs crates/memory-mcp/tests/http_registry_storage.rs crates/memory-mcp/tests/http_crash_recovery.rs
GIT_EDITOR=true git commit -m "feat: run fenced production tenant provisioning"
```

### Task 5: Apply runtime configuration, per-Tenant concurrency, eviction, and scheduler lifecycle

**Files:**
- Modify: `crates/memory-mcp/src/http/config.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs`
- Modify: `crates/memory-mcp/src/http/runtime/{pool.rs,lifecycle.rs,activation.rs,guard.rs,storage.rs}`
- Modify: `crates/memory-mcp/src/http/leases/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/app_sessions/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/tasks/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/logging.rs`, `metrics.rs`, `health.rs`, `shutdown.rs`
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs`
- Test: `crates/memory-mcp/tests/http_load_concurrency.rs`, existing runtime unit tests

**Interfaces:**
- Consumes: Task 4 production `RegistryHandle`, `PrivilegedEngine`, and due queries.
- Produces: `RuntimePoolConfig`, `AdmissionConfig`, and an explicit scheduler job set. `Pool::new` must receive configured values rather than silently using defaults; `eviction_scheduler_job(Arc<Pool>)` is a tracked `SchedulerJob`.

The finalized environment keys are:

```text
MEMORY_MCP_HTTP_POOL_CAP
MEMORY_MCP_HTTP_RUNTIME_IDLE_TTL_SECS
MEMORY_MCP_HTTP_RUNTIME_CAPACITY_WAIT_MS
MEMORY_MCP_HTTP_RUNTIME_ACTIVATION_TIMEOUT_SECS
MEMORY_MCP_HTTP_GLOBAL_REQUEST_LIMIT
MEMORY_MCP_HTTP_SUBSCRIPTION_LIMIT
MEMORY_MCP_HTTP_MAINTENANCE_PARALLELISM
MEMORY_MCP_HTTP_TASK_RETENTION_SECS
MEMORY_MCP_HTTP_TASK_QUEUE_CAPACITY
MEMORY_MCP_HTTP_TASK_SYNC_MAX_BYTES
MEMORY_MCP_HTTP_SUBSCRIPTION_QUEUE_CAPACITY
MEMORY_MCP_HTTP_SUBSCRIPTION_AUTH_RECHECK_SECS
MEMORY_MCP_HTTP_MAX_INGESTED_BYTES
MEMORY_MCP_HTTP_MAX_EPISODE_COUNT
MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS
MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS
MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY
MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY
MEMORY_MCP_HTTP_INGEST_PER_MINUTE
- `MEMORY_MCP_HTTP_OPERATOR_IDENTITIES`
- `MEMORY_MCP_HTTP_REPLICA_ID`
- `MEMORY_MCP_CONTROL_PLANE_UI_DIST`
```

`MEMORY_MCP_HTTP_MAX_*`, `MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY`, `MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY`, and `MEMORY_MCP_HTTP_INGEST_PER_MINUTE` are required explicitly when `MEMORY_MCP_HTTP_SIGNUP_MODE=open`; other profiles may use documented safe defaults. `MEMORY_MCP_HTTP_REPLICA_ID` should be set in multi-replica deployments so lease ownership is stable and observable; the process-id fallback is safe only for single-process/local operation.

- [ ] **Step 1: Add failing config tests for every operational limit.** Cover pool capacity, idle TTL, capacity wait, activation timeout, global request/subscription admission, maintenance parallelism, Task retention/queue, subscription queue/recheck intervals, synchronous extract policy, and all quota values. Verify open signup fails unless each required quota is explicitly present.
- [ ] **Step 2: Run config/runtime tests and verify unused-field/no-op behavior is exposed.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::config
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::runtime::pool
```

Expected: current `open_signup_quotas_set()` returns false unconditionally and pool fields are marked unused.
- [ ] **Step 3: Implement typed environment parsing and redacted startup summary.** Parse the exact keys listed above, reject zero/overflow/incompatible values, require `MEMORY_MCP_HTTP_OPERATOR_IDENTITIES` when operator routes are enabled, reject `fs-watch`, reject wildcard/missing production Origin/Host policy, and never print secrets.
- [ ] **Step 4: Add per-Tenant semaphore permits to `OperationGuard`.** `Pool::acquire_or_wait` must acquire global admission, Tenant request capacity, and runtime pin in a rollback-safe order. A subscription path receives a subscription permit but does not acquire a full runtime guard.
- [ ] **Step 5: Implement true eviction.** The idle/pressure job marks only unpinned Ready slots draining, rejects new work, waits a bounded drain period, closes tenant stores, removes the slot, and wakes capacity waiters. Replace `runtime/activation.rs::placeholder()` with the actual single-flight helper or remove the unused module and all dangling references.
- [ ] **Step 6: Register all jobs from the binary.** The final hook list must include provisioning, runtime eviction, App Session cleanup, Task retry/reconciliation/retention, quota reconciliation, deletion, and subscription/outbox repair. Every handle is joined during shutdown; no job is silently dropped.
- [ ] **Step 7: Replace `tracing_warn()` no-ops with bounded structured internal events.** Use the repository logger/metrics seam, bounded enum fields, correlation IDs, and redacted Tenant fingerprints. Add readiness checks for registry and mandatory common dependencies; readiness becomes false before admission closes.
- [ ] **Step 8: Add load/concurrency unit tests.** Assert pool capacity, single-flight activation, per-Tenant limit, separate subscription budget, pinned non-eviction, shutdown cancellation, and configured values.
- [ ] **Step 9: Run and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::runtime::pool
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_load_concurrency --no-fail-fast
GIT_EDITOR=true git add crates/memory-mcp/src/http/config.rs crates/memory-mcp/src/http/mod.rs crates/memory-mcp/src/http/runtime crates/memory-mcp/src/http/leases/scheduler.rs crates/memory-mcp/src/http/app_sessions/scheduler.rs crates/memory-mcp/src/http/tasks/scheduler.rs crates/memory-mcp/src/http/subscriptions/scheduler.rs crates/memory-mcp/src/http/logging.rs crates/memory-mcp/src/http/metrics.rs crates/memory-mcp/src/http/health.rs crates/memory-mcp/src/http/shutdown.rs crates/memory-mcp/src/bin/memory_mcp_http.rs crates/memory-mcp/tests/http_load_concurrency.rs
GIT_EDITOR=true git commit -m "feat: wire bounded runtime and maintenance lifecycle"
```

---

## Phase C — Durable App Sessions

### Task 6: Replace HTTP App Session in-memory dispatch with atomic tenant-bound storage

**Files:**
- Modify: `crates/memory-mcp/src/http/app_sessions/store.rs`
- Modify: `crates/memory-mcp/src/http/app_sessions/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/app_sessions/mod.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers/apps.rs`
- Modify: `crates/memory-mcp/src/service/apps/session_lifecycle.rs`
- Modify: `crates/memory-mcp/src/http/runtime/storage.rs`
- Modify: `crates/memory-mcp/migrations/040_app_sessions.surql`
- Test: `crates/memory-mcp/tests/http_app_sessions_optimistic.rs`

**Interfaces:**
- Consumes: Task 5 `TenantRuntime` with immutable `tenant_id`, `BoundDbClient`, configured `PlanLimits`, and the existing stdio `SessionManager`.
- Produces: `AppSessionStore::open/get/command/close/delete_expired` where every operation accepts the authenticated runtime Tenant ID; HTTP `MemoryMcp` uses the durable backend, while `MemoryMcp::new` for stdio keeps `SessionManager`.
- The durable record shape is explicit: `AppSessionRecord { handle: String, tenant_id: String, app: String, version: u64, payload: Value, idle_expiry: DateTime<Utc>, absolute_expiry: DateTime<Utc> }`.

```rust
pub async fn get(&self, tenant_id: &str, handle: &str) -> Result<Option<AppSessionRecord>, MemoryError>;
pub async fn command(&self, tenant_id: &str, handle: &str, expected_version: u64, mutation: Value) -> Result<AppSessionRecord, MemoryError>;
pub async fn close(&self, tenant_id: &str, handle: &str) -> Result<bool, MemoryError>;
pub async fn delete_expired(&self, now: DateTime<Utc>) -> Result<u64, MemoryError>;
```

- [ ] **Step 1: Add failing tests for cross-Tenant access and cap races.** Two stores in two namespaces must not read/close/update each other’s handles. Spawn more than 32 concurrent opens for one Tenant and assert exactly 32 succeed, with no count drift. Assert two simultaneous version-1 commands produce one success and one conflict.
- [ ] **Step 2: Run the tests and verify current implementation fails.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_app_sessions_optimistic
```

Expected: the current count-then-create race and `close(handle)` without tenant predicate are exposed.
- [ ] **Step 3: Add an atomic Tenant-local counter/transaction.** Open must reserve a slot and create the row in one transaction or equivalent CAS; close and expiry cleanup decrement the counter only when a row was actually removed. Reconciliation repairs counter drift but is not the admission authority.
- [ ] **Step 4: Add Tenant predicates to every query.** `get`, `command`, `close`, count, cleanup, and resource read must include `tenant_id = $tenant_id`; missing/foreign/expired handles are indistinguishable at the protocol boundary.
- [ ] **Step 5: Wire the durable backend through all App handlers.** `open_app`, `app_command`, and `read_resource` must use the runtime Tenant store. `service::apps::session_lifecycle` receives a small backend seam instead of assuming `SessionManager`; stdio keeps its current implementation unchanged.
- [ ] **Step 6: Make cleanup scheduler query `list_ready_tenants`, bind each namespace, delete only expired rows, and unload the temporary runtime.** It must not call `list_due_provisioning`; subscription event coupling is validated separately by Task 9 before the HTTP release gate.
- [ ] **Step 7: Add restart and isolation tests, then commit.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_app_sessions_optimistic
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures http::app_sessions
GIT_EDITOR=true git add crates/memory-mcp/src/http/app_sessions crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/mcp/handlers/apps.rs crates/memory-mcp/src/service/apps/session_lifecycle.rs crates/memory-mcp/src/http/runtime/storage.rs crates/memory-mcp/migrations/040_app_sessions.surql crates/memory-mcp/tests/http_app_sessions_optimistic.rs
GIT_EDITOR=true git commit -m "feat: make HTTP app sessions durable and tenant-bound"
```

---

## Phase D — Durable extraction Tasks

### Task 7: Complete Task state, schema, CAS, cancellation, and retention

**Files:**
- Modify: `crates/memory-mcp/src/http/tasks/state.rs`
- Modify: `crates/memory-mcp/src/http/tasks/worker.rs`
- Modify: `crates/memory-mcp/src/http/tasks/scheduler.rs`
- Modify: `crates/memory-mcp/migrations/041_tenant_tasks.surql`
- Modify: `crates/memory-mcp/migrations/043_tenant_task_unique_fingerprint.surql`
- Modify: `crates/memory-mcp/migrations/044_task_artifacts.surql`
- Test: `crates/memory-mcp/tests/http_durable_tasks.rs`

**Interfaces:**
- Consumes: Task 3 migration catalog and Task 5 runtime/scheduler.
- Produces: `TenantTaskRecord` including `params`, attempt/retry metadata, bounded progress, artifact reference, and retention; `TaskStore` methods whose every read/write contains the runtime Tenant binding and appropriate state/version/fence predicate.
- The artifact type is defined here for the worker/reconciler tasks that follow:

```rust
pub enum ArtifactState { Staged, Committed }

pub struct TaskArtifact {
    pub task_id: String,
    pub fingerprint: String,
    pub episode_id: Option<String>,
    pub fact_ids: Vec<String>,
    pub state: ArtifactState,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}
```

- [ ] **Step 1: Add failing state-transition tests.** Cover queued cancellation → cancelled, running cancellation → cancel_requested, cancellation before commit → cancelled_before_commit, committed work followed by cancellation → completed_before_cancel, expired lease takeover with higher generation, retry exhaustion → failed, terminal immutability, and retention not deleting running/nonterminal rows.
- [ ] **Step 2: Run current durable task tests and verify failures.**

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_durable_tasks
```

Expected: current `claim_next_due` can claim `cancel_requested`, `reconcile_artifacts` always returns zero, and `delete_expired` has no terminal-state predicate.
- [ ] **Step 3: Extend schema and projections.** Add `attempt_count`, `max_attempts`, artifact identity, and any required retry timestamps; enforce valid state strings and field types. Store only bounded result/error/progress payloads.
- [ ] **Step 4: Implement state CAS correctly.** `claim_next_due` claims queued tasks only, atomically converts a queued cancellation to cancelled, and takes over expired running tasks only through a generation/version CAS. `set_cancellation_intent` must be idempotent and must not change terminal records.
- [ ] **Step 5: Implement fenced progress/complete/fail/requeue methods.** Every update checks task ID, Tenant ID, owner, generation, version/state as appropriate; stale workers receive `Conflict` and cannot change result/error/state.
- [ ] **Step 6: Implement retention safely.** Delete only terminal rows with expired retention. Keep memory artifacts under memory invalidation policy; delete no fact/claim/audit record as part of Task retention.
- [ ] **Step 7: Add exact projection/error tests and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_durable_tasks
cargo test -p memory_mcp --features streamable-http,test-fixtures http::tasks
GIT_EDITOR=true git add crates/memory-mcp/src/http/tasks crates/memory-mcp/migrations/041_tenant_tasks.surql crates/memory-mcp/migrations/043_tenant_task_unique_fingerprint.surql crates/memory-mcp/migrations/044_task_artifacts.surql crates/memory-mcp/tests/http_durable_tasks.rs
GIT_EDITOR=true git commit -m "feat: harden durable task state and fencing"
```

### Task 8: Execute durable extraction workers and wire Tasks to the scheduler

**Files:**
- Modify: `crates/memory-mcp/src/http/tasks/worker.rs`
- Modify: `crates/memory-mcp/src/http/tasks/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/runtime/storage.rs`
- Modify: `crates/memory-mcp/src/http/runtime/pool.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Create: `crates/memory-mcp/src/service/durable_extract.rs`
- Modify: `crates/memory-mcp/src/service.rs`
- Modify: `crates/memory-mcp/src/service/capabilities/extract.rs`
- Modify: `crates/memory-mcp/src/tools/extract.rs` where the shared execution seam is exposed
- Test: `crates/memory-mcp/tests/http_durable_tasks.rs`, `crates/memory-mcp/tests/http_isolation.rs`

**Interfaces:**
- Consumes: Task 7 `DurableTaskStore`, Task 5 `Pool`, and existing `crate::tools::extract`/`ExtractParams`.
- Produces: a process-level `TaskWorker`/scheduler path that claims and executes work, plus a service-level extraction execution seam usable by both synchronous `extract` and durable workers.

```rust
pub struct ExtractionExecution {
    pub task_id: String,
    pub cancellation: CancellationToken,
    pub progress: Arc<dyn Fn(Value) -> ProgressFuture + Send + Sync>,
}

pub type ProgressFuture = Pin<Box<dyn Future<Output = Result<(), MemoryError>> + Send>>;

pub async fn run_one(
    runtime: &TenantRuntime,
    replica_id: &str,
    shutdown: CancellationToken,
) -> Result<bool, MemoryError>;

pub async fn execute_extract(
    service: &MemoryService,
    params: ExtractParams,
    execution: &ExtractionExecution,
) -> Result<ToolResponse<ExtractResult>, MemoryError>;
```

- [ ] **Step 1: Add failing execution tests.** Enqueue a Task, run one worker pass, and assert progress/result/terminal state are durable. Restart the process/runtime and retrieve the same result. Assert duplicate fingerprint returns one Task and repeated extraction does not create duplicate facts/claims.
- [ ] **Step 2: Add cancellation and crash tests.** Inject cancellation before commit, after artifact staging, after fact commit, and before Task terminal update; assert the documented terminal state and no rollback of committed facts.
- [ ] **Step 3: Run the tests and verify no worker currently changes the Task.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_durable_tasks
```

Expected: Task remains `working` because `extract` currently enqueues but no scheduler worker executes it.
- [ ] **Step 4: Persist the Task artifact schema.** Store the `TaskArtifact` defined by Task 7 in `task_artifact`, constrain it by Task fingerprint and Tenant namespace, and add indexes for Task ID and fingerprint.
- [ ] **Step 5: Implement a bounded extraction worker.** Claim from the Tenant namespace, acquire the configured extraction semaphore, load typed params, run shared extraction with a cancellation token, emit bounded progress, and complete/fail through fenced TaskStore methods. Do not spawn a detached worker from the MCP handler.
- [ ] **Step 6: Implement the artifact atomicity boundary.** Mark the artifact terminal only after memory writes are durable; reconcile a committed artifact with a nonterminal Task after a crash. Add task provenance to canonical extraction writes without exposing Task internals to clients.
- [ ] **Step 7: Wire `tasks::scheduler_job(Arc<Pool>)`.** It must select ready Tenants through a dedicated due query, acquire a runtime guard, process a bounded number of Tasks, requeue/repair expired work, and release the runtime. In-memory `TaskManager` remains stdio-only.
- [ ] **Step 8: Fix protocol mapping.** `get_task` maps all terminal states correctly, returns the stored durable result/error, and returns tenant-scoped not-found. `cancel_task` maps missing/foreign/terminal behavior without leaking storage. Clients without Tasks get bounded synchronous extraction or a preflight rejection before expensive work.
- [ ] **Step 9: Run durable task/isolation tests and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_durable_tasks
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_isolation
GIT_EDITOR=true git add crates/memory-mcp/src/http/tasks crates/memory-mcp/src/http/runtime crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/service crates/memory-mcp/src/tools/extract.rs crates/memory-mcp/tests/http_durable_tasks.rs crates/memory-mcp/tests/http_isolation.rs
GIT_EDITOR=true git commit -m "feat: execute extraction through durable tasks"
```

---

## Phase E — Transactional outbox and subscriptions

### Task 9: Add a real transaction seam and integrate canonical mutation events

**Files:**
- Modify: `crates/memory-mcp/src/storage/client.rs`
- Modify: `crates/memory-mcp/src/storage/context_store.rs`
- Modify: `crates/memory-mcp/src/storage/app_store.rs`
- Modify: `crates/memory-mcp/src/service/service_context.rs`
- Modify: `crates/memory-mcp/src/service/capabilities/ingest.rs`
- Modify: `crates/memory-mcp/src/service/capabilities/resolve.rs`
- Modify: `crates/memory-mcp/src/service/capabilities/invalidate.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/outbox.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Modify: `crates/memory-mcp/migrations/042_tenant_change_event.surql`
- Test: `crates/memory-mcp/tests/http_subscription_replica.rs`

**Interfaces:**
- Consumes: Task 3 migration schema and existing `TenantChangeEvent`.
- Produces: a bound transaction helper and canonical mutation hook. The public helper must not accept arbitrary client SQL; callers construct validated internal statements/variables from typed domain operations.

```rust
use std::future::Future;
use std::pin::Pin;
use serde_json::Value;

pub(crate) struct InternalMutation {
    pub(crate) sql: String,
    pub(crate) vars: Value,
}

pub(crate) type ChangeCommitFuture = Pin<Box<dyn Future<Output = Result<(), MemoryError>> + Send>>;

pub(crate) async fn transaction(
    &self,
    script: &str,
    vars: Option<Value>,
) -> Result<Value, MemoryError>;

pub(crate) trait ChangeEventSink: Send + Sync {
    fn commit(
        &self,
        mutation: InternalMutation,
        event: TenantChangeEvent,
    ) -> ChangeCommitFuture;
}

pub(crate) struct DurableChangeEventSink {
    db: Arc<BoundDbClient>,
}
```

`DurableChangeEventSink` is the HTTP-only `ServiceContext` dependency; stdio constructs no sink and continues using the existing direct storage path.

- [ ] **Step 1: Add failing atomicity tests.** A mutation failure must leave both the sequence and event absent/unchanged; concurrent successful mutations must receive unique monotonic sequences; event payload must not contain a full resource body.
- [ ] **Step 2: Run current outbox tests and verify the sequence bug.**

```bash
cargo test -p memory_mcp --features streamable-http,test-fixtures http::subscriptions::outbox
```

Expected: current implementation increments the sequence in one query and performs mutation/event in a second query, so failure can leave a sequence gap outside a transaction.
- [ ] **Step 3: Implement bound transaction execution.** Use SurrealDB transaction/script semantics supported by the pinned version, parse all statements explicitly, rollback on any error, and keep the namespace fixed by `BoundDbClient`.
- [ ] **Step 5: Implement `commit_mutation_with_event`.** Create/use a singleton sequence row inside the same transaction, apply typed mutation, insert `{sequence, resource_id, revision, change_kind, created_at}`, and commit only after all statements succeed.
- [ ] **Step 5: Integrate the sink into every canonical App/resource mutation.** Cover successful `ingest`, `resolve`, `invalidate`, App Session open/command/close/resource revisions, and durable resource changes. Keep stdio on its existing path by injecting no HTTP sink there. Do not emit events for rolled-back or purely internal non-resource changes.
- [ ] **Step 6: Add event revision/idempotency tests.** Repeating a domain retry must converge to the same resource revision/fingerprint semantics and must not create unbounded duplicate notifications.
- [ ] **Step 7: Run transaction/outbox tests and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures http::subscriptions::outbox
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_subscription_replica
GIT_EDITOR=true git add crates/memory-mcp/src/storage/client.rs crates/memory-mcp/src/storage/context_store.rs crates/memory-mcp/src/storage/app_store.rs crates/memory-mcp/src/service crates/memory-mcp/src/http/subscriptions/outbox.rs crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/migrations/042_tenant_change_event.surql crates/memory-mcp/tests/http_subscription_replica.rs
GIT_EDITOR=true git commit -m "feat: make tenant mutations transactional with outbox"
```

### Task 10: Implement filtered, bounded, revocation-aware subscriptions

**Files:**
- Modify: `crates/memory-mcp/src/http/subscriptions/mod.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/outbox.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/stream.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/scheduler.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Modify: `crates/memory-mcp/src/http/middleware.rs`, `runtime/pool.rs`, `shutdown.rs`
- Test: `crates/memory-mcp/tests/http_subscription_replica.rs`, `crates/memory-mcp/tests/http_proto_conformance.rs`

**Interfaces:**
- Consumes: Task 9 atomic outbox and authenticated principal/authenticator.
- Produces: `SubscriptionStore::next_batch(after_sequence, &AcceptedSubscriptionFilter)`, validated filter conversion from rmcp’s native `SubscriptionFilter`, bounded send/poll behavior, and `subscription_repair_job(RegistryHandle)`.

- [ ] **Step 1: Add failing filter/backpressure/auth tests.** Unsupported filters are rejected by `accepted_subscription_filter`; accepted filters deliver only matching App/resource events; a full/slow sink disconnects within the configured bound; revoked/suspended/deleting principals are terminated within 60 seconds; shutdown ends the stream.
- [ ] **Step 2: Run current subscription tests and verify failures.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_subscription_replica
```

Expected: current filter is cloned without validation, event delivery ignores the filter, `poll_and_repair_all` is empty, and sending has no bounded slow-consumer policy.
- [ ] **Step 3: Implement filter validation and event selection.** Accept only the resource/App URI forms exposed by the server’s resource catalog; reject tool/prompt list changes and unknown filters. Keep `resource_id`, Tenant ID, and subscription identity out of metrics labels.
- [ ] **Step 4: Implement bounded stream delivery.** Use a bounded queue/cursor, coalesce consecutive updates for the same resource when safe, wrap sends/polls in configured timeouts, and return a typed slow-consumer error. Do not buffer full resource bodies.
- [ ] **Step 5: Rework `listen`.** Start from the current sequence, poll the durable outbox, wait on a bounded wake channel or poll interval, recheck authorization no less often than 30 seconds, and select on rmcp cancellation and global shutdown. Do not hold a full runtime guard for the stream.
- [ ] **Step 6: Implement cross-replica repair.** Add a bounded repair pass over ready Tenants that verifies sequence/outbox health and publishes wake hints. Polling remains authoritative, so a lost wake cannot lose an event. No `Last-Event-ID` replay is added.
- [ ] **Step 7: Run protocol/subscription tests and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_subscription_replica
cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_proto_conformance
GIT_EDITOR=true git add crates/memory-mcp/src/http/subscriptions crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/http/middleware.rs crates/memory-mcp/src/http/runtime/pool.rs crates/memory-mcp/src/http/shutdown.rs crates/memory-mcp/tests/http_subscription_replica.rs crates/memory-mcp/tests/http_proto_conformance.rs
GIT_EDITOR=true git commit -m "feat: add bounded durable subscriptions"
```

---

## Phase F — Quotas and deletion lifecycle

### Task 11: Make plan/usage enforcement durable and transactional

**Files:**
- Modify: `crates/memory-mcp/src/http/registry/models.rs`
- Modify: `crates/memory-mcp/src/http/registry/plan.rs`
- Modify: `crates/memory-mcp/src/http/registry/storage.rs`
- Modify: `crates/memory-mcp/src/http/registry/surreal_store.rs`
- Modify: `crates/memory-mcp/src/service/service_context.rs`
- Modify: `crates/memory-mcp/src/service/capabilities/ingest.rs`
- Modify: `crates/memory-mcp/src/tools/ingest.rs`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs`
- Modify: `crates/memory-mcp/src/http/app_sessions/store.rs`
- Modify: `crates/memory-mcp/src/http/tasks/worker.rs`
- Modify: `crates/memory-mcp/src/http/leases/scheduler.rs`
- Test: `crates/memory-mcp/tests/http_isolation.rs`, `crates/memory-mcp/tests/http_registry_storage.rs`

**Interfaces:**
- Consumes: Task 1 Registry plan/usage methods and Task 9 transaction seam.
- Produces: one `QuotaEnforcer` used by HTTP `ingest`, API-key creation, App Session open, Task admission, and runtime per-Tenant request concurrency. Explicit quota denial maps to `429`; capacity overload remains `503`.

```rust
pub struct QuotaEnforcer {
    pub tenant_id: String,
    pub plan: crate::http::registry::models::Plan,
    pub registry: Arc<dyn crate::http::registry::RegistryStore>,
}

impl QuotaEnforcer {
    pub async fn reserve_ingest(&self, source_bytes: u64) -> Result<QuotaDecision, MemoryError>;
    pub async fn reconcile(&self, expected: UsageSnapshot) -> Result<(), MemoryError>;
}
```

- [ ] **Step 1: Add failing concurrent quota tests.** Concurrent ingests cannot exceed byte/count/rate limits beyond the documented bounded overshoot; API-key cap cannot be exceeded by races; App Session count follows the configured plan rather than a hardcoded unrelated default; retrieval still works after write quota denial.
- [ ] **Step 2: Run the tests and verify current non-durable behavior.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_registry_storage quota
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_isolation quota
```

Expected: current plan values are not loaded from the Registry, API-key count is not atomic, and handler paths do not perform durable usage reservation.
- [ ] **Step 3: Implement durable plan/usage rows and atomic reservations.** Use Tenant-local counters/window fields, one transaction/CAS per admission, and explicit source-byte measurement before content preparation. Reject oversized work before model/provider I/O.
- [ ] **Step 4: Wire every admission point.** `ingest` reserves source bytes and episode count before the durable episode commit and compensates/reconciles only through defined durable rules; App Session and API-key writes reserve their counters in the same transaction as their row; extraction concurrency uses the configured semaphore and plan.
- [ ] **Step 5: Implement quota reconciliation.** A tracked job lists ready Tenants, recalculates source counts/bytes from canonical tables, and repairs drift above the plan threshold without disabling retrieval or deleting data. It must not reuse provisioning due queries.
- [ ] **Step 6: Map errors and guidance.** Add retry-after and safe guidance for explicit quota denial; never return registry SQL/namespace/dependency details. Add bounded metrics categories.
- [ ] **Step 7: Run and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_registry_storage quota
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_isolation quota
GIT_EDITOR=true git add crates/memory-mcp/src/http/registry crates/memory-mcp/src/service crates/memory-mcp/src/tools/ingest.rs crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/http/app_sessions/store.rs crates/memory-mcp/src/http/tasks/worker.rs crates/memory-mcp/src/http/leases/scheduler.rs crates/memory-mcp/tests/http_registry_storage.rs crates/memory-mcp/tests/http_isolation.rs
GIT_EDITOR=true git commit -m "feat: enforce durable tenant quotas"
```

### Task 12: Implement irreversible Account deletion with durable tombstones

**Files:**
- Modify: `crates/memory-mcp/src/control/deletion.rs`
- Modify: `crates/memory-mcp/src/control/account_api.rs`
- Modify: `crates/memory-mcp/src/control/session.rs`
- Modify: `crates/memory-mcp/src/http/registry/models.rs`
- Modify: `crates/memory-mcp/src/http/registry/storage.rs`
- Modify: `crates/memory-mcp/src/http/registry/surreal_store.rs`
- Modify: `crates/memory-mcp/src/http/registry/provisioning.rs`
- Modify: `crates/memory-mcp/src/http/leases/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/middleware.rs`, `principal/auth.rs`
- Modify: `crates/memory-mcp/migrations/045_deletion_and_usage_hardening.surql`
- Test: `crates/memory-mcp/tests/http_control_plane.rs`, `crates/memory-mcp/tests/http_crash_recovery.rs`, `crates/memory-mcp/tests/http_isolation.rs`

**Interfaces:**
- Consumes: Task 2 durable session/store, Task 5 scheduler, Task 11 usage, and existing `require_recent_auth`/typed phrase.
- Produces: durable one-use challenge flow and `deletion_job`. The challenge stores only a keyed verifier bound to Account + session + expiry; the client receives an opaque token once.

```rust
pub async fn process_deletion_tenant(
    store: Arc<dyn RegistryStore>,
    engine: PrivilegedEngine,
    tenant: Tenant,
    replica_id: &str,
) -> Result<(), MemoryError>;
```

- [ ] **Step 1: Add failing end-to-end deletion tests.** Require recent OIDC auth, exact phrase, no export/recovery disclosure, one-use token, all API keys/session revocation, Account/Tenant deletion transition, data-plane denial, namespace non-reuse, and durable tombstone after restart.
- [ ] **Step 2: Run current control-plane tests and verify stubs fail.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_control_plane deletion
```

Expected: start/confirm deletion currently return `Unavailable`, and `execute_deletion` revokes only the current session and writes Account state non-atomically.
- [ ] **Step 3: Implement challenge issuance/consumption atomically.** Validate recent auth before issuance, generate a random token, store HMAC verifier/Account/session/expiry, return `Cache-Control: no-store`, and consume with a single conditional update so replay fails.
- [ ] **Step 4: Implement the durable deletion transition.** In one control transaction revoke all keys/sessions, move Account to `Deleting`, move Tenant to `Deleting`, append an audit event, and enqueue deletion work. There is no cancellation window.
- [ ] **Step 5: Implement deletion worker/recovery.** Bind the immutable namespace under a fenced lease, logically invalidate memory according to domain policy, remove only expired Task/App Session rows, mark Tenant `Purged` and Account terminal, retain registry/tombstone/identity/binding/audit history, and make every step idempotent.
- [ ] **Step 6: Harden all auth/resolution paths.** `Deleting`/terminal Accounts, Tenants, revoked keys, and deleted sessions fail closed; cache invalidation keeps the documented 60-second maximum; no error reveals whether a foreign account exists.
- [ ] **Step 7: Add crash injections before and after every deletion transition and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,mcp-apps,test-fixtures --test http_control_plane deletion
cargo test -p memory_mcp --features streamable-http,control-plane,mcp-apps,test-fixtures --test http_crash_recovery deletion
cargo test -p memory_mcp --features streamable-http,control-plane,mcp-apps,test-fixtures --test http_isolation deletion
GIT_EDITOR=true git add crates/memory-mcp/src/control crates/memory-mcp/src/http/registry crates/memory-mcp/src/http/leases/scheduler.rs crates/memory-mcp/src/http/middleware.rs crates/memory-mcp/src/http/principal crates/memory-mcp/migrations/045_deletion_and_usage_hardening.surql crates/memory-mcp/tests/http_control_plane.rs crates/memory-mcp/tests/http_crash_recovery.rs crates/memory-mcp/tests/http_isolation.rs
GIT_EDITOR=true git commit -m "feat: implement irreversible durable account deletion"
```

---

## Phase G — OIDC control plane, router, and Dioxus SPA

### Task 13: Finish OIDC identity linking, account provisioning, and browser sessions

**Files:**
- Modify: `crates/memory-mcp/src/control/oidc.rs`
- Modify: `crates/memory-mcp/src/control/session.rs`
- Modify: `crates/memory-mcp/src/control/csrf.rs`
- Modify: `crates/memory-mcp/src/control/recent_auth.rs`
- Modify: `crates/memory-mcp/src/http/registry/storage.rs`
- Modify: `crates/memory-mcp/src/http/registry/surreal_store.rs`
- Modify: `crates/memory-mcp/src/http/config.rs`
- Test: `crates/memory-mcp/tests/http_control_plane.rs`

**Interfaces:**
- Consumes: Task 1 identity/bundle methods and Task 2 durable OIDC/session storage.
- Produces: `validated issuer + subject → SubjectVerifier → ExternalIdentity → Account → Tenant → provisioning event`, plus these control-plane seams:

```rust
pub async fn resolve_control_session(
    state: &HttpState,
    cookie_value: &str,
) -> Result<(ControlPlaneSession, Account), ApiError>;

pub async fn touch_session(
    state: &HttpState,
    session: &ControlPlaneSession,
) -> Result<(), ApiError>;
```

Recent-auth and CSRF helpers continue to receive the resolved session/account rather than raw cookies.

- [ ] **Step 1: Add failing OIDC tests.** Cover exact issuer/audience/nonce/state/PKCE/algorithm validation, unknown-key bounded refresh, duplicate callback atomic state consumption, first open-signup account bundle, invite-only rejection for unknown identities, and no raw subject in serialized records/log captures.
- [ ] **Step 2: Run focused control-plane tests and verify the incomplete account chain.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_control_plane oidc
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::control::oidc
```

Expected: `upsert_account_for_identity` currently creates a raw UUID Account with an empty tenant and no ExternalIdentity.
- [ ] **Step 3: Implement transactional identity/account/tenant provisioning.** Normalize only the issuer according to the configured OIDC rules, derive the keyed subject verifier from the validated raw claim, discard the raw subject after the transaction input is built, and use a unique identity constraint to converge concurrent callbacks. Open signup passes `Some(&ExternalIdentity)` to `create_account_bundle`; invite-only operator provisioning may create the Account/Tenant with `None` and links an identity only through the authenticated linking action.
- [ ] **Step 4: Implement invite-only linking as an authenticated control-plane action.** If an operator supplies a transient subject for linking, compute the verifier immediately, do not persist/log the subject, require configured issuer/identity authorization, and reject linking the last identity unless the account lifecycle explicitly permits it. Never link by email.
- [ ] **Step 5: Complete browser session behavior.** Store only keyed cookie verifier, enforce idle and absolute expiry, atomically touch idle expiry, rotate after login, invalidate on logout/deletion/key rotation, and issue CSRF tokens bound to session/account.
- [ ] **Step 6: Run and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_control_plane oidc
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_control_plane session
GIT_EDITOR=true git add crates/memory-mcp/src/control/oidc.rs crates/memory-mcp/src/control/session.rs crates/memory-mcp/src/control/csrf.rs crates/memory-mcp/src/control/recent_auth.rs crates/memory-mcp/src/http/registry crates/memory-mcp/src/http/config.rs crates/memory-mcp/tests/http_control_plane.rs
GIT_EDITOR=true git commit -m "feat: complete oidc identity and browser sessions"
```

### Task 14: Mount all control-plane routes with correct middleware and operator authorization

**Files:**
- Modify: `crates/memory-mcp/src/http/router.rs`
- Modify: `crates/memory-mcp/src/http/middleware.rs`
- Modify: `crates/memory-mcp/src/control/account_api.rs`
- Modify: `crates/memory-mcp/src/control/operator.rs`
- Modify: `crates/memory-mcp/src/control/error.rs`
- Modify: `crates/memory-mcp/src/http/oauth/mod.rs` and its handlers
- Modify: `crates/memory-mcp/src/control/static_assets.rs`
- Test: `crates/memory-mcp/tests/http_control_plane.rs`, `crates/memory-mcp/tests/http_proto_conformance.rs`

**Interfaces:**
- Consumes: Task 13 session/OIDC middleware and Task 12 deletion/account methods.
- Produces: mounted canonical routes:

```text
POST /mcp
GET|DELETE /mcp -> 405
GET  /auth/oidc/authorize
GET  /auth/oidc/callback
POST /auth/oidc/logout
GET  /api/v1/account
GET  /api/v1/account/csrf
GET|POST|DELETE /api/v1/account/api_keys...
GET|POST|DELETE /api/v1/account/identity_links...
POST /api/v1/account/delete
POST /api/v1/account/delete/confirm
GET|POST /api/v1/operator/...
GET /.well-known/oauth-protected-resource
GET /, /assets/*, SPA fallback
GET /health/live, /health/ready, /metrics
```

- [ ] **Step 1: Add route/middleware tests before mounting.** Assert cookie auth never reaches `/mcp`, Bearer API keys never authenticate browser APIs, mutations reject missing/tampered CSRF and disallowed Origin, stale auth returns reauth, operator routes reject normal Accounts, and API routes win over SPA fallback.
- [ ] **Step 2: Run tests and verify current router exposes only health/MCP/metrics.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_control_plane
```

Expected: control routes are not mounted and operator/deletion handlers return stubs.
- [ ] **Step 3: Build route groups separately.** Apply Host/Origin and correlation middleware globally, Bearer/prevalidation/runtime middleware only to `/mcp`, session/CSRF/recent-auth middleware only to control mutations, and OIDC flow middleware only to OIDC routes. Keep `/metrics` unauthenticated at app level but document/protect it at proxy.
- [ ] **Step 4: Implement typed control API errors.** Return a stable envelope with correlation ID, map Conflict/Unavailable/Unauthorized/Forbidden/NotFound correctly, and log internal details only through redacted structured events.
- [ ] **Step 5: Implement account/identity/operator handlers.** Account endpoints use session Account ID, API-key creation uses atomic plan limit, identity list omits verifiers/subjects, unlink is guarded, operator allowlist uses issuer + keyed verifier configuration, and destructive operator operations require recent auth/CSRF/audit.
- [ ] **Step 6: Mount Protected Resource Metadata and verify OAuth validation.** Keep OAuth Resource Server support separate from API-key auth, validate exact issuer/resource/audience/expiry/algorithm, use string/array audience decoding, and fail closed on bounded JWKS refresh failure.
- [ ] **Step 7: Run route/control/conformance tests and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_control_plane
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_proto_conformance
GIT_EDITOR=true git add crates/memory-mcp/src/http/router.rs crates/memory-mcp/src/http/middleware.rs crates/memory-mcp/src/control crates/memory-mcp/src/http/oauth crates/memory-mcp/src/control/static_assets.rs crates/memory-mcp/tests/http_control_plane.rs crates/memory-mcp/tests/http_proto_conformance.rs
GIT_EDITOR=true git commit -m "feat: mount authenticated control plane routes"
```

### Task 15: Serve the real Dioxus bundle and complete the browser UI

**Files:**
- Create: `crates/memory-mcp/build.rs`
- Modify: `crates/memory-mcp/src/control/static_assets.rs`
- Modify: `crates/memory-mcp/src/http/router.rs`
- Modify: `crates/control-plane-ui/Cargo.toml`
- Modify: `crates/control-plane-ui/src/main.rs`
- Modify: `crates/control-plane-ui/src/router.rs`
- Modify: `crates/control-plane-ui/src/api.rs`
- Modify: `crates/control-plane-ui/src/pages/{login.rs,status.rs,keys.rs,delete.rs}`
- Create: `crates/control-plane-ui/assets/` and the release bundle output used by the build pipeline
- Test: `crates/memory-mcp/tests/http_control_plane.rs`, browser smoke test documented in `docs/operations/CONFORMANCE.md`

**Interfaces:**
- Consumes: Task 14 mounted control routes and the separate `control-plane-ui` crate.
- Produces: `serve_asset(path: &str) -> Response` backed by a generated compile-time asset manifest, not hardcoded placeholder HTML; same-origin UI calls with credentials and CSRF; no secret in URL/storage.

- [ ] **Step 1: Add failing asset tests.** A known bundle asset returns the correct content type/body, unknown assets return 404, `/` and client-side routes return the compiled `index.html`, CSP has no `unsafe-eval`, `object-src 'none'`, `frame-ancestors 'none'`, restrictive `connect-src`, `base-uri 'none'`, `form-action 'self'`, no-sniff, and strict referrer policy.
- [ ] **Step 2: Run UI/asset tests and verify the current literal shell is insufficient.**

```bash
cargo check -p control-plane-ui
cargo test -p memory_mcp --features streamable-http,control-plane,control-plane-ui,test-fixtures control::static_assets
```

Expected: the UI has a broken route match (`routek`), minimal pages, and `serve_asset()` returns hardcoded HTML instead of the compiled Dioxus bundle.
- [ ] **Step 3: Implement the build contract.** Build the UI bundle with Dioxus CLI 0.7, pass it through the exact `MEMORY_MCP_CONTROL_PLANE_UI_DIST` directory input to `build.rs`, generate an `OUT_DIR` manifest using `include_bytes!`, and fail compilation when `control-plane-ui` is enabled without a complete bundle. Do not fetch network assets during build.
- [ ] **Step 4: Fix Dioxus routing/pages and API client.** Use the pinned Dioxus 0.7 APIs, implement status/login/keys/delete pages, include provisioning state, key expiry/last-used, one-time secret display, deletion no-export/recovery disclosure and exact phrase confirmation, and map typed API errors. Send browser cookies on same-origin calls; hold newly-created API secret only in component/page memory.
- [ ] **Step 5: Add browser security and route precedence tests.** Verify API routes are not swallowed by SPA fallback, assets never return secrets, and all security headers are present.
- [ ] **Step 6: Build the UI and run smoke checks.** Run `dx --version` and require the 0.7 toolchain, run `dx bundle --platform web --release --out-dir target/control-plane-ui-dist`, set `MEMORY_MCP_CONTROL_PLANE_UI_DIST=target/control-plane-ui-dist`, compile `memory_mcp`, and run the control-plane HTTP integration suite against the embedded bundle.
- [ ] **Step 7: Commit.**

```bash
cargo check -p control-plane-ui
cargo test -p memory_mcp --features streamable-http,control-plane,control-plane-ui,test-fixtures --test http_control_plane
GIT_EDITOR=true git add crates/memory-mcp/build.rs crates/memory-mcp/src/control/static_assets.rs crates/memory-mcp/src/http/router.rs crates/control-plane-ui crates/memory-mcp/tests/http_control_plane.rs
GIT_EDITOR=true git commit -m "feat: serve complete dioxus control plane"
```

---

## Phase H — Operational completion and release gates

### Task 16: Make crash/recovery, load, proxy, and interoperability tests executable evidence

**Files:**
- Create: `crates/memory-mcp/tests/http_crash_recovery.rs`
- Modify: `crates/memory-mcp/tests/http_load_concurrency.rs`
- Modify: `crates/memory-mcp/tests/http_proto_conformance.rs`
- Modify: `crates/memory-mcp/tests/http_isolation.rs`
- Modify: `crates/memory-mcp/tests/http_proxy_streaming.rs`
- Modify: `crates/memory-mcp/tests/http_durable_tasks.rs`
- Modify: `crates/memory-mcp/tests/http_subscription_replica.rs`
- Create: `docs/operations/HTTP_INTEROP_MATRIX.md`

**Interfaces:**
- Consumes: all production paths from Tasks 1–15.
- Produces: non-placeholder release evidence for spec §§20.1–20.5. No correctness/release test is marked `#[ignore]`; the 500-Tenant contingency run is a separately invoked release profile/CI job, not a skipped test.

- [ ] **Step 1: Replace the load placeholders.** Implement a reusable HTTP server fixture with unique embedded storage per test, 20 active Tenant classes at expected QPS, bounded latency/error assertions, isolation probes, and metrics collection. The same test file must expose a named `load_500_tenants_under_contingency_qps` test that requires `MEMORY_MCP_RUN_500_LOAD=1`; the release job runs it with explicit resource/time limits rather than marking it ignored.
- [ ] **Step 2: Run the 20-Tenant test and verify real request traffic.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_20_active_tenants_under_expected_qps -- --test-threads=1
```

Expected: the test performs real HTTP calls and fails on latency/error/isolation regression; it must not print a placeholder message.
- [ ] **Step 3: Run the 500-Tenant contingency gate separately.**

```bash
MEMORY_MCP_RUN_500_LOAD=1 cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_500_tenants_under_contingency_qps -- --test-threads=1
```

Expected: the release command runs the named test and records latency, error rate, peak memory, and configured capacity; an unset gate variable is a configuration error in the release job, not a skipped production requirement.
- [ ] **Step 4: Implement deterministic crash tests.** Use a test-only `FaultInjector` seam to stop execution at every provisioning, migration, lease, Task, outbox, and deletion transition. Restart the same durable database, run reconciliation, and assert convergence/no cross-Tenant effects.
- [ ] **Step 5: Strengthen protocol tests.** Cover modern metadata/envelopes, notifications `202`, header mismatch before auth, SSE final response, body ownership/drop, no session headers/resume, all HTTP status boundaries, Tasks/subscriptions capability gating, and no accidental `ping`/initialize/legacy behavior.
- [ ] **Step 6: Strengthen isolation tests.** Alternate two Tenants under high concurrency and verify memory, App, Task, quota, event, cache, runtime, and deletion data cannot cross; verify stale fences and subscription capacity behavior.
- [ ] **Step 7: Add proxy/interoperability evidence.** Run an actual reverse-proxy integration configuration proving `/mcp` streaming/no buffering, `/metrics` restriction, read timeout greater than ordinary 120 seconds, and no header rewrite. Document tested official SDK/selected clients and record incompatibilities without enabling legacy fallback.
- [ ] **Step 8: Run all release test targets and commit.**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_proto_conformance
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_isolation -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_crash_recovery
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_subscription_replica
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_proxy_streaming
GIT_EDITOR=true git add crates/memory-mcp/tests docs/operations/HTTP_INTEROP_MATRIX.md
GIT_EDITOR=true git commit -m "test: make HTTP release gates executable"
```

### Task 17: Correct operational documentation, restore/rotation drills, and final quality gate

**Files:**
- Modify: `docs/operations/RESTORE_DRILL.md`
- Modify: `docs/operations/CREDENTIAL_ROTATION.md`
- Modify: `docs/operations/CONFORMANCE.md`
- Modify: `README.md` and the current HTTP configuration documentation location
- Modify: `docs/superpowers/specs/2026-08-27-streamable-http-saas.md`
- Modify: `docs/adr/0052-streamable-http-saas-profile.md`
- Modify: `.github/workflows/ci.yml` if required to execute the release profile
- Test/evidence: `docs/operations/HTTP_RELEASE_GATE.md`

**Interfaces:**
- Consumes: all implemented runtime/control/storage/release gates.
- Produces: an operator-readable release checklist with exact environment names, no false claims, remote restore evidence, credential rotation evidence, embedded limitations, provider privacy, no export/recovery behavior, and an accurate implementation status.

- [ ] **Step 1: Fix documentation drift.** Replace stale restore variables such as `MEMORY_MCP_SURREALDB_URL`, `MEMORY_MCP_SURREALDB_NS`, `MEMORY_MCP_SURREALDB_DB`, and `MEMORY_MCP_HTTP_SESSION_SIGNING_KEY` with the finalized `HttpConfig::from_env` names. Document separate control/tenant targets, embedded data directories, reverse-proxy requirements, and the fact that logical deletion does not edit immutable backups.
- [ ] **Step 2: Write the remote restore drill.** Snapshot and restore a real supported remote SurrealDB deployment into fresh bindings, disable ingress, rotate API-key pepper, identity-index, session, OIDC state/nonce, CSRF, and any handle-signing keys, verify old credentials fail, require OIDC relinking/new keys, run conformance/isolation, and record the accepted backup-resurrection limitation. An embedded run cannot satisfy this gate.
- [ ] **Step 3: Write and execute credential rotation evidence.** Verify each key’s impact, rolling restart behavior, old/new overlap policy if enabled, and no sensitive material in logs. Keep historical backups immutable.
- [ ] **Step 4: Update spec/ADR status only after all tests pass.** Change the spec’s stale `Implementation status: Not implemented` to the repository’s truthful final status, add a short implementation note pointing to this completion plan, and preserve the approved decisions/no-export/no-recovery/embedded boundary.
- [ ] **Step 5: Run the full final matrix.**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets \\
  --features fs-watch,mcp-apps,streamable-http,control-plane,test-fixtures \\
  --locked -- -D warnings
cargo clippy -p control-plane-ui --all-targets --locked -- -D warnings
cargo check --workspace --locked
cargo check -p control-plane-ui
cargo test -p memory_mcp
cargo test -p memory_mcp --features fs-watch,mcp-apps,streamable-http,control-plane,test-fixtures
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_proto_conformance
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_isolation -- --test-threads=1
git diff --check
```

Expected: default stdio suite remains green and unchanged; feature suite, all HTTP integration suites, UI check, fmt, clippy, and workspace check pass with zero warnings/errors. The 500-Tenant release job and remote restore drill must have attached evidence before enabling open signup.
- [ ] **Step 6: Perform a source and wiring audit before declaring completion.** Use project structural/semantic tools to confirm:
  - no production `SurrealRegistryStore` placeholder or unconditional in-memory startup;
  - no `NoopMigrations`/empty `poll_and_repair_all`/`runtime::activation::placeholder()` in production paths;
  - no `TODO`, `TBD`, `todo!()`, `unimplemented!()`, ignored release test, or “later milestone” claim in shipped HTTP/control-plane code;
  - every public route is mounted behind the intended feature/middleware;
  - every scheduler factory is registered and joined;
  - `list_due_provisioning` is not used for Ready-Tenant maintenance;
  - every durable store query has an explicit Tenant/Account predicate where required;
  - every production configuration field changes runtime behavior or is removed;
  - no raw OIDC subject, secret, namespace, SQL, body, or provider error crosses logs/errors;
  - no MCP argument or URL field selects a namespace;
  - stdio behavior and default features remain unchanged.
- [ ] **Step 7: Commit the documentation and final audit.**

```bash
GIT_EDITOR=true git add docs README.md .github/workflows/ci.yml
GIT_EDITOR=true git commit -m "docs: publish HTTP SaaS release and recovery gates"
```

---

## Completion criteria

The work is complete only when all of the following are true:

1. A normal `memory_mcp_http` build starts against a configured remote Registry/tenant SurrealDB, runs control migrations, and fails closed on unavailable/mismatched storage; test fixtures are not selected.
2. A new Account/ExternalIdentity/Tenant follows the durable provisioning chain and becomes data-plane accessible only after namespace creation, complete tenant migrations, schema postconditions, and fenced Ready transition.
3. Two replicas can race provisioning, Task claims, deletion, and maintenance without stale commits or namespace reuse.
4. HTTP App Sessions, extraction Tasks, usage counters, and subscription events survive the intended restart boundary and are tenant-isolated.
5. Every canonical resource mutation that should notify subscribers is transactionally coupled to its outbox event; filters, queue limits, auth rechecks, cancellation, and shutdown are effective.
6. Control-plane OIDC, secure sessions, CSRF, recent-auth, identity linking, API-key lifecycle, operator authorization, deletion, static assets, and SPA routes are mounted and exercised end to end.
7. The profile advertises only capabilities actually wired and does not advertise legacy sessions, Tasks, Apps, or subscriptions when their feature/runtime path is disabled.
8. The 20-Tenant load test, 500-Tenant contingency release run, crash/recovery suite, remote restore drill, credential rotation drill, proxy contract, interoperability matrix, stdio regression, fmt, clippy, and workspace checks all have passing results/evidence.
9. The final source audit finds no production placeholders, no-op jobs, dangling public APIs, unmounted route groups, ignored release gates, sensitive-data leaks, or type/signature mismatches.

## Self-Review

### Spec coverage

- §§1–2: Tasks 5, 14, 15, and 17 preserve two binaries, additive features, environment-only config, Dioxus boundary, and all stated non-goals.
- §3: Tasks 5, 14, 16 cover route topology, proxy contract, SSE headers/deadlines, Host/Origin, metrics restriction, and route precedence.
- §4: Tasks 9, 10, 16 preserve the modern-only `2026-07-28` contract, validation order, error boundary, cancellation, and domain retry semantics.
- §5: Tasks 1, 2, 12, 13, and 14 implement Registry identity, API keys, OIDC, browser sessions, future OAuth resource-server validation, and operator authorization.
- §§6–8: Tasks 2–5, 7–8, 12, and 16 implement Registry/provisioning, immutable runtime pool, leases, migration rollout, tracked scheduler jobs, quotas, and recovery.
- §9: Task 6 implements durable App Sessions, optimistic versioning, TTL, cap, resource ownership, and cleanup.
- §10: Tasks 7–8 implement durable extraction Tasks, worker fencing, cancellation, artifacts, retries, retention, and sync fallback.
- §11: Tasks 9–10 implement outbox atomicity, filters, bounded listeners, cross-replica repair, revocation, and shutdown.
- §12–13: Tasks 5 and 11 implement quotas, rate/capacity distinction, inline-only input, `fs-watch` rejection, and provider privacy boundary.
- §§14–15: Tasks 12–15 and 17 implement control-plane API/SPA, deletion/tombstone policy, data protection, backup limitations, rotation, and restore runbook.
- §§16–19: Tasks 5, 13–17 implement observability, health/readiness, shutdown, embedded warning, full environment contract, and redacted startup.
- §20: Task 16 and Task 17 implement protocol, isolation, crash/recovery, compatibility, load, proxy, restore, rotation, and documentation gates.
- §21: Task ordering follows the approved dependency order while preserving the namespace-never-in-request invariant.

### Placeholder/no-op audit

The plan explicitly removes or wires every known unfinished production surface: `SurrealRegistryStore`, `HttpState::build_registry`, registry migration recorder, `NoopMigrations` production branch, `runtime/activation.rs::placeholder()`, no-op subscription repair, empty App Session cleanup, empty Task reconciliation, unused pool configuration, `open_signup_quotas_set() == false`, unmounted control routes, hardcoded static assets, ignored load tests, and stale operational documentation.

### Type and wiring audit

All later tasks consume interfaces defined in earlier tasks: durable storage first, then migrations/provisioning, then runtime/schedulers, then App/Task/subscription handlers, then quotas/deletion/control-plane/UI, then release gates. Any implementation that changes a signature must update every listed producer, consumer, fixture, and test in the same task; a green compile with an unmounted or unused API is not sufficient.

### Intentional boundaries retained

No new MCP tool, no user export, no user restore, no application-level memory encryption, no request-selected namespace, no mandatory `Idempotency-Key`, no legacy transport compatibility, no production application egress allowlist, and no physical deletion of memory/audit history are introduced by this plan.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-01-streamable-http-saas-completion.md`. Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task and review between tasks.
2. **Inline Execution** — execute the tasks in this session using the executing-plans skill with checkpoints.

Before implementation, obtain the dependency/migration approval required by `AGENTS.md`, preserve the existing uncommitted review fixes, and start with Task 1 rather than re-running the historical scaffolding plan.