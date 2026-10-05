#![cfg(all(
    feature = "streamable-http",
    feature = "control-plane",
    feature = "test-fixtures"
))]

//! HTTP-profile reembed: the operator-triggered whole-namespace rewrite.
//!
//! Backfill (`http_embedding_backfill`) fills gaps. This file covers the
//! irreversible half — a pass that rewrites *every* vector and owns the HNSW
//! index — driven end to end through the durable task the operator route
//! enqueues.
//!
//! The test that matters here is the degraded tenant. A namespace whose stored
//! vectors were written by another provider is exactly the namespace whose
//! *runtime* provider the activation path disables (`resolve_tenant_embedding`
//! downgrades it), because serving it would write vectors its own index cannot
//! accept. The tenant that most needs a reembed is therefore the one tenant
//! whose serving provider is disabled — so the executor must build its own
//! force-enabled service rather than reuse the runtime's degraded one. If that
//! step is skipped, that test goes red and the other two stay green, which is
//! precisely the discrimination it exists to provide.
//!
//! Run:
//! cargo test -p memory_mcp --tests --features streamable-http,mcp-apps,control-plane,test-fixtures \
//!     --test http_embedding_reembed

use std::sync::Arc;

use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::runtime::storage::EmbeddingPolicy;
use memory_mcp::http::tasks::DurableTaskTestDriver;
use memory_mcp::http::tasks::scheduler::{
    execute_one_task_with_policy, execute_reembed_task_for_test,
};
use memory_mcp::http::tasks::state::{TASK_KIND_REEMBED, TaskState};
use memory_mcp::http::tasks::worker::DurableTaskStore;
use memory_mcp::storage::{BoundDbClient, DbClient, SurrealDbClient};

/// The dimension every pass in this file writes at. Deliberately not the
/// default 1536 the tenant migrations render, so an index or vector that was
/// never rewritten is visible as such rather than coincidentally correct.
const TARGET_DIMENSION: usize = 2048;

/// A deterministic, enabled provider: same text in, same vector out, no
/// network. Its dimension is the only thing that varies.
struct StaticEmbeddingProvider {
    dimension: usize,
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

    async fn embed(&self, input: &str) -> Result<Vec<f64>, memory_mcp::error::MemoryError> {
        // Non-zero and content-derived, so "was this fact rewritten by *this*
        // pass" is answerable from the stored vector alone rather than only
        // from the metadata columns.
        let seed = input
            .chars()
            .fold(0.25f64, |acc, ch| (acc + f64::from(ch as u32 % 97)) % 1.0);
        Ok(vec![seed; self.dimension])
    }
}

fn embedding_policy(dimension: usize) -> EmbeddingPolicy {
    EmbeddingPolicy {
        provider: Arc::new(StaticEmbeddingProvider { dimension }),
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

/// One ready tenant namespace with the storage schema and the durable task
/// table applied, plus the two stores the tick and the assertions use.
struct ReembedHarness {
    driver: DurableTaskTestDriver,
    store: DurableTaskStore,
    db: Arc<SurrealDbClient>,
    namespace: String,
    _registry: RegistryHandle,
}

async fn harness() -> ReembedHarness {
    let namespace = format!("tns_reembed_{}", uuid::Uuid::new_v4().simple());
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let db = registry
        .tenant_engine_optional()
        .expect("in-memory engine wired")
        .bind_to_test_namespace(&namespace)
        .await;
    let driver = DurableTaskTestDriver::new_with_options(
        Arc::new(BoundDbClient::new(db.clone(), namespace.clone())),
        "ten_reembed".to_string(),
        3600,
        16,
    );
    // Storage migrations render the `fact` table and its 1536-wide HNSW index;
    // the driver additionally applies the HTTP `tenant_task` migrations.
    driver
        .apply_migrations_for_test(&namespace)
        .await
        .expect("apply tenant migrations");
    let store = DurableTaskStore::new_with_options(
        Arc::new(BoundDbClient::new(db.clone(), namespace.clone())),
        "ten_reembed".to_string(),
        3600,
        16,
    );
    ReembedHarness {
        driver,
        store,
        db,
        namespace,
        _registry: registry,
    }
}

/// Run exactly one reembed tick against the harness's tenant.
async fn one_tick(h: &ReembedHarness, policy: &EmbeddingPolicy) {
    execute_reembed_task_for_test(&h.store, h.db.clone(), &h.namespace, policy)
        .await
        .expect("one reembed tick");
}

/// A fact carrying no vector at all: the state an outage, or a deployment that
/// enabled embeddings after ingest, leaves behind.
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

/// A fact carrying a vector another provider wrote: the Class B case. The
/// width is the default 1536 so the seed is accepted by the provisioned index,
/// and the signature is deliberately not the target's.
async fn seed_fact_with_foreign_vector(
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
            "embedding_signature": "embsig:previous-provider",
            "embedding_updated_at": now,
        }),
        namespace,
        memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS,
    )
    .await
    .expect("seed a fact carrying a foreign vector");
}

