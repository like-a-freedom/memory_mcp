# Hobby-Scale Post-Refactor Stabilization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove every grounded post-refactor correctness and security risk, while replacing unfinished SaaS release-certification machinery with a small validation loop suitable for a single-user hobby project. Benchmark integrity remains a mandatory P0 workstream.

**Architecture:** Keep the current protocol, durable state machines, and `RegistryStore` composition unchanged. Reconcile deletion results against durable state, propagate constructor errors, carry the episode-fallback decision once, remove the unused Registry capability experiment, harden URL fetching, and rely on ordinary Rust checks plus bounded regressions instead of a release-evidence matrix. Execute the companion evaluation-integrity plan before claiming stabilization complete.

**Tech Stack:** Rust 2024, Rust 1.97.1, Tokio, Axum, SurrealDB, Cargo, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-02-architecture-audit-remediation-design.md` (correctness requirements only). This plan supersedes its unfinished capability-split and release-evidence work for the current single-user hobby scope.

**Required companion plan:** `docs/superpowers/plans/2026-09-04-evaluation-integrity.md`. Benchmark work is not deferred and completion of its integrity gate is required before this plan closes.

## Global Constraints

- The supported Rust toolchain is exactly `1.97.1`; CI must not claim an older MSRV.
- Do not add or change dependencies in `Cargo.toml`.
- Do not add MCP tools; the eight-tool surface remains frozen.
- Do not modify migrations or generated files.
- Keep business logic out of `main.rs` and transport handlers.
- Preserve bi-temporal facts and deletion tombstones; no fact deletion is introduced.
- Production code must return `Result` rather than use `unwrap()` or `expect()` for fallible initialization.
- The target is one human user. There is no 50- or 500-concurrent-Tenant SLA.
- Keep the existing 20-Tenant in-memory HTTP test as a cheap isolation/concurrency regression, not as a production capacity claim.
- Proxy certification, multi-SDK interoperability, remote restore drills, and credential-rotation drills become optional deployment checklists, not merge or release blockers.
- No generated evidence bundle, `gates.tsv`, or committed test snapshot is required; the commit SHA and CI logs are sufficient evidence for this scope.
- Every benchmark and registered evaluation suite must have an explicit profile or platform-specific execution row. Missing benchmark prerequisites are incomplete coverage, never a green result.

## File Structure

- `crates/memory-mcp/src/control/deletion.rs` — reconcile a worker error with the durable `Purged` terminal state.
- `crates/memory-mcp/tests/http_crash_recovery.rs` — deterministic single-attempt deletion regression test.
- `crates/memory-mcp/src/service/core/builder.rs` — propagate default extractor construction failure.
- `crates/memory-mcp/src/control/application/api_keys.rs` — redact the one-time API-key secret from `Debug` and borrow the configured pepper.
- `crates/memory-mcp/src/control/application/oidc_signup.rs` — test the actual create-conflict/reread race.
- `crates/memory-mcp/src/http/registry/storage.rs` — provide a deterministic test-only OIDC conflict injection seam.
- `crates/memory-mcp/src/service/context/pipeline.rs` — carry one episode-fallback decision between phases.
- `crates/memory-mcp/src/service/content_extraction.rs` — enforce bounded, non-private URL retrieval.
- `crates/memory-mcp/src/http/registry/mod.rs` — stop compiling the unused Registry capability module.
- `crates/memory-mcp/src/http/registry/capabilities.rs` — delete the nominal eight-trait experiment and its witness tests.
- `docs/adr/0054-capability-specific-control-registry-interfaces.md` — mark the capability decision superseded and record the KISS rationale.
- `crates/memory-mcp/tests/http_load_concurrency.rs` — remove the irrelevant 500-Tenant release gate while retaining the 20-Tenant regression.
- `.github/workflows/ci.yml` — align the toolchain with `rust-version` and describe the retained test truthfully.
- `scripts/http_release_evidence.sh` — delete the unfinished evidence orchestrator.
- `docs/operations/HTTP_RELEASE_GATE.md` — replace the certification matrix with the small hobby validation contract.
- `docs/operations/HTTP_INTEROP_MATRIX.md` — make client interoperability explicitly optional and manually recorded.
- `docs/operations/RESTORE_DRILL.md` — decouple the useful runbook from the deleted evidence script.
- `docs/superpowers/plans/2026-09-02-architecture-audit-remediation.md` — add a supersession note pointing at this plan; do not rewrite its historical task bodies.
- `crates/memory-mcp/tests/http_durable_tasks.rs` — remove an unproved process-restart claim from a cross-handle test.
- `crates/memory-mcp/tests/http_subscription_replica.rs` — rename the live second-replica test truthfully.
- `docs/superpowers/plans/2026-09-04-evaluation-integrity.md` — mandatory benchmark/profile integrity workstream.

---

### Task 1: Make committed deletion return a successful outcome

**Files:**
- Modify: `crates/memory-mcp/tests/http_crash_recovery.rs:479-608`
- Modify: `crates/memory-mcp/src/control/deletion.rs:110-190`

**Interfaces:**
- Consumes: `RegistryStore::find_tenant_by_id(&str) -> Result<Option<Tenant>, MemoryError>` and `TenantStatus::Purged`.
- Produces: `async fn deletion_is_purged(store: &dyn RegistryStore, tenant_id: &str) -> Result<bool, MemoryError>`; `run_deletion_worker` treats a durable `Purged` state as success even if heartbeat or post-commit fault reporting won the race.

- [ ] **Step 1: Replace the retry-masked recovery assertion with a single-attempt regression**

In `deletion_recovers_after_finalize_transient`, keep the provisioning and sibling setup, then replace the eight-attempt loop and `consumed <= 1` assertion with:

```rust
let fault_injector = Arc::new(FailOnceAt::new(FaultPoint::AccountDeletionFinalized));
memory_mcp::control::deletion::run_deletion_worker(
    registry.clone(),
    fault_injector.clone(),
)
.await
.expect("a durably committed deletion is reported as success");

