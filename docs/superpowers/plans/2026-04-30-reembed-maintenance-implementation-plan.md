# Embedding Rebuild Maintenance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** add a dedicated `memory_mcp reembed` maintenance mode that rewrites every `fact.embedding` for the current embedding provider/model, auto-detects embedding dimension when no explicit override is set, keeps existing databases compatible, and exposes clear progress plus debug-grade structured logs without changing the MCP tool surface.

**Architecture:** Introduce a small embedding control plane built from three pieces: a deterministic `embedding_signature`, per-namespace `embedding_state` rows, and a persisted `embedding_job` progress record. Resolve the target embedding dimension automatically from provider/model metadata or a one-shot probe when `SURREALDB_EMBEDDING_DIMENSION` is unset, but still allow an explicit override as a strict validation guard. Normal `serve` and `watch` stay simple and safe by disabling semantic retrieval when the current config is not known-ready; the dedicated `reembed` mode rewrites facts batch-by-batch, records durable progress, emits structured events for every meaningful phase, and resumes safely after restart by reloading job state from the database.

**Tech Stack:** Rust 2024, SurrealDB 3.x, rmcp, serde/serde_json, sha2/hex, existing `MemoryService` + `DbClient` abstractions, integration tests under `tests/`, markdown docs

---

## Scope guardrails

- Do **not** edit `migrations/__Initial.surql` or `migrations/008_fact_semantic_embeddings.surql`.
- Add exactly one new schema migration: `migrations/019_embedding_rebuild_maintenance.surql`.
- Auto-detect the target embedding dimension when `SURREALDB_EMBEDDING_DIMENSION` is not explicitly set.
- Keep the public MCP contract unchanged.
- Rewrite embeddings for **all** facts, including invalidated and historical facts.
- Keep normal startup available even when a rebuild is pending by falling back to lexical and graph retrieval only.
- Emit structured logs rich enough to debug startup decisions, resume behavior, cursor movement, and failure causes without adding ad hoc instrumentation later.
- Reuse the exact same canonical text input for `add_fact()` and `reembed`.
- Prefer focused helpers and query builders over widening public APIs unnecessarily.

## File map

### Application files

- Create: `migrations/019_embedding_rebuild_maintenance.surql` — additive schema for fact embedding metadata plus `embedding_state` and `embedding_job`
- Modify: `src/config/embedding.rs:18-160` — dimension override semantics and signature input helpers
- Modify: `src/service/embedding.rs:1-220` — resolved embedding target helper and auto-detect bootstrap flow
- Modify: `src/service/embedding/local.rs:1-252` — detect local-candle dimension from model metadata or probe output
- Modify: `src/service/embedding/remote.rs:1-220` — remote provider dimension probe and explicit override validation
- Modify: `src/storage/migrations.rs:1-120` — register migration `019` and add targeted compatibility for historical migration `008`
- Modify: `src/service/startup.rs:1-26` — startup decision helper for `ready`, `rebuilding`, and `failed` embedding state
- Modify: `src/service/core/builder.rs:46-145` — evaluate namespace embedding state and globally enable or disable semantic retrieval
- Modify: `src/service/core.rs:392-450,572-606` — extract canonical fact embedding input helper and reuse it from `add_fact()`
- Modify: `src/service/core/helpers.rs:1-120` — shared structured event helpers for new startup / reembed operations
- Modify: `src/storage/queries.rs:1-220` — query builders for fact counts, batch selection, progress sampling, and index DDL
- Modify: `src/storage/client.rs:480-540,1674-1703` — execute new query builders and keep migration bookkeeping compatible
- Create: `src/service/reembed.rs` — batch rewrite orchestration, durable progress, structured phase logging, ETA, and finalization
- Modify: `src/service.rs:1-60` — wire in the `reembed` module and re-export `EmbeddingActivationMode` for crate-local CLI access
- Modify: `src/cli.rs:33-195` — add `RunMode::Reembed` and `run_reembed_mode()`
- Modify: `src/main.rs:1-24` — dispatch `reembed`

### Tests

- Create: `tests/embedded_reembed.rs` — end-to-end rewrite, restart-safe resume, and failure-path coverage
- Modify: `src/config/embedding.rs` tests — signature stability and drift detection
- Modify: `src/service/embedding.rs` tests — auto-detect dimension and explicit override behavior
- Modify: `src/storage/migrations.rs` tests — historical migration compatibility for `008`
- Modify: `src/service/startup.rs` tests — startup gating decision helpers
- Modify: `src/logging.rs` tests — structured event formatting and redaction invariants for new maintenance events
- Modify: `src/cli.rs` tests — `reembed` parser coverage

### Docs

- Modify: `README.md` — operator workflow for provider switching and `memory_mcp reembed`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md` — runtime behavior when embedding state is not ready
- Reference only: `docs/superpowers/specs/2026-04-30-reembed-maintenance-design.md`

## Logging contract for this feature

The implementation must follow the repository's existing structured logger conventions (`StdoutLogger`, `LogLevel`, stable `op=...` keys) rather than inventing a second logging format.

Required event families:

- `embedding.preflight_*`
- `embedding.startup_state_*`
- `embedding.startup_decision`
- `reembed.job_*`
- `reembed.namespace_*`
- `reembed.batch_*`
- `reembed.cursor_*`
- `reembed.index_*`
- `reembed.progress`
- `reembed.fact_failed`

Required terminal summary events:

- `reembed.job_completed`
- `reembed.job_failed`
- `main.reembed_completed`
- `main.reembed_failed`

Required safety rules:

- never log API keys, auth headers, raw embedding vectors, or whole source documents;
- log identifiers and summaries instead (`fact_id`, `namespace`, `dimension`, counts, durations, signatures);
- per-fact success logs belong at `trace`, not `info`.

Required terminal summary fields:

- `processed_facts`
- `succeeded_facts`
- `failed_facts`
- `total_facts`
- `duration_ms`
- `provider`
- `model`
- `target_dimension`
- `target_signature`
- `resumed`
- `facts_per_second`

---

### Task 1: Add auto-detected embedding targets, signature, and maintenance schema

**Files:**
- Create: `migrations/019_embedding_rebuild_maintenance.surql`
- Modify: `src/config/embedding.rs:18-160`
- Modify: `src/service/embedding.rs:1-220`
- Modify: `src/service/embedding/local.rs:1-252`
- Modify: `src/service/embedding/remote.rs:1-220`
- Modify: `src/storage/migrations.rs:1-46`
- Test: `src/config/embedding.rs`
- Test: `src/service/embedding.rs`
- Test: `src/storage/migrations.rs`

- [ ] **Step 1: Write the failing unit tests for signature drift and migration registration**

Add these tests before adding implementation:

```rust
#[test]
fn embedding_signature_changes_when_model_changes() {
    let first = build_embedding_signature(
        "openai-compatible",
        Some("text-embedding-3-small"),
        Some("https://api.openai.com/v1"),
        1536,
    );
    let second = build_embedding_signature(
        "openai-compatible",
        Some("text-embedding-3-large"),
        Some("https://api.openai.com/v1"),
        1536,
    );

    assert_ne!(first, second);
}

