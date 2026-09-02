# Architecture Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve every verified architecture-audit finding while preserving the frozen MCP surface, stdio behavior, namespace isolation, existing schemas, and public compatibility.

**Architecture:** Repair the feature matrix first, then make HTTP production/test composition explicit before building release evidence on that seam. After correctness and release coverage are trustworthy, replace the monolithic Registry interface with capability-specific interfaces, extract two verified control-plane workflows, deepen internal service seams, and reconcile public interfaces and documentation.

**Tech Stack:** Rust 1.97.1, Tokio, Axum, rmcp, SurrealDB embedded/remote engines, reqwest, serde/serde_json, thiserror, Cargo feature flags, shell release scripts.

**Spec:** `docs/superpowers/specs/2026-09-02-architecture-audit-remediation-design.md`

## Global Constraints

- Rust version remains `1.97.1`.
- Do not add or change dependencies.
- Do not modify generated code or existing migration files.
- Do not add an MCP tool; the eight-tool public surface remains frozen.
- Default Cargo features remain `[]`; feature flags remain additive.
- `main.rs` remains CLI parsing and dispatch only.
- MCP arguments, URLs, and claims never select a namespace.
- Production errors use `MemoryError`; production code does not use `unwrap()`.
- Preserve existing stdio behavior and existing public `MemoryService` constructors.
- Preserve uncommitted user changes in `README.md`; edit only exact, verified lines and never overwrite the file.
- Remote restore, rotation, proxy, interoperability, and 500-tenant evidence must record real observations; never manufacture passing evidence.
- Every task ends with focused tests and an independently reviewable commit.

---

## File structure

### New architecture and composition files

- `docs/adr/0053-explicit-http-storage-and-migration-composition.md` — durable decision that Cargo test features do not select production adapters.
- `docs/adr/0054-capability-specific-control-registry-interfaces.md` — durable decision for narrow Registry capabilities and atomic storage operations.
- `crates/memory-mcp/src/http/composition.rs` — production HTTP composition and test-only explicit composition values.
- `crates/memory-mcp/src/http/test_state.rs` — feature-gated `HttpStateTestBuilder`.
- `crates/memory-mcp/src/http/fault_injection.rs` — named fault points and no-op/test injectors.
- `crates/memory-mcp/src/control/application/mod.rs` — control-plane application module exports.
- `crates/memory-mcp/src/control/application/api_keys.rs` — API-key creation workflow.
- `crates/memory-mcp/src/control/application/oidc_signup.rs` — verified-identity signup workflow.

### New integration and release files

- `crates/memory-mcp/tests/common/mod.rs` — integration-test helpers module.
- `crates/memory-mcp/tests/common/http_server.rs` — shared subprocess and durable-storage fixture.
- `crates/memory-mcp/tests/http_registry_storage.rs` — durable production-composition coverage.
- `crates/memory-mcp/tests/http_crash_recovery.rs` — deterministic durable-transition recovery.
- `crates/memory-mcp/tests/http_durable_tasks.rs` — Task restart, fencing, cancellation, and retention.
- `crates/memory-mcp/tests/http_subscription_replica.rs` — durable outbox and replica polling.
- `crates/memory-mcp/tests/http_control_plane.rs` — end-to-end OIDC-independent control-plane operations.
- `scripts/http_release_evidence.sh` — reproducible release command runner and evidence manifest.
- `docs/operations/HTTP_INTEROP_MATRIX.md` — exact client/version interoperability record.
- `docs/operations/HTTP_RELEASE_GATE.md` — local and external evidence status.

### Existing files with focused changes

- `crates/memory-mcp/src/http/mod.rs` — one state assembly path; exports composition, test builder, and fault seam.
- `crates/memory-mcp/src/http/runtime/bootstrap.rs` — always selects production composition.
- `crates/memory-mcp/src/http/registry/{mod.rs,storage.rs,surreal_store.rs}` — capability aggregation and implementations.
- `crates/memory-mcp/src/http/leases/{mod.rs,migration.rs}` — narrow provisioning dependency and injected migrations/fault points.
- `crates/memory-mcp/src/http/principal/auth.rs` — account and credential capabilities only.
- `crates/memory-mcp/src/http/registry/{account.rs,plan.rs,provisioning.rs}` — narrow capability inputs.
- `crates/memory-mcp/src/http/{middleware.rs,test_bootstrap.rs}` — narrow capability access and explicit test setup.
- `crates/memory-mcp/src/control/{mod.rs,account_api.rs,oidc.rs,session.rs,deletion.rs,operator.rs}` — thin adapters over application operations and narrow stores.
- `crates/memory-mcp/src/service/core/builder.rs` — crate-private dependency bundle.
- `crates/memory-mcp/src/service/context/pipeline.rs` — characterization and concrete phase extraction.
- `crates/memory-mcp/tests/{http_proto_conformance.rs,http_isolation.rs,http_proxy_streaming.rs,http_load_concurrency.rs}` — shared fixture and stronger evidence.
- `.github/workflows/ci.yml` — explicit additive-feature compile/clippy and 20-tenant gate.
- `docs/adr/0052-streamable-http-saas-profile.md` and `docs/superpowers/specs/2026-08-27-streamable-http-saas.md` — truthful implementation/release status.
- `README.md` — canonical CSRF route correction only, preserving unrelated edits.

---

### Task 1: Record explicit HTTP composition policy

**Files:**
- Create: `docs/adr/0053-explicit-http-storage-and-migration-composition.md`
- Modify: `docs/adr/0052-streamable-http-saas-profile.md:3-8`

**Interfaces:**
- Consumes: ADR-0011 append-only migrations, ADR-0038 One Active Namespace, ADR-0052 HTTP composition.
- Produces: the policy consumed by Tasks 3–8: features expose helpers, constructors select adapters explicitly, and release evidence distinguishes local from external execution.

- [ ] **Step 1: Write ADR-0053**

Use this decision text:

```markdown
# ADR-0053: Make HTTP storage and migration composition explicit

## Status

Accepted — 2026-09-02, architecture audit remediation.

## Context

The `test-fixtures` Cargo feature currently changes `memory_mcp_http` composition: `HttpState` selects an in-memory Registry and the provisioning scheduler selects `NoopMigrations`. Black-box commands that enable the feature therefore do not exercise the durable production Registry or tenant migration adapter. Cargo features are compile-time capability gates, not deployment configuration.

## Decision

1. `memory_mcp_http` always constructs production Registry and tenant migration adapters from validated `HttpConfig`.
2. `test-fixtures` exposes builders, deterministic bootstrap, and fault injectors only. Enabling it does not select storage or migrations.
3. In-memory adapters are selected only through explicit test composition values.
4. Registry and tenant migration catalogs remain distinct and append-only. Existing migration files and schema versions are unchanged.
5. Normal CI exercises durable embedded composition. Remote, multi-replica, restore, rotation, proxy, interoperability, and 500-tenant evidence are separate release gates.
6. Request handling never runs migrations and never accepts a namespace selector.

## Consequences

Tests must state which adapters they use. Production-like tests are slightly more expensive but prove the actual composition seam. External evidence remains pending until executed against a supported environment; documents cannot mark an unexecuted gate as passed.

## Relationships

This decision refines ADR-0011, ADR-0038, and ADR-0052. It does not change their migration, tenancy, or protocol semantics.
```

- [ ] **Step 2: Correct ADR-0052 status language**

Replace the status with:

