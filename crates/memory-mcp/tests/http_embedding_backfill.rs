#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

//! HTTP-profile embedding maintenance: the activation-path index reconcile and
//! the automatic per-tenant backfill tick.
//!
//! Two things live here because they are the two halves of one boundary. The
//! reconcile must not touch a namespace that already holds vectors — those
//! belong to an operator reembed, and re-declaring the index under them
//! strands the namespace in a state nothing can exit. The backfill tick must
//! fill the gaps in a namespace that holds none, without ever rewriting an
//! existing vector.
//!
//! Run:
//! cargo test -p memory_mcp --tests --features streamable-http,mcp-apps,control-plane,test-fixtures \
//!     --test http_embedding_backfill

use std::sync::Arc;

use memory_mcp::http::embedding::backfill_scheduler::backfill_scheduler_job;
use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
use memory_mcp::http::runtime::bootstrap::DeploymentPolicy;
use memory_mcp::http::runtime::storage::{
    EmbeddingPolicy, RuntimeOptions, build_runtime_with_options,
};
use memory_mcp::storage::{DbClient, SurrealDbClient};

/// An enabled provider that answers deterministically and offline.
///
/// It counts its own `embed` calls. A tick that declines to touch a tenant and
/// a tick that attempts the write and watches it fail both leave the fact
/// without a vector, so the stored row alone cannot tell them apart — the call
/// count can, and it is the cost a real deployment would have paid.
struct StaticEmbeddingProvider {
    dimension: usize,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

fn static_provider(dimension: usize) -> Arc<StaticEmbeddingProvider> {
    Arc::new(StaticEmbeddingProvider {
        dimension,
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    })
}

#[async_trait::async_trait]
impl memory_mcp::embedding::providers::EmbeddingProvider for StaticEmbeddingProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "openai-compatible"
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f64>, memory_mcp::error::MemoryError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(vec![0.0; self.dimension])
    }
}

fn embedding_policy(dimension: usize) -> EmbeddingPolicy {
    embedding_policy_for(static_provider(dimension), dimension)
}

fn embedding_policy_for(
    provider: Arc<StaticEmbeddingProvider>,
    dimension: usize,
) -> EmbeddingPolicy {
    EmbeddingPolicy {
        provider,
        dimension,
        signature: memory_mcp::config::build_embedding_signature(
            "openai-compatible",
            Some("test-model"),
            Some("https://embeddings.invalid/v1"),
            dimension,
        ),
        model: Some("test-model".to_string()),
        provider_label: "openai-compatible",
    }
}

/// The deployment policy the backfill job is registered with.
fn deployment_policy(dimension: usize, auto_recovery: bool) -> DeploymentPolicy {
    deployment_policy_from(embedding_policy(dimension), auto_recovery)
}

fn deployment_policy_from(embedding: EmbeddingPolicy, auto_recovery: bool) -> DeploymentPolicy {
    DeploymentPolicy {
        embedding: Some(embedding),
        entity_extractor: None,
        lifecycle: memory_mcp::config::LifecycleConfig::default(),
        cache_limits: memory_mcp::config::CacheLimits::profile_default(),
        auto_recovery,
        query_logging_enabled: false,
        query_log_retention_days: memory_mcp::config::DEFAULT_QUERY_LOG_RETENTION_DAYS,
        embedding_similarity_threshold: memory_mcp::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        claim_config: memory_mcp::config::claims::ClaimConfig::default(),
    }
}

fn tenant(namespace: &str) -> Tenant {
    Tenant {
        id: "ten_backfill".to_string(),
        status: TenantStatus::Ready,
        namespace_binding: NamespaceBinding {
            namespace: namespace.to_string(),
            database: "memory".into(),
        },
        plan_version: 1,
        schema_version: 0,
        retry_stage: None,
        provisioning_lease: None,
        created_at: chrono::Utc::now(),
        version: 0,
    }
}

async fn seed_fact_with_vector(
    db: &SurrealDbClient,
    namespace: &str,
    fact_id: &str,
    dimension: usize,
) {
    let now = memory_mcp::shared::temporal::normalize_dt(chrono::Utc::now());
    db.create(
        fact_id,
        serde_json::json!({
            "fact_id": fact_id,
            "fact_type": "note",
            "content": format!("content {fact_id}"),
            "quote": format!("content {fact_id}"),
            "source_episode": "episode:seed",
            "t_valid": now,
            "t_ingested": now,
            "confidence": 0.9,
            "index_keys": [],
            "access_count": 0,
            "entity_links": [],
            "scope": namespace,
            "policy_tags": [],
            "provenance": {"source_episode": "episode:seed"},
            "embedding": vec![0.1f64; dimension],
            "embedding_provider": "legacy-test",
            "embedding_model": "legacy-model",
            "embedding_dimension": dimension,
            "embedding_signature": "embsig:legacy",
            "embedding_updated_at": now,
        }),
        namespace,
        memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS,
    )
    .await
    .expect("seed a fact carrying a vector");
}