let after_first = store
    .find_tenant_by_id(&tenant_id)
    .await
    .expect("tenant lookup")
    .expect("tenant present");
assert_eq!(after_first.status, TenantStatus::Purged);
assert_eq!(
    fault_injector.consumed(),
    1,
    "the post-finalize fault must be exercised exactly once"
);
```

Keep the second `run_deletion_worker(..., NoFaults)` call and sibling assertions to preserve idempotency and isolation coverage.

- [ ] **Step 2: Run the focused test and verify the race is exposed**

Run:

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_crash_recovery deletion_recovers_after_finalize_transient -- --test-threads=1
```

Expected before the fix: intermittent or deterministic failure with `provisioning lease lost` or the injected post-finalize transient instead of success.

- [ ] **Step 3: Add durable terminal-state reconciliation**

Add this private helper beside `run_deletion_worker`:

```rust
async fn deletion_is_purged(
    store: &dyn crate::http::registry::storage::RegistryStore,
    tenant_id: &str,
) -> Result<bool, MemoryError> {
    Ok(store
        .find_tenant_by_id(tenant_id)
        .await?
        .is_some_and(|tenant| {
            tenant.status == crate::http::registry::models::TenantStatus::Purged
        }))
}
```

Replace the current `if let Err(error) = cleanup` branch with:

```rust
if let Err(error) = cleanup {
    if deletion_is_purged(store.as_ref(), &tenant_id).await? {
        continue;
    }
    let _ = lease.release(store.as_ref(), &tenant_id).await;
    if first_error.is_none() {
        first_error = Some(error);
    }
}
```

This is intentionally local to deletion. Do not weaken generic lease fencing and do not add timing delays or a biased `tokio::select!`.

- [ ] **Step 4: Prove the regression is deterministic**

Run the focused test ten times:

```bash
for run in {1..10}; do
  cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_crash_recovery deletion_recovers_after_finalize_transient -- --test-threads=1 || exit 1
done
```

Expected: all ten runs pass, every run consumes the fault exactly once, and the sibling remains `Ready`.

- [ ] **Step 5: Commit the deletion fix**

```bash
git add crates/memory-mcp/src/control/deletion.rs crates/memory-mcp/tests/http_crash_recovery.rs
git commit -m "fix(http): reconcile committed deletion outcome"
```

