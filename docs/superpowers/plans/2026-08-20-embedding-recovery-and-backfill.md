# Embedding Recovery and Deferred Backfill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a long-lived `memory_mcp serve` process recover a configured remote embedding provider after an air-gap startup, then embed facts created during degraded mode without rebuilding the vector index.

**Architecture:** Keep startup preflight fail-fast and preserve the current degraded lexical/graph-only behavior. Add one `EmbeddingRuntimeState` behind `Arc<std::sync::RwLock<_>>`; requests clone the provider and identity from this state, while a cancellable recovery worker probes the configured remote target, atomically swaps a compatible provider, invalidates the context cache, and runs a narrow `embedding IS NONE` backfill. Persist `embedding_state.status = "backfill_pending"` before a compatible swap and restore `ready` only after backfill, so crashes resume without a migration; signature-mismatch recovery backfills missing vectors while retaining the old persisted signature until operator-driven `reembed` handles stale vectors.

**Tech Stack:** Rust, Tokio, `async-trait`, `reqwest`, SurrealDB, `serde_json`, `tokio_util::sync::CancellationToken`, existing `MemoryError` and structured logger; no new dependencies.

**Spec:** `docs/adr/0042-embedding-recovery-and-backfill.md`

## Global Constraints

- The package default feature set remains unchanged; no dependency is added.
- Startup must continue to degrade after one bounded remote preflight failure rather than invoking the runtime retry loop.
- The recovery worker is spawned only for the exact preflight-failure decision, `ResumePendingBackfill`, or `RecoverMissingEmbeddings`, plus a remote provider, normal mode, and enabled automatic recovery.
- `EMBEDDINGS_RECOVERY_INTERVAL_SECS` defaults to `60` seconds and must be strictly positive.
- `EMBEDDINGS_AUTO_RECOVERY` is an opt-out; an unset value means enabled, and `false` disables the worker.
- Recovery probe backoff is `15s`, then doubles, capped at `300s`; all probe errors, including HTTP `404`, remain retryable at the worker boundary.
- After three consecutive probe failures, the repetitive failure event uses `Debug` instead of `Warn`.
- The index dimension is `embedding_state.dimension` when present, otherwise `EmbeddingConfig::fallback_dimension()`.
- A provider is enabled only when its probed dimension matches the index dimension.
- Same-signature recovery backfills only records matching the narrow `embedding IS NONE` predicate, in batches of `100`, ordered by `fact_id` cursor.
- Backfill never drops or recreates `fact_embedding_hnsw` and never rewrites an existing vector with a stale signature.
- The `std::sync::RwLock` guard must be dropped before any `.await`; production code must not use `unwrap()`.
- Existing reembed behavior, including HNSW index replacement and broad stale-signature selection, must remain unchanged; automatic recovery never makes stale vectors authoritative.
- Preserve the existing uncommitted embedding vocabulary section in `CONTEXT.md`; commit it together with the ADR and implementation.
- Before shipping, run `cargo test -p memory_mcp`, strict clippy with `cli-watch,mcp-apps`, and `cargo fmt --all --check`.

---

## File structure and responsibilities

### New files

- `crates/memory-mcp/src/service/embedding_recovery.rs` — recovery backend seam, probe/backoff state machine, cancellable worker runtime, narrow backfill orchestration, and focused unit/integration tests.
- `crates/memory-mcp/src/storage/embedding_backfill_store.rs` — namespace-bound count/select/update operations whose only selection predicate is `embedding IS NONE`.
- `docs/adr/0042-embedding-recovery-and-backfill.md` — accepted architectural decision and boundaries.
- `docs/superpowers/plans/2026-08-20-embedding-recovery-and-backfill.md` — this implementation plan.

### Modified files

- `crates/memory-mcp/src/config/constants.rs` — recovery defaults.
- `crates/memory-mcp/src/config/embedding.rs` — parse and expose the two recovery settings.
- `crates/memory-mcp/src/service/embedding_runtime.rs` — shared runtime-state type and state-independent recovery helpers.
- `crates/memory-mcp/src/service/embedding.rs` — expose one network probe façade while keeping the HTTP client construction private.
- `crates/memory-mcp/src/service/startup.rs` — expose the existing persisted embedding-state read helper to the recovery worker.
- `crates/memory-mcp/src/service.rs` — register/re-export the recovery module where needed.
- `crates/memory-mcp/src/service/core/builder.rs` — replace four plain fields with the runtime state, initialize identity, and apply the exact worker spawn gate.
- `crates/memory-mcp/src/service/core.rs` — snapshot/swap helpers, context wiring, and recovery shutdown.
- `crates/memory-mcp/src/service/reembed.rs` — read one runtime snapshot instead of removed plain fields; preserve all reembed semantics.
- `crates/memory-mcp/src/storage.rs` — register the new backfill store.
- `crates/memory-mcp/README.md` — explain startup degradation, recovery, backfill, provider-switch behavior, and logs.
- `.env.example` — document the two recovery variables and their defaults.
- `CONTEXT.md` — retain the already-present vocabulary change and record the implemented lifecycle if wording needs a factual update.

Existing shutdown call sites in `crates/memory-mcp/src/cli/runtime.rs` already call `MemoryService::shutdown_lifecycle_background_workers`; keep those call sites and make that method also shut down the embedding recovery runtime.

---

### Task 1: Add and validate recovery configuration

**Files:**
- Modify: `crates/memory-mcp/src/config/constants.rs`
- Modify: `crates/memory-mcp/src/config/embedding.rs`
- Test: `crates/memory-mcp/src/config/embedding.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Consumes: existing `parse_env`, `parse_bool_env`, `EmbeddingConfig::from_env`, and the shared `env_lock()` test helper.
- Produces: `DEFAULT_EMBEDDING_RECOVERY_INTERVAL_SECS: u64`, `DEFAULT_EMBEDDING_RECOVERY_BACKOFF_SECS: u64`, `DEFAULT_EMBEDDING_RECOVERY_MAX_BACKOFF_SECS: u64`, `EmbeddingConfig::recovery_interval_secs: u64`, and `EmbeddingConfig::auto_recovery: bool`.

- [ ] **Step 1: Write the failing configuration tests**

Add tests that assert the default, explicit values, opt-out, and invalid zero interval:

```rust
#[test]
fn embedding_config_defaults_to_enabled_recovery() {
    with_env_vars(
        &[
            ("EMBEDDINGS_ENABLED", Some("true")),
            ("EMBEDDINGS_PROVIDER", Some("openai-compatible")),
            ("EMBEDDINGS_MODEL", Some("test-model")),
            ("EMBEDDINGS_RECOVERY_INTERVAL_SECS", None),
            ("EMBEDDINGS_AUTO_RECOVERY", None),
        ],
        || {
            let config = EmbeddingConfig::from_env().expect("config from env");
            assert_eq!(config.recovery_interval_secs, 60);
            assert!(config.auto_recovery);
        },
    );
}

#[test]
fn embedding_config_parses_recovery_interval_and_opt_out() {
    with_env_vars(
        &[
            ("EMBEDDINGS_ENABLED", Some("true")),
            ("EMBEDDINGS_PROVIDER", Some("openai-compatible")),
            ("EMBEDDINGS_MODEL", Some("test-model")),
            ("EMBEDDINGS_RECOVERY_INTERVAL_SECS", Some("17")),
            ("EMBEDDINGS_AUTO_RECOVERY", Some("false")),
        ],
        || {
            let config = EmbeddingConfig::from_env().expect("config from env");
            assert_eq!(config.recovery_interval_secs, 17);
            assert!(!config.auto_recovery);
        },
    );
}