#[test]
fn embedding_signature_is_stable_for_equivalent_config() {
    let left = build_embedding_signature(
        "local-candle",
        Some("intfloat/multilingual-e5-small"),
        None,
        384,
    );
    let right = build_embedding_signature(
        "local-candle",
        Some("intfloat/multilingual-e5-small"),
        None,
        384,
    );

    assert_eq!(left, right);
}

#[tokio::test]
async fn local_candle_detects_dimension_from_model_metadata_when_override_missing() {
    let config = EmbeddingConfig {
        provider: EmbeddingProviderKind::LocalCandle,
        model: Some("intfloat/multilingual-e5-small".to_string()),
        model_dir: Some("./tests/models/intfloat/multilingual-e5-small".to_string()),
        dimension_override: None,
        ..EmbeddingConfig::default()
    };

    let resolved = resolve_embedding_target_identity(&config, ".").await.expect("resolved target");
    assert_eq!(resolved.dimension, 384);
}

#[tokio::test]
async fn remote_probe_detects_dimension_when_override_missing() {
    let provider = ProbeOnlyTestProvider::new(vec![0.1, 0.2, 0.3, 0.4]);
    let resolved = resolve_dimension_override_or_probe(None, &provider)
        .await
        .expect("detected dimension");

    assert_eq!(resolved, 4);
}

#[tokio::test]
async fn explicit_dimension_override_mismatch_fails_fast() {
    let provider = ProbeOnlyTestProvider::new(vec![0.1, 0.2, 0.3, 0.4]);
    let err = resolve_dimension_override_or_probe(Some(3), &provider)
        .await
        .expect_err("override mismatch should fail");

    assert!(err.to_string().contains("embedding dimension mismatch"));
}

#[test]
fn versioned_migrations_includes_embedding_rebuild_maintenance() {
    assert!(versioned_migrations()
        .iter()
        .any(|migration| migration.file_name == "019_embedding_rebuild_maintenance.surql"));
}
```

- [ ] **Step 2: Run the focused tests and confirm they fail for the expected reasons**

Run:

```bash
git diff -- src/config/embedding.rs src/service/embedding.rs src/storage/migrations.rs
cargo test embedding_signature_changes_when_model_changes --lib
cargo test local_candle_detects_dimension_from_model_metadata_when_override_missing --lib
cargo test versioned_migrations_includes_embedding_rebuild_maintenance --lib
```

Expected:
- `build_embedding_signature()` does not exist yet
- `resolve_embedding_target_identity()` / probe helper do not exist yet
- migration `019_embedding_rebuild_maintenance.surql` is not yet registered

- [ ] **Step 3: Add the additive maintenance schema and resolved embedding target helpers**

Create `migrations/019_embedding_rebuild_maintenance.surql` with additive fields only:

```sql
DEFINE FIELD embedding_provider ON fact TYPE option<string>;
DEFINE FIELD embedding_model ON fact TYPE option<string>;
DEFINE FIELD embedding_dimension ON fact TYPE option<int>;
DEFINE FIELD embedding_signature ON fact TYPE option<string>;
DEFINE FIELD embedding_updated_at ON fact TYPE option<datetime>;

DEFINE TABLE embedding_state SCHEMAFULL;
DEFINE FIELD status ON embedding_state TYPE string;
DEFINE FIELD active_signature ON embedding_state TYPE option<string>;
DEFINE FIELD provider ON embedding_state TYPE option<string>;
DEFINE FIELD model ON embedding_state TYPE option<string>;
DEFINE FIELD dimension ON embedding_state TYPE option<int>;
DEFINE FIELD last_job_id ON embedding_state TYPE option<string>;
DEFINE FIELD updated_at ON embedding_state TYPE datetime;

DEFINE TABLE embedding_job SCHEMAFULL;
DEFINE FIELD job_id ON embedding_job TYPE string;
DEFINE FIELD status ON embedding_job TYPE string;
DEFINE FIELD target_signature ON embedding_job TYPE string;
DEFINE FIELD provider ON embedding_job TYPE string;
DEFINE FIELD model ON embedding_job TYPE option<string>;
DEFINE FIELD dimension ON embedding_job TYPE int;
DEFINE FIELD namespaces ON embedding_job TYPE array;
DEFINE FIELD requested_at ON embedding_job TYPE datetime;
DEFINE FIELD total_facts ON embedding_job TYPE int;
DEFINE FIELD processed_facts ON embedding_job TYPE int;
DEFINE FIELD succeeded_facts ON embedding_job TYPE int;
DEFINE FIELD failed_facts ON embedding_job TYPE int;
DEFINE FIELD facts_per_second ON embedding_job TYPE option<float>;
DEFINE FIELD eta_seconds ON embedding_job TYPE option<int>;
DEFINE FIELD current_namespace ON embedding_job TYPE option<string>;
DEFINE FIELD namespace_progress ON embedding_job TYPE object;
DEFINE FIELD last_error ON embedding_job TYPE option<string>;
DEFINE FIELD started_at ON embedding_job TYPE option<datetime>;
DEFINE FIELD updated_at ON embedding_job TYPE option<datetime>;
DEFINE FIELD finished_at ON embedding_job TYPE option<datetime>;
```

Keep the migration as above, but move target resolution out of raw config and into a resolved helper.

Important prerequisite: `EmbeddingConfig::from_env()` must preserve whether `SURREALDB_EMBEDDING_DIMENSION` was explicitly set. Do **not** collapse that distinction immediately into a fully resolved `usize`, or later startup / reembed code will be unable to tell override-from-auto-detect.

In `src/config/embedding.rs`, preserve the operator override explicitly:

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
}
```

Add a deterministic helper to `src/config/embedding.rs` for provider labels only:

```rust
pub fn build_embedding_signature(
    provider_label: &str,
    model: Option<&str>,
    base_url: Option<&str>,
    dimension: usize,
) -> String {
    use sha2::{Digest, Sha256};

    let material = serde_json::json!({
        "provider": provider_label,
        "model": model,
        "base_url": base_url.map(|url| url.trim_end_matches('/')),
        "dimension": dimension,
    });

    let mut hasher = Sha256::new();
    hasher.update(material.to_string().as_bytes());
    format!("embsig:{}", hex::encode(hasher.finalize()))
}

pub fn provider_label(&self) -> &'static str {
    match self.provider {
        EmbeddingProviderKind::Disabled => "disabled",
        EmbeddingProviderKind::LocalCandle => "local-candle",
        EmbeddingProviderKind::OpenAiCompatible => "openai-compatible",
        EmbeddingProviderKind::Ollama => "ollama",
    }
}
```