/// The declared width of the namespace's `fact_embedding_hnsw` index, read
/// the way the schema reports it.
///
/// The production reader of this lives on `ReembedStoreClient`, which is
/// crate-private, so the test reads the same `INFO FOR TABLE` statement over
/// the public `DbClient::query` surface. Two readers of one schema statement
/// is acceptable here precisely because the test asserts the *schema* rather
/// than a code path: if the reconcile re-declared the index, this number moves.
async fn declared_index_dimension(db: &SurrealDbClient, namespace: &str) -> Option<usize> {
    let raw = db
        .query("INFO FOR TABLE fact", None, namespace)
        .await
        .expect("index probe");
    // `DbClient::query` returns the result set as an array of records; the
    // `INFO FOR TABLE` payload is the first one.
    let record = raw.as_array()?.first()?;
    // SurrealDB renders each index as its own `DEFINE` statement, so the width
    // is a number in text rather than a field.
    let define = record
        .get("indexes")
        .and_then(|indexes| indexes.get("fact_embedding_hnsw"))
        .and_then(|value| value.as_str())?
        .to_string();
    let after = &define[define.find("DIMENSION ")? + "DIMENSION ".len()..];
    let width: String = after.chars().take_while(char::is_ascii_digit).collect();
    width.parse().ok()
}

/// An operator who changed provider dimension gets a namespace whose vectors
/// were written by the previous provider. Activation must leave the index
/// alone: re-declaring it at the new width while old-width vectors sit inside
/// is the state only a reembed can exit, and activation is not a reembed.
#[tokio::test]
async fn activation_does_not_redeclare_the_index_of_a_namespace_that_holds_vectors() {
    let namespace = format!("tns_backfill_reconcile_{}", uuid::Uuid::new_v4().simple());
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    // `bind_to_test_namespace` uses the default-dimension constructor, which
    // is exactly what an existing tenant has: its migrations rendered
    // `fact_embedding_hnsw` at 1536.
    let provisioned = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(&namespace)
        .await;
    provisioned
        .apply_migrations(&namespace)
        .await
        .expect("apply tenant migrations");
    seed_fact_with_vector(&provisioned, &namespace, "fact:one", 1536).await;
    seed_fact_with_vector(&provisioned, &namespace, "fact:two", 1536).await;

    let options = RuntimeOptions::default().with_embedding_policy(embedding_policy(2048));
    let runtime = build_runtime_with_options(&registry, &tenant(&namespace), options)
        .await
        .expect("runtime activation must succeed");

    // The index must keep its original width while old-width vectors remain:
    // re-declaring it would strand the namespace in a state only reembed can exit.
    assert_eq!(
        declared_index_dimension(&runtime.tenant_db, &namespace).await,
        Some(1536)
    );
}

/// A tenant whose facts were written while the provider was unreachable holds
/// no vectors at all. One backfill tick must fill every gap at the deployment
/// dimension, leave the already-correct index alone, and clear the durable
/// `backfill_pending` marker — without which the namespace keeps reporting
/// itself as not-ready forever.
#[tokio::test]
async fn one_backfill_tick_fills_every_missing_vector_at_the_deployment_dimension() {
    let namespace = format!("tns_backfill_tick_{}", uuid::Uuid::new_v4().simple());
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let provisioned = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(&namespace)
        .await;
    provisioned
        .apply_migrations(&namespace)
        .await
        .expect("apply tenant migrations");
    seed_fact_without_vector(&provisioned, &namespace, "fact:one").await;
    seed_fact_without_vector(&provisioned, &namespace, "fact:two").await;
    seed_fact_without_vector(&provisioned, &namespace, "fact:three").await;
    // The namespace is registered as ready, so the tick's bounded tenant walk
    // actually reaches it; a registry with no ready tenant would pass a
    // backfill assertion without running any backfill.
    registry
        .tenants()
        .write_tenant(&tenant(&namespace))
        .await
        .expect("register the tenant as ready");
    let index_before = declared_index_dimension(&provisioned, &namespace).await;

    let policy = embedding_policy(1536);
    let job = backfill_scheduler_job(deployment_policy(1536, true));
    job(registry).await.expect("one backfill tick must succeed");

    for fact_id in ["fact:one", "fact:two", "fact:three"] {
        let fact = provisioned
            .select_one(fact_id, &namespace)
            .await
            .expect("read the backfilled fact")
            .expect("the fact is still there");
        assert_eq!(
            fact.get("embedding_dimension")
                .and_then(serde_json::Value::as_u64),
            Some(1536),
            "{fact_id} must gain a vector at the deployment dimension"
        );
        assert_eq!(
            fact.get("embedding_signature")
                .and_then(serde_json::Value::as_str),
            Some(policy.signature.as_str()),
            "{fact_id} must carry the deployment's signature"
        );
    }

    assert_eq!(
        declared_index_dimension(&provisioned, &namespace).await,
        index_before,
        "an index already at the deployment width is left as it is — backfill \
         repairs a wrong one only when no vector stands under it"
    );
    assert_eq!(
        read_embedding_state(&provisioned, &namespace).await,
        Some("ready".to_string()),
        "a completed backfill must stop reporting the namespace as pending"
    );
}