---

### Task 2: Propagate default extractor initialization errors

**Files:**
- Modify: `crates/memory-mcp/src/service/core/builder.rs:112-130`
- Modify: `crates/memory-mcp/src/service/core/builder.rs:477-496`
- Test: `crates/memory-mcp/src/service/core/builder.rs:666-804`

**Interfaces:**
- Consumes: `AnnoEntityExtractor::new() -> Result<AnnoEntityExtractor, MemoryError>`.
- Produces: `MemoryServiceDependencies::with_db_client(Arc<dyn DbClient>) -> Result<Self, MemoryError>`.

- [ ] **Step 1: Add a compile-time-shape unit test for the fallible constructor**

Reuse the in-memory SurrealDB setup from `triple_extractor_is_retained_by_the_service` and add:

```rust
#[tokio::test]
async fn default_dependencies_propagate_extractor_initialization() {
    let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
        surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem engine init");
    db.use_ns("test_namespace")
        .use_db("memory")
        .await
        .expect("bind");
    let db_client: Arc<dyn crate::storage::DbClient> = Arc::new(
        crate::storage::SurrealDbClient::from_prebound_mem(
            db,
            "test_namespace",
            "error",
        ),
    );

    MemoryServiceDependencies::with_db_client(db_client)
        .expect("default dependencies initialize without panicking");
}
```

- [ ] **Step 2: Run the unit test and verify it fails to compile against the infallible signature**

Run:

```bash
cargo test -p memory_mcp service::core::builder::tests::default_dependencies_propagate_extractor_initialization --locked
```

Expected before the fix: compile failure because `with_db_client` returns `Self`, not `Result<Self, MemoryError>`.

- [ ] **Step 3: Change the constructor to propagate the error**

Implement:

```rust
pub(crate) fn with_db_client(
    db_client: Arc<dyn DbClient>,
) -> Result<Self, MemoryError> {
    Ok(Self {
        db_client,
        entity_extractor: Arc::new(AnnoEntityExtractor::new()?),
        embedding_provider: Arc::new(DisabledEmbeddingProvider::new(
            crate::config::DEFAULT_EMBEDDING_DIMENSION,
        )),
        triple_extractor: Arc::new(RuleBasedTripleExtractor::new()),
    })
}
```

At the public constructor call site, pass `MemoryServiceDependencies::with_db_client(db_client)?` into `Self::build`.

- [ ] **Step 4: Run focused and production-crate tests**

```bash
cargo test -p memory_mcp service::core::builder::tests --locked
cargo test -p memory_mcp --locked
```

Expected: both pass; production initialization contains no `expect("default entity extractor")`.

- [ ] **Step 5: Commit the constructor fix**

```bash
git add crates/memory-mcp/src/service/core/builder.rs
git commit -m "fix(service): propagate default extractor errors"
```

---

### Task 3: Redact API-key secrets and stop cloning the pepper per request

**Files:**
- Modify: `crates/memory-mcp/src/control/application/api_keys.rs:29-38`
- Test: `crates/memory-mcp/src/control/application/api_keys.rs:145-410`

**Interfaces:**
- Consumes: the existing `CreatedApiKey` fields and HTTP response mapping.
- Produces: a manual `Debug` implementation that never formats `secret`; `ApiKeyCreation<'a>` borrows pepper bytes; no response schema changes.

- [ ] **Step 1: Add a redaction regression test**

```rust
#[test]
fn created_api_key_debug_redacts_secret() {
    let created = CreatedApiKey {
        id: "ak_test".to_string(),
        secret: "mem_sk_should_never_appear".to_string(),
        name: "test key".to_string(),
        expires_at: None,
    };

    let rendered = format!("{created:?}");
    assert!(!rendered.contains("mem_sk_should_never_appear"));
    assert!(rendered.contains("[REDACTED]"));
}
```

- [ ] **Step 2: Run the test and verify the derived Debug leaks the secret**

```bash
cargo test -p memory_mcp control::application::api_keys::tests::created_api_key_debug_redacts_secret --features control-plane --locked
```

Expected before the fix: failure because the rendered struct contains the full secret.