```markdown
Accepted; core implementation complete, release verification incomplete — 2026-08-27 design. The Streamable HTTP profile is not production-ready. Public open signup remains blocked until the executable and operational evidence in the completion plan and `docs/operations/HTTP_RELEASE_GATE.md` has passed.
```

- [ ] **Step 3: Check the ADR diff**

Run:

```bash
git diff --check -- docs/adr/0053-explicit-http-storage-and-migration-composition.md docs/adr/0052-streamable-http-saas-profile.md
```

Expected: exit 0 and no whitespace errors.

- [ ] **Step 4: Commit**

```bash
git add docs/adr/0053-explicit-http-storage-and-migration-composition.md docs/adr/0052-streamable-http-saas-profile.md
git commit -m "docs: define explicit HTTP composition policy"
```

---

### Task 2: Consolidate `HttpState` assembly and restore the feature matrix

**Files:**
- Create: `crates/memory-mcp/src/http/test_state.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs:23-247`
- Modify: `crates/memory-mcp/src/control/account_api.rs:474-505`
- Modify: `crates/memory-mcp/src/control/oidc.rs:854-886`
- Modify: `.github/workflows/ci.yml:74-109`

**Interfaces:**
- Consumes: existing `HttpConfig`, `RegistryHandle`, `MetricsHandle`, `Authenticator`, `AccountResolver`, and `Pool`.
- Produces: `HttpState::assemble`, `HttpStateTestBuilder`, and a feature-matrix regression gate used by every later HTTP task.

- [ ] **Step 1: Add a compile-failing regression test through the existing all-targets command**

Run before editing:

```bash
cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,prometheus,control-plane,test-fixtures --locked
```

Expected: FAIL with `E0063` at `control/account_api.rs:495` and `control/oidc.rs:863` because `metrics_handle` is missing.

- [ ] **Step 2: Introduce one shared state assembly function**

Keep the existing public `HttpState::new` signatures for compatibility, but make both wrappers call this implementation:

```rust
impl HttpState {
    async fn assemble(
        config: HttpConfig,
        registry: registry::RegistryHandle,
        #[cfg(feature = "prometheus")] metrics_handle: Option<MetricsHandle>,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let signup_plan = registry::models::Plan {
            id: "free".into(),
            version: 1,
            limits: config.signup_plan_limits.clone().unwrap_or_default(),
        };
        registry.ensure_plan(&signup_plan).await?;
        let pool = Arc::new(runtime::pool::Pool::from_http_config(
            &config,
            Arc::new(registry.clone()),
        ));
        let store = registry.store_clone();
        let authenticator = Arc::new(principal::auth::Authenticator::new(
            store.clone(),
            Arc::new(principal::cache::PrincipalCache::new(1024)),
            config.api_key_pepper.as_bytes().to_vec(),
            Arc::new(principal::auth::RateLimiter::new(
                4096,
                std::time::Duration::from_secs(1),
                20,
            )),
        ));
        let account_resolver = Arc::new(registry::account::AccountResolver::new(store));
        #[cfg(feature = "control-plane")]
        let oidc_client = if config.enable_control_plane {
            Some(Arc::new(
                crate::control::oidc::OidcClient::new(
                    &config.oidc_issuer,
                    &config.oidc_client_id,
                    &config.oidc_audience,
                    &config.oidc_redirect_uri,
                    &config.oidc_allowed_alg,
                )
                .await?,
            ))
        } else {
            None
        };
        Ok(Arc::new(Self {
            config: config.clone(),
            pool,
            shutdown: shutdown::ShutdownState::new(),
            admission: Arc::new(runtime::pool::AdmissionGate::new_with_limits(
                config.global_request_limit,
                config.subscription_limit,
            )),
            registry,
            authenticator,
            account_resolver,
            #[cfg(feature = "control-plane")]
            oidc_client,
            #[cfg(feature = "prometheus")]
            metrics_handle,
        }))
    }
}
```

The two existing public `HttpState::new` variants remain feature-gated wrappers: each obtains its Registry through the current `build_registry` method in this task and delegates to `assemble`. Task 3 replaces `build_registry`; Tasks 9–10 introduce and migrate capability accessors.

- [ ] **Step 3: Add the test builder**

Create `test_state.rs` with this interface:

```rust
#[cfg(any(test, feature = "test-fixtures"))]
pub struct HttpStateTestBuilder {
    config: super::config::HttpConfig,
    registry: super::registry::RegistryHandle,
    #[cfg(feature = "prometheus")]
    metrics_handle: Option<super::MetricsHandle>,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpStateTestBuilder {
    pub async fn new() -> Self {
        Self {
            config: super::config::HttpConfig::default_for_test(),
            registry: super::registry::RegistryHandle::in_memory_with_default_mem_engine().await,
            #[cfg(feature = "prometheus")]
            metrics_handle: super::test_metrics_handle(),
        }
    }

    pub fn with_config(mut self, config: super::config::HttpConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_registry(mut self, registry: super::registry::RegistryHandle) -> Self {
        self.registry = registry;
        self
    }

    #[cfg(feature = "prometheus")]
    pub fn with_metrics_handle(mut self, handle: Option<super::MetricsHandle>) -> Self {
        self.metrics_handle = handle;
        self
    }

    pub async fn build(self) -> Result<std::sync::Arc<super::HttpState>, crate::error::MemoryError> {
        super::HttpState::assemble(
            self.config,
            self.registry,
            #[cfg(feature = "prometheus")]
            self.metrics_handle,
        )
        .await
    }
}
```

Export it only under `cfg(any(test, feature = "test-fixtures"))`. Make `default_for_test()` delegate to the builder.

- [ ] **Step 4: Replace direct state literals**

In both failing tests, construct state as:

```rust
let state = crate::http::test_state::HttpStateTestBuilder::new()
    .await
    .with_registry(registry)
    .build()
    .await
    .expect("test HTTP state");
```

Delete manually constructed pools, authenticators, resolvers, and feature-gated fields made redundant by the builder.

- [ ] **Step 5: Add the explicit CI compile gate**

Add this command to the HTTP/all-features job in `.github/workflows/ci.yml`:

```yaml
- name: Check additive HTTP feature composition
  run: >-
    cargo check -p memory_mcp --all-targets --no-default-features
    --features streamable-http,prometheus,control-plane,test-fixtures
    --locked
```

- [ ] **Step 6: Verify focused tests and feature compilation**

Run:

```bash
cargo test -p memory_mcp --no-default-features --features streamable-http,prometheus,control-plane,test-fixtures control::account_api::tests
cargo test -p memory_mcp --no-default-features --features streamable-http,prometheus,control-plane,test-fixtures control::oidc::tests
cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,prometheus,control-plane,test-fixtures --locked
```

Expected: both commands pass.

- [ ] **Step 7: Commit**

```bash
git add crates/memory-mcp/src/http/mod.rs crates/memory-mcp/src/http/test_state.rs crates/memory-mcp/src/control/account_api.rs crates/memory-mcp/src/control/oidc.rs .github/workflows/ci.yml
git commit -m "refactor: centralize HTTP state construction"
```

---

### Task 3: Make production and test composition explicit