#[test]
fn embedding_config_rejects_zero_recovery_interval() {
    with_env_vars(
        &[
            ("EMBEDDINGS_ENABLED", Some("true")),
            ("EMBEDDINGS_PROVIDER", Some("openai-compatible")),
            ("EMBEDDINGS_MODEL", Some("test-model")),
            ("EMBEDDINGS_RECOVERY_INTERVAL_SECS", Some("0")),
        ],
        || {
            let result = EmbeddingConfig::from_env();
            assert!(matches!(
                result,
                Err(MemoryError::ConfigInvalid(message))
                    if message.contains("EMBEDDINGS_RECOVERY_INTERVAL_SECS")
            ));
        },
    );
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test -p memory_mcp config::embedding::tests::embedding_config_defaults_to_enabled_recovery -- --exact
cargo test -p memory_mcp config::embedding::tests::embedding_config_parses_recovery_interval_and_opt_out -- --exact
cargo test -p memory_mcp config::embedding::tests::embedding_config_rejects_zero_recovery_interval -- --exact
```

Expected: compilation/test failure because the two fields and recovery defaults do not exist.

- [ ] **Step 3: Implement the minimal configuration surface**

Add constants:

```rust
pub const DEFAULT_EMBEDDING_RECOVERY_INTERVAL_SECS: u64 = 60;
pub const DEFAULT_EMBEDDING_RECOVERY_BACKOFF_SECS: u64 = 15;
pub const DEFAULT_EMBEDDING_RECOVERY_MAX_BACKOFF_SECS: u64 = 300;
```

Add fields and defaults:

```rust
pub struct EmbeddingConfig {
    pub provider: EmbeddingProviderKind,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
    pub dimension_override: Option<usize>,
    pub max_tokens: usize,
    pub similarity_threshold: f64,
    pub model_dir: Option<String>,
    pub recovery_interval_secs: u64,
    pub auto_recovery: bool,
}
```

In `Default` use `recovery_interval_secs: DEFAULT_EMBEDDING_RECOVERY_INTERVAL_SECS` and `auto_recovery: true`. At the start of `from_env`, before the disabled-provider early return, parse and validate:

```rust
let recovery_interval_secs = parse_env::<u64>("EMBEDDINGS_RECOVERY_INTERVAL_SECS")?
    .unwrap_or(DEFAULT_EMBEDDING_RECOVERY_INTERVAL_SECS);
if recovery_interval_secs == 0 {
    return Err(MemoryError::ConfigInvalid(
        "EMBEDDINGS_RECOVERY_INTERVAL_SECS must be greater than zero".to_string(),
    ));
}
let auto_recovery = parse_bool_env("EMBEDDINGS_AUTO_RECOVERY").unwrap_or(true);
```

Carry both values into the disabled and enabled `EmbeddingConfig` constructors. Update the four explicit local-provider test literals in `service/embedding.rs` with `..EmbeddingConfig::default()` or the two explicit fields so all struct literals remain complete.

- [ ] **Step 4: Run the focused tests and the existing embedding configuration tests**

Run:

```bash
cargo test -p memory_mcp config::embedding::tests -- --nocapture
```

Expected: PASS, including the pre-existing signature, dimension, and environment parsing tests.

- [ ] **Step 5: Commit the configuration slice**

```bash
git add crates/memory-mcp/src/config/constants.rs crates/memory-mcp/src/config/embedding.rs crates/memory-mcp/src/service/embedding.rs
git commit -m "feat: configure embedding recovery worker"
```

---

### Task 2: Replace mutable embedding fields with one runtime state

**Files:**
- Modify: `crates/memory-mcp/src/service/embedding_runtime.rs`
- Modify: `crates/memory-mcp/src/service/core/builder.rs`
- Modify: `crates/memory-mcp/src/service/core.rs`
- Modify: `crates/memory-mcp/src/service/reembed.rs`
- Test: `crates/memory-mcp/src/service/core.rs`, `crates/memory-mcp/src/service/reembed.rs`

**Interfaces:**
- Consumes: `EmbeddingProvider`, `MemoryService`, existing test providers, and `std::sync::RwLock`.
- Produces: `EmbeddingRuntimeState`, `MemoryService::embedding_runtime_snapshot()`, `MemoryService::replace_embedding_runtime_state()`, and a lock-safe `build_context()` swap point.

- [ ] **Step 1: Add a failing state-swap test**

Add a test beside the existing service tests that constructs a service with a disabled provider, replaces its state, and verifies the next context sees the replacement:

```rust
#[tokio::test]
async fn build_context_uses_the_latest_embedding_runtime_state() {
    let service = create_test_service("org");
    let replacement = Arc::new(StaticTestEmbeddingProvider::new());
    service.replace_embedding_runtime_state(EmbeddingRuntimeState::new(
        replacement,
        Some("embsig:test".to_string()),
        Some("test-model".to_string()),
        Some(DEFAULT_EMBEDDING_DIMENSION),
    ));

    let context = service.build_context();
    assert_eq!(context.embedding_service.embedding_provider().provider_name(), "test");
    assert_eq!(
        context.embedding_service.current_embedding_signature(),
        Some("embsig:test")
    );
    assert_eq!(context.embedding_service.current_embedding_dimension(), Some(1536));
}
```

- [ ] **Step 2: Run the state-swap test and verify it fails to compile**

Run:

```bash
cargo test -p memory_mcp service::core::tests::build_context_uses_the_latest_embedding_runtime_state -- --exact
```

Expected: failure because `EmbeddingRuntimeState` and the replacement methods do not exist.

- [ ] **Step 3: Implement the state type and lock-safe service methods**

In `embedding_runtime.rs` add:

```rust
use std::sync::Arc;

use super::EmbeddingProvider;

#[derive(Clone)]
pub(crate) struct EmbeddingRuntimeState {
    pub(crate) provider: Arc<dyn EmbeddingProvider>,
    pub(crate) signature: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) dimension: Option<usize>,
}

impl EmbeddingRuntimeState {
    pub(crate) fn new(
        provider: Arc<dyn EmbeddingProvider>,
        signature: Option<String>,
        model: Option<String>,
        dimension: Option<usize>,
    ) -> Self {
        Self { provider, signature, model, dimension }
    }
}
```

In `MemoryService`, replace `embedding_provider`, `current_embedding_signature`, `current_embedding_model`, and `current_embedding_dimension` with:

```rust
pub(crate) embedding_runtime_state:
    Arc<std::sync::RwLock<crate::service::embedding_runtime::EmbeddingRuntimeState>>,