- [ ] **Step 3: Replace derived Debug with a manual redacted implementation**

Remove `#[derive(Debug)]` and add:

```rust
impl std::fmt::Debug for CreatedApiKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreatedApiKey")
            .field("id", &self.id)
            .field("secret", &"[REDACTED]")
            .field("name", &self.name)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
```

Do not add a secret-wrapper dependency in this hobby stabilization pass.

- [ ] **Step 4: Borrow the configured pepper instead of cloning it**

Change the workflow to:

```rust
pub struct ApiKeyCreation<'a> {
    store: RegistryHandle,
    api_key_pepper: &'a [u8],
}

impl<'a> ApiKeyCreation<'a> {
    pub fn new(store: RegistryHandle, api_key_pepper: &'a str) -> Self {
        Self { store, api_key_pepper: api_key_pepper.as_bytes() }
    }
}
```

Pass `&state.config.api_key_pepper` from the handler and call `KeyedVerifier::compute(self.api_key_pepper, secret.as_bytes())`. In tests, bind generated peppers to a local variable before constructing the workflow so no temporary is borrowed across `.await`.

- [ ] **Step 5: Run the API-key workflow tests**

```bash
cargo test -p memory_mcp control::application::api_keys::tests --features control-plane --locked
```

Expected: all workflow and redaction tests pass.

- [ ] **Step 6: Commit the API-key hardening**

```bash
git add crates/memory-mcp/src/control/application/api_keys.rs crates/memory-mcp/src/control/account_api.rs
git commit -m "fix(control): protect API key secret material"
```

---

### Task 4: Compute the episode fallback decision once

**Files:**
- Modify: `crates/memory-mcp/src/service/context/pipeline.rs:239-261`
- Modify: `crates/memory-mcp/src/service/context/pipeline.rs:545-706`
- Test: `crates/memory-mcp/src/service/context/pipeline.rs:710-855`

**Interfaces:**
- Consumes: `EpisodeFallbackStrategy::decide(...) -> FallbackDecision`.
- Produces: `SelectedContext::prefer_episode_content: bool`, consumed by finalization without recomputation.

- [ ] **Step 1: Add a shape test that requires the decision to travel with selection**

```rust
#[test]
fn selected_context_carries_episode_fallback_decision() {
    let selection = SelectedContext {
        selected: Vec::new(),
        ranked_candidates: Vec::new(),
        episode_fallback_items: Vec::new(),
        prefer_episode_content: true,
    };

    assert!(selection.prefer_episode_content);
}
```

- [ ] **Step 2: Run the test and verify the field is missing**

```bash
cargo test -p memory_mcp service::context::pipeline::tests::selected_context_carries_episode_fallback_decision --locked
```

Expected before the fix: compile failure for unknown field `prefer_episode_content`.

- [ ] **Step 3: Carry the decision through `SelectedContext`**

Add the field:

```rust
struct SelectedContext {
    selected: Vec<RankedContextFact>,
    ranked_candidates: Vec<RankedContextFact>,
    episode_fallback_items: Vec<AssembledContextItem>,
    prefer_episode_content: bool,
}
```

Set it to `false` in both early-return constructors. In the normal constructor, store the already computed `prefer_episode_content` and remove `let _ = prefer_episode_content`.

Destructure the field in `finalize_with_first_person_appenders`:

```rust
let SelectedContext {
    selected: selected_ranked,
    ranked_candidates,
    episode_fallback_items,
    prefer_episode_content,
} = selection;
```

Delete the second call to `EpisodeFallbackStrategy.decide`. Also remove the unused `service` parameter from `finalize_with_first_person_appenders` and update its single caller.

- [ ] **Step 4: Run context tests**

```bash
cargo test -p memory_mcp service::context --locked
```

Expected: all context ordering, fallback, budget, and rescue tests pass.

- [ ] **Step 5: Commit the pipeline cleanup**

```bash
git add crates/memory-mcp/src/service/context/pipeline.rs
git commit -m "refactor(context): carry fallback decision between phases"
```

---

### Task 5: Remove the nominal Registry capability split

