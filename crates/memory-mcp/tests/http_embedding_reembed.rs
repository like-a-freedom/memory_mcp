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
use std::sync::atomic::{AtomicUsize, Ordering};

use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::runtime::storage::EmbeddingPolicy;
use memory_mcp::http::tasks::DurableTaskTestDriver;
use memory_mcp::http::tasks::scheduler::execute_one_task_with_policy;
use memory_mcp::http::tasks::state::{TASK_KIND_REEMBED, TaskState};
use memory_mcp::http::tasks::worker::DurableTaskStore;
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::storage::table_scope::KnowledgeTables;
use memory_mcp::storage::{BoundDbClient, DbClient, ReleaseOwnedTable, SurrealDbClient};

/// The dimension every pass in this file writes at. Deliberately not the
/// default 1536 the tenant migrations render, so an index or vector that was
/// never rewritten is visible as such rather than coincidentally correct.
const TARGET_DIMENSION: usize = 2048;

/// A deterministic, enabled provider: same text in, same vector out, no
/// network. Its dimension is the only thing that varies.
struct StaticEmbeddingProvider {
    dimension: usize,
    calls: Arc<AtomicUsize>,
    fail_on_call: Option<usize>,
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
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_on_call == Some(call) {
            return Err(memory_mcp::error::MemoryError::Storage(
                "injected deterministic reembed provider failure".to_string(),
            ));
        }
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
    embedding_policy_with_failure(dimension, None).0
}