In `src/service/embedding.rs`, add a resolved target object, a preflight identity resolver, and a final provider constructor:

```rust
pub(crate) struct ResolvedEmbeddingTarget {
    pub provider_label: &'static str,
    pub model: Option<String>,
    pub dimension: usize,
    pub signature: String,
}

pub(crate) async fn resolve_embedding_target_identity(
    config: &EmbeddingConfig,
    data_dir: &str,
) -> Result<ResolvedEmbeddingTarget, MemoryError> {
    let dimension = resolve_embedding_dimension(config, data_dir).await?;
    let signature = build_embedding_signature(
        config.provider_label(),
        config.model.as_deref(),
        config.base_url.as_deref(),
        dimension,
    );

    Ok(ResolvedEmbeddingTarget {
        provider_label: config.provider_label(),
        model: config.model.clone(),
        dimension,
        signature,
    })
}

pub(crate) async fn create_embedding_provider_with_dimension(
    config: &EmbeddingConfig,
    data_dir: &str,
    resolved_dimension: usize,
) -> Result<Arc<dyn EmbeddingProvider>, MemoryError> {
    // same provider-construction logic as today, but parameterized by resolved_dimension
}
```

Detection rules to encode in implementation:

- local-candle: derive the dimension from `config.json` / model metadata before building the final provider, then validate probe output only if needed;
- remote providers: if `dimension_override` is `None`, issue a lightweight preflight probe request and use the returned vector length before building the final provider object;
- explicit override present: validate against actual provider output and fail fast on mismatch.

Register the migration in `versioned_migrations()` immediately after `018_query_log.surql`.

- [ ] **Step 4: Run the tests again and verify they pass**

Run:

```bash
cargo test embedding_signature_changes_when_model_changes --lib
cargo test embedding_signature_is_stable_for_equivalent_config --lib
cargo test local_candle_detects_dimension_from_model_metadata_when_override_missing --lib
cargo test remote_probe_detects_dimension_when_override_missing --lib
cargo test explicit_dimension_override_mismatch_fails_fast --lib
cargo test versioned_migrations_includes_embedding_rebuild_maintenance --lib
```

Expected:
- all focused target-resolution and migration tests PASS

- [ ] **Step 5: Commit the schema and signature scaffold**

Run:

```bash
git add migrations/019_embedding_rebuild_maintenance.surql src/config/embedding.rs src/service/embedding.rs src/service/embedding/local.rs src/service/embedding/remote.rs src/storage/migrations.rs
git commit -m "feat: auto-detect embedding targets for rebuild"
```

---

### Task 2: Make startup safe for legacy and drifted databases

**Files:**
- Modify: `src/storage/migrations.rs:48-120`
- Modify: `src/service/startup.rs:1-160`
- Modify: `src/service/core/builder.rs:46-145`
- Test: `src/storage/migrations.rs`
- Test: `src/service/startup.rs`

- [ ] **Step 1: Write the failing tests for historical migration `008` and startup gating**

Add a targeted migration-compatibility test:

```rust
#[test]
fn validate_applied_migration_allows_dynamic_embedding_checksum_drift_for_008() {
    let existing = serde_json::json!({
        "script_name": "008_fact_semantic_embeddings.surql",
        "checksum": "checksum-from-384-database",
        "executed_at": "2026-04-30T00:00:00Z"
    });

    let result = validate_applied_migration(
        &existing,
        "008_fact_semantic_embeddings.surql",
        "checksum-from-1536-render",
    );

    assert!(result.is_ok());
}
```

Add startup decision tests in `src/service/startup.rs`:

```rust
#[test]
fn decide_embedding_startup_disables_semantic_when_any_namespace_is_rebuilding() {
    let states = std::collections::HashMap::from([
        ("org".to_string(), Some(serde_json::json!({"status": "ready", "active_signature": "embsig:ok"}))),
        ("personal".to_string(), Some(serde_json::json!({"status": "rebuilding", "active_signature": "embsig:ok"}))),
    ]);

    let decision = decide_embedding_startup(
        &["org".to_string(), "personal".to_string()],
        &states,
        "embsig:ok",
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        384,
    );

    assert!(matches!(
        decision,
        EmbeddingStartupDecision::DisableSemantic { .. }
    ));
}

#[test]
fn decide_embedding_startup_bootstraps_legacy_ready_when_dimensions_match() {
    let states = std::collections::HashMap::from([("org".to_string(), None)]);
    let decision = decide_embedding_startup(
        &["org".to_string()],
        &states,
        "embsig:new",
        &std::collections::HashMap::from([("org".to_string(), vec![384usize])]),
        &std::collections::HashMap::from([("org".to_string(), 12usize)]),
        384,
    );

    assert!(matches!(
        decision,
        EmbeddingStartupDecision::BootstrapReadyNamespaces { .. }
    ));
}

#[test]
fn decide_embedding_startup_bootstraps_missing_namespace_without_ignoring_existing_ready_state() {
    let states = std::collections::HashMap::from([
        ("org".to_string(), Some(serde_json::json!({"status": "ready", "active_signature": "embsig:new"}))),
        ("personal".to_string(), None),
    ]);

    let decision = decide_embedding_startup(
        &["org".to_string(), "personal".to_string()],
        &states,
        "embsig:new",
        &std::collections::HashMap::from([("personal".to_string(), vec![384usize])]),
        &std::collections::HashMap::from([("org".to_string(), 10usize), ("personal".to_string(), 2usize)]),
        384,
    );

    assert!(matches!(
        decision,
        EmbeddingStartupDecision::BootstrapReadyNamespaces { ref namespaces, .. }
            if namespaces == &vec!["personal".to_string()]
    ));
}
```

- [ ] **Step 2: Run the focused tests and verify the failure mode is correct**

Run:

```bash
cargo test validate_applied_migration_allows_dynamic_embedding_checksum_drift_for_008 --lib
cargo test decide_embedding_startup_disables_semantic_when_any_namespace_is_rebuilding --lib
cargo test decide_embedding_startup_bootstraps_legacy_ready_when_dimensions_match --lib
```

Expected:
- migration validation still rejects mismatched checksums for `008`
- `EmbeddingStartupDecision` and `decide_embedding_startup()` do not exist yet
- partial-namespace bootstrap is not handled yet