**Files:**
- Delete: `crates/memory-mcp/src/http/registry/capabilities.rs`
- Modify: `crates/memory-mcp/src/http/registry/mod.rs:6-12`
- Modify: `docs/adr/0054-capability-specific-control-registry-interfaces.md`

**Interfaces:**
- Consumes: the fact that production consumers still use `Arc<dyn RegistryStore>` and no production adapter implements the eight Registry capability traits.
- Produces: one honest internal storage interface; ADR-0054 status `Superseded` with an explicit reintroduction trigger.

- [ ] **Step 1: Confirm the module is isolated before deletion**

Run Octocode structural reference searches for `assert_registry_capabilities`, `RegistryHealth`, `AccountIdentityStore`, and `CredentialStore`.

Expected: all Registry-capability references are definitions or witness tests inside `http/registry/capabilities.rs`; `http/registry/mod.rs` only declares the module. Do not confuse these types with the separate, actively used `service::capabilities` module.

- [ ] **Step 2: Remove the unused module**

Delete this declaration from `http/registry/mod.rs`:

```rust
pub mod capabilities;
```

Delete `crates/memory-mcp/src/http/registry/capabilities.rs` in full. Do not add replacement traits or an aggregator.

- [ ] **Step 3: Supersede ADR-0054**

Replace its status and decision summary with:

```markdown
## Status

Superseded — 2026-09-04, hobby-scope simplification.

## Superseding decision

The project keeps the existing crate-private `RegistryStore` seam. The proposed
eight capability traits had no production consumers or adapter implementations,
so they increased surface area without changing runtime boundaries, security, or
testability for the current single-user deployment.

Reconsider a narrower split only when at least two production consumers need
materially different subsets of Registry operations, or when a concrete test
cannot be written without implementing unrelated methods. At that point, design
the smallest split demanded by those callers rather than restoring all eight
speculative traits.
```

Keep the original Context/Decision text below under `## Historical decision` so the rationale remains auditable.

- [ ] **Step 4: Verify all feature combinations still compile**

```bash
cargo check -p memory_mcp --all-targets --no-default-features --locked
cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,control-plane,test-fixtures --locked
```

Expected: both pass with no dead capability module and no missing imports.

- [ ] **Step 5: Commit the simplification**

```bash
git add crates/memory-mcp/src/http/registry/mod.rs docs/adr/0054-capability-specific-control-registry-interfaces.md
git add -u crates/memory-mcp/src/http/registry/capabilities.rs
git commit -m "refactor(http): remove unused Registry capability split"
```

---

### Task 6: Exercise the real OIDC create-conflict/reread path

**Files:**
- Modify: `crates/memory-mcp/src/http/registry/storage.rs`
- Modify: `crates/memory-mcp/src/control/application/oidc_signup.rs`

**Interfaces:**
- Consumes: the existing `create_account_bundle` conflict recovery path.
- Produces: a `#[cfg(test)]`/`test-fixtures` one-shot conflict directive in `InMemoryStore`; production store behavior is unchanged.

- [ ] Add a one-shot test-fixture directive that makes the next `create_account_bundle` either atomically install a supplied winning account/tenant/identity and return `MemoryError::Conflict`, or return Conflict without a winner. Reuse the store's existing lock order; the seam must not exist in a production build.
- [ ] Replace the current pre-seeded `conflict_is_resolved_by_reread` test, which exits before create, with two deterministic tests: create loses then reread returns the winner; create conflicts and reread finds nothing, preserving Conflict. Assert neither loser appends a duplicate provisioning event.
- [ ] Run:

```bash
cargo test -p memory_mcp control::application::oidc_signup::tests --features control-plane,test-fixtures --locked
```

- [ ] Commit: `test(control): cover OIDC signup conflict recovery`.

---

### Task 7: Harden URL ingestion against SSRF and unbounded responses

**Files:**
- Modify: `crates/memory-mcp/src/service/content_extraction.rs`
- Test: `crates/memory-mcp/src/service/content_extraction.rs`

**Interfaces:**
- Produces: `validate_public_fetch_url`, `is_public_ip`, a bounded shared `reqwest::Client`, and an injected resolver seam for deterministic tests.
- Preserves: explicit `http`/`https` URL ingestion for public destinations; redirects are disabled.