/// The declared width of the namespace's `fact_embedding_hnsw` index, read the
/// way the schema reports it — the same `INFO FOR TABLE` read the backfill
/// suite makes, duplicated here because neither test can reach the crate-private
/// `ReembedStoreClient`. Asserting the schema, not a code path, is the point:
/// if reembed fails to re-declare the index, this number moves.
async fn declared_index_dimension(db: &SurrealDbClient, namespace: &str) -> Option<usize> {
    let raw = db
        .query("INFO FOR TABLE fact", None, namespace)
        .await
        .expect("index probe");
    let record = raw.as_array()?.first()?;
    let define = record
        .get("indexes")
        .and_then(|indexes| indexes.get("fact_embedding_hnsw"))
        .and_then(|value| value.as_str())?
        .to_string();
    let after = &define[define.find("DIMENSION ")? + "DIMENSION ".len()..];
    let width: String = after.chars().take_while(char::is_ascii_digit).collect();
    width.parse().ok()
}

/// The stored vector width and signature of one fact.
async fn stored_vector(
    db: &SurrealDbClient,
    namespace: &str,
    fact_id: &str,
) -> (Option<u64>, Option<String>) {
    let fact = db
        .select_one(fact_id, namespace)
        .await
        .expect("read the fact")
        .expect("the fact is still there");
    let dimension = fact
        .get("embedding_dimension")
        .and_then(serde_json::Value::as_u64);
    let signature = fact
        .get("embedding_signature")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let vector_len = fact
        .get("embedding")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len);
    assert_eq!(
        vector_len.map(|len| len as u64),
        dimension,
        "{fact_id}: the stored vector's length and its recorded dimension must agree"
    );
    (dimension, signature)
}

/// The durable `embedding_state` status, read over the public client surface
/// rather than through the seam the pass writes it with.
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

async fn enqueue(h: &ReembedHarness, fingerprint: &str) -> String {
    h.driver
        .enqueue(
            TASK_KIND_REEMBED,
            fingerprint,
            // The wire form the operator route writes, so the executor's
            // option parsing is exercised against the real durable payload.
            serde_json::json!({ "max_failures": null, "retry_failed": false }),
        )
        .await
        .expect("enqueue a reembed task")
}