- [ ] **Step 3: Add the targeted `008` compatibility rule and startup decision helper**

Keep checksum validation strict by default, but carve out the historical embedding migration explicitly:

```rust
fn is_dynamic_embedding_migration(file_name: &str) -> bool {
    matches!(file_name, "008_fact_semantic_embeddings.surql")
}

pub fn validate_applied_migration(
    existing: &Value,
    expected_file_name: &str,
    expected_checksum: &str,
) -> Result<(), MemoryError> {
    // ... existing field validation omitted ...

    if applied_checksum != expected_checksum && !is_dynamic_embedding_migration(expected_file_name) {
        return Err(MemoryError::ConfigInvalid(format!(
            "applied migration {expected_file_name} was modified after execution"
        )));
    }

    Ok(())
}
```

Add a startup decision helper to `src/service/startup.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmbeddingStartupDecision {
    UseConfiguredProvider,
    BootstrapReadyNamespaces { namespaces: Vec<String>, active_signature: String },
    DisableSemantic { reason: String },
}

pub(crate) fn decide_embedding_startup(
    configured_namespaces: &[String],
    namespace_states: &std::collections::HashMap<String, Option<serde_json::Value>>,
    target_signature: &str,
    sample_dimensions: &std::collections::HashMap<String, Vec<usize>>,
    fact_counts: &std::collections::HashMap<String, usize>,
    target_dimension: usize,
) -> EmbeddingStartupDecision {
    let mut namespaces_to_bootstrap = Vec::new();

    for namespace in configured_namespaces {
        match namespace_states.get(namespace).and_then(|value| value.as_ref()) {
            Some(state)
                if state.get("status").and_then(serde_json::Value::as_str) == Some("rebuilding")
                    || state.get("status").and_then(serde_json::Value::as_str) == Some("failed") => {
                return EmbeddingStartupDecision::DisableSemantic {
                    reason: format!("embedding maintenance is incomplete in namespace `{namespace}`"),
                };
            }
            Some(state)
                if state.get("status").and_then(serde_json::Value::as_str) == Some("ready")
                    && state
                    .get("active_signature")
                    .and_then(serde_json::Value::as_str)
                    == Some(target_signature) => {}
            Some(state) if state.get("status").and_then(serde_json::Value::as_str) == Some("ready") => {
                return EmbeddingStartupDecision::DisableSemantic {
                    reason: format!("configured embedding signature differs from persisted state in namespace `{namespace}`"),
                };
            }
            None => {
                let fact_count = *fact_counts.get(namespace).unwrap_or(&0);
                if fact_count == 0 {
                    namespaces_to_bootstrap.push(namespace.clone());
                    continue;
                }

                let sampled = sample_dimensions.get(namespace).cloned().unwrap_or_default();
                if !sampled.is_empty() && sampled.iter().all(|dimension| *dimension == target_dimension) {
                    namespaces_to_bootstrap.push(namespace.clone());
                    continue;
                }

                return EmbeddingStartupDecision::DisableSemantic {
                    reason: format!("legacy embeddings in namespace `{namespace}` require reembed before semantic search can resume"),
                };
            }
            Some(_) => {
                return EmbeddingStartupDecision::DisableSemantic {
                    reason: format!("embedding state in namespace `{namespace}` is invalid or incomplete"),
                };
            }
        }
    }

    if namespaces_to_bootstrap.is_empty() {
        EmbeddingStartupDecision::UseConfiguredProvider
    } else {
        EmbeddingStartupDecision::BootstrapReadyNamespaces {
            namespaces: namespaces_to_bootstrap,
            active_signature: target_signature.to_string(),
        }
    }
}
```

- [ ] **Step 4: Wire the decision into `MemoryService` startup**

Refactor startup so that semantic enablement is a global process decision across all namespaces and add an explicit maintenance bypass:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddingActivationMode {
    Standard,
    ForceEnabledForReembed,
}

let namespace_states = load_embedding_states(&db_client, &config.namespaces).await?;
let fact_counts = count_facts_per_namespace(&db_client, &config.namespaces).await?;
let sample_dimensions = sample_stored_embedding_dimensions(
    &db_client,
    &config.namespaces,
    LEGACY_EMBEDDING_SAMPLE_SIZE,
).await?;
let target = match resolve_embedding_target_identity(&config.embedding, &effective_data_dir).await {
    Ok(target) => Some(target),
    Err(err) if mode == EmbeddingActivationMode::Standard => {
        startup_logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), serde_json::json!("embedding.preflight_failed")),
                ("error".to_string(), serde_json::json!(err.to_string())),
            ]),
            crate::logging::LogLevel::Warn,
        );
        None
    }
    Err(err) => return Err(err),
};
let decision = if let Some(target) = target.as_ref() {
    decide_embedding_startup(
        &config.namespaces,
        &namespace_states,
        &target.signature,
        &sample_dimensions,
        &fact_counts,
        target.dimension,
    )
} else {
    EmbeddingStartupDecision::DisableSemantic {
        reason: "embedding target preflight failed".to_string(),
    }
};

startup_logger.log(
    std::collections::HashMap::from([
        ("op".to_string(), serde_json::json!("embedding.startup_decision")),
        ("decision".to_string(), serde_json::json!(format!("{:?}", decision))),
        ("namespaces".to_string(), serde_json::json!(config.namespaces.clone())),
        ("target_signature".to_string(), serde_json::json!(target.as_ref().map(|value| value.signature.clone()))),
    ]),
    crate::logging::LogLevel::Info,
);