- [ ] Add table tests rejecting credentials in URLs, localhost names, loopback, private, link-local, unspecified, multicast, documentation/test ranges, IPv4-mapped IPv6, and non-HTTP schemes. Accept representative public IPv4/IPv6 addresses.
- [ ] Add resolver tests where a hostname resolves to a mixed public/private set and where a second lookup changes from public to private; both must be rejected. The fetch must use only addresses validated by the same resolution result, not perform an unpinned second DNS resolution.
- [ ] Replace `reqwest::get` with one shared client configured with connect/request timeouts and `redirect::Policy::none()`. Resolve the host with the injected resolver, reject the request unless every candidate is globally routable, and pin those addresses into the request client's resolution override.
- [ ] Reject a declared `Content-Length` above the configured maximum and stream chunks while enforcing the same maximum when length is missing or false. Return a descriptive `MemoryError`; never buffer an unbounded body.
- [ ] Change the current localhost URL classification test: syntactic URL recognition may remain true, but fetching localhost must be rejected before a socket is opened. Add a bounded local-server test only through an explicitly test-only resolver/client seam.
- [ ] Run:

```bash
cargo test -p memory_mcp service::content_extraction --locked
cargo test -p memory_mcp --locked
```

- [ ] Commit: `fix(ingest): bound and validate URL retrieval`.

---

### Task 8: Remove unproved process-restart claims

**Files:**
- Modify: `crates/memory-mcp/tests/http_durable_tasks.rs`
- Modify: `crates/memory-mcp/tests/http_subscription_replica.rs`
- Modify: `docs/superpowers/specs/2026-08-27-streamable-http-saas.md`
- Modify: affected active evaluation/operations docs found by semantic search

**Interfaces:**
- Preserves the existing cross-handle/shared-store tests.
- Produces truthful names that distinguish a second live handle/replica from a process restart.

- [ ] Rename `restart_picks_up_from_committed_cursor` to `second_replica_picks_up_from_committed_cursor` and update its comments. Rewrite the durable-task file header to say cross-handle visibility, not restart persistence.
- [ ] Use Octocode semantic search for active claims containing restart, crash recovery, durable task, and committed cursor. Qualify every claim whose only evidence is two handles over one in-memory engine.
- [ ] Document process-restart durability as unverified for hobby scope until a test actually closes one process/database handle and reopens persistent RocksDB or a remote SurrealDB deployment. Do not delete the useful cross-replica tests.
- [ ] Run both integration test binaries with their required feature set and confirm their renamed test inventory:

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_durable_tasks -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_subscription_replica -- --test-threads=1
```
- [ ] Commit: `test(http): describe cross-replica durability truthfully`.

---

### Task 9: Replace release certification with hobby-scale validation

**Files:**
- Delete: `scripts/http_release_evidence.sh`
- Modify: `crates/memory-mcp/tests/http_load_concurrency.rs:311-348`
- Modify: `.github/workflows/ci.yml:1-110`
- Rewrite: `docs/operations/HTTP_RELEASE_GATE.md`
- Modify: `docs/operations/HTTP_INTEROP_MATRIX.md`
- Modify: `docs/operations/RESTORE_DRILL.md:58-74`
- Modify: `docs/superpowers/specs/2026-08-27-streamable-http-saas.md:835-848`
- Modify: `docs/superpowers/plans/2026-09-02-architecture-audit-remediation.md:1-8`

**Interfaces:**
- Consumes: Cargo checks, GitHub Actions logs, and `load_20_active_tenants_under_expected_qps`.
- Produces: a documented three-command local validation contract; no custom evidence artifact format.

- [ ] **Step 1: Remove the irrelevant 500-Tenant test**

Delete `load_500_tenants_under_contingency_qps` and its `MEMORY_MCP_RUN_500_LOAD` / `MEMORY_MCP_HTTP_500_TENANT` environment handling. Keep `load_20_active_tenants_under_expected_qps` unchanged.

Run:

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_20_active_tenants_under_expected_qps -- --test-threads=1
```

Expected: the retained test passes with 20 successful isolated tenants and no ignored 500-Tenant test in the binary.

- [ ] **Step 2: Delete the evidence orchestrator**