/// An operator asked for a pass on a namespace whose facts hold no vector at
/// all. One tick must rewrite every fact at the deployment's width and
/// signature, leave the namespace `ready`, re-declare the HNSW index at that
/// width, and commit the durable row as completed.
#[tokio::test]
async fn a_reembed_task_rewrites_every_fact_and_redefines_the_index() {
    let h = harness().await;
    seed_fact_without_vector(&h.db, &h.namespace, "fact:one").await;
    seed_fact_without_vector(&h.db, &h.namespace, "fact:two").await;
    let task_id = enqueue(&h, "reembed:class-a").await;
    let policy = embedding_policy(TARGET_DIMENSION);

    one_tick(&h, &policy).await;

    for fact_id in ["fact:one", "fact:two"] {
        let (dimension, signature) = stored_vector(&h.db, &h.namespace, fact_id).await;
        assert_eq!(
            dimension,
            Some(TARGET_DIMENSION as u64),
            "{fact_id} must gain a vector at the deployment dimension"
        );
        assert_eq!(
            signature.as_deref(),
            Some(policy.signature.as_str()),
            "{fact_id} must carry the deployment's signature"
        );
    }
    assert_eq!(
        read_embedding_state(&h.db, &h.namespace).await.as_deref(),
        Some("ready"),
        "a completed pass must leave the namespace semantically ready"
    );
    assert_eq!(
        declared_index_dimension(&h.db, &h.namespace).await,
        Some(TARGET_DIMENSION),
        "reembed owns the index and must re-declare it at the target width"
    );

    let record = h
        .driver
        .load(&task_id)
        .await
        .expect("load the task row")
        .expect("the row is present");
    assert_eq!(record.state, TaskState::Completed);
    assert_eq!(
        record
            .result
            .as_ref()
            .and_then(|result| result.get("outcome"))
            .and_then(serde_json::Value::as_str),
        Some("completed"),
        "the durable result must name the pass's outcome: {record:?}"
    );
    assert_eq!(
        record
            .result
            .as_ref()
            .and_then(|result| result.get("succeeded_facts"))
            .and_then(serde_json::Value::as_u64),
        Some(2),
        "both facts were rewritten: {record:?}"
    );
}

/// The test this whole task exists for.
///
/// A namespace holding vectors another provider wrote is, by
/// `resolve_tenant_embedding`, a tenant whose *serving* provider is disabled:
/// its stored vectors disagree with the deployment's signature, so serving it
/// would write vectors its own index rejects. That tenant is exactly the one
/// that needs a reembed — and it is the one whose runtime cannot provide a
/// provider. So the executor must build its own force-enabled service from the
/// deployment policy rather than inherit the namespace's degraded one.
///
/// If the force-enabled step is dropped, this test goes red with the vector
/// still at 1536 under the old signature, while the vector-less namespace
/// above stays green.
#[tokio::test]
async fn a_reembed_task_rewrites_a_namespace_whose_runtime_provider_is_degraded() {
    let h = harness().await;
    seed_fact_with_foreign_vector(&h.db, &h.namespace, "fact:one", 1536).await;
    let task_id = enqueue(&h, "reembed:class-b").await;
    let policy = embedding_policy(TARGET_DIMENSION);

    one_tick(&h, &policy).await;

    let (dimension, signature) = stored_vector(&h.db, &h.namespace, "fact:one").await;
    assert_eq!(
        dimension,
        Some(TARGET_DIMENSION as u64),
        "a degraded tenant's vector must be rewritten at the deployment width, \
         even though the namespace's own runtime provider is disabled"
    );
    assert_eq!(
        signature.as_deref(),
        Some(policy.signature.as_str()),
        "the rewritten vector must carry the deployment's signature"
    );
    assert_eq!(
        declared_index_dimension(&h.db, &h.namespace).await,
        Some(TARGET_DIMENSION),
        "the index must follow the vectors, not the other way round"
    );

    let record = h
        .driver
        .load(&task_id)
        .await
        .expect("load the task row")
        .expect("the row is present");
    assert_eq!(record.state, TaskState::Completed);
}