let embedding_provider: Arc<dyn EmbeddingProvider> = match &decision {
    _ if mode == EmbeddingActivationMode::ForceEnabledForReembed => {
        create_embedding_provider_with_dimension(
            &config.embedding,
            &effective_data_dir,
            target.as_ref().expect("reembed target required").dimension,
        ).await?
    }
    EmbeddingStartupDecision::UseConfiguredProvider
    | EmbeddingStartupDecision::BootstrapReadyNamespaces { .. } => {
        let target = target.as_ref().expect("enabled embeddings require resolved target");
        create_embedding_provider_with_dimension(&config.embedding, &effective_data_dir, target.dimension).await?
    }
    EmbeddingStartupDecision::DisableSemantic { reason } => {
        startup_logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), serde_json::json!("embedding.rebuild_required")),
                ("reason".to_string(), serde_json::json!(reason)),
                ("target_signature".to_string(), serde_json::json!(target.as_ref().map(|value| value.signature.clone()))),
            ]),
            crate::logging::LogLevel::Warn,
        );
        Arc::new(DisabledEmbeddingProvider::new(
            target.as_ref().map(|value| value.dimension).unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION)
        ))
    }
};
```

In prose: normal startup may degrade to a disabled embedding provider when preflight probing fails; `ForceEnabledForReembed` must stay strict and return the preflight error instead.

Also add explicit structured events for:

- `embedding.preflight_started`
- `embedding.preflight_succeeded`
- `embedding.preflight_failed`
- `embedding.startup_state_loaded`
- `embedding.bootstrap_ready_written`

Use `debug` for detailed state payloads and `info`/`warn` for the final operator-visible outcome.

Store the active target identity on `MemoryService` for the rewrite runner:

```rust
pub(crate) current_embedding_signature: Option<String>,
pub(crate) current_embedding_model: Option<String>,
pub(crate) current_embedding_dimension: Option<usize>,
```

Populate them from `target.signature`, `target.model.clone()`, and `target.dimension` whenever embeddings are enabled.

Re-export the activation mode from `src/service.rs` so `src/cli.rs` can use it without reaching into a private module:

```rust
pub(crate) use startup::EmbeddingActivationMode;
```

When the decision is `BootstrapReadyNamespaces`, create or update `embedding_state:fact` only in the listed namespaces with:

```rust
serde_json::json!({
    "status": "ready",
    "active_signature": target_signature,
    "provider": config.embedding.provider_label(),
    "model": config.embedding.model.clone(),
    "dimension": target.dimension,
    "updated_at": chrono::Utc::now().to_rfc3339(),
})
```

- [ ] **Step 5: Run the tests again and verify they pass**

Run:

```bash
cargo test validate_applied_migration_allows_dynamic_embedding_checksum_drift_for_008 --lib
cargo test decide_embedding_startup_disables_semantic_when_any_namespace_is_rebuilding --lib
cargo test decide_embedding_startup_bootstraps_legacy_ready_when_dimensions_match --lib
cargo test decide_embedding_startup_bootstraps_missing_namespace_without_ignoring_existing_ready_state --lib
```

Expected:
- all tests PASS

- [ ] **Step 6: Commit the startup compatibility work**

Run:

```bash
git add src/storage/migrations.rs src/service/startup.rs src/service/core/builder.rs src/service/core.rs
git commit -m "fix: gate semantic startup behind embedding state"
```

---

### Task 3: Implement batch reembed orchestration with durable progress

**Files:**
- Create: `src/service/reembed.rs`
- Modify: `src/service.rs:1-60`
- Modify: `src/service/core.rs:392-450,572-606`
- Modify: `src/service/core/helpers.rs:1-120`
- Modify: `src/storage/queries.rs:1-220`
- Modify: `src/storage/client.rs:1674-1703`
- Create: `tests/embedded_reembed.rs`

- [ ] **Step 1: Write the failing integration tests for full rewrite, resume, and failure state**

Create `tests/embedded_reembed.rs` with a static provider and four end-to-end tests:

```rust
#[tokio::test]
async fn reembed_rewrites_all_facts_and_marks_job_completed() {
    let db = make_in_memory_db(&["org", "personal"]).await;
    seed_fact_with_embedding(&db, "org", "fact:one", vec![1.0, 0.0], "embsig:old").await;
    seed_fact_with_embedding(&db, "personal", "fact:two", vec![1.0, 0.0], "embsig:old").await;

    let service = make_reembed_service(db.clone(), 3).await;
    let summary = service.reembed_all_facts().await.expect("reembed should succeed");

    assert_eq!(summary.total_facts, 2);
    assert_eq!(summary.failed_facts, 0);

    let updated = db.select_one("fact:one", "org").await.unwrap().unwrap();
    assert_eq!(updated.get("embedding_dimension"), Some(&serde_json::json!(3)));
    assert!(updated.get("embedding_signature").is_some());
}

#[tokio::test]
async fn reembed_resume_after_restart_uses_persisted_job_state() {
    let db = make_in_memory_db(&["org"]).await;
    seed_fact_with_embedding(&db, "org", "fact:one", vec![1.0, 0.0], "embsig:old").await;
    seed_fact_with_embedding(&db, "org", "fact:two", vec![1.0, 0.0], "embsig:old").await;

    let interrupted = make_interrupting_reembed_service(db.clone(), 3, 1).await;
    interrupted
        .reembed_all_facts()
        .await
        .expect_err("first run should stop after one fact");

    let resumed = make_reembed_service(db.clone(), 3).await;
    let summary = resumed.reembed_all_facts().await.unwrap();

    assert_eq!(summary.succeeded_facts, 1);

    let job = db.select_one("embedding_job:fact_reembed", "org").await.unwrap().unwrap();
    assert_eq!(job.get("status"), Some(&serde_json::json!("completed")));
}

#[tokio::test]
async fn reembed_resume_after_failure_retries_failed_fact_instead_of_skipping_it() {
    let db = make_in_memory_db(&["org"]).await;
    seed_fact_with_embedding(&db, "org", "fact:one", vec![1.0, 0.0], "embsig:old").await;
    seed_fact_with_embedding(&db, "org", "fact:two", vec![1.0, 0.0], "embsig:old").await;

    let first = make_fail_on_second_fact_service(db.clone(), 3).await;
    first.reembed_all_facts().await.expect_err("first run should fail on second fact");

    let resumed = make_reembed_service(db.clone(), 3).await;
    let summary = resumed.reembed_all_facts().await.unwrap();

    assert_eq!(summary.succeeded_facts, 1);
    let updated = db.select_one("fact:two", "org").await.unwrap().unwrap();
    assert!(updated.get("embedding_signature").is_some());
}

#[tokio::test]
async fn reembed_failure_marks_job_failed_and_leaves_startup_in_lexical_only_mode() {
    let db = make_in_memory_db(&["org"]).await;
    seed_fact_with_embedding(&db, "org", "fact:bad", vec![1.0, 0.0], "embsig:old").await;

    let service = make_failing_reembed_service(db.clone()).await;
    let error = service.reembed_all_facts().await.expect_err("reembed should fail");

    assert!(error.to_string().contains("reembed failed"));

    let job = db.select_one("embedding_job:fact_reembed", "org").await.unwrap().unwrap();
    assert_eq!(job.get("status"), Some(&serde_json::json!("failed")));
}
```

- [ ] **Step 2: Run the new test file and confirm the failures are meaningful**

Run:

```bash
cargo test --test embedded_reembed -- --nocapture
```

Expected:
- `reembed_all_facts()` does not exist yet
- the bookkeeping tables or metadata fields are not fully used yet
- canonical rewrite helper is missing
- restart-safe resume path is not implemented yet
- stable cursor semantics do not prevent skipping a failed fact yet

- [ ] **Step 3: Add the shared canonical fact embedding input helper and query builders**

Factor the string formatting already used by `add_fact()` into one helper in `src/service/core.rs`:

```rust
pub(crate) fn build_fact_embedding_input(fact_type: &str, content: &str, quote: &str) -> String {
    format!("{fact_type}\n{content}\n{quote}")
}
```

Update `add_fact()` to use that helper.

Add query builders in `src/storage/queries.rs`:

```rust
pub fn build_count_facts_needing_reembed_query(target_signature: &str) -> (String, Value) {
    (
        "SELECT count() AS total FROM fact WHERE embedding_signature IS NONE OR embedding_signature != $target_signature GROUP ALL".to_string(),
        serde_json::json!({ "target_signature": target_signature }),
    )
}