**Files:**
- Create: `crates/memory-mcp/src/http/composition.rs`
- Create: `crates/memory-mcp/tests/http_registry_storage.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs:1-215`
- Modify: `crates/memory-mcp/src/http/runtime/bootstrap.rs:15-39`
- Modify: `crates/memory-mcp/src/http/leases/scheduler.rs:39-113`
- Modify: `crates/memory-mcp/src/http/leases/migration.rs:53-193,378-387`
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs:31-80`
- Modify: `crates/memory-mcp/src/http/test_bootstrap.rs:26-132`

**Interfaces:**
- Consumes: `HttpState::assemble` and `HttpStateTestBuilder` from Task 2.
- Produces: `HttpProductionComposition::connect`, `HttpTestComposition::in_memory`, `HttpRuntime`, and explicit `Arc<dyn ApplyMigrations>` injection used by scheduler hooks and recovery tests.

- [ ] **Step 1: Write a failing composition test**

Add `http_registry_storage.rs` with a subprocess test that builds `memory_mcp_http` with `test-fixtures`, points `MEMORY_MCP_HTTP_CONTROL_SURREALDB_URL` at an invalid durable URL, and asserts startup fails rather than becoming ready through an in-memory fallback.

Core assertion:

```rust
assert!(!status.success(), "test-fixtures must not replace production storage");
assert!(stderr.contains("registry") || stderr.contains("storage"));
```

Run:

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_registry_storage production_binary_does_not_select_fixture_storage -- --exact
```

Expected: FAIL because the current binary silently selects in-memory storage.

- [ ] **Step 2: Implement production composition**

Create:

```rust
pub struct HttpProductionComposition {
    pub registry: super::registry::RegistryHandle,
    pub tenant_migrations: std::sync::Arc<dyn super::leases::migration::ApplyMigrations>,
}

impl HttpProductionComposition {
    pub async fn connect(config: &super::config::HttpConfig) -> Result<Self, crate::error::MemoryError> {
        let store = super::registry::SurrealRegistryStore::connect(&config.control_db).await?;
        let engine = if config.control_db.url == config.tenant_db.url
            && config.control_db.username == config.tenant_db.username
            && config.control_db.password == config.tenant_db.password
        {
            store.privileged_engine()
        } else {
            super::registry::SurrealRegistryStore::connect_engine(&config.tenant_db).await?
        };
        let migrations = std::sync::Arc::new(
            super::leases::migration::SurrealTenantMigrations::new(engine.clone()),
        );
        Ok(Self {
            registry: super::registry::RegistryHandle::from_durable(
                std::sync::Arc::new(store),
                engine,
            ),
            tenant_migrations: migrations,
        })
    }
}
```

Create the explicit test counterpart under `cfg(any(test, feature = "test-fixtures"))`:

```rust
pub struct HttpTestComposition {
    pub registry: super::registry::RegistryHandle,
    pub tenant_migrations: std::sync::Arc<dyn super::leases::migration::ApplyMigrations>,
}

impl HttpTestComposition {
    pub async fn in_memory() -> Self {
        Self {
            registry: super::registry::RegistryHandle::in_memory_with_default_mem_engine().await,
            tenant_migrations: std::sync::Arc::new(
                super::leases::migration::NoopMigrations,
            ),
        }
    }
}
```

- [ ] **Step 3: Remove feature-driven adapter selection**

Delete `HttpState::build_registry` and its `cfg(any(test, feature = "test-fixtures"))` branches. Introduce this startup value in `runtime/bootstrap.rs`:

```rust
pub struct HttpRuntime {
    pub state: std::sync::Arc<HttpState>,
    pub tenant_migrations: std::sync::Arc<dyn crate::http::leases::migration::ApplyMigrations>,
}
```

Change `runtime::bootstrap::build_state` to call `HttpProductionComposition::connect(cfg)`, pass `composition.registry` into `HttpState::assemble`, and return `HttpRuntime { state, tenant_migrations: composition.tenant_migrations }` after preserving the existing metrics error mapping.

Change `SchedulerHooks::with_provisioning_only` to accept `Arc<dyn ApplyMigrations>`. Its provisioning closure clones that adapter and calls a revised `run_due_provisioning(registry, migrations)`. Remove migration construction and every `cfg(any(test, feature = "test-fixtures"))` adapter-selection branch from `run_due_provisioning`; keep `run_due_provisioning_for` as the lower-level test seam.

In `memory_mcp_http.rs`, bind the result as `runtime`, use `runtime.state` for bootstrap/router/shutdown, and construct hooks with:

```rust
let scheduler_hooks = memory_mcp::http::leases::scheduler::SchedulerHooks::with_provisioning_only(
    runtime.tenant_migrations.clone(),
)
```

This is the only production scheduler construction path, so the migration adapter selected by `HttpProductionComposition` reaches the provisioning job without feature-dependent replacement.

- [ ] **Step 4: Keep test bootstrap as data seeding only**

`MEMORY_MCP_HTTP_TEST_BOOTSTRAP` may create deterministic accounts, tenants, and credentials against the already-selected state. It must not create or select a Registry adapter. Add a module test that calls it with an explicitly composed in-memory state.

- [ ] **Step 5: Add durable embedded composition coverage**

In `http_registry_storage.rs`, create a unique temporary RocksDB URL, construct production composition twice against the same path sequentially, and assert the second handle can read the plan and tenant written by the first. Use existing `tempfile`; do not add dependencies.

Assert:

```rust
assert_eq!(reloaded_tenant.id, tenant.id);
assert_eq!(reloaded_plan.version, 1);
```

- [ ] **Step 6: Run composition tests**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_registry_storage -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::test_bootstrap
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::leases::migration
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::leases::scheduler
cargo check -p memory_mcp --all-targets --features streamable-http,prometheus,control-plane,test-fixtures --locked
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/memory-mcp/src/http/composition.rs crates/memory-mcp/src/http/mod.rs crates/memory-mcp/src/http/runtime/bootstrap.rs crates/memory-mcp/src/http/leases/scheduler.rs crates/memory-mcp/src/http/leases/migration.rs crates/memory-mcp/src/bin/memory_mcp_http.rs crates/memory-mcp/src/http/test_bootstrap.rs crates/memory-mcp/tests/http_registry_storage.rs
git commit -m "refactor: make HTTP composition explicit"
```

---

### Task 4: Build one durable HTTP subprocess fixture

**Files:**
- Create: `crates/memory-mcp/tests/common/mod.rs`
- Create: `crates/memory-mcp/tests/common/http_server.rs`
- Modify: `crates/memory-mcp/tests/http_proto_conformance.rs:1-135`
- Modify: `crates/memory-mcp/tests/http_isolation.rs:1-175`
- Modify: `crates/memory-mcp/tests/http_proxy_streaming.rs:1-121`

**Interfaces:**
- Consumes: explicit production composition from Task 3.
- Produces: `HttpServerFixture`, `HttpServerConfig`, `TestTenant`, `modern_meta`, and `mcp_call` used by Tasks 5–8.

- [ ] **Step 1: Characterize existing test behavior**

Run all three suites before moving helpers:

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_proto_conformance
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_isolation -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures --test http_proxy_streaming -- --test-threads=1
```

Expected: record pass/fail status; do not change assertions to hide a pre-existing failure.

- [ ] **Step 2: Implement the fixture interface**

```rust
pub struct TestTenant {
    pub name: String,
    pub api_key: String,
}

pub struct HttpServerConfig {
    pub tenants: Vec<TestTenant>,
    pub extra_env: Vec<(String, String)>,
}

pub struct HttpServerFixture {
    child: std::process::Child,
    pub base_url: String,
    storage: tempfile::TempDir,
    config: HttpServerConfig,
}

impl HttpServerFixture {
    pub async fn spawn(config: HttpServerConfig) -> Self;
    pub fn client(&self) -> reqwest::Client;
    pub async fn wait_ready(&self);
    pub fn kill(&mut self);
    pub async fn restart(&mut self);
}

pub fn modern_meta() -> serde_json::Value;

pub async fn mcp_call(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    method: &str,
    params: serde_json::Value,
) -> reqwest::Response;
```