/// A second operator pass over a namespace the first pass just brought current
/// has nothing to do. It must say so, rewrite no fact, and leave the index
/// exactly where the first pass put it — a second pass that re-declared the
/// index for no reason would churn a namespace's schema on every retry.
#[tokio::test]
async fn a_second_pass_over_an_already_current_namespace_does_nothing() {
    let h = harness().await;
    seed_fact_without_vector(&h.db, &h.namespace, "fact:one").await;
    seed_fact_with_foreign_vector(&h.db, &h.namespace, "fact:two", 1536).await;
    let policy = embedding_policy(TARGET_DIMENSION);
    // The first pass brings the namespace current: one fact had no vector, one
    // was written by another provider. Both must now agree with the deployment.
    enqueue(&h, "reembed:idempotence-first").await;
    one_tick(&h, &policy).await;
    let after_first = stored_vector(&h.db, &h.namespace, "fact:two").await;
    assert_eq!(
        after_first.0,
        Some(TARGET_DIMENSION as u64),
        "the first pass must have rewritten the foreign vector, or this test \
         would assert a no-op over a namespace that was never current"
    );
    let index_after_first = declared_index_dimension(&h.db, &h.namespace).await;

    // A second, distinct fingerprint: the first task is terminal, and reusing
    // its fingerprint would dedupe into the same row rather than run a pass.
    let second_id = enqueue(&h, "reembed:class-b-retry").await;
    one_tick(&h, &policy).await;

    assert_eq!(
        stored_vector(&h.db, &h.namespace, "fact:two").await,
        after_first,
        "a pass with nothing to do must not rewrite a fact"
    );
    assert_eq!(
        declared_index_dimension(&h.db, &h.namespace).await,
        index_after_first,
        "a pass with nothing to do must leave the index alone"
    );

    let record = h
        .driver
        .load(&second_id)
        .await
        .expect("load the second task row")
        .expect("the row is present");
    assert_eq!(record.state, TaskState::Completed);
    assert_eq!(
        record
            .result
            .as_ref()
            .and_then(|result| result.get("outcome"))
            .and_then(serde_json::Value::as_str),
        Some("nothing_to_do"),
        "an already-current namespace is a no-op, not a rewrite: {record:?}"
    );
    assert_eq!(
        record
            .result
            .as_ref()
            .and_then(|result| result.get("processed_facts"))
            .and_then(serde_json::Value::as_u64),
        Some(0),
        "no fact may be counted as processed by a no-op pass: {record:?}"
    );
}

/// The composition neither neighbour covers: a claimed row travelling claim →
/// `kind` dispatch → the real executor → a fenced completion, *while carrying
/// the deployment's policy*.
///
/// The two halves are tested separately and each passes on its own terms:
/// `task_scheduler` dispatches with no policy and asserts the row fails loudly,
/// and every other test here calls the executor directly, bypassing dispatch
/// altogether. A change that stopped threading the policy through the `match`
/// arm would keep both green — the row would fail with "no deployment embedding
/// policy is configured" while every test here still passed, because none of
/// them reaches the dispatch.
///
/// This is the seam the operator route actually depends on: `POST /reembed`
/// enqueues a row, and a later tick must claim it and find the policy the
/// composition root captured at startup.
#[tokio::test]
async fn a_claimed_reembed_row_completes_through_the_full_dispatch_with_a_policy() {
    let h = harness().await;
    seed_fact_without_vector(&h.db, &h.namespace, "fact:one").await;
    let task_id = enqueue(&h, "reembed:full-dispatch").await;
    let policy = embedding_policy(TARGET_DIMENSION);

    // The extractor is never called for a `reembed` row. Its presence is the
    // point: it proves dispatch did not quietly take the extract arm, which is
    // the only other arm that would read it.
    let extractor: memory_mcp::http::tasks::scheduler::ExtractorFn =
        Arc::new(|_params| Box::pin(async { Ok(serde_json::json!({"unused": true})) }));
    let no_faults: Arc<dyn memory_mcp::platform::fault_injection::FaultInjector> =
        Arc::new(memory_mcp::platform::fault_injection::NoFaults);

    execute_one_task_with_policy(
        &h.store,
        h.db.clone(),
        &h.namespace,
        &no_faults,
        extractor,
        Some(&policy),
    )
    .await
    .expect("one tick carrying the deployment policy");

    let record = h
        .driver
        .load(&task_id)
        .await
        .expect("load the task row")
        .expect("the row is present");
    assert_eq!(
        record.state,
        TaskState::Completed,
        "the row must complete through dispatch carrying the policy: {record:?}"
    );
    assert_eq!(
        record.kind, TASK_KIND_REEMBED,
        "dispatch must not have reinterpreted the row: {record:?}"
    );
    let (dimension, signature) = stored_vector(&h.db, &h.namespace, "fact:one").await;
    assert_eq!(
        dimension,
        Some(TARGET_DIMENSION as u64),
        "the vector must be written at the deployment dimension"
    );
    assert_eq!(
        signature.as_deref(),
        Some(policy.signature.as_str()),
        "the vector must carry the deployment's signature"
    );
}