pub fn build_select_facts_needing_reembed_query(
    target_signature: &str,
    last_completed_fact_id: Option<&str>,
    limit: i32,
) -> (String, Value) {
    let cursor_clause = if last_completed_fact_id.is_some() {
        "AND fact_id > $last_completed_fact_id"
    } else {
        ""
    };

    (
        format!(
            "SELECT * FROM fact WHERE (embedding_signature IS NONE OR embedding_signature != $target_signature) {cursor_clause} ORDER BY fact_id ASC LIMIT $limit"
        ),
        serde_json::json!({
            "target_signature": target_signature,
            "last_completed_fact_id": last_completed_fact_id,
            "limit": limit,
        }),
    )
}

pub fn build_drop_fact_embedding_index_query() -> String {
    "REMOVE INDEX IF EXISTS fact_embedding_hnsw ON TABLE fact".to_string()
}

pub fn build_create_fact_embedding_index_query(dimension: usize) -> String {
    format!(
        "DEFINE INDEX fact_embedding_hnsw ON TABLE fact FIELDS embedding HNSW DIMENSION {dimension}"
    )
}
```

- [ ] **Step 4: Implement `src/service/reembed.rs` with durable progress and ETA**

Use a small internal progress tracker and one public service entry point:

```rust
#[derive(Debug, Clone, Default)]
pub(crate) struct ReembedSummary {
    pub total_facts: usize,
    pub processed_facts: usize,
    pub succeeded_facts: usize,
    pub failed_facts: usize,
}