`spawn` must allocate one unique embedded directory, set all required security keys explicitly, set `MEMORY_MCP_HTTP_TEST_BOOTSTRAP` from `tenants`, wait for `/health/ready`, and preserve the directory across `restart`. `Drop` kills and waits for the child.

- [ ] **Step 3: Migrate protocol tests without changing assertions**

Replace local `Server`, `base_env`, `spawn_server`, `client`, and `modern_meta` definitions with imports from `common::http_server`.

- [ ] **Step 4: Migrate isolation tests without changing assertions**

Replace local process setup and `mcp_call` with the fixture. Preserve the two distinct API keys and all existing high-concurrency assertions.

- [ ] **Step 5: Migrate proxy tests without changing assertions**

Replace local process setup and modern metadata with the fixture. Preserve header assertions exactly.

- [ ] **Step 6: Re-run characterization suites**

Run the three commands from Step 1. Expected: results match the pre-refactor baseline, with no new failures.

- [ ] **Step 7: Commit**

```bash
git add crates/memory-mcp/tests/common crates/memory-mcp/tests/http_proto_conformance.rs crates/memory-mcp/tests/http_isolation.rs crates/memory-mcp/tests/http_proxy_streaming.rs
git commit -m "test: share durable HTTP server fixture"
```

---

### Task 5: Replace HTTP load placeholders with executable tests

**Files:**
- Modify: `crates/memory-mcp/tests/http_load_concurrency.rs:1-30`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `HttpServerFixture` and `mcp_call` from Task 4.
- Produces: executable `load_20_active_tenants_under_expected_qps` and gated `load_500_tenants_under_contingency_qps`.

- [ ] **Step 1: Remove both `#[ignore]` attributes and placeholder bodies**

Define a local result type:

```rust
#[derive(serde::Serialize)]
struct LoadEvidence {
    tenant_count: usize,
    request_count: usize,
    success_count: usize,
    error_count: usize,
    p50_ms: u128,
    p95_ms: u128,
    p99_ms: u128,
    max_ms: u128,
}
```

- [ ] **Step 2: Write the 20-tenant workload**

Bootstrap 20 unique tenants. For each tenant, ingest a tenant-unique marker and issue concurrent recall requests. Collect elapsed milliseconds, HTTP status, JSON-RPC error state, and returned content.

Required assertions:

```rust
assert_eq!(evidence.tenant_count, 20);
assert_eq!(evidence.error_count, 0);
assert_eq!(evidence.success_count, evidence.request_count);
assert!(responses.iter().all(|r| r.contains(&r.expected_marker)));
assert!(responses.iter().all(|r| !r.contains_other_tenant_marker));
```

Use a generous deterministic CI ceiling configured in the test, not a marketing SLO: `p95_ms <= 5_000` and `max_ms <= 15_000`. Print one JSON `LoadEvidence` record to stderr.

- [ ] **Step 3: Write the 500-tenant release workload**

At the start of the test require:

```rust
assert_eq!(
    std::env::var("MEMORY_MCP_RUN_500_LOAD").as_deref(),
    Ok("1"),
    "release gate requires MEMORY_MCP_RUN_500_LOAD=1"
);
```

Run the same workload and invariants for 500 tenants. Do not mark the test ignored. The normal CI command selects only the 20-tenant test by name, so the release-only test is not accidentally executed.

- [ ] **Step 4: Add the normal CI gate**

```yaml
- name: Run 20-tenant HTTP load gate
  run: >-
    cargo test -p memory_mcp
    --features streamable-http,mcp-apps,control-plane,test-fixtures
    --test http_load_concurrency
    load_20_active_tenants_under_expected_qps
    -- --test-threads=1
```

- [ ] **Step 5: Run the normal load gate**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_20_active_tenants_under_expected_qps -- --test-threads=1
```

Expected: real HTTP traffic, zero errors, isolation assertions pass, and one JSON evidence line is emitted.

- [ ] **Step 6: Verify the release gate fails closed when not configured**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_500_tenants_under_contingency_qps -- --test-threads=1
```

Expected: FAIL with `release gate requires MEMORY_MCP_RUN_500_LOAD=1`; this proves an unconfigured release job cannot report a skipped/pass result.

- [ ] **Step 7: Commit**

```bash
git add crates/memory-mcp/tests/http_load_concurrency.rs .github/workflows/ci.yml
git commit -m "test: make HTTP load gates executable"
```

---

### Task 6: Add deterministic crash and recovery evidence

**Files:**
- Create: `crates/memory-mcp/src/http/fault_injection.rs`
- Create: `crates/memory-mcp/tests/http_crash_recovery.rs`
- Modify: `crates/memory-mcp/src/http/mod.rs`
- Modify: `crates/memory-mcp/src/http/composition.rs`
- Modify: `crates/memory-mcp/src/http/leases/migration.rs`
- Modify: `crates/memory-mcp/src/http/tasks/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/scheduler.rs`
- Modify: `crates/memory-mcp/src/control/deletion.rs`

**Interfaces:**
- Consumes: explicit composition and restartable fixture.
- Produces: `FaultPoint`, `FaultInjector`, `NoFaults`, and deterministic test injectors used only through explicit composition.

- [ ] **Step 1: Define named transition points**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    ProvisioningLeaseClaimed,
    NamespaceCreated,
    TenantMigrationsApplied,
    TenantReadyCommitted,
    TaskClaimed,
    TaskArtifactCommitted,
    TaskCompleted,
    OutboxMutationCommitted,
    AccountDeletionStarted,
    AccountDeletionFinalized,
}

pub trait FaultInjector: Send + Sync + 'static {
    fn hit(&self, point: FaultPoint) -> Result<(), crate::error::MemoryError>;
}

#[derive(Default)]
pub struct NoFaults;

impl FaultInjector for NoFaults {
    fn hit(&self, _point: FaultPoint) -> Result<(), crate::error::MemoryError> {
        Ok(())
    }
}
```

Production composition always injects `Arc::new(NoFaults)`.

- [ ] **Step 2: Add a deterministic test injector**

Under `cfg(any(test, feature = "test-fixtures"))`, add `FailOnceAt` backed by an atomic boolean. Its first matching `hit` returns `MemoryError::Transient`; subsequent calls pass. This tests recovery without terminating the test runner.

- [ ] **Step 3: Thread the injector through worker options**

Add `Arc<dyn FaultInjector>` to the existing runtime/scheduler option objects rather than global state. Call `hit` immediately after the durable transition named by each enum value. Do not call it before a transition whose recovery is being tested.

- [ ] **Step 4: Write provisioning recovery tests**

For each provisioning point, execute with `FailOnceAt`, assert the first pass returns the injected transient error, reconstruct composition against the same embedded database, rerun reconciliation, and assert the tenant becomes `Ready` exactly once with schema version `CURRENT_SCHEMA_VERSION`.

- [ ] **Step 5: Write Task, outbox, and deletion recovery tests**

Assert after restart:

```rust
assert_eq!(completed_task_count, 1);
assert_eq!(task_artifact_count, 1);
assert_eq!(delivered_change_sequence, committed_change_sequence);
assert_eq!(finalized_deletion_count, 1);
assert!(other_tenant_state_is_unchanged);
```

- [ ] **Step 6: Run the suite**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_crash_recovery -- --test-threads=1
```