Delete `scripts/http_release_evidence.sh`. Do not replace it with another wrapper: Cargo and CI already provide command output, exit codes, and commit association.

- [ ] **Step 3: Align CI with the actual toolchain and scope**

In `.github/workflows/ci.yml`, change the `msrv` job toolchain from:

```yaml
toolchain: 1.88
```

to:

```yaml
toolchain: 1.97.1
```

Rename the job from `Rust MSRV` to `Pinned Rust`. Update the 20-Tenant step comment to:

```yaml
# Cheap in-memory isolation/concurrency regression. This is not a production
# capacity claim and there is no 50- or 500-Tenant release gate.
```

Do not add a `push` trigger merely to make old documentation true; the current `pull_request`, `release`, `workflow_dispatch`, and scheduled triggers remain unchanged.

- [ ] **Step 4: Rewrite the operational contract**

Replace `docs/operations/HTTP_RELEASE_GATE.md` with:

````markdown
# HTTP Hobby Validation

`memory_mcp_http` is currently maintained as a single-user hobby project. It
does not claim production SaaS certification, a concurrent-Tenant SLA, or a
tested disaster-recovery objective.

Before merging a meaningful HTTP change, run:

```bash
cargo fmt --all --check
cargo test -p memory_mcp --locked
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane --locked -- -D warnings
```

Changes to HTTP concurrency or tenant isolation must additionally run:

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_20_active_tenants_under_expected_qps -- --test-threads=1
```

The commit SHA and local or GitHub Actions logs are sufficient evidence. The
project does not generate or commit a separate gate matrix.

## When to raise the bar

Add deployment-specific proxy, SDK interoperability, restore, credential
rotation, and capacity checks only before exposing a shared remote deployment
to other users. Define the expected workload first; do not infer a 50- or
500-concurrent-Tenant target from the current test fixtures.
````

- [ ] **Step 5: Remove stale release-blocking claims from adjacent docs**

In `HTTP_INTEROP_MATRIX.md`, state that rows are optional compatibility notes and remove references to `gates.tsv` and the interop environment variable. In `RESTORE_DRILL.md`, keep the manual restore procedure but remove the “Verification with the release gate” command block; state that it applies only when a remote SurrealDB deployment is actually used.

In `docs/superpowers/specs/2026-08-27-streamable-http-saas.md`, preface section 20.5 with:

```markdown
The following controls are deferred until the project is intentionally operated
as a shared public SaaS. They are not release gates for the current single-user
hobby deployment; the active validation contract is
`docs/operations/HTTP_RELEASE_GATE.md`.
```

At the top of the superseded remediation plan, add:

```markdown
> **Scope update (2026-09-04):** Remaining capability-split and HTTP
> release-evidence work is superseded by
> `docs/superpowers/plans/2026-09-04-hobby-stabilization.md`.
```

- [ ] **Step 6: Check that no active documentation invokes the deleted script**

Use Octocode semantic search for `http_release_evidence`, `gates.tsv`, `MEMORY_MCP_HTTP_500_TENANT`, and `MEMORY_MCP_RUN_500_LOAD`.

Expected: occurrences may remain only inside explicitly historical plan text. No active CI, script, test, README, or operations document instructs the user to run them.

- [ ] **Step 7: Commit the scope reduction**

```bash
git add .github/workflows/ci.yml crates/memory-mcp/tests/http_load_concurrency.rs docs/operations/HTTP_RELEASE_GATE.md docs/operations/HTTP_INTEROP_MATRIX.md docs/operations/RESTORE_DRILL.md docs/superpowers/specs/2026-08-27-streamable-http-saas.md docs/superpowers/plans/2026-09-02-architecture-audit-remediation.md docs/superpowers/plans/2026-09-04-hobby-stabilization.md
git add -u scripts/http_release_evidence.sh
git commit -m "chore(http): adopt hobby-scale validation"
```

---

### Task 10: Run the final proportionate quality gate

**Files:**
- Verify only; no planned source changes.

**Interfaces:**
- Consumes: all deliverables from Tasks 1-9 and the completed companion evaluation-integrity plan.
- Produces: a clean working tree with passing required checks and no separate release-evidence artifact.

- [ ] **Step 1: Run formatting**

```bash
cargo fmt --all --check
```

Expected: exit 0 with no diff.

- [ ] **Step 2: Run the production crate tests**

```bash
cargo test -p memory_mcp --locked
```

Expected: all default-feature tests pass.

- [ ] **Step 3: Run the affected HTTP suites**

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_crash_recovery -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_20_active_tenants_under_expected_qps -- --test-threads=1
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_control_plane -- --test-threads=1
```