impl MemoryService {
    pub async fn reembed_all_facts(&self) -> Result<ReembedSummary, MemoryError> {
        let target_signature = self
            .current_embedding_signature
            .clone()
            .ok_or_else(|| {
                MemoryError::ConfigInvalid(
                    "reembed requires an enabled embedding signature".to_string(),
                )
            })?;
        let target_dimension = self.current_embedding_dimension.ok_or_else(|| {
            MemoryError::ConfigInvalid(
                "reembed requires a resolved target dimension".to_string(),
            )
        })?;
        let job_id = "embedding_job:fact_reembed";
        let mut summary = ReembedSummary::default();
        let started_at = std::time::Instant::now();

        self.logger.log(
            log_event(
                "reembed.job_started",
                serde_json::json!({
                    "job_id": job_id,
                    "target_signature": target_signature,
                    "target_dimension": target_dimension,
                    "namespaces": self.namespaces,
                }),
                serde_json::json!({"status": "starting"}),
                None,
                None,
                None,
            ),
            LogLevel::Info,
        );

        let existing_job = self.load_reembed_job(self.default_namespace.as_str(), job_id).await?;
        self.mark_namespaces_rebuilding(job_id, &target_signature, target_dimension)
            .await?;
        self.drop_embedding_indexes().await?;
        summary.total_facts = existing_job
            .as_ref()
            .map(|job| job.total_facts)
            .unwrap_or(self.count_facts_needing_reembed(&target_signature).await?);

        for namespace in &self.namespaces {
            let mut last_completed_fact_id = existing_job
                .as_ref()
                .and_then(|job| job.namespace_last_completed_fact_id(namespace, &target_signature));

            self.logger.log(
                log_event(
                    "reembed.namespace_started",
                    serde_json::json!({
                        "job_id": job_id,
                        "namespace": namespace,
                        "resume_cursor": last_completed_fact_id,
                    }),
                    serde_json::json!({"status": "running"}),
                    None,
                    None,
                    None,
                ),
                LogLevel::Info,
            );

            loop {
                let batch = self
                    .select_facts_needing_reembed(namespace, &target_signature, last_completed_fact_id.as_deref(), 100)
                    .await?;
                if batch.is_empty() {
                    break;
                }

                self.logger.log(
                    log_event(
                        "reembed.batch_fetched",
                        serde_json::json!({
                            "job_id": job_id,
                            "namespace": namespace,
                            "batch_size": batch.len(),
                            "after_cursor": last_completed_fact_id,
                        }),
                        serde_json::json!({"status": "ok"}),
                        None,
                        None,
                        None,
                    ),
                    LogLevel::Debug,
                );

                for record in batch {
                    let fact_id = record.get("fact_id").and_then(|v| v.as_str()).unwrap().to_string();

                    self.logger.log(
                        log_event(
                            "reembed.fact_rewrite_started",
                            serde_json::json!({
                                "job_id": job_id,
                                "namespace": namespace,
                                "fact_id": fact_id,
                            }),
                            serde_json::json!({"status": "started"}),
                            None,
                            None,
                            None,
                        ),
                        LogLevel::Trace,
                    );

                    match self.rewrite_fact_record(namespace, &record, &target_signature).await {
                        Ok(()) => {
                            summary.processed_facts += 1;
                            summary.succeeded_facts += 1;
                            last_completed_fact_id = Some(fact_id.clone());

                            self.logger.log(
                                log_event(
                                    "reembed.cursor_advanced",
                                    serde_json::json!({
                                        "job_id": job_id,
                                        "namespace": namespace,
                                        "last_completed_fact_id": last_completed_fact_id,
                                    }),
                                    serde_json::json!({"status": "ok"}),
                                    None,
                                    None,
                                    None,
                                ),
                                LogLevel::Trace,
                            );
                        }
                        Err(err) => {
                            summary.processed_facts += 1;
                            summary.failed_facts += 1;
                            self.persist_job_failure(job_id, namespace, &fact_id, &err).await?;

                            self.logger.log(
                                log_event(
                                    "reembed.fact_failed",
                                    serde_json::json!({
                                        "job_id": job_id,
                                        "namespace": namespace,
                                        "fact_id": fact_id,
                                    }),
                                    serde_json::json!({
                                        "status": "failed",
                                        "error": err.to_string(),
                                    }),
                                    None,
                                    None,
                                    None,
                                ),
                                LogLevel::Warn,
                            );
                            self.persist_job_progress(job_id, namespace, &summary, started_at.elapsed())
                                .await?;
                            self.mark_namespaces_failed(job_id, &target_signature, target_dimension)
                                .await?;
                            self.finish_job_failed(job_id, &summary).await?;
                            return Err(MemoryError::Storage(format!(
                                "reembed failed for fact {fact_id}; fix the provider and rerun `memory_mcp reembed`"
                            )));
                        }
                    }
                }

                self.persist_job_progress(job_id, namespace, &summary, started_at.elapsed())
                    .await?;
                self.log_reembed_progress(namespace, &summary, started_at.elapsed());
            }

            self.logger.log(
                log_event(
                    "reembed.namespace_completed",
                    serde_json::json!({
                        "job_id": job_id,
                        "namespace": namespace,
                        "processed_facts": summary.processed_facts,
                        "failed_facts": summary.failed_facts,
                    }),
                    serde_json::json!({"status": "done"}),
                    None,
                    None,
                    None,
                ),
                LogLevel::Info,
            );
        }

        if summary.failed_facts == 0 {
            self.create_embedding_indexes(target_dimension).await?;
            self.mark_namespaces_ready(job_id, &target_signature, target_dimension)
                .await?;
            self.finish_job_completed(job_id, &summary).await?;

            self.logger.log(
                log_event(
                    "reembed.job_completed",
                    serde_json::json!({
                        "job_id": job_id,
                        "provider": self.embedding_provider.provider_name(),
                        "model": self.current_embedding_model,
                        "target_dimension": target_dimension,
                        "target_signature": target_signature,
                        "resumed": existing_job.is_some(),
                    }),
                    serde_json::json!({
                        "status": "completed",
                        "total_facts": summary.total_facts,
                        "processed_facts": summary.processed_facts,
                        "succeeded_facts": summary.succeeded_facts,
                        "failed_facts": summary.failed_facts,
                        "facts_per_second": if started_at.elapsed().as_secs_f64() > 0.0 {
                            summary.processed_facts as f64 / started_at.elapsed().as_secs_f64()
                        } else {
                            0.0
                        },
                        "duration_ms": started_at.elapsed().as_millis() as u64,
                    }),
                    None,
                    None,
                    None,
                ),
                LogLevel::Info,
            );
            return Ok(summary);
        }

        self.mark_namespaces_failed(job_id, &target_signature, target_dimension)
            .await?;
        self.finish_job_failed(job_id, &summary).await?;

        self.logger.log(
            log_event(
                "reembed.job_failed",
                serde_json::json!({
                    "job_id": job_id,
                    "provider": self.embedding_provider.provider_name(),
                    "model": self.current_embedding_model,
                    "target_dimension": target_dimension,
                    "target_signature": target_signature,
                    "resumed": existing_job.is_some(),
                }),
                serde_json::json!({
                    "status": "failed",
                    "total_facts": summary.total_facts,
                    "processed_facts": summary.processed_facts,
                    "succeeded_facts": summary.succeeded_facts,
                    "failed_facts": summary.failed_facts,
                    "facts_per_second": if started_at.elapsed().as_secs_f64() > 0.0 {
                        summary.processed_facts as f64 / started_at.elapsed().as_secs_f64()
                    } else {
                        0.0
                    },
                    "duration_ms": started_at.elapsed().as_millis() as u64,
                }),
                None,
                None,
                None,
            ),
            LogLevel::Warn,
        );
        Err(MemoryError::Storage(format!(
            "reembed failed for {} facts; fix the provider and rerun `memory_mcp reembed`",
            summary.failed_facts
        )))
    }
}
```

Compute ETA from batch throughput:

```rust
let rate_fps = if elapsed.as_secs_f64() > 0.0 {
    summary.processed_facts as f64 / elapsed.as_secs_f64()
} else {
    0.0
};
let remaining = summary.total_facts.saturating_sub(summary.processed_facts) as f64;
let eta_seconds = if rate_fps > 0.0 {
    Some((remaining / rate_fps).ceil() as u64)
} else {
    None
};
```

Required logging invariants for this task:

- every phase transition emits one stable `op=...` event;
- per-fact success is logged at `trace` only;
- per-fact failure is logged at `warn` with `fact_id`;
- successful and failed runs emit one terminal summary event with counts, elapsed time, and target parameters;
- no event includes raw embedding vectors, API keys, or full fact content;
- every terminal error can be correlated to `job_id`, `namespace`, and current cursor.

- [ ] **Step 5: Run the focused integration tests and verify they pass**

Run:

```bash
cargo test --test embedded_reembed -- --nocapture
```

Expected:
- full rewrite test PASS
- restart-safe resume test PASS
- failed-row retry test PASS
- failure path test PASS

And add at least one focused test that asserts log payload construction for:

- `reembed.progress`
- `reembed.fact_failed`
- redaction / omission of raw embedding vectors

- [ ] **Step 6: Commit the rewrite orchestration work**

Run:

```bash
git add src/service/reembed.rs src/service.rs src/service/core.rs src/storage/queries.rs src/storage/client.rs tests/embedded_reembed.rs
git commit -m "feat: add persisted reembed maintenance runner"
```

---

### Task 4: Add the `reembed` CLI mode and process-level logging

**Files:**
- Modify: `src/cli.rs:33-195`
- Modify: `src/main.rs:1-24`
- Test: `src/cli.rs`

- [ ] **Step 1: Write the failing parser tests for the new CLI mode**

Add these tests to `src/cli.rs`:

```rust
#[test]
fn parse_cli_args_builds_reembed_mode() {
    let mode = parse_cli_args([
        "memory_mcp".to_string(),
        "reembed".to_string(),
    ])
    .expect("reembed mode should parse");

    assert_eq!(mode, RunMode::Reembed);
}