Expected: all transition-point recovery cases pass.

- [ ] **Step 7: Commit**

```bash
git add crates/memory-mcp/src/http/fault_injection.rs crates/memory-mcp/src/http/mod.rs crates/memory-mcp/src/http/composition.rs crates/memory-mcp/src/http/leases/migration.rs crates/memory-mcp/src/http/tasks/scheduler.rs crates/memory-mcp/src/http/subscriptions/scheduler.rs crates/memory-mcp/src/control/deletion.rs crates/memory-mcp/tests/http_crash_recovery.rs
git commit -m "test: add deterministic HTTP crash recovery gates"
```

---

### Task 7: Add durable Task and subscription-replica suites

**Files:**
- Create: `crates/memory-mcp/tests/http_durable_tasks.rs`
- Create: `crates/memory-mcp/tests/http_subscription_replica.rs`
- Modify: `crates/memory-mcp/src/http/tasks/mod.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/mod.rs`

**Interfaces:**
- Consumes: durable fixture and fault seam from Tasks 4 and 6.
- Produces: black-box evidence for restart, fencing, cancellation, retention, polling repair, cursor behavior, and tenant isolation.

- [ ] **Step 1: Expose narrow test drivers**

Under `cfg(any(test, feature = "test-fixtures"))`, expose one Task driver and one subscription driver. They call existing durable stores and schedulers but do not expose raw SurrealDB queries:

```rust
pub struct DurableTaskTestDriver {
    store: crate::http::tasks::worker::DurableTaskStore,
}

pub struct SubscriptionTestDriver {
    store: crate::http::subscriptions::DurableSubscriptionStore,
}
```

Methods must correspond to user-visible transitions: enqueue, claim, cancel, reconcile, retain, append change, poll, and read cursor.

- [ ] **Step 2: Write Task lifecycle tests**

Cover enqueue deduplication, capacity rejection, fenced claim/completion, cancellation before commit, completion before cancellation, stale-worker rejection, restart persistence, artifact reconciliation, cross-tenant denial, and retention cleanup.

- [ ] **Step 3: Write subscription replica tests**

Use two independent store/driver handles bound to the same tenant database. Cover filter validation, monotonic sequence, bounded coalescing, missed wakeup repaired by durable polling, restart from cursor, authorization expiry, slow-consumer failure, and cross-tenant denial.

- [ ] **Step 4: Run focused suites**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_durable_tasks -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_subscription_replica -- --test-threads=1
```

Expected: all cases pass against durable embedded storage.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/http/tasks/mod.rs crates/memory-mcp/src/http/subscriptions/mod.rs crates/memory-mcp/tests/http_durable_tasks.rs crates/memory-mcp/tests/http_subscription_replica.rs
git commit -m "test: cover durable tasks and subscription replicas"
```

---

### Task 8: Complete protocol, isolation, proxy, control-plane, and release evidence

**Files:**
- Create: `crates/memory-mcp/tests/http_control_plane.rs`
- Create: `scripts/http_release_evidence.sh`
- Create: `docs/operations/HTTP_INTEROP_MATRIX.md`
- Create: `docs/operations/HTTP_RELEASE_GATE.md`
- Modify: `crates/memory-mcp/tests/http_proto_conformance.rs`
- Modify: `crates/memory-mcp/tests/http_isolation.rs`
- Modify: `crates/memory-mcp/tests/http_proxy_streaming.rs`
- Modify: `docs/operations/RESTORE_DRILL.md`
- Modify: `docs/operations/CREDENTIAL_ROTATION.md`

**Interfaces:**
- Consumes: all production paths and integration fixtures from Tasks 3–7.
- Produces: truthful executable/local evidence and explicit pending external gates.

- [ ] **Step 1: Extend protocol coverage**

Add tests for SSE final response, response-body drop releasing admission/runtime guards, modern result envelopes, complete HTTP/JSON-RPC status mapping, Apps/Tasks/subscriptions capability gating, absence of session-resume headers, notifications returning `202`, header mismatch before authentication, and rejection of initialize/ping/legacy behavior.

- [ ] **Step 2: Extend isolation coverage**

Alternate two tenants under concurrency and assert isolation for episodes/facts, App Sessions, Tasks, quotas, outbox events, principal cache results, runtime identities, deletion state, and stale fencing generations.

- [ ] **Step 3: Add control-plane black-box coverage**

Without performing external OIDC login, seed an authenticated control-plane session explicitly and exercise account retrieval, CSRF issuance, API-key create/list/revoke, identity list, deletion challenge, operator authorization, and static route precedence. Assert the one-time key response has `Cache-Control: no-store` and the CSRF response is not shared across sessions.

- [ ] **Step 4: Make proxy evidence executable**

`scripts/http_release_evidence.sh` must require `MEMORY_MCP_TEST_PROXY_BIN` for proxy checks, record its version and config, and fail the proxy gate when absent. The test must prove `/mcp` streaming is unbuffered, MCP headers survive unchanged, proxy read timeout exceeds 120 seconds, and `/metrics` is blocked from the public listener.

- [ ] **Step 5: Write the interoperability matrix**

Use this table structure:

```markdown
| Client/SDK | Exact version | Protocol | Discover | Tool call | Notification | SSE final response | Result | Evidence |
|---|---:|---|---|---|---|---|---|---|
```

Include an explicit `Not executed` result until each client is actually run. Do not enter `Pass` from code inspection.

- [ ] **Step 6: Write the release-gate document**

Separate gates into:

```markdown
## Automated local gates
## External environment gates
## Release decision
```

Each row contains command, commit, timestamp, environment, result, and evidence path. Initial external results are `Not executed — release blocked`.

- [ ] **Step 7: Implement the evidence script**

The script uses `set -eu`, creates `target/http-release-evidence`, records `git rev-parse HEAD`, `rustc --version`, commands, exit codes, and load JSON. It runs local automated gates. It runs 500-tenant, proxy, interoperability, restore, and rotation commands only when their explicit environment gates are present; otherwise it writes `not_executed` and exits nonzero when invoked in release mode.

- [ ] **Step 8: Run local evidence**

```bash
scripts/http_release_evidence.sh local
```

Expected: local automated gates execute; the manifest records their actual results. External sections remain not executed.

- [ ] **Step 9: Commit**

```bash
git add crates/memory-mcp/tests/http_control_plane.rs crates/memory-mcp/tests/http_proto_conformance.rs crates/memory-mcp/tests/http_isolation.rs crates/memory-mcp/tests/http_proxy_streaming.rs scripts/http_release_evidence.sh docs/operations/HTTP_INTEROP_MATRIX.md docs/operations/HTTP_RELEASE_GATE.md docs/operations/RESTORE_DRILL.md docs/operations/CREDENTIAL_ROTATION.md
git commit -m "test: publish executable HTTP release gates"
```

---

### Task 9: Record and introduce capability-specific Registry interfaces

**Files:**
- Create: `docs/adr/0054-capability-specific-control-registry-interfaces.md`
- Modify: `crates/memory-mcp/src/http/registry/storage.rs:90-513,568-1665`
- Modify: `crates/memory-mcp/src/http/registry/surreal_store.rs:973-2369`
- Modify: `crates/memory-mcp/src/http/registry/mod.rs:16-284`

**Interfaces:**
- Consumes: explicit composition from Task 3 and existing atomic Registry behavior.
- Produces: eight crate-private capability traits and `RegistryStores`; later Tasks remove the omnibus trait after callers migrate.