/// `EMBEDDINGS_AUTO_RECOVERY` is the deployment's opt-in, and an operator who
/// enabled embeddings but left recovery off must not have every tenant scanned
/// on every scheduler tick — and must not have any fact rewritten by it either.
///
/// The line the tick emits about itself is asserted by a unit test in
/// `http::embedding::backfill_scheduler`, which can reach the crate-private
/// log capture this file cannot.
#[tokio::test]
async fn the_tick_writes_nothing_when_auto_recovery_is_off() {
    let namespace = format!("tns_backfill_off_{}", uuid::Uuid::new_v4().simple());
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let provisioned = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(&namespace)
        .await;
    provisioned
        .apply_migrations(&namespace)
        .await
        .expect("apply tenant migrations");
    seed_fact_without_vector(&provisioned, &namespace, "fact:one").await;
    registry
        .tenants()
        .write_tenant(&tenant(&namespace))
        .await
        .expect("register the tenant as ready");

    let job = backfill_scheduler_job(deployment_policy(1536, false));

    let observed = job(registry).await;

    assert!(observed.is_ok(), "a disabled tick is a successful no-op");
    let fact = provisioned
        .select_one("fact:one", &namespace)
        .await
        .expect("read the untouched fact")
        .expect("the fact is still there");
    assert!(
        fact.get("embedding").is_none_or(|value| value.is_null()),
        "recovery off means the fact keeps no vector: {fact}"
    );
}

/// A tenant whose stored vectors were written at another provider's dimension
/// is Class B: only an operator's reembed may touch it.
///
/// Backfill would write a 2048-wide vector into a 1536-wide index, which the
/// database rejects — but only after the provider has been paid for the call.
/// On a scheduler tick that repeats for every affected tenant, forever, and
/// reports the job degraded every time. So the tick must decline before it
/// spends anything.
///
/// The assertion is the provider's call count, not the stored row: a write
/// that fails and a write that was never attempted both leave the fact without
/// a vector, so the row alone would let a broken implementation pass.
#[tokio::test]
async fn the_tick_declines_a_tenant_whose_vectors_are_at_another_dimension() {
    let namespace = format!("tns_backfill_foreign_{}", uuid::Uuid::new_v4().simple());
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    // The default-dimension constructor renders the index at 1536, which is
    // what a tenant provisioned by an earlier deployment looks like.
    let provisioned = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(&namespace)
        .await;
    provisioned
        .apply_migrations(&namespace)
        .await
        .expect("apply tenant migrations");
    seed_fact_with_vector(&provisioned, &namespace, "fact:old", 1536).await;
    seed_fact_without_vector(&provisioned, &namespace, "fact:gap").await;
    registry
        .tenants()
        .write_tenant(&tenant(&namespace))
        .await
        .expect("register the tenant as ready");

    // The deployment wants 2048; the namespace stores 1536.
    let provider = static_provider(2048);
    let calls = provider.calls.clone();
    let policy = deployment_policy_from(embedding_policy_for(provider, 2048), true);

    let job = backfill_scheduler_job(policy);
    job(registry).await.expect("one backfill tick");

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a tenant whose vectors are at another dimension is a reembed's job, \
         so the tick must decline it before spending a provider call"
    );
    let fact = provisioned
        .select_one("fact:gap", &namespace)
        .await
        .expect("read the untouched fact")
        .expect("the fact is still there");
    assert!(
        fact.get("embedding").is_none_or(|value| value.is_null()),
        "declining means no vector was written: {fact}"
    );
}

/// The durable `embedding_state` status, read over the public client surface.
///
/// `load_embedding_state` is crate-private, so the test reads the row the
/// scheduler wrote rather than reaching through the same seam the code does —
/// which is the point: the assertion is about what a tenant runtime would
/// observe, not about which function wrote it.
async fn read_embedding_state(db: &SurrealDbClient, namespace: &str) -> Option<String> {
    db.select_one("embedding_state:fact", namespace)
        .await
        .expect("read embedding state")
        .and_then(|record| {
            record
                .get("status")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

/// Seed a fact that has no `embedding` field at all: the state an outage, or a
/// deployment that enabled embeddings after ingest, leaves behind.
async fn seed_fact_without_vector(db: &SurrealDbClient, namespace: &str, fact_id: &str) {
    let now = memory_mcp::shared::temporal::normalize_dt(chrono::Utc::now());
    db.create(
        fact_id,
        serde_json::json!({
            "fact_id": fact_id,
            "fact_type": "note",
            "content": format!("content {fact_id}"),
            "quote": format!("content {fact_id}"),
            "source_episode": "episode:seed",
            "t_valid": now,
            "t_ingested": now,
            "confidence": 0.9,
            "index_keys": [],
            "access_count": 0,
            "entity_links": [],
            "scope": namespace,
            "policy_tags": [],
            "provenance": {"source_episode": "episode:seed"},
        }),
        namespace,
        memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS,
    )
    .await
    .expect("seed a fact carrying no vector");
}