#[test]
fn parse_cli_args_rejects_extra_reembed_args() {
    let error = parse_cli_args([
        "memory_mcp".to_string(),
        "reembed".to_string(),
        "--unexpected".to_string(),
    ])
    .expect_err("reembed should reject additional args in v1");

    assert!(error.contains("reembed does not accept additional arguments"));
}
```

- [ ] **Step 2: Run the parser tests and confirm they fail for the expected reason**

Run:

```bash
cargo test parse_cli_args_builds_reembed_mode --lib
cargo test parse_cli_args_rejects_extra_reembed_args --lib
```

Expected:
- `RunMode::Reembed` does not exist yet
- `parse_cli_args()` still treats `reembed` as an unknown subcommand

- [ ] **Step 3: Implement `RunMode::Reembed`, handler dispatch, and structured logs**

Update `src/cli.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunMode {
    Serve,
    Watch(WatchCommand),
    Reembed,
}
```

Parse the new mode:

```rust
if subcommand == "reembed" {
    if args.next().is_some() {
        return Err("reembed does not accept additional arguments".to_string());
    }
    return Ok(RunMode::Reembed);
}
```

Add the handler:

```rust
pub async fn run_reembed_mode(
    logger: &StdoutLogger,
) -> Result<(), Box<dyn std::error::Error>> {
    let started_at = std::time::Instant::now();
    logger.log(event!("op" => json!("main.reembed_starting")), LogLevel::Info);

    let memory_service = MemoryService::new_from_env_with_mode(
        crate::service::EmbeddingActivationMode::ForceEnabledForReembed,
    )
    .await
    .map_err(|err| log_and_return_error(logger, "main.reembed_startup_failed", err))?;
    let summary = memory_service
        .reembed_all_facts()
        .await
        .map_err(|err| log_and_return_error(logger, "main.reembed_failed", err))?;

    logger.log(
        event!(
            "op" => json!("main.reembed_completed"),
            "processed_facts" => json!(summary.processed_facts),
            "failed_facts" => json!(summary.failed_facts),
            "facts_per_second" => json!(if started_at.elapsed().as_secs_f64() > 0.0 {
                summary.processed_facts as f64 / started_at.elapsed().as_secs_f64()
            } else {
                0.0
            }),
            "duration_ms" => json!(started_at.elapsed().as_millis() as u64)
        ),
        LogLevel::Info,
    );

    Ok(())
}
```

Also require process-level events for:

- `main.reembed_starting`
- `main.reembed_startup_failed`
- `main.reembed_failed`
- `main.reembed_completed`

These should summarize the run without duplicating the lower-level per-namespace and per-fact detail.

`main.reembed_completed` should be intentionally short, while `reembed.job_completed` carries the richer maintenance summary.

Update `src/main.rs`:

```rust
let mode_label = match &run_mode {
    RunMode::Serve => "serve",
    RunMode::Watch(_) => "watch",
    RunMode::Reembed => "reembed",
};

match run_mode {
    RunMode::Serve => run_stdio_server(&logger).await?,
    RunMode::Watch(watch) => run_watch_mode(&logger, watch).await?,
    RunMode::Reembed => run_reembed_mode(&logger).await?,
}
```

- [ ] **Step 4: Run the parser tests again and verify they pass**

Run:

```bash
cargo test parse_cli_args_builds_reembed_mode --lib
cargo test parse_cli_args_rejects_extra_reembed_args --lib
```

Expected:
- both tests PASS

- [ ] **Step 5: Commit the CLI integration**

Run:

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: expose reembed as a dedicated CLI mode"
```

---

### Task 5: Update operator docs and verify the whole feature

**Files:**
- Modify: `README.md`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Verify only: repository root

- [ ] **Step 1: Update `README.md` with the operator workflow**

Add a short maintenance section near the existing provider-switching docs:

```md
#### Full embedding rebuild after provider or model changes

If you change `EMBEDDINGS_PROVIDER`, `EMBEDDINGS_MODEL`, or an explicit `SURREALDB_EMBEDDING_DIMENSION` override, restart-time configuration alone is not enough to make old semantic vectors correct.

If `SURREALDB_EMBEDDING_DIMENSION` is unset, the server resolves the target dimension automatically from model metadata or a one-shot provider probe. Keep the variable only when you need an explicit validation override.

Run the dedicated maintenance mode before returning the server to normal use: `memory_mcp reembed`

What it does:
- rewrites every `fact.embedding` across every configured namespace;
- records progress, phase transitions, and failure context in structured logs;
- emits a final summary log with counts, elapsed time, throughput, and target parameters;
- recreates the HNSW index for the current dimension only after a fully successful run.

If the command fails because of a disconnect, reboot, OOM, or another interruption, fix the underlying issue and run `memory_mcp reembed` again. The new process resumes from persisted job state and also skips already rewritten facts for the same embedding signature.
```

- [ ] **Step 2: Update `docs/MEMORY_SYSTEM_SPEC.md` with startup gating semantics**

Add a runtime note that normal startup disables semantic retrieval when embedding state is not ready:

```md
### Embedding state safety

The server enables semantic retrieval only when every configured namespace is known-ready for the current embedding signature.

If any namespace is in `rebuilding`, `failed`, or `signature mismatch` state, the process remains available but falls back to lexical and graph retrieval only. Operators must run `memory_mcp reembed` to restore semantic retrieval after an embedding provider/model switch.

All startup and maintenance transitions are emitted as structured logs so operators can trace the decision path without adding ad hoc debug prints.
```

- [ ] **Step 3: Run formatting and verification**

Run:

```bash
cargo fmt --all --check
cargo test parse_cli_args_builds_reembed_mode --lib
cargo test --test embedded_reembed -- --nocapture
cargo test -q
```

Expected:
- formatting check PASS
- focused library test PASS
- rewrite integration test PASS
- full test suite PASS

- [ ] **Step 4: Inspect the final diff shape before merge**

Run:

```bash
git diff -- migrations/019_embedding_rebuild_maintenance.surql src/config/embedding.rs src/storage/migrations.rs src/service/startup.rs src/service/core/builder.rs src/service/core.rs src/storage/queries.rs src/storage/client.rs src/service/reembed.rs src/service.rs src/cli.rs src/main.rs tests/embedded_reembed.rs README.md docs/MEMORY_SYSTEM_SPEC.md
```

Expected:
- only the planned files are changed
- no historical migration file is modified
- no MCP handler / params surface was widened unnecessarily

- [ ] **Step 5: Commit the docs and verified implementation**

Run:

```bash
git add README.md docs/MEMORY_SYSTEM_SPEC.md
git commit -m "docs: describe reembed maintenance workflow"
```

---

## Self-review checklist

- [ ] The plan never edits an existing migration file.
- [ ] The plan rewrites **all** facts, not only active facts.
- [ ] The plan keeps the MCP surface unchanged.
- [ ] The plan adds startup safety when the stored embedding corpus is not known-ready.
- [ ] The plan auto-detects target dimension when no explicit override is provided.
- [ ] The plan includes progress, counts, throughput, and ETA.
- [ ] The plan resumes safely after interruption from persisted DB state, not only from same-process reruns.
- [ ] The plan does not advance the stable cursor past a failed fact.
- [ ] The plan requires structured logs for startup decisions, resume, cursor movement, and terminal failures.
- [ ] The plan requires a terminal summary log with counts, elapsed time, throughput, and target parameters.
- [ ] The plan explicitly avoids logging secrets, raw embeddings, or whole source content.
- [ ] The plan reuses one canonical embedding input helper across write and rewrite paths.
- [ ] The plan preserves repository-fit simplicity: one maintenance command, one rewrite runner, additive schema only.
- [ ] Every code snippet uses symbols that are introduced in this plan and named consistently across tasks.