- [ ] **Step 1: Write ADR-0054**

Record that consumer-oriented capability traits refine ADR-0044 and ADR-0052, required operations have no defaults, atomic cross-row operations remain storage-owned, one concrete allocation backs all trait views, and the split is crate-private.

- [ ] **Step 2: Add compile-time capability assertions before removing the old trait**

Add tests asserting both adapters implement all enabled capabilities:

```rust
fn assert_registry_capabilities<T>()
where
    T: RegistryHealth
        + AccountIdentityStore
        + CredentialStore
        + TenantProvisioningStore
        + PlanUsageStore,
{
}
```

Add control-plane bounds under `cfg(feature = "control-plane")`.

- [ ] **Step 3: Declare the eight traits with exact existing method signatures**

Move methods from `RegistryStore` into `RegistryHealth`, `AccountIdentityStore`, `CredentialStore`, `TenantProvisioningStore`, `PlanUsageStore`, `OidcRequestStore`, `ControlSessionStore`, and `AccountDeletionStore`. Use no method bodies. Keep `create_account_bundle`, `begin_account_deletion`, and `finalize_account_deletion` atomic.

Keep `ping(&self) -> bool` in this migration to avoid changing readiness semantics while splitting interfaces.

- [ ] **Step 4: Split both adapter implementations**

Replace each omnibus `impl RegistryStore for ...` with capability impl blocks. Move method bodies unchanged. Remove the stale `unavailable()` helper after no default calls remain.

- [ ] **Step 5: Add capability aggregation**

```rust
#[derive(Clone)]
pub(crate) struct RegistryStores {
    health: Arc<dyn RegistryHealth>,
    accounts: Arc<dyn AccountIdentityStore>,
    credentials: Arc<dyn CredentialStore>,
    provisioning: Arc<dyn TenantProvisioningStore>,
    plans: Arc<dyn PlanUsageStore>,
    #[cfg(feature = "control-plane")]
    oidc_requests: Arc<dyn OidcRequestStore>,
    #[cfg(feature = "control-plane")]
    sessions: Arc<dyn ControlSessionStore>,
    #[cfg(feature = "control-plane")]
    deletion: Arc<dyn AccountDeletionStore>,
}
```

Construct all fields from clones of one `Arc<SurrealRegistryStore>` or one `Arc<InMemoryStore>`. Add narrow accessors on `RegistryHandle`; do not expose `RegistryStores` publicly.

- [ ] **Step 6: Run adapter tests**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::registry::storage::tests
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::registry::surreal_store::tests
```

Expected: existing atomicity, fencing, identity, credential, plan, session, and deletion tests pass unchanged.

- [ ] **Step 7: Commit**

```bash
git add docs/adr/0054-capability-specific-control-registry-interfaces.md crates/memory-mcp/src/http/registry/storage.rs crates/memory-mcp/src/http/registry/surreal_store.rs crates/memory-mcp/src/http/registry/mod.rs
git commit -m "refactor: split control registry capabilities"
```

---

### Task 10: Migrate Registry consumers and remove the omnibus seam

**Files:**
- Modify: `crates/memory-mcp/src/http/principal/auth.rs`
- Modify: `crates/memory-mcp/src/http/registry/account.rs`
- Modify: `crates/memory-mcp/src/http/registry/plan.rs`
- Modify: `crates/memory-mcp/src/http/registry/provisioning.rs`
- Modify: `crates/memory-mcp/src/http/leases/mod.rs`
- Modify: `crates/memory-mcp/src/http/leases/migration.rs`
- Modify: `crates/memory-mcp/src/http/middleware.rs`
- Modify: `crates/memory-mcp/src/http/app_sessions/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/tasks/scheduler.rs`
- Modify: `crates/memory-mcp/src/http/subscriptions/scheduler.rs`
- Modify: `crates/memory-mcp/src/control/{account_api.rs,oidc.rs,session.rs,deletion.rs,operator.rs}`
- Modify: `crates/memory-mcp/src/http/registry/{mod.rs,storage.rs}`

**Interfaces:**
- Consumes: capability traits and accessors from Task 9.
- Produces: consumers that know only required capabilities; removes `RegistryStore` and `RegistryHandle::store_clone()`.

- [ ] **Step 1: Migrate authentication and account resolution**

Change `Authenticator` to own `Arc<dyn AccountIdentityStore>` and `Arc<dyn CredentialStore>`. Change `AccountResolver` to own `Arc<dyn TenantProvisioningStore>`. Update constructors and tests.

- [ ] **Step 2: Migrate OIDC and session consumers**

Use `oidc_requests()`, `sessions()`, `accounts()`, and `provisioning()` accessors. No handler receives the aggregate stores object.

- [ ] **Step 3: Migrate plan, quota, and maintenance consumers**

Schedulers receive `Arc<dyn TenantProvisioningStore>` for tenant enumeration plus their tenant-local stores. Plan reconciliation receives `PlanUsageStore`. Middleware receives only account, credential, provisioning, and plan capabilities used by each step.

- [ ] **Step 4: Migrate provisioning and deletion workers**

Change `ProvisioningLease::{heartbeat,release}`, `provision_one`, transitions, reconciliation, and deletion operations to the narrow traits. Preserve all atomic operation calls.

- [ ] **Step 5: Remove low-level production writes from the broad surface**

Keep direct `write_account`, `write_tenant`, and `write_api_key` behind fixture-only setup helpers or adapter tests. Normal application code must use `create_account_bundle` and `create_api_key_if_below_limit`.

- [ ] **Step 6: Prove the broad seam is gone**

Run structural searches and require zero production references to `RegistryStore` and `store_clone`. Then delete the old trait and accessor.

- [ ] **Step 7: Run focused suites**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::principal::auth
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::registry
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures http::leases
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures control::
```

Expected: pass with no capability fallback behavior.

- [ ] **Step 8: Commit**

```bash
git add crates/memory-mcp/src/http crates/memory-mcp/src/control
git commit -m "refactor: narrow registry dependencies by use case"
```

---

### Task 11: Extract API-key creation from Axum

**Files:**
- Create: `crates/memory-mcp/src/control/application/mod.rs`
- Create: `crates/memory-mcp/src/control/application/api_keys.rs`
- Modify: `crates/memory-mcp/src/control/mod.rs`
- Modify: `crates/memory-mcp/src/control/account_api.rs:95-208`

**Interfaces:**
- Consumes: Account, provisioning, plan, and credential capabilities from Task 10.
- Produces: `ApiKeyCreation`, `CreateApiKeyCommand`, and `CreatedApiKey`.

- [ ] **Step 1: Write application tests first**

Cover missing account, missing tenant, active-key cap, deterministic expiry from supplied `now`, generated verifier matching the returned secret, and successful one-time-secret result. Tests instantiate `ApiKeyCreation` with in-memory capability adapters and no Axum router.

- [ ] **Step 2: Implement the workflow**

```rust
pub(crate) struct CreateApiKeyCommand {
    pub account_id: String,
    pub name: String,
    pub expires_in_days: Option<u32>,
}

pub(crate) struct CreatedApiKey {
    pub id: String,
    pub secret: String,
    pub name: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) struct ApiKeyCreation {
    accounts: Arc<dyn AccountIdentityStore>,
    provisioning: Arc<dyn TenantProvisioningStore>,
    plans: Arc<dyn PlanUsageStore>,
    credentials: Arc<dyn CredentialStore>,
    api_key_pepper: Arc<[u8]>,
}

impl ApiKeyCreation {
    pub(crate) async fn execute(
        &self,
        command: CreateApiKeyCommand,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<CreatedApiKey, crate::error::MemoryError>;
}
```