fn embedding_policy_with_failure(
    dimension: usize,
    fail_on_call: Option<usize>,
) -> (EmbeddingPolicy, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let policy = EmbeddingPolicy {
        provider: Arc::new(StaticEmbeddingProvider {
            dimension,
            calls: calls.clone(),
            fail_on_call,
        }),
        dimension,
        signature: memory_mcp::config::build_embedding_signature(
            "openai-compatible",
            Some("test-model"),
            Some("https://embeddings.invalid/v1"),
            dimension,
        ),
        model: Some("test-model".to_string()),
        provider_label: "openai-compatible",
    };
    (policy, calls)
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

/// An extractor that is never called: a `reembed` row never reaches the
/// extract arm. Supplying one is what lets this file drive the real dispatch
/// rather than a helper that claims on its own.
fn unused_extractor() -> memory_mcp::http::tasks::scheduler::ExtractorFn {
    Arc::new(|_params| Box::pin(async { Ok(serde_json::json!({"unused": true})) }))
}

/// An injector that never fires, so the happy path runs to completion.
fn no_faults() -> Arc<dyn memory_mcp::platform::fault_injection::FaultInjector> {
    Arc::new(memory_mcp::platform::fault_injection::NoFaults)
}

/// Run exactly one tick against the harness's tenant, through the production
/// dispatch: claim → `kind` match → the real reembed executor → a fenced commit.
///
/// Every test in this file goes through here, so a regression in the dispatch
/// — a `kind` that routes to the wrong arm, or a deployment policy that stops
/// being threaded into the `match` — fails them all rather than only a test
/// that happened to call the executor directly.
async fn one_tick(h: &ReembedHarness, policy: &EmbeddingPolicy) {
    execute_one_task_with_policy(
        &h.store,
        h.db.clone(),
        &h.namespace,
        &no_faults(),
        unused_extractor(),
        Some(policy),
    )
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
    enqueue_with_options(h, fingerprint, None, false).await
}

async fn enqueue_with_options(
    h: &ReembedHarness,
    fingerprint: &str,
    max_failures: Option<usize>,
    retry_failed: bool,
) -> String {
    h.driver
        .enqueue(
            TASK_KIND_REEMBED,
            fingerprint,
            // The wire form the operator route writes, so the executor's
            // option parsing is exercised against the real durable payload.
            serde_json::json!({ "max_failures": max_failures, "retry_failed": retry_failed }),
        )
        .await
        .expect("enqueue a reembed task")
}

async fn set_index_dimension(h: &ReembedHarness, dimension: usize) {
    h.db.query(
        "REMOVE INDEX fact_embedding_hnsw ON TABLE fact",
        None,
        &h.namespace,
    )
    .await
    .expect("remove the fixture index before provisioning its prior dimension");
    h.db.query(
        &format!(
            "DEFINE INDEX fact_embedding_hnsw ON TABLE fact FIELDS embedding HNSW DIMENSION {dimension}"
        ),
        None,
        &h.namespace,
    )
    .await
    .expect("provision the fixture index at its prior dimension");
    assert_eq!(
        declared_index_dimension(&h.db, &h.namespace).await,
        Some(dimension),
        "the old index dimension is independently verified before seeding vectors"
    );
}

async fn assert_reembed_dimension_transition(old_dimension: usize, target_dimension: usize) {
    let h = harness().await;
    if old_dimension != 1536 {
        set_index_dimension(&h, old_dimension).await;
    }
    seed_fact_with_foreign_vector(
        &h.db,
        &h.namespace,
        "fact:dimension-transition",
        old_dimension,
    )
    .await;
    let old_vector = stored_vector(&h.db, &h.namespace, "fact:dimension-transition").await;
    assert_eq!(old_vector.0, Some(old_dimension as u64));
    assert_eq!(old_vector.1.as_deref(), Some("embsig:previous-provider"));

    let task_id = enqueue(&h, "reembed:dimension-transition").await;
    let policy = embedding_policy(target_dimension);
    one_tick(&h, &policy).await;

    let new_vector = stored_vector(&h.db, &h.namespace, "fact:dimension-transition").await;
    assert_eq!(new_vector.0, Some(target_dimension as u64));
    assert_eq!(new_vector.1.as_deref(), Some(policy.signature.as_str()));
    assert_ne!(new_vector.1, old_vector.1);
    assert_eq!(
        declared_index_dimension(&h.db, &h.namespace).await,
        Some(target_dimension)
    );
    assert_eq!(
        read_embedding_state(&h.db, &h.namespace).await.as_deref(),
        Some("ready")
    );
    let task = h
        .driver
        .load(&task_id)
        .await
        .expect("load completed dimension transition")
        .expect("dimension transition task exists");
    assert_eq!(task.state, TaskState::Completed);
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
    let (policy, provider_calls) = embedding_policy_with_failure(TARGET_DIMENSION, None);
    // The first pass brings the namespace current: one fact had no vector, one
    // was written by another provider. Both must now agree with the deployment.
    enqueue(&h, "reembed:idempotence-first").await;
    one_tick(&h, &policy).await;
    assert_eq!(
        provider_calls.load(Ordering::SeqCst),
        2,
        "the first pass rewrites exactly the two stale facts"
    );
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
        provider_calls.load(Ordering::SeqCst),
        2,
        "the no-op pass must not call the embedding provider"
    );

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

#[tokio::test]
async fn a_reembed_task_does_not_modify_another_tenant() {
    let h = harness().await;
    let other_namespace = format!("tns_reembed_other_{}", uuid::Uuid::new_v4().simple());
    let other_db = h
        ._registry
        .tenant_engine_optional()
        .expect("in-memory tenant engine wired")
        .bind_to_test_namespace(&other_namespace)
        .await;
    other_db
        .apply_migrations(&other_namespace)
        .await
        .expect("other tenant migrations applied");
    seed_fact_with_foreign_vector(&other_db, &other_namespace, "fact:other-tenant", 1536).await;

    let other_vector_before = stored_vector(&other_db, &other_namespace, "fact:other-tenant").await;
    let other_index_before = declared_index_dimension(&other_db, &other_namespace).await;
    assert_eq!(other_vector_before.0, Some(1536));
    assert_eq!(
        other_vector_before.1.as_deref(),
        Some("embsig:previous-provider")
    );
    assert_eq!(other_index_before, Some(1536));

    seed_fact_with_foreign_vector(&h.db, &h.namespace, "fact:primary-tenant", 1536).await;
    let task_id = enqueue(&h, "reembed:tenant-isolation").await;
    let policy = embedding_policy(TARGET_DIMENSION);
    one_tick(&h, &policy).await;

    assert_eq!(
        stored_vector(&h.db, &h.namespace, "fact:primary-tenant").await,
        (
            Some(TARGET_DIMENSION as u64),
            Some(policy.signature.clone())
        )
    );
    assert_eq!(
        stored_vector(&other_db, &other_namespace, "fact:other-tenant").await,
        other_vector_before,
        "the primary task must not rewrite another tenant's fact"
    );
    assert_eq!(
        declared_index_dimension(&other_db, &other_namespace).await,
        other_index_before,
        "the primary task must not drop or recreate another tenant's index"
    );
    let task = h
        .driver
        .load(&task_id)
        .await
        .expect("load primary tenant reembed task")
        .expect("primary tenant task exists");
    assert_eq!(task.state, TaskState::Completed);
}

/// The composition named explicitly: a claimed row travelling claim → `kind`
/// dispatch → the real executor → a fenced completion, *while carrying the
/// deployment's policy*.
///
/// `one_tick` reaches the same seam today, so the three tests above fail if
/// policy threading breaks. This one calls [`execute_one_task_with_policy`]
/// directly anyway, and says what it is for, because the seam is the load-
/// bearing part: if `one_tick` were later simplified to call the executor
/// directly — a small, tempting change — every other test in this file would
/// silently stop exercising dispatch while staying green.
///
/// The alternative this replaced was worse: dispatch existed as a second copy
/// of itself for tests, so no test reached the `match` the scheduler runs. A
/// change that stopped threading the policy into it would have passed
/// everything here and broken the operator's reembed route in production.
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

#[tokio::test]
async fn reembed_384_to_2048_rewrites_vectors_and_index_through_http_dispatch() {
    assert_reembed_dimension_transition(384, 2048).await;
}

#[tokio::test]
async fn reembed_2048_to_384_rewrites_vectors_and_index_through_http_dispatch() {
    assert_reembed_dimension_transition(2048, 384).await;
}

#[tokio::test]
async fn reembed_1536_to_3072_rewrites_vectors_and_index_through_http_dispatch() {
    assert_reembed_dimension_transition(1536, 3072).await;
}

#[tokio::test]
async fn reembed_changes_signature_at_768_through_http_dispatch() {
    assert_reembed_dimension_transition(768, 768).await;
}

#[tokio::test]
async fn reembed_resumes_after_101_successes_and_retries_the_failed_fact() {
    const FACT_COUNT: usize = 205;
    const TARGET_DIMENSION: usize = 2048;
    const FIRST_FAILURE_CALL: usize = 102;
    const FAILED_FACT_ID: &str = "fact:resume_101";
    const LAST_SUCCESSFUL_FACT_ID: &str = "fact:resume_100";
    let h = harness().await;
    for index in 0..FACT_COUNT {
        seed_fact_with_foreign_vector(
            &h.db,
            &h.namespace,
            &format!("fact:resume_{index:03}"),
            1536,
        )
        .await;
    }
    h.db.update(
        FAILED_FACT_ID,
        serde_json::json!({
            "content": "Cedar continuity marker survives an interrupted vector reembed.",
            "quote": "Cedar continuity marker"
        }),
        &h.namespace,
        &[],
    )
    .await
    .expect("give the resume fact a unique lexical query");

    let (failing_policy, first_pass_calls) =
        embedding_policy_with_failure(TARGET_DIMENSION, Some(FIRST_FAILURE_CALL));
    let failed_task_id =
        enqueue_with_options(&h, "reembed:failure-after-101", Some(0), false).await;
    one_tick(&h, &failing_policy).await;

    assert_eq!(
        first_pass_calls.load(Ordering::SeqCst),
        FIRST_FAILURE_CALL,
        "the deterministic provider fails on call 102, after 101 successful writes"
    );
    assert_eq!(
        declared_index_dimension(&h.db, &h.namespace).await,
        None,
        "a failed pass must not publish a target-width index"
    );
    assert_ne!(
        read_embedding_state(&h.db, &h.namespace).await.as_deref(),
        Some("ready"),
        "a partially rewritten namespace is not semantically ready"
    );
    assert_eq!(
        stored_vector(&h.db, &h.namespace, FAILED_FACT_ID).await,
        (Some(1536), Some("embsig:previous-provider".to_string())),
        "the fact at the failure cursor retains its prior vector and identity"
    );
    let partial_facts =
        h.db.select_table(KnowledgeTables::table("fact"), &h.namespace)
            .await
            .expect("read partial durable facts after the failed pass");
    assert_eq!(partial_facts.len(), FACT_COUNT);
    assert_eq!(
        partial_facts
            .iter()
            .filter(|fact| fact["embedding_signature"] == failing_policy.signature)
            .count(),
        101,
        "only the 101 successful facts carry the target identity"
    );

    let failed_task = h
        .driver
        .load(&failed_task_id)
        .await
        .expect("load failed task result")
        .expect("failed task row exists");
    assert_eq!(failed_task.state, TaskState::Failed);
    let job =
        h.db.select_one("embedding_job:fact_reembed", &h.namespace)
            .await
            .expect("read durable reembed cursor after task failure")
            .expect("durable reembed job exists");
    let namespace_progress = &job["namespace_progress"][&h.namespace];
    assert_eq!(namespace_progress["succeeded_facts"], 101);
    assert_eq!(namespace_progress["failed_facts"], 1);
    assert_eq!(
        namespace_progress["last_completed_fact_id"],
        LAST_SUCCESSFUL_FACT_ID
    );
    assert_eq!(
        namespace_progress["failed_fact_ids"],
        serde_json::json!([FAILED_FACT_ID])
    );

    let lexical_service = MemoryService::new(
        h.db.clone(),
        h.namespace.clone(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("disabled lexical service starts while semantic readiness is degraded");
    let lexical_items = AssembleContextCapability::assemble_context_from_service(
        &lexical_service,
        AssembleContextRequest {
            query: "Cedar continuity marker".to_string(),
            as_of: Some(chrono::Utc::now() + chrono::Duration::seconds(1)),
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            fact_types: Vec::new(),
            compact: true,
        },
    )
    .await
    .expect("lexical retrieval remains available during reembed recovery");
    assert!(
        lexical_items
            .iter()
            .any(|item| item.fact_id == FAILED_FACT_ID)
    );

    let (resume_policy, resume_calls) = embedding_policy_with_failure(TARGET_DIMENSION, None);
    let resumed_task_id = enqueue_with_options(&h, "reembed:resume-after-101", None, false).await;
    one_tick(&h, &resume_policy).await;

    assert_eq!(
        resume_calls.load(Ordering::SeqCst),
        FACT_COUNT - 101,
        "resume starts after the durable cursor and retries the first failed fact"
    );
    let final_facts =
        h.db.select_table(KnowledgeTables::table("fact"), &h.namespace)
            .await
            .expect("read all durable facts after resume");
    assert_eq!(final_facts.len(), FACT_COUNT);
    assert!(final_facts.iter().all(|fact| {
        fact["embedding"].as_array().map(Vec::len) == Some(TARGET_DIMENSION)
            && fact["embedding_dimension"].as_u64() == Some(TARGET_DIMENSION as u64)
            && fact["embedding_signature"] == resume_policy.signature
    }));
    assert_eq!(
        declared_index_dimension(&h.db, &h.namespace).await,
        Some(TARGET_DIMENSION)
    );
    assert_eq!(
        read_embedding_state(&h.db, &h.namespace).await.as_deref(),
        Some("ready")
    );
    let resumed_task = h
        .driver
        .load(&resumed_task_id)
        .await
        .expect("load resumed task result")
        .expect("resumed task row exists");
    assert_eq!(resumed_task.state, TaskState::Completed);
}