```

Initialize it in `build()` with the provider and `None` metadata. Add methods using poison recovery without `unwrap()`:

```rust
pub(crate) fn embedding_runtime_snapshot(
    &self,
) -> crate::service::embedding_runtime::EmbeddingRuntimeState {
    self.embedding_runtime_state
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(crate) fn replace_embedding_runtime_state(
    &self,
    state: crate::service::embedding_runtime::EmbeddingRuntimeState,
) {
    *self
        .embedding_runtime_state
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
}
```

In `build_context()`, take one snapshot before constructing `EmbeddingService`:

```rust
let embedding_state = self.embedding_runtime_snapshot();
```

Pass `embedding_state.provider`, `.signature`, `.model`, and `.dimension`; do not read the lock inside the `EmbeddingService` constructor and do not hold a guard across an await.

In the environment builder, clone the provider before passing the original into `new_with_embedding_provider`, then replace the three assignments with one state call:

```rust
let runtime_provider = embedding_provider.clone();
let mut service = Self::new_with_embedding_provider(
    db_client.clone(),
    config.active_namespace().as_str().to_string(),
    config.log_level,
    50,
    100,
    embedding_provider,
    config.embedding.similarity_threshold,
    entity_extractor,
)?;
service.replace_embedding_runtime_state(EmbeddingRuntimeState::new(
    runtime_provider,
    target.as_ref().map(|value| value.signature.clone()),
    target.as_ref().and_then(|value| value.model.clone()),
    target.as_ref().map(|value| value.dimension),
));
```

Use the same state initialization in `new_with_embedding_provider`/`build()` so test services remain enabled when given an enabled provider, even though their identity metadata starts as `None`.

- [ ] **Step 4: Update reembed to snapshot state without changing its semantics**

At each reembed operation that formerly read removed fields, obtain a local snapshot and use its fields. For example, replace the start of `prepare_reembed_pass` with:

```rust
let embedding_state = self.embedding_runtime_snapshot();
if !embedding_state.provider.is_enabled() {
    return Err(MemoryError::Validation(
        "reembed requires an enabled embedding provider".to_string(),
    ));
}
let target_signature = embedding_state.signature.clone().ok_or_else(|| {
    MemoryError::Validation("reembed requires an enabled embedding signature".to_string())
})?;
let target_dimension = embedding_state.dimension.ok_or_else(|| {
    MemoryError::Validation("reembed requires a resolved target dimension".to_string())
})?;
```

Apply the same snapshot rule to job-start/progress/finalization logging, `rewrite_fact_embedding`, `write_embedding_state`, and `persist_reembed_job`. The existing `ReembedStoreClient` broad stale-signature queries and the index drop/recreate calls must remain untouched. Update test fixtures to call `replace_embedding_runtime_state()` instead of assigning removed fields.

- [ ] **Step 5: Run the focused tests and compile all service code**

Run:

```bash
cargo test -p memory_mcp service::core::tests -- --nocapture
cargo test -p memory_mcp service::reembed::tests -- --nocapture
cargo check -p memory_mcp
```

Expected: PASS; all existing background embedding and reembed tests must still observe their configured signatures and dimensions.

- [ ] **Step 6: Commit the runtime-state slice**

```bash
git add crates/memory-mcp/src/service/embedding_runtime.rs crates/memory-mcp/src/service/core.rs crates/memory-mcp/src/service/core/builder.rs crates/memory-mcp/src/service/reembed.rs
 git commit -m "refactor: centralize embedding runtime state"
```

---

### Task 3: Add the remote probe façade and narrow backfill store

**Files:**
- Modify: `crates/memory-mcp/src/service/embedding.rs`
- Modify: `crates/memory-mcp/src/service/startup.rs`
- Create: `crates/memory-mcp/src/storage/embedding_backfill_store.rs`
- Modify: `crates/memory-mcp/src/storage.rs`
- Test: `crates/memory-mcp/src/storage/embedding_backfill_store.rs`

**Interfaces:**
- Consumes: existing remote one-shot probe functions, `EmbeddingConfig`, `BoundDbClient`, and the `ReembedStoreClient` in-memory test pattern.
- Produces: `probe_remote_embedding_dimension(&EmbeddingConfig)`, `startup::load_embedding_state`, `EmbeddingBackfillStoreClient::count_facts_missing_embeddings`, `select_facts_missing_embeddings`, and `update_embedding_fields`.

- [ ] **Step 1: Write failing store tests**

Add a test fixture that applies migrations, seeds one fact without `embedding`, one fact with an existing stale signature and a 1536-vector, and checks only the missing fact is selected:

```rust
#[tokio::test]
async fn narrow_backfill_store_selects_only_facts_without_embedding() {
    let db = make_db().await;
    seed_missing_fact(&db, "fact:missing").await;
    seed_fact_with_embedding(&db, "fact:stale", "embsig:old").await;
    let store = EmbeddingBackfillStoreClient::new(db.clone(), "org");

    assert_eq!(store.count_facts_missing_embeddings().await.expect("count"), 1);
    let rows = store
        .select_facts_missing_embeddings(None, 100)
        .await
        .expect("select");
    let ids: Vec<&str> = rows
        .iter()
        .filter_map(|row| row.get("fact_id").and_then(serde_json::Value::as_str))
        .collect();
    assert_eq!(ids, vec!["fact:missing"]);
}

#[tokio::test]
async fn narrow_backfill_store_respects_fact_id_cursor() {
    let db = make_db().await;
    seed_missing_fact(&db, "fact:1").await;
    seed_missing_fact(&db, "fact:2").await;
    seed_missing_fact(&db, "fact:3").await;
    let store = EmbeddingBackfillStoreClient::new(db, "org");

    let page = store
        .select_facts_missing_embeddings(Some("fact:1"), 2)
        .await
        .expect("page");
    let ids: Vec<&str> = page
        .iter()
        .filter_map(|row| row.get("fact_id").and_then(serde_json::Value::as_str))
        .collect();
    assert_eq!(ids, vec!["fact:2", "fact:3"]);
}
```

- [ ] **Step 2: Run the store tests and verify the new type is missing**

Run:

```bash
cargo test -p memory_mcp storage::embedding_backfill_store::tests -- --nocapture
```

Expected: compilation failure because the module and store methods do not exist.

- [ ] **Step 3: Implement the narrow store with separate SQL**

Register `pub(crate) mod embedding_backfill_store;` in `storage.rs` and implement a namespace-bound client. The selection SQL must remain visibly distinct from `ReembedStoreClient`:

```rust
pub async fn count_facts_missing_embeddings(&self) -> Result<usize, MemoryError> {
    let rows = self
        .db
        .query_rows(
            "SELECT count() AS count FROM fact WHERE embedding IS NONE GROUP ALL",
            None,
        )
        .await?;
    Ok(rows
        .first()
        .and_then(|row| row.get("count"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0))
}

pub async fn select_facts_missing_embeddings(
    &self,
    last_completed_fact_id: Option<&str>,
    limit: i32,
) -> Result<Vec<Value>, MemoryError> {
    let sql = if last_completed_fact_id.is_some() {
        "SELECT * FROM (SELECT * FROM fact WHERE embedding IS NONE) \
         WHERE fact_id > $last_completed_fact_id ORDER BY fact_id ASC LIMIT $limit"
            .to_string()
    } else {
        "SELECT * FROM fact WHERE embedding IS NONE ORDER BY fact_id ASC LIMIT $limit"
            .to_string()
    };
    self.db
        .query_rows(
            &sql,
            Some(json!({
                "last_completed_fact_id": last_completed_fact_id,
                "limit": limit,
            })),
        )
        .await
}

pub async fn update_embedding_fields(
    &self,
    fact_id: &str,
    fields: Value,
) -> Result<(), MemoryError> {
    self.db.update(fact_id, fields).await.map(|_| ())
}
```

Use the existing `BoundDbClient::query_rows` missing-table behavior and the existing 1536-dimension fixture vectors. Do not add a signature condition to either query.

- [ ] **Step 4: Expose one probe façade without exposing the HTTP client builder**

In `embedding.rs`, add a crate-visible function that reuses `build_probe_http_client` and the existing one-shot remote functions:

```rust
pub(crate) async fn probe_remote_embedding_dimension(
    config: &EmbeddingConfig,
) -> Result<usize, MemoryError> {
    let client = build_probe_http_client(config.timeout_secs)?;
    match config.provider {
        EmbeddingProviderKind::OpenAiCompatible => detect_openai_embedding_dimension(
            &client,
            config
                .base_url
                .as_deref()
                .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string()))?,
            config
                .model
                .as_deref()
                .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string()))?,
            config.api_key.as_deref(),
        )
        .await,
        EmbeddingProviderKind::Ollama => detect_ollama_embedding_dimension(
            &client,
            config
                .base_url
                .as_deref()
                .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string()))?,
            config
                .model
                .as_deref()
                .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string()))?,
        )
        .await,
        _ => Err(MemoryError::Validation(
            "embedding recovery probe requires a remote provider".to_string(),
        )),
    }
}
```

Make `startup::load_embedding_state` `pub(crate)` while leaving the record ID and startup decision behavior unchanged.

- [ ] **Step 5: Run storage, startup, and embedding tests**

Run:

```bash
cargo test -p memory_mcp storage::embedding_backfill_store::tests -- --nocapture
cargo test -p memory_mcp service::startup::tests -- --nocapture
cargo test -p memory_mcp service::embedding::tests -- --nocapture
```

Expected: PASS, including the existing broad reembed-store tests and the existing fast-preflight regression.

- [ ] **Step 6: Commit the probe/store slice**

```bash
git add crates/memory-mcp/src/service/embedding.rs crates/memory-mcp/src/service/startup.rs crates/memory-mcp/src/storage.rs crates/memory-mcp/src/storage/embedding_backfill_store.rs
git commit -m "feat: add remote recovery probe and narrow backfill store"
```

---

### Task 4: Implement injectable recovery decisions and backoff

**Files:**
- Create: `crates/memory-mcp/src/service/embedding_recovery.rs`
- Modify: `crates/memory-mcp/src/service.rs`
- Test: `crates/memory-mcp/src/service/embedding_recovery.rs`

**Interfaces:**
- Consumes: `EmbeddingConfig`, `ResolvedEmbeddingTarget`, `EmbeddingProvider`, `probe_remote_embedding_dimension`, and `create_embedding_provider_with_dimension`.
- Produces: `EmbeddingRecoveryBackend`, `ConfiguredEmbeddingRecoveryBackend`, `RecoveryDecision`, `choose_recovery_decision`, `recovery_backoff`, and `RecoveryWorkerSettings`.

- [ ] **Step 1: Write failing decision and backoff tests**

Add pure tests for all three Q9 branches and the exact backoff sequence:

```rust
#[test]
fn recovery_decision_requires_same_dimension_and_same_or_absent_signature() {
    let config = remote_config("openai-compatible", 1536);
    let full = choose_recovery_decision(&config, 1536, None, 1536);
    assert!(matches!(full, RecoveryDecision::FullRecovery(target) if target.dimension == 1536));

    let enabled = choose_recovery_decision(&config, 1536, Some("embsig:old"), 1536);
    assert!(matches!(enabled, RecoveryDecision::EnableForNewFacts(target) if target.dimension == 1536));

    let incompatible = choose_recovery_decision(&config, 1536, Some("embsig:new"), 768);
    assert!(matches!(incompatible, RecoveryDecision::DimensionMismatch { index_dimension: 1536, probed_dimension: 768 }));
}