Generate the secret inside `execute`; return it once. Use the supplied `now` for both creation and expiry.

- [ ] **Step 3: Thin the handler**

Keep body parsing/default-name compatibility and response serialization in `account_api.rs`. Replace lines 153–189 with construction and execution of `ApiKeyCreation`. Preserve `Cache-Control: no-store` and `201 Created`.

- [ ] **Step 4: Run workflow and handler tests**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures control::application::api_keys
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures control::account_api
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/control/application crates/memory-mcp/src/control/mod.rs crates/memory-mcp/src/control/account_api.rs
git commit -m "refactor: extract API key creation workflow"
```

---

### Task 12: Extract OIDC signup from the HTTP adapter

**Files:**
- Create: `crates/memory-mcp/src/control/application/oidc_signup.rs`
- Modify: `crates/memory-mcp/src/control/application/mod.rs`
- Modify: `crates/memory-mcp/src/control/oidc.rs:684-835`

**Interfaces:**
- Consumes: account and provisioning capabilities from Task 10.
- Produces: `VerifiedExternalIdentity` and `OidcSignup::resolve_or_create`.

- [ ] **Step 1: Write application tests first**

Cover existing identity, invite-only rejection at the adapter before workflow invocation, successful open signup, atomic bundle persistence, provisioning-event append, uniqueness conflict followed by successful reread, and conflict preserved when reread finds no account. Assert the input type contains no raw OIDC subject.

- [ ] **Step 2: Implement verified-identity signup**

```rust
pub(crate) struct VerifiedExternalIdentity {
    pub issuer: String,
    pub subject_verifier: SubjectVerifier,
}

pub(crate) struct OidcSignup {
    accounts: Arc<dyn AccountIdentityStore>,
    provisioning: Arc<dyn TenantProvisioningStore>,
}