Expected: all pass; deletion recovery is no longer flaky and the retained load test has zero request errors.

- [ ] **Step 4: Run the repository-required zero-warning clippy command**

```bash
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane --locked -- -D warnings
```

Expected: exit 0 with zero warnings.

- [ ] **Step 5: Review the final diff and working tree**

```bash
git diff --check
git status --short
```

Expected: no whitespace errors; only intentional uncommitted files appear. Do not create `target/http-release-evidence` and do not commit generated logs.

- [ ] **Step 6: Run the benchmark completion gate**

Execute every applicable command in `docs/superpowers/plans/2026-09-04-evaluation-integrity.md`. At minimum the current machine must run PR, release, nightly, response-size, NER-quality, and Criterion CPU profiles. A data- or hardware-specific profile may remain incomplete only with the exact missing prerequisite recorded; it must not be represented as passing.

## Deferred Work and Reintroduction Triggers

- Cost-aware routing, uncertainty modeling, outcome feedback, consolidation, and multimodal memory remain research backlog items. Promote one only after a benchmark or observed user failure defines a measurable target.
- Reintroduce deployment evidence only when there is a real shared deployment, named supported clients, a selected remote database, and an explicit recovery/capacity objective.

## Self-Review

### Spec coverage

- Lease heartbeat ambiguity: Task 1.
- Production constructor panic regression: Task 2.
- One-time secret exposure through `Debug`: Task 3.
- Per-request pepper clone: Task 3.
- Duplicate fallback decision and unused parameter: Task 4.
- Nominal Registry capability split: Task 5.
- OIDC conflict branch not actually tested: Task 6.
- URL-fetch SSRF, DNS rebinding, redirect, timeout, and body bounds: Task 7.
- Unproved restart claims: Task 8.
- Excessive release evidence and irrelevant 500-Tenant gate: Task 9.
- Complete benchmark/profile integrity: companion evaluation plan and Task 10.
- Proportionate repository verification: Task 10.

The broader 2026-09-02 remediation spec contains public-SaaS and speculative architecture requirements that the user explicitly removed from current scope. Task 9 documents that supersession instead of pretending those requirements were implemented. Benchmark correctness is explicitly excluded from that scope reduction.

### Placeholder scan

The plan contains no deferred implementation placeholders. Every grounded correctness/security finding has an owned task. Academic and Criterion benchmark execution is mandatory in the companion plan; only new memory algorithms remain research backlog.

### Type consistency

- `deletion_is_purged` accepts the same `dyn RegistryStore` exposed by `RegistryHandle::store_clone`.
- `MemoryServiceDependencies::with_db_client` returns the same `MemoryError` already returned by `MemoryService::new`.
- `SelectedContext::prefer_episode_content` is produced in selection and consumed once in finalization.
- The OIDC conflict seam is test-only and does not alter the production `RegistryStore` contract.
- URL fetch validation binds authorization to the resolved addresses actually used by the client.
- No public MCP, HTTP response, database, or migration type changes.

### Disposition of reviewed findings

- Broad context-pipeline characterization was rechecked and is already covered by existing fallback, lexical, semantic, community, origin-priority, and experience tests; Task 4 adds only the missing decision-flow regression.
- The 500-Tenant failure is removed as an unsupported SLA, while the 20-Tenant isolation regression remains.
- Release evidence is simplified because deployment certification is disproportionate for one user; evaluation evidence is retained because benchmark truthfulness is product-critical.
- No finding is silently deferred: lease ambiguity, constructor panic, secret handling, pepper ownership, fallback duplication, nominal capability traits, OIDC race coverage, SSRF, restart wording, CI/toolchain drift, and all benchmark defects have an implementation owner.