#[test]
fn recovery_backoff_is_15_seconds_then_doubles_and_caps() {
    assert_eq!(recovery_backoff(1), Duration::from_secs(15));
    assert_eq!(recovery_backoff(2), Duration::from_secs(30));
    assert_eq!(recovery_backoff(3), Duration::from_secs(60));
    assert_eq!(recovery_backoff(6), Duration::from_secs(300));
    assert_eq!(recovery_backoff(20), Duration::from_secs(300));
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests::recovery_decision_requires_same_dimension_and_same_or_absent_signature -- --exact
cargo test -p memory_mcp service::embedding_recovery::tests::recovery_backoff_is_15_seconds_then_doubles_and_caps -- --exact
```

Expected: compilation failure because the recovery module and decision functions do not exist.

- [ ] **Step 3: Define the injectable backend and decision seam**

Register `mod embedding_recovery;` in `service.rs`. In the new module define an object-safe async backend:

```rust
#[async_trait]
pub(crate) trait EmbeddingRecoveryBackend: Send + Sync {
    async fn probe_dimension(&self) -> Result<usize, MemoryError>;
    async fn create_provider(
        &self,
        dimension: usize,
    ) -> Result<Arc<dyn EmbeddingProvider>, MemoryError>;
}

pub(crate) struct ConfiguredEmbeddingRecoveryBackend {
    config: EmbeddingConfig,
    data_dir: String,
}

impl ConfiguredEmbeddingRecoveryBackend {
    pub(crate) fn new(config: EmbeddingConfig, data_dir: String) -> Self {
        Self { config, data_dir }
    }
}
```

Implement the concrete backend by delegating `probe_dimension()` to `probe_remote_embedding_dimension(&self.config)` and `create_provider()` to `create_embedding_provider_with_dimension(&self.config, &self.data_dir, dimension)`. Tests use a fake backend with an atomic probe-failure counter and a fake provider; production code never needs a network call to construct the provider after the probe.

Build a target signature from `provider_label`, model, base URL, and probed dimension. Implement:

```rust
pub(crate) enum RecoveryDecision {
    FullRecovery(ResolvedEmbeddingTarget),
    EnableForNewFacts(ResolvedEmbeddingTarget),
    DimensionMismatch { index_dimension: usize, probed_dimension: usize },
}

pub(crate) fn choose_recovery_decision(
    config: &EmbeddingConfig,
    index_dimension: usize,
    stored_signature: Option<&str>,
    probed_dimension: usize,
) -> RecoveryDecision {
    let target = ResolvedEmbeddingTarget {
        provider_label: config.provider_label(),
        model: config.model.clone(),
        dimension: probed_dimension,
        signature: build_embedding_signature(
            config.provider_label(),
            config.model.as_deref(),
            config.base_url.as_deref(),
            probed_dimension,
        ),
    };
    if probed_dimension != index_dimension {
        RecoveryDecision::DimensionMismatch { index_dimension, probed_dimension }
    } else if stored_signature.is_some_and(|signature| signature != target.signature) {
        RecoveryDecision::EnableForNewFacts(target)
    } else {
        RecoveryDecision::FullRecovery(target)
    }
}
```

Implement `RecoveryWorkerSettings` with production values and test-overridable durations:

```rust
#[derive(Clone, Copy)]
pub(crate) struct RecoveryWorkerSettings {
    pub(crate) initial_probe_delay: Duration,
    pub(crate) backoff_base: Duration,
    pub(crate) backoff_cap: Duration,
    pub(crate) warn_demote_after: u32,
    pub(crate) batch_size: i32,
}

impl RecoveryWorkerSettings {
    pub(crate) fn production(interval_secs: u64) -> Self {
        Self {
            initial_probe_delay: Duration::from_secs(interval_secs),
            backoff_base: Duration::from_secs(DEFAULT_EMBEDDING_RECOVERY_BACKOFF_SECS),
            backoff_cap: Duration::from_secs(DEFAULT_EMBEDDING_RECOVERY_MAX_BACKOFF_SECS),
            warn_demote_after: 3,
            batch_size: 100,
        }
    }
}
```

Use `backoff_base = 15s`, `backoff_cap = 300s`, `warn_demote_after = 3`, and `batch_size = 100` in the production constructor. Implement `recovery_backoff(failures)` with saturating arithmetic and the cap.

- [ ] **Step 4: Run the decision/backoff tests**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests -- --nocapture
```

Expected: PASS for the pure decision and backoff tests; the worker tests remain absent until Tasks 5–6.

- [ ] **Step 5: Commit the recovery seam**

```bash
git add crates/memory-mcp/src/service.rs crates/memory-mcp/src/service/embedding_recovery.rs
git commit -m "feat: define injectable embedding recovery decisions"
```

---

### Task 5: Implement safe deferred backfill

**Files:**
- Modify: `crates/memory-mcp/src/service/embedding_recovery.rs`
- Modify: `crates/memory-mcp/src/service/embedding_runtime.rs` if the metadata-field helper belongs there
- Test: `crates/memory-mcp/src/service/embedding_recovery.rs`

**Interfaces:**
- Consumes: `MemoryService::reembed_store`-style namespace access, `EmbeddingBackfillStoreClient`, `EmbeddingProvider`, `FactService::build_fact_embedding_input`, `EmbeddingRuntimeState`, and `invalidate_cache`.
- Produces: `run_backfill`, `BackfillOutcome`, and `embedding_fields_for_backfill`.

- [ ] **Step 1: Write failing backfill tests**

Use the existing in-memory SurrealDB fixture pattern and a fake 1536-dimensional provider. Add tests that prove missing facts are filled, stale existing vectors are unchanged, and a transient provider failure returns retry without dropping the index:

```rust
#[tokio::test]
async fn backfill_embeds_missing_facts_but_does_not_touch_existing_vectors() {
    let db = make_in_memory_db("backfill_missing").await;
    seed_missing_fact(&db, "fact:missing", "offline fact").await;
    seed_fact_with_embedding(&db, "fact:stale", "embsig:old").await;
    let service = make_disabled_service(db.clone(), "org");
    let provider = Arc::new(FakeEmbeddingProvider::new(1536));

    let outcome = run_backfill(
        &service,
        provider,
        "embsig:target",
        Some("test-model"),
        1536,
        100,
    )
    .await
    .expect("backfill");
    assert!(matches!(outcome, BackfillOutcome::Complete { processed: 1 }));

    let missing = db.select_one("fact:missing", "org").await.expect("read").expect("fact");
    assert_eq!(missing.get("embedding_dimension").and_then(Value::as_u64), Some(1536));
    assert_eq!(missing.get("embedding_signature").and_then(Value::as_str), Some("embsig:target"));
    assert_eq!(missing.get("embedding_provider").and_then(Value::as_str), Some("openai-compatible"));

    let stale = db.select_one("fact:stale", "org").await.expect("read").expect("fact");
    assert_eq!(stale.get("embedding_signature").and_then(Value::as_str), Some("embsig:old"));
}

#[tokio::test]
async fn backfill_returns_retry_when_provider_is_temporarily_unavailable() {
    let db = make_in_memory_db("backfill_retry").await;
    seed_missing_fact(&db, "fact:missing", "offline fact").await;
    let service = make_disabled_service(db, "org");
    let provider = Arc::new(FailingEmbeddingProvider::transient_once(1536));

    let result = run_backfill(
        &service,
        provider,
        "embsig:target",
        None,
        1536,
        100,
    )
    .await;
    assert!(matches!(result, Err(MemoryError::Transient(_))));
}
```

- [ ] **Step 2: Run the backfill tests and verify they fail**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests::backfill_embeds_missing_facts_but_does_not_touch_existing_vectors -- --exact
cargo test -p memory_mcp service::embedding_recovery::tests::backfill_returns_retry_when_provider_is_temporarily_unavailable -- --exact
```

Expected: compilation failure because `run_backfill` and the fake-test fixture seam do not exist.

- [ ] **Step 3: Implement one batch-cursor backfill loop**

The implementation must select through `EmbeddingBackfillStoreClient`, not `ReembedStoreClient`. For every selected record, build the same input used by normal fact creation and update only embedding fields:

```rust
pub(crate) async fn run_backfill(
    service: &MemoryService,
    provider: Arc<dyn EmbeddingProvider>,
    signature: &str,
    model: Option<&str>,
    dimension: usize,
    batch_size: i32,
) -> Result<BackfillOutcome, MemoryError> {
    let store = EmbeddingBackfillStoreClient::new(
        service.db_client.clone(),
        service.active_namespace.clone(),
    );
    let mut cursor: Option<String> = None;
    let mut processed = 0usize;

    loop {
        let batch = store
            .select_facts_missing_embeddings(cursor.as_deref(), batch_size)
            .await?;
        if batch.is_empty() {
            return Ok(BackfillOutcome::Complete { processed });
        }

        for fact in batch {
            let fact_id = required_string(&fact, "fact_id")?;
            let fact_type = required_string(&fact, "fact_type")?;
            let content = required_string(&fact, "content")?;
            let quote = required_string(&fact, "quote")?;
            let input = FactService::build_fact_embedding_input(&fact_type, &content, &quote);
            let embedding = provider.embed(&input).await?;
            let fields = embedding_fields_for_backfill(
                &provider,
                embedding,
                signature,
                model,
                dimension,
            )?;
            store.update_embedding_fields(&fact_id, fields).await?;
            crate::service::invalidate_cache(&service.context_cache).await;
            cursor = Some(fact_id);
            processed += 1;
        }
    }
}
```

`embedding_fields_for_backfill` must validate `embedding.len() == dimension` and produce `embedding`, `embedding_provider`, optional `embedding_model`, `embedding_dimension`, `embedding_signature`, and normalized `embedding_updated_at`. A missing required fact field returns `MemoryError::Validation`; a provider/storage error propagates to the worker so it can re-probe. Do not call `remove_embedding_index`, `define_embedding_index`, broad stale-signature queries, or `reembed_all_facts`.

- [ ] **Step 4: Run the backfill tests and the existing reembed tests**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests::backfill -- --nocapture
cargo test -p memory_mcp service::reembed::tests -- --nocapture
```

Expected: PASS; the stale fact remains unchanged and the HNSW index continues to accept 1536-dimensional vectors.

- [ ] **Step 5: Commit the backfill slice**

```bash
git add crates/memory-mcp/src/service/embedding_recovery.rs crates/memory-mcp/src/service/embedding_runtime.rs
git commit -m "feat: backfill facts missing embeddings"
```

---

### Task 6: Implement the cyclic recovery worker and cancellation runtime

**Files:**
- Modify: `crates/memory-mcp/src/service/embedding_recovery.rs`
- Test: `crates/memory-mcp/src/service/embedding_recovery.rs`

**Interfaces:**
- Consumes: `EmbeddingRecoveryBackend`, `RecoveryDecision`, `RecoveryWorkerSettings`, `run_backfill`, `startup::load_embedding_state`, `write_bootstrap_ready_state`, `EmbeddingRuntimeState`, and `CancellationToken`.
- Produces: `run_recovery_worker(service, config, backend, settings, shutdown)`, `EmbeddingRecoveryRuntime`, and `EmbeddingRecoveryRuntime::spawn/shutdown`.

- [ ] **Step 1: Write failing worker tests**

Add a fake backend and tests for fail→succeed, signature mismatch, dimension mismatch, and backfill interruption followed by a second probe. The settings use millisecond delays so tests never wait for production seconds:

```rust
fn test_settings() -> RecoveryWorkerSettings {
    RecoveryWorkerSettings {
        initial_probe_delay: Duration::ZERO,
        backoff_base: Duration::from_millis(1),
        backoff_cap: Duration::from_millis(4),
        warn_demote_after: 3,
        batch_size: 100,
    }
}

#[tokio::test]
async fn worker_retries_probe_then_recovers_and_exits_when_backfill_is_empty() {
    let db = make_in_memory_db("worker_retry").await;
    let service = make_disabled_service(db, "org");
    let backend = Arc::new(FakeRecoveryBackend::fail_probes_then_succeed(2, 1536));
    let shutdown = CancellationToken::new();

    run_recovery_worker(
        service.clone(),
        remote_config("openai-compatible", 1536),
        backend.clone(),
        test_settings(),
        shutdown,
    )
    .await;

    assert_eq!(backend.probe_calls(), 3);
    assert!(service.embedding_runtime_snapshot().provider.is_enabled());
}

#[test]
fn signature_mismatch_enables_new_fact_provider_and_backfills_only_missing_facts() {
    let config = remote_config("openai-compatible", 1536);
    let action = choose_recovery_decision(&config, 1536, Some("embsig:old"), 1536);
    assert!(matches!(action, RecoveryDecision::EnableForNewFacts(_)));
}

#[test]
fn dimension_mismatch_keeps_provider_disabled() {
    let config = remote_config("openai-compatible", 1536);
    let action = choose_recovery_decision(&config, 1536, None, 768);
    assert!(matches!(action, RecoveryDecision::DimensionMismatch { .. }));
}

#[tokio::test]
async fn worker_returns_to_probe_after_backfill_network_failure() {
    let db = make_in_memory_db("worker_backfill_cycle").await;
    seed_missing_fact(&db, "fact:offline", "offline fact").await;
    let service = make_disabled_service(db.clone(), "org");
    let backend = Arc::new(FakeRecoveryBackend::probe_succeeds_with_flaky_provider(1536, 1));
    let config = remote_config("openai-compatible", 1536);
    let expected_signature = build_embedding_signature(
        config.provider_label(),
        config.model.as_deref(),
        config.base_url.as_deref(),
        1536,
    );

    run_recovery_worker(
        service.clone(),
        config,
        backend.clone(),
        test_settings(),
        CancellationToken::new(),
    )
    .await;

    assert!(backend.probe_calls() >= 2);
    let fact = db.select_one("fact:offline", "org").await.expect("read").expect("fact");
    assert_eq!(
        fact.get("embedding_signature").and_then(Value::as_str),
        Some(expected_signature.as_str())
    );
}
```

- [ ] **Step 2: Run the worker tests and verify they fail**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests::worker_retries_probe_then_recovers_and_exits_when_backfill_is_empty -- --exact
cargo test -p memory_mcp service::embedding_recovery::tests::worker_returns_to_probe_after_backfill_network_failure -- --exact
```

Expected: compilation failure because the worker loop and runtime do not exist.

- [ ] **Step 3: Implement the one-cycle recovery algorithm**

Load persisted state through the Active Namespace, derive:

```rust
let index_dimension = state
    .as_ref()
    .and_then(|record| record.get("dimension"))
    .and_then(Value::as_u64)
    .and_then(|value| usize::try_from(value).ok())
    .unwrap_or_else(|| config.fallback_dimension());
let stored_signature = state
    .as_ref()
    .and_then(|record| record.get("active_signature"))
    .and_then(Value::as_str);
```

On a successful probe, choose the Q9 decision. For `FullRecovery`, persist `embedding_state:fact.status = "backfill_pending"` before the provider swap, replace the runtime state, invalidate the context cache, log `embedding.recovered`, and run `run_backfill`; persist `status = "ready"` only after `run_backfill` returns `Complete`. For `EnableForNewFacts`, create/swap/invalidate, log `embedding.reembed_required`, and backfill only missing vectors; do not rewrite existing vectors and do not replace the old persisted signature, so a restart remains safely degraded until `reembed`. `RecoverMissingEmbeddings` is the startup-side resume decision for this signature-mismatch path. For `DimensionMismatch`, leave semantic mode disabled, log `embedding.reembed_required`, and return a `MemoryError::Validation` from the cycle so the outer loop schedules another probe without enabling an incompatible provider.

Define the cycle result as `RecoveryCycleOutcome::Completed` and let every probe, provider, persistence, and backfill failure return `Err(MemoryError)`. The outer loop handles dimension-mismatch validation errors and transport/storage errors identically for backoff purposes.

The worker receives `EmbeddingConfig` explicitly; the backend is responsible for probing/building the provider, while the worker needs the config to compute the target signature and fallback index dimension.

Use a single worker loop with cancellation-aware waits:

```rust
pub(crate) async fn run_recovery_worker(
    service: MemoryService,
    config: EmbeddingConfig,
    backend: Arc<dyn EmbeddingRecoveryBackend>,
    settings: RecoveryWorkerSettings,
    shutdown: CancellationToken,
) {
    let mut consecutive_failures = 0u32;
    let mut delay = settings.initial_probe_delay;

    loop {
        let waited = tokio::select! {
            _ = shutdown.cancelled() => false,
            _ = tokio::time::sleep(delay) => true,
        };
        if !waited {
            return;
        }

        match run_recovery_cycle(&service, &config, backend.as_ref(), &settings).await {
            Ok(RecoveryCycleOutcome::Completed) => return,
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let level = if consecutive_failures >= settings.warn_demote_after {
                    LogLevel::Debug
                } else {
                    LogLevel::Warn
                };
                log_recovery_probe_failure(&service.logger, consecutive_failures, &error, level);
                delay = recovery_backoff_with_settings(
                    consecutive_failures,
                    settings.backoff_base,
                    settings.backoff_cap,
                );
            }
        }
    }
}
```

Reset `consecutive_failures` after a fully successful cycle only; a backfill failure returns through the same error path and therefore re-enters probe with backoff. Treat all probe and provider errors as worker-retry conditions so a `404` cannot silently kill the recovery task. Cancellation must win during every wait and shutdown join.

- [ ] **Step 4: Add the tracked runtime and shutdown join**

Implement the existing worker shape:

```rust
#[derive(Clone)]
pub(crate) struct EmbeddingRecoveryRuntime {
    shutdown: CancellationToken,
    handles: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl EmbeddingRecoveryRuntime {
    pub(crate) fn new() -> Self {
        Self {
            shutdown: CancellationToken::new(),
            handles: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub(crate) async fn spawn(
        &self,
        service: MemoryService,
        config: EmbeddingConfig,
        backend: Arc<dyn EmbeddingRecoveryBackend>,
        settings: RecoveryWorkerSettings,
    ) {
        let shutdown = self.shutdown.clone();
        let handle = tokio::spawn(async move {
            run_recovery_worker(service, config, backend, settings, shutdown).await;
        });
        self.handles.lock().await.push(handle);
    }

    pub(crate) async fn shutdown(&self) {
        self.shutdown.cancel();
        let handles = std::mem::take(&mut *self.handles.lock().await);
        for handle in handles {
            let _ = handle.await;
        }
    }
}
```

- [ ] **Step 5: Run the worker tests and inspect logs**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests -- --nocapture
```

Expected: PASS. The fail→succeed test must show no warning after the successful probe; the backfill interruption test must show a second probe and a final completion.

- [ ] **Step 6: Commit the worker slice**

```bash
git add crates/memory-mcp/src/service/embedding_recovery.rs
git commit -m "feat: recover remote embeddings with cancellable backfill worker"
```

---

### Task 7: Wire the worker into startup and shutdown with the exact gate

**Files:**
- Modify: `crates/memory-mcp/src/service/core/builder.rs`
- Modify: `crates/memory-mcp/src/service/core.rs`
- Modify: `crates/memory-mcp/src/service.rs`
- Test: `crates/memory-mcp/src/service/core/builder.rs` or `crates/memory-mcp/src/service/embedding_recovery.rs`

**Interfaces:**
- Consumes: `EmbeddingActivationMode`, `EmbeddingStartupDecision`, `EmbeddingConfig`, `is_remote_embedding_provider`, `ConfiguredEmbeddingRecoveryBackend`, `RecoveryWorkerSettings`, and `EmbeddingRecoveryRuntime`.
- Produces: a `MemoryService::embedding_recovery_runtime` field, `start_embedding_recovery_worker`, and an expanded existing shutdown method.

- [ ] **Step 1: Write failing spawn-gate tests**

Add a pure gate helper and cover every disallowed path:

```rust
#[test]
fn recovery_worker_gate_requires_exact_preflight_failure_and_remote_provider() {
    let remote = EmbeddingConfig {
        provider: EmbeddingProviderKind::OpenAiCompatible,
        ..EmbeddingConfig::default()
    };
    let local = EmbeddingConfig {
        provider: EmbeddingProviderKind::LocalCandle,
        ..EmbeddingConfig::default()
    };
    assert!(should_spawn_embedding_recovery(
        EmbeddingActivationMode::Standard,
        &EmbeddingStartupDecision::DisableSemantic {
            reason: "embedding target preflight failed".to_string(),
        },
        &remote,
    ));
    assert!(!should_spawn_embedding_recovery(
        EmbeddingActivationMode::ForceEnabledForReembed,
        &EmbeddingStartupDecision::DisableSemantic {
            reason: "embedding target preflight failed".to_string(),
        },
        &remote,
    ));
    assert!(!should_spawn_embedding_recovery(
        EmbeddingActivationMode::Standard,
        &EmbeddingStartupDecision::DisableSemantic {
            reason: "configured embedding signature differs".to_string(),
        },
        &remote,
    ));
    assert!(!should_spawn_embedding_recovery(
        EmbeddingActivationMode::Standard,
        &EmbeddingStartupDecision::DisableSemantic {
            reason: "embedding target preflight failed".to_string(),
        },
        &local,
    ));
}
```

Add a second assertion using `auto_recovery: false` and verify it returns false.

- [ ] **Step 2: Run the gate test and verify it fails**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests::recovery_worker_gate_requires_exact_preflight_failure_and_remote_provider -- --exact
```

Expected: compilation failure because the gate and runtime field do not exist.

- [ ] **Step 3: Add the runtime field and initialize it safely**

Add to `MemoryService`:

```rust
pub(crate) embedding_recovery_runtime:
    Option<crate::service::embedding_recovery::EmbeddingRecoveryRuntime>,
```

Initialize it as `None` in `build()`. Keep `MemoryService` cloneable through the runtime's cloned cancellation/handle Arcs. Add:

```rust
pub(crate) async fn start_embedding_recovery_worker(
    &self,
    config: crate::config::EmbeddingConfig,
    data_dir: String,
) -> crate::service::embedding_recovery::EmbeddingRecoveryRuntime {
    let runtime = crate::service::embedding_recovery::EmbeddingRecoveryRuntime::new();
    let backend = std::sync::Arc::new(
        crate::service::embedding_recovery::ConfiguredEmbeddingRecoveryBackend::new(
            config.clone(),
            data_dir,
        ),
    );
    runtime
        .spawn(
            self.clone(),
            config.clone(),
            backend,
            crate::service::embedding_recovery::RecoveryWorkerSettings::production(
                config.recovery_interval_secs,
            ),
        )
        .await;
    runtime
}
```

- [ ] **Step 4: Implement the exact builder spawn gate**

Define the gate with no broad `DisableSemantic` match; it also admits the explicit resumable decisions:

```rust
fn should_spawn_embedding_recovery(
    mode: EmbeddingActivationMode,
    decision: &EmbeddingStartupDecision,
    config: &EmbeddingConfig,
) -> bool {
    matches!(mode, EmbeddingActivationMode::Standard)
        && config.auto_recovery
        && is_remote_embedding_provider(config.provider_label())
        && (matches!(
            decision,
            EmbeddingStartupDecision::DisableSemantic { reason }
                if reason == "embedding target preflight failed"
        ) || matches!(
            decision,
            EmbeddingStartupDecision::ResumePendingBackfill { .. }
                | EmbeddingStartupDecision::RecoverMissingEmbeddings { .. }
        ))
}
```

In `new_from_env_with_mode_and_progress`, calculate this boolean from the original decision before returning the service. After `check_surrealdb_connection`, claim scheduling, and lifecycle worker setup, start the recovery worker only when the gate is true and store the returned runtime in `service.embedding_recovery_runtime`. The disabled provider must be installed before the worker starts, so all requests before the first successful swap remain degraded.

- [ ] **Step 5: Extend shutdown without changing CLI call sites**

Update `shutdown_lifecycle_background_workers()` so it joins both runtimes:

```rust
pub async fn shutdown_lifecycle_background_workers(&self) {
    if let Some(runtime) = &self.lifecycle_background_workers {
        runtime.shutdown().await;
    }
    if let Some(runtime) = &self.embedding_recovery_runtime {
        runtime.shutdown().await;
    }
}
```

Keep the existing calls in `cli/runtime.rs` unchanged; verify stdio, watch, and reembed modes all call this method on every exit path.

- [ ] **Step 6: Run gate, shutdown, builder, and CLI tests**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests -- --nocapture
cargo test -p memory_mcp cli::runtime -- --nocapture
cargo check -p memory_mcp --all-targets
```

Expected: PASS; normal startup with auto recovery disabled remains degraded and no recovery task is spawned, while force-enabled reembed mode never starts the recovery worker.

- [ ] **Step 7: Commit the wiring slice**

```bash
git add crates/memory-mcp/src/service/core.rs crates/memory-mcp/src/service/core/builder.rs crates/memory-mcp/src/service.rs
 git commit -m "feat: wire embedding recovery into service lifecycle"
```

---

### Task 8: Add a real HTTP recovery integration test

**Files:**
- Modify: `crates/memory-mcp/src/service/embedding_recovery.rs`
- Test: `crates/memory-mcp/src/service/embedding_recovery.rs` integration-style test module

**Interfaces:**
- Consumes: `tokio::net::TcpListener`, the concrete configured recovery backend, the real OpenAI-compatible probe/provider, and the worker runtime from Tasks 4–7.
- Produces: one deterministic hand-rolled HTTP test proving failed probe → recovered probe → fact backfill.

- [ ] **Step 1: Write the failing listener-backed test**

Create a `TcpListener` stub that returns one retryable failure (`503`) and then valid OpenAI-compatible JSON containing a 1536-vector. The test must read and discard each request through the HTTP header terminator, write a `Content-Length`, and stop after the worker completes. The test scenario is:

```rust
#[tokio::test]
async fn tcp_listener_recovery_probe_failure_then_success_backfills_offline_fact() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("address"));
    let failures = Arc::new(AtomicUsize::new(1));
    let server_failures = failures.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.expect("accept");
            read_http_request(&mut socket).await.expect("request");
            let status = if server_failures.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                n.checked_sub(1)
            }).is_ok() { "503 Service Unavailable" } else { "200 OK" };
            write_embedding_response(&mut socket, status, 1536).await.expect("response");
        }
    });

    let db = make_in_memory_db("tcp_recovery").await;
    seed_missing_fact(&db, "fact:offline", "saved while disconnected").await;
    let service = make_disabled_service(db.clone(), "org");
    let config = remote_config_with_base_url(base_url.clone());
    let backend = Arc::new(ConfiguredEmbeddingRecoveryBackend::new(
        config,
        ".".to_string(),
    ));
    let settings = RecoveryWorkerSettings {
        initial_probe_delay: Duration::ZERO,
        backoff_base: Duration::from_millis(1),
        backoff_cap: Duration::from_millis(2),
        warn_demote_after: 3,
        batch_size: 100,
    };

    tokio::time::timeout(
        Duration::from_secs(5),
        run_recovery_worker(
            service.clone(),
            remote_config_with_base_url(base_url.clone()),
            backend,
            settings,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("worker should recover and finish backfill");
    server.abort();

    assert!(service.embedding_runtime_snapshot().provider.is_enabled());
    let fact = db.select_one("fact:offline", "org").await.expect("read").expect("fact");
    assert_eq!(fact.get("embedding_dimension").and_then(Value::as_u64), Some(1536));
    assert_eq!(fact.get("embedding_signature").and_then(Value::as_str), Some("embsig:test"));
}
```

Use a test configuration whose model and base URL produce the same expected test signature (`embsig:test`) or assert against `build_embedding_signature(...)` rather than hard-code an unrelated value. The response body must use the existing OpenAI shape `{"data":[{"embedding":[...]}]}`.

- [ ] **Step 2: Run the integration test and verify it fails**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests::tcp_listener_recovery_probe_failure_then_success_backfills_offline_fact -- --exact --nocapture
```

Expected: failure until the worker, concrete backend, state swap, and backfill are all wired correctly.

- [ ] **Step 3: Implement only the test HTTP helpers needed by this scenario**

Use `tokio::io::{AsyncReadExt, AsyncWriteExt}`. `read_http_request` reads one byte at a time into a bounded buffer until `\r\n\r\n`; if the bound is exceeded, return `MemoryError::Validation`. `write_embedding_response` serializes a vector with `vec![0.0; dimension]` and sets element zero to `1.0`, then writes:

```text
HTTP/1.1 200 OK\r
Content-Type: application/json\r
Content-Length: <bytes>\r
Connection: close\r
\r
{"data":[{"embedding":[...]}]}
```

For the failure response use an empty body and `503 Service Unavailable`. Ensure the listener task is aborted after the worker returns so the test cannot leak a server task.

- [ ] **Step 4: Run the listener-backed test and the whole crate test target**

Run:

```bash
cargo test -p memory_mcp service::embedding_recovery::tests::tcp_listener_recovery_probe_failure_then_success_backfills_offline_fact -- --exact --nocapture
cargo test -p memory_mcp --all-targets
```

Expected: PASS; the first probe receives `503`, a later probe succeeds, the provider is enabled without a restart, and the offline fact receives a vector plus provider/signature/dimension/timestamp metadata.

- [ ] **Step 5: Commit the integration test**

```bash
git add crates/memory-mcp/src/service/embedding_recovery.rs
git commit -m "test: cover remote embedding recovery over HTTP"
```

---

### Task 9: Document the observable air-gap lifecycle

**Files:**
- Modify: `README.md`
- Modify: `.env.example`
- Modify: `CONTEXT.md` only if the existing vocabulary needs an implemented-behavior correction

**Interfaces:**
- Consumes: the accepted ADR, existing embedding-provider configuration section, actual structured log event names, and the startup behavior already shipped in commit `96c8cba5`.
- Produces: operator-facing documentation that distinguishes an unavailable endpoint from a misconfigured endpoint and explains what happens when connectivity returns.

- [ ] **Step 1: Write documentation assertions as a review checklist**

Before editing, ensure the README text will answer all of these concrete questions:

```text
1. Does startup block on the remote provider? No: one bounded preflight fails and serve degrades.
2. Which variables control automatic recovery? EMBEDDINGS_AUTO_RECOVERY and EMBEDDINGS_RECOVERY_INTERVAL_SECS.
3. When does the worker start? Only for the exact preflight-failure reason and remote provider.
4. What happens after connectivity returns? Probe, dimension/signature check, runtime swap, cache invalidation, ready-state write, then missing-vector backfill.
5. What happens to facts created offline? They are selected by embedding IS NONE and processed in fact_id order in batches of 100.
6. What is never changed automatically? Existing stale vectors, HNSW dimension, and provider-switch state; use reembed.
7. What does a 404 mean? It is retried by the recovery worker but usually indicates endpoint/model/path misconfiguration and must be corrected.
8. Which logs show progress? embedding.preflight_failed, embedding.startup_decision, embedding.recovery_probe_failed, embedding.recovered, embedding.reembed_required, embedding.backfill_progress, embedding.backfill_completed.
```

- [ ] **Step 2: Add the two variables to `.env.example`**

Place the comments next to the existing embedding timeout/provider settings:

```dotenv
# Periodic recovery worker delay after startup enters degraded mode.
# The worker starts after this delay; failed probes use 15s, 30s, ... up to 300s.
# EMBEDDINGS_RECOVERY_INTERVAL_SECS=60

# Automatic in-process recovery is enabled by default. Set false to keep the
# startup-degraded behavior until the next process restart/operator action.
# EMBEDDINGS_AUTO_RECOVERY=true
```

- [ ] **Step 3: Update the README configuration table and lifecycle section**

Add both variables to the advanced runtime table and append an “Air-gapped startup and recovery” subsection after the existing provider-switch explanation. Include this sequence verbatim in substance:

```text
1. Startup performs one bounded dimension preflight. If the remote endpoint is unavailable, the server logs embedding.preflight_failed and starts with semantic retrieval disabled; MCP/lexical/graph operations continue.
2. If automatic recovery is enabled, a background worker waits EMBEDDINGS_RECOVERY_INTERVAL_SECS, then probes with 15s→30s→60s exponential backoff capped at 300s. It keeps trying after transport failures and HTTP errors, including 404; repeated failures become debug-level after the third failure.
3. A matching dimension plus the same or absent persisted signature enables the provider, invalidates the context cache, writes ready state, and backfills only facts with embedding IS NONE. Backfill never drops the HNSW index.
4. A same-dimension signature change enables new writes but logs embedding.reembed_required and does not rewrite old vectors. A dimension mismatch keeps semantic mode disabled. Run reembed after correcting the target when a provider/model/dimension change is intentional.
5. The worker stops after compatible recovery and an empty missing-embedding set. Shutdown cancels and joins it through the existing lifecycle shutdown path.
```

Explicitly retain the distinction that the observed startup `404` is not proof of an air gap: it commonly means the configured URL, route, model, or provider API shape is wrong.

- [ ] **Step 4: Review links and configuration consistency**

Run:

```bash
cargo test -p memory_mcp config::embedding::tests -- --nocapture
```

Then manually compare README, `.env.example`, `EmbeddingConfig::from_env`, and ADR-0042 so defaults are all `60`/enabled and no text claims that backfill rewrites stale signatures.

- [ ] **Step 5: Commit the documentation slice including `CONTEXT.md`**

```bash
git add README.md .env.example CONTEXT.md
git commit -m "docs: explain embedding air gap recovery lifecycle"
```

---

### Task 10: Run the final TDD and repository quality gates

**Files:**
- Verify: all files changed in Tasks 1–9
- Test: existing crate/unit/integration test targets

**Interfaces:**
- Consumes: all implementation and documentation slices above.
- Produces: verified fast offline startup, automatic recovery, safe backfill, clean shutdown, and no regression in reembed or existing embedding behavior.

- [ ] **Step 1: Run formatting and fix only formatting changes**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS with no diff. If it fails, run `cargo fmt --all`, inspect the diff, and commit only formatting in the active implementation commit.

- [ ] **Step 2: Run the production crate tests**

Run:

```bash
cargo test -p memory_mcp
```

Expected: PASS, including startup fast-failure, config, runtime-state, backfill-store, recovery-worker, reembed, and lifecycle shutdown tests.

- [ ] **Step 3: Run strict workspace clippy with the required features**

Run:

```bash
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
```

Expected: PASS with zero warnings. In particular, verify no `await_holding_lock`, dead-code, visibility, or `too_many_arguments` warnings are introduced.

- [ ] **Step 4: Run the offline startup reproduction**

Run with a fresh embedded data directory:

```bash
EMBEDDINGS_ENABLED=true EMBEDDINGS_PROVIDER=openai-compatible EMBEDDINGS_MODEL=test-embed EMBEDDINGS_BASE_URL=http://192.0.2.1:9999/v1 SURREALDB_EMBEDDED=true SURREALDB_DATA_DIR=/tmp/memory-mcp-airgap-check cargo run -q -- serve < /dev/null
```

Expected: the process reaches its normal serve-start/shutdown path without waiting for the remote runtime retry budget. Logs show `embedding.preflight_failed`, `embedding.startup_decision`, and (when auto recovery is enabled) a background recovery task; stderr remains the log destination and stdout remains JSON-RPC-safe.

Run the opt-out regression as well:

```bash
EMBEDDINGS_ENABLED=true EMBEDDINGS_AUTO_RECOVERY=false EMBEDDINGS_PROVIDER=openai-compatible EMBEDDINGS_MODEL=test-embed EMBEDDINGS_BASE_URL=http://192.0.2.1:9999/v1 SURREALDB_EMBEDDED=true SURREALDB_DATA_DIR=/tmp/memory-mcp-airgap-check-optout cargo run -q -- serve < /dev/null
```

Expected: fast degraded startup and no recovery probe events after startup.

- [ ] **Step 5: Inspect the final diff and commit the plan/ADR relationship**

Run:

```bash
git --no-pager diff --check HEAD~10..HEAD
git --no-optional-locks status --short
```

Expected: no whitespace errors, no accidental changes to unrelated files, ADR-0042 points to the plan, and `CONTEXT.md` embedding vocabulary remains present.

- [ ] **Step 6: Record the final verification result**

Add the actual commands and PASS/FAIL outcomes to the final implementation summary. Do not claim the air-gap scenario is verified until the TCP listener test, crate tests, strict clippy, formatting, and both offline/opt-out startup runs have completed.

---

## Self-review before execution

### Spec coverage

- Fast startup after remote preflight failure: Tasks 1, 7, and 10.
- Automatic in-process recovery: Tasks 2, 4, 6, 7, and 8.
- Exponential backoff and warning demotion: Tasks 4 and 6.
- Recovery disabled opt-out and exact spawn gate: Tasks 1 and 7.
- Atomic provider/identity swap with no lock across await: Task 2.
- Cache invalidation after recovery: Tasks 5–7.
- Persisted dimension/signature Q9 decisions: Tasks 4 and 6.
- Narrow missing-embedding selection and cursor pagination: Task 3.
- Background backfill with provider/network interruption returning to probe: Tasks 5 and 6.
- No HNSW rebuild and no stale-vector rewrite: Tasks 3, 5, and 9.
- Cancellable shutdown in all existing serve/watch/reembed exits: Task 7.
- Injectable unit seam plus real `TcpListener` test: Tasks 4, 6, and 8.
- Operator documentation, 404 distinction, and logs: Task 9.
- Required repository quality gates: Task 10.

### Placeholder scan

The plan contains no `TBD`, `TODO`, or unspecified “handle edge cases” implementation steps. Every implementation boundary names its file, function/type interface, test command, expected result, and the concrete behavior to verify.

### Type consistency review

- `EmbeddingRuntimeState` owns `Arc<dyn EmbeddingProvider>`, `Option<String>` signature/model, and `Option<usize>` dimension; both builder initialization and recovery replacement use those exact fields.
- `EmbeddingRecoveryBackend::probe_dimension()` returns `Result<usize, MemoryError>` and `create_provider(usize)` returns `Result<Arc<dyn EmbeddingProvider>, MemoryError>`; the configured and fake backends implement those same signatures.
- `choose_recovery_decision()` returns the three decisions consumed by `run_recovery_cycle()`.
- `run_backfill()` returns `Result<BackfillOutcome, MemoryError>`; the worker treats `Complete` as terminal and all errors as probe-cycle retries.
- `RecoveryWorkerSettings` supplies `initial_probe_delay`, `backoff_base`, `backoff_cap`, `warn_demote_after`, and `batch_size` to both production and tests.
- The existing `MemoryService::shutdown_lifecycle_background_workers()` remains the shutdown entry point used by `cli/runtime.rs` and now joins the optional embedding recovery runtime too.

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-20-embedding-recovery-and-backfill.md`. Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task and review between tasks.
2. **Inline Execution** — execute the tasks in this session using `superpowers:executing-plans`, with TDD checkpoints.

The implementation must not start until one of these execution modes is selected; either mode must follow the task order and the failing-test → implementation → passing-test sequence above.