impl OidcSignup {
    pub(crate) async fn resolve_or_create(
        &self,
        identity: VerifiedExternalIdentity,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Account, crate::error::MemoryError>;
}
```

On `MemoryError::Conflict` from `create_account_bundle`, reread by identity and return the winner when present; otherwise return the original conflict. Append the provisioning event only for the account created by this call.

- [ ] **Step 3: Thin the OIDC callback**

Keep state lookup, payload unsealing, expiry/issuer/nonce checks, token exchange, claim validation, blind-index computation, browser-session creation, cookie headers, and redirects in `oidc.rs`. Replace `upsert_account_for_identity` with `OidcSignup` invocation. Enforce `SignupMode::InviteOnly` before calling the creation path.

- [ ] **Step 4: Run tests**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures control::application::oidc_signup
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures control::oidc
```

Expected: pass, including logout and cryptographic helper tests.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/control/application/oidc_signup.rs crates/memory-mcp/src/control/application/mod.rs crates/memory-mcp/src/control/oidc.rs
git commit -m "refactor: extract OIDC signup workflow"
```

---

### Task 13: Add an internal `MemoryService` dependency seam

**Files:**
- Modify: `crates/memory-mcp/src/service/core/builder.rs:28-562`

**Interfaces:**
- Consumes: existing `DbClient`, `EntityExtractor`, `EmbeddingProvider`, and `TripleExtractor`.
- Produces: crate-private `MemoryServiceDependencies` while preserving all public constructors.

- [ ] **Step 1: Add a test proving custom triple extraction can be injected**

Create a small test `TripleExtractor` adapter and assert a service built through the crate-private seam retains it. The test must not change public constructor signatures.

- [ ] **Step 2: Define the dependency bundle**

```rust
pub(crate) struct MemoryServiceDependencies {
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
    pub(crate) embedding_provider: Arc<dyn EmbeddingProvider>,
    pub(crate) triple_extractor: Arc<dyn TripleExtractor>,
}
```

Do not include caches, loggers, rate limiters, stores derived from `DbClient`, semaphores, lifecycle workers, or runtime state.

- [ ] **Step 3: Change the private builder only**

Change `MemoryService::build` to accept `MemoryServiceDependencies` plus the existing namespace, log level, and `ServiceBuildConfig`. Make `new` and `new_with_embedding_provider` construct the bundle with existing defaults. Replace only the hard-coded `RuleBasedTripleExtractor` assignment with the injected dependency.

- [ ] **Step 4: Run service tests**

```bash
cargo test -p memory_mcp service::core::builder::tests
cargo test -p memory_mcp service::
```

Expected: pass with public constructor behavior unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service/core/builder.rs
git commit -m "refactor: add internal memory service dependency seam"
```

---

### Task 14: Characterize and decompose context assembly

**Files:**
- Modify: `crates/memory-mcp/src/service/context/pipeline.rs:182-688`

**Interfaces:**
- Consumes: existing `RetrievalContext`, retrieval helpers, rankers, and fallback strategy.
- Produces: `RetrievedFactChannels`, `SelectedContext`, `retrieve_fact_channels`, `rank_fact_channels`, and `select_context_facts` as concrete private phases.

- [ ] **Step 1: Add characterization tests before moving code**

Add tests for deterministic direct ordering, cross-channel deduplication, temporal-window timing, timeline versus relevance selection, empty-fact episode fallback, episode rescue winning over facts, graph tier preventing episode rescue, first-person appenders respecting budget, semantic availability propagation, and rescue logging before return.

- [ ] **Step 2: Run characterization tests**

```bash
cargo test -p memory_mcp service::context::pipeline::tests
```

Expected: pass before structural changes.

- [ ] **Step 3: Extract concrete phase result types**

```rust
struct RetrievedFactChannels {
    direct: Vec<FactRecord>,
    graph: Vec<GraphFactCandidate>,
    community: Vec<CommunityFactCandidate>,
    semantic: Vec<SemanticFactCandidate>,
    direct_tier: RetrievalTier,
}

struct SelectedContext {
    selected: Vec<RankedContextFact>,
    all_ranked: Vec<RankedContextFact>,
}
```

Use the exact existing record types from the pipeline; do not introduce traits.

- [ ] **Step 4: Extract three substantial functions**

```rust
async fn retrieve_fact_channels(...) -> Result<RetrievedFactChannels, MemoryError>;
fn rank_fact_channels(...) -> Vec<RankedContextFact>;
fn select_context_facts(...) -> SelectedContext;
```

Preserve the current sequential order because alias, triple, graph, community, and semantic channels depend on accumulated exclusion sets and score adjustments. Keep episode rescue orchestration in `assemble_default_context` so the top-level policy remains visible.

- [ ] **Step 5: Re-run context and production suites**

```bash
cargo test -p memory_mcp service::context::pipeline::tests
cargo test -p memory_mcp service::context
cargo test -p memory_mcp
```

Expected: pass with no snapshot/result changes.

- [ ] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/service/context/pipeline.rs
git commit -m "refactor: clarify context assembly phases"
```

---

### Task 15: Classify public wrappers and reconcile documentation

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/storage.rs:77,167-176`
- Modify: `crates/memory-mcp/src/http/tasks/scheduler.rs:19-21`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs:304-310`
- Modify: `crates/memory-mcp/src/control/account_api.rs:1-4,82-91`
- Modify: `crates/memory-mcp/src/http/registry/storage.rs:1-7`
- Modify: `crates/memory-mcp/src/http/health.rs:51-57`
- Modify: `docs/superpowers/plans/2026-09-01-streamable-http-saas-completion.md:910-925,993-1111`
- Modify: `docs/superpowers/specs/2026-08-27-streamable-http-saas.md:1-7`
- Modify exact matching lines in: `README.md`

**Interfaces:**
- Consumes: completed implementation and evidence state from prior tasks.
- Produces: deliberate compatibility status and truthful documentation.

- [ ] **Step 1: Classify wrappers using public documentation and package usage**

For `build_runtime`, `TenantRuntime::from_bound_client`, `MemoryMcp::service`, and `tasks::scheduler_job`, search documentation/examples and inspect package exports. Record each result in the commit message body or PR notes.

- [ ] **Step 2: Deprecate unintentional public wrappers without removing them**

For wrappers with no documented external contract, add a specific replacement:

```rust
#[deprecated(note = "use build_runtime_with_options")]
```

Use equivalent precise notes for `scheduler_job`. Keep `MemoryMcp::service` when library consumers need service access; otherwise deprecate it with the exact supported replacement. Do not remove any wrapper in this non-breaking program.

- [ ] **Step 3: Correct CSRF behavior and documentation**

Treat `/api/v1/account/csrf` as canonical. Add `Cache-Control: no-store` to the CSRF response specifically, then add a handler test asserting it. Update only exact README/spec/plan route references from `/api/v1/account/session/csrf` to `/api/v1/account/csrf`.

- [ ] **Step 4: Remove stale source commentary**

Replace comments claiming the Registry or readiness implementation is a placeholder. Replace the `account_api.rs` “Stub” module description with a factual description of implemented account/control routes.

- [ ] **Step 5: Mark plan items from actual evidence**

In the 2026-09-01 completion plan, check only steps with passing recorded evidence. Leave external gates unchecked and link them to `HTTP_RELEASE_GATE.md`. Keep status language “core implementation complete; release verification incomplete; not production-ready” until every external gate passes.

- [ ] **Step 6: Verify README user changes were preserved**

Run:

```bash
git diff -- README.md
```

Expected: the existing user edits remain; this task adds only the exact CSRF/status corrections described above.

- [ ] **Step 7: Run focused checks**

```bash
cargo test -p memory_mcp --features streamable-http,control-plane,test-fixtures control::account_api
cargo check -p memory_mcp --all-targets --features streamable-http,mcp-apps,control-plane,test-fixtures --locked
git diff --check
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add crates/memory-mcp/src/http/runtime/storage.rs crates/memory-mcp/src/http/tasks/scheduler.rs crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/control/account_api.rs crates/memory-mcp/src/http/registry/storage.rs crates/memory-mcp/src/http/health.rs docs/superpowers/plans/2026-09-01-streamable-http-saas-completion.md docs/superpowers/specs/2026-08-27-streamable-http-saas.md README.md
git commit -m "docs: reconcile HTTP interfaces and release status"
```

---

### Task 16: Run the final quality and release matrix

**Files:**
- Modify only when a failure is caused by this plan's changes.
- Evidence: `docs/operations/HTTP_RELEASE_GATE.md`

**Interfaces:**
- Consumes: all prior tasks.
- Produces: final local quality evidence and an explicit release-blocked state for any unexecuted external gate.

- [ ] **Step 1: Run formatting**

```bash
cargo fmt --all --check
```

Expected: exit 0. If formatting differs, run `cargo fmt --all`, inspect the diff, and rerun the check.

- [ ] **Step 2: Run workspace checks**

```bash
cargo check --workspace --all-targets --locked
cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,prometheus,control-plane,test-fixtures --locked
```

Expected: both pass.

- [ ] **Step 3: Run default and feature tests**

```bash
cargo test -p memory_mcp
cargo test -p memory_mcp --features fs-watch,mcp-apps,streamable-http,control-plane,test-fixtures
```

Expected: pass.

- [ ] **Step 4: Run every HTTP integration target**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_registry_storage -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_proto_conformance
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_isolation -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_proxy_streaming -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_control_plane -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_durable_tasks -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_subscription_replica -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_crash_recovery -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_20_active_tenants_under_expected_qps -- --test-threads=1
```

Expected: all pass.

- [ ] **Step 5: Run the required zero-warning clippy gate**

```bash
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane,prometheus,test-fixtures --locked -- -D warnings
```

Expected: pass with zero warnings.

- [ ] **Step 6: Run source and documentation checks**

Confirm zero production occurrences of `RegistryStore`, `store_clone`, feature-driven `NoopMigrations` selection, direct test `HttpState` literals, ignored HTTP release tests, placeholder messages, stale `/api/v1/account/session/csrf`, and Rust placeholder macros.

Run:

```bash
git diff --check
scripts/http_release_evidence.sh local
```

Expected: both pass; local evidence is updated with actual commit and results.

- [ ] **Step 7: Record external gates truthfully**

If `MEMORY_MCP_RUN_500_LOAD`, remote SurrealDB credentials, proxy binary, and client SDK environments are unavailable, retain `Not executed — release blocked` for those rows. If available, run the exact release script modes and attach generated evidence. Do not change the production-ready status until every required external row passes.

- [ ] **Step 8: Commit final local evidence**

```bash
git add docs/operations/HTTP_RELEASE_GATE.md docs/operations/HTTP_INTEROP_MATRIX.md
git commit -m "test: record architecture remediation release evidence"
```

---

## Self-review

### Spec coverage

- Feature-matrix correctness: Tasks 2 and 16.
- Explicit production/test composition and `test-fixtures` semantics: Tasks 1 and 3.
- Durable embedded black-box composition: Tasks 3 and 4.
- Load, crash/recovery, Task, subscription, protocol, isolation, proxy, control-plane, and operational evidence: Tasks 5–8 and 16.
- ADR-0053 and ADR-0054: Tasks 1 and 9.
- Capability-specific Registry interfaces and removal of permissive defaults: Tasks 9 and 10.
- API-key and OIDC application workflows: Tasks 11 and 12.
- `MemoryServiceDependencies`: Task 13.
- Context characterization and concrete phase extraction: Task 14.
- Public-wrapper classification, CSRF drift, stale comments, and truthful status: Task 15.
- Required project quality gate: Task 16.

No approved design requirement is unassigned.

### Placeholder scan

The plan contains no implementation placeholders. Operational `Not executed` values are deliberate truthful evidence states, not missing implementation instructions.

### Type consistency

- `HttpStateTestBuilder` always delegates to `HttpState::assemble`.
- `HttpProductionComposition` and `HttpTestComposition` both produce `RegistryHandle` plus `Arc<dyn ApplyMigrations>`; `HttpRuntime` carries the production adapter from `build_state` into `SchedulerHooks`.
- `RegistryHandle` accessors return the eight capability trait objects introduced in Task 9.
- `Authenticator` consumes account and credential capabilities; `AccountResolver` consumes provisioning.
- `ApiKeyCreation` consumes accounts, provisioning, plans, and credentials.
- `OidcSignup` consumes accounts and provisioning and accepts only `SubjectVerifier`, never raw `sub`.
- `MemoryServiceDependencies` contains only the four approved variable collaborators.
- Context phase types remain private concrete structs and do not become new public interfaces.

### Scope and risk review

- No dependency or migration change is planned.
- No new MCP interface is planned.
- Registry splitting occurs only after trustworthy composition and release-test seams exist.
- Atomic storage operations remain intact.
- Public wrappers are deprecated rather than removed.
- External release evidence is explicitly separated from local CI evidence.
- Existing `README.md` work is protected by exact-line edits and a diff review.
