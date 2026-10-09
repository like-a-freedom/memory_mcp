#![cfg(feature = "eval-support")]

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use memory_mcp::MemoryError;
use memory_mcp::embedding::providers::EmbeddingProvider;
use memory_mcp::eval_support::retrieval::{EvalRetrievalConfig, new_eval_retrieval_service};
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::models::Provenance;
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use memory_mcp::tools::assemble_context::assemble_context;
use memory_mcp::tools::params::AssembleContextParams;

static NEXT_DB_ID: AtomicUsize = AtomicUsize::new(1);

struct FixedProvider {
    calls: Arc<AtomicUsize>,
    vector: Vec<f64>,
}

#[async_trait]
impl EmbeddingProvider for FixedProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "eval-component-fixed"
    }

    fn dimension(&self) -> usize {
        self.vector.len()
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.vector.clone())
    }
}

struct FailingProvider {
    calls: Arc<AtomicUsize>,
}

struct MappedProvider {
    calls: Arc<AtomicUsize>,
    embeddings: HashMap<String, Vec<f64>>,
}

#[async_trait]
impl EmbeddingProvider for MappedProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "eval-component-mapped-vectors"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.embeddings.get(input).cloned().ok_or_else(|| {
            MemoryError::Validation(format!(
                "component provider has no vector for input of {} bytes",
                input.len()
            ))
        })
    }
}

#[async_trait]
impl EmbeddingProvider for FailingProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "eval-component-outage"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(MemoryError::Validation(
            "controlled component provider outage".to_string(),
        ))
    }
}

async fn test_db() -> Arc<SurrealDbClient> {
    let id = NEXT_DB_ID.fetch_add(1, Ordering::Relaxed);
    let db_name = format!("retrieval_quality_eval_{id}");
    let namespaces = vec!["main".to_string()];
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
            .await
            .expect("connect isolated eval database"),
    );
    for namespace in &namespaces {
        db.apply_migrations(namespace)
            .await
            .expect("apply production migrations to eval database");
    }
    db
}

fn eval_config(
    provider: Arc<dyn EmbeddingProvider>,
    semantic_threshold: f64,
) -> EvalRetrievalConfig {
    EvalRetrievalConfig {
        embedding_provider: provider,
        semantic_threshold,
        claim_stage: "evidence".to_string(),
    }
}

fn provider(calls: Arc<AtomicUsize>) -> Arc<dyn EmbeddingProvider> {
    Arc::new(FixedProvider {
        calls,
        vector: {
            let mut vector = vec![0.0; 1536];
            vector[0] = 1.0;
            vector
        },
    })
}

fn assert_config_error_contains<T>(result: Result<T, MemoryError>, expected: &str) {
    match result {
        Err(MemoryError::ConfigInvalid(message)) => assert!(
            message.contains(expected),
            "expected configuration error containing {expected:?}, got {message:?}"
        ),
        _ => panic!("expected ConfigInvalid containing {expected:?}"),
    }
}

fn assemble_request(query: &str) -> AssembleContextRequest {
    AssembleContextRequest {
        query: query.to_string(),
        fact_types: Vec::new(),
        as_of: None,
        budget: 5,
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: true,
    }
}

fn verbose_tool_params(query: &str) -> AssembleContextParams {
    AssembleContextParams {
        query: query.to_string(),
        fact_types: Vec::new(),
        as_of: String::new(),
        budget: 5,
        view_mode: None,
        window_start: None,
        window_end: None,
        compact: false,
    }
}

#[tokio::test]
async fn eval_service_rejects_non_finite_or_out_of_range_thresholds() {
    let db = test_db().await;
    for threshold in [f64::NAN, f64::INFINITY, -1.01, 1.01] {
        let result = new_eval_retrieval_service(
            db.clone() as Arc<dyn DbClient>,
            "main".to_string(),
            eval_config(provider(Arc::new(AtomicUsize::new(0))), threshold),
        )
        .await;

        assert_config_error_contains(result, "similarity threshold");
    }
}

#[tokio::test]
async fn eval_service_rejects_invalid_namespace_and_claim_stage() {
    let db = test_db().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let invalid_namespace = new_eval_retrieval_service(
        db.clone() as Arc<dyn DbClient>,
        "main,other".to_string(),
        eval_config(provider(calls.clone()), 0.7),
    )
    .await;
    assert_config_error_contains(invalid_namespace, "namespace");
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let invalid_stage = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        EvalRetrievalConfig {
            embedding_provider: provider(calls.clone()),
            semantic_threshold: 0.7,
            claim_stage: "not-a-stage".to_string(),
        },
    )
    .await;
    assert_config_error_contains(invalid_stage, "stage");
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn eval_service_constructs_from_injected_provider_and_explicit_stage() {
    let db = test_db().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let service = new_eval_retrieval_service(
        db.clone() as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(provider(calls.clone()), 0.7),
    )
    .await
    .expect("valid eval-only retrieval service configuration");

    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("query whose embedding proves provider injection"),
    )
    .await
    .expect("empty isolated database still executes semantic query generation");

    assert!(items.is_empty(), "the isolated database contains no facts");
    assert!(
        calls.load(Ordering::Relaxed) > 0,
        "assemble_context must call the provider injected through eval support"
    );
}

#[tokio::test]
async fn community_summary_admits_a_persisted_member_fact_with_only_one_fact_term() {
    let memory = common::TestMemory::new(false).await;
    let member_entity_id = "entity:product-launch".to_string();
    common::seed_entity(
        &memory.db_client,
        "org",
        &member_entity_id,
        "project",
        "Product Launch",
        &[],
    )
    .await;
    let fact_id = common::seed_fact_with_links(
        &memory.service,
        "org",
        "The launch note records the revised status.",
        Utc::now(),
        vec![member_entity_id.clone()],
    )
    .await;
    common::seed_community(
        &memory.db_client,
        "org",
        "community:launch-workstream",
        &[member_entity_id],
        "Launch workstream coordination",
        Utc::now(),
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        assemble_request("launch workstream"),
    )
    .await
    .expect("assemble context from the real embedded community and fact records");

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].fact_id, fact_id);
    assert_eq!(
        items[0].provenance["retrieval_admission"]["basis"],
        "community_summary"
    );
    assert_eq!(
        items[0].provenance["retrieval_admission"]["community"]["matched_summary_terms"],
        serde_json::json!(["launch", "workstream"])
    );
}

#[tokio::test]
async fn eval_service_rejects_query_vectors_that_do_not_match_the_persisted_index() {
    let db = test_db().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(FixedProvider {
                calls: calls.clone(),
                vector: vec![1.0, 0.0],
            }),
            0.7,
        ),
    )
    .await
    .expect("service configuration itself is valid");

    let result = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "query with an incompatible vector dimension".to_string(),
            fact_types: Vec::new(),
            as_of: Some(Utc::now()),
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: true,
        },
    )
    .await;

    assert!(matches!(result, Err(MemoryError::Storage(_))));
    assert!(calls.load(Ordering::Relaxed) > 0);

    let tool_result = assemble_context(
        &service,
        verbose_tool_params("query with an incompatible vector dimension"),
    )
    .await;
    assert!(matches!(tool_result, Err(MemoryError::Storage(_))));
}

#[tokio::test]
async fn provider_outage_keeps_the_lexical_retrieval_path_available() {
    let db = test_db().await;
    let seed_service = MemoryService::new(
        db.clone() as Arc<dyn DbClient>,
        "main".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service for writing the component fixture");
    let fact_id = common::seed_fact_at(
        &seed_service,
        "component",
        "The release uses cargo nextest for Rust unit tests.",
        Utc::now(),
    )
    .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(FailingProvider {
                calls: calls.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("valid service with a failing component provider");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("cargo nextest Rust release tests"),
    )
    .await
    .expect("provider outage must not fail lexical retrieval");

    assert!(
        calls.load(Ordering::Relaxed) > 0,
        "provider outage was exercised"
    );
    assert!(
        items.iter().any(|item| item.fact_id == fact_id),
        "the lexical fact should remain available during embedding outage"
    );
}

#[tokio::test]
async fn high_level_semantic_paraphrase_survives_zero_lexical_grounding() {
    // Controlled embeddings are allowed here because this is a component
    // test of the production assembly path, not a quality acceptance case.
    let db = test_db().await;
    let fact_content = "HNSW vectors retrieve stored facts by cosine similarity.";
    let query = "How does the service find conceptually related recollections?";
    let mut fact_vector = vec![0.0; 1536];
    fact_vector[0] = 0.9;
    fact_vector[1] = (1.0_f64 - 0.9_f64.powi(2)).sqrt();
    let mut query_vector = vec![0.0; 1536];
    query_vector[0] = 1.0;
    let calls = Arc::new(AtomicUsize::new(0));
    let embeddings = HashMap::from([
        (format!("note\n{fact_content}\n{fact_content}"), fact_vector),
        (
            "service find conceptually related recollection".to_string(),
            query_vector,
        ),
    ]);
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(MappedProvider {
                calls: calls.clone(),
                embeddings,
            }),
            0.7,
        ),
    )
    .await
    .expect("valid isolated semantic service");
    let fact_id = service
        .add_fact(
            "note",
            fact_content,
            fact_content,
            "episode:component-paraphrase",
            Utc::now(),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:component-paraphrase"),
        )
        .await
        .expect("write component fact through production fact service");

    let items =
        AssembleContextCapability::assemble_context_from_service(&service, assemble_request(query))
            .await
            .expect("assemble context through production path");

    assert!(calls.load(Ordering::Relaxed) >= 2);
    assert!(items.iter().any(|item| item.fact_id == fact_id));

    let wire = assemble_context(&service, verbose_tool_params(query))
        .await
        .expect("verbose tool response from the same production provider and index");
    let wire_item = wire
        .result
        .as_array()
        .expect("tool result array")
        .iter()
        .find(|item| item["fact_id"] == fact_id)
        .expect("semantic fact remains selected on the tool path");
    assert_eq!(
        wire_item["retrieval_evidence"]["policy_version"],
        "retrieval-admission-v1"
    );
    assert_eq!(
        wire_item["retrieval_evidence"]["admission_basis"],
        "semantic"
    );
    let similarity = wire_item["retrieval_evidence"]["semantic_similarity"]
        .as_f64()
        .expect("raw ANN cosine is visible in verbose evidence");
    assert!(
        (similarity - 0.9).abs() < 0.01,
        "observed raw cosine {similarity} must match the mapped provider/index input"
    );
}

#[tokio::test]
async fn episode_fallback_rejects_a_single_keyword_match_for_a_multiterm_query() {
    let db = test_db().await;
    let seed_service = MemoryService::new(
        db.clone() as Arc<dyn DbClient>,
        "main".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service for writing a real episode");
    let unrelated_episode_id =
        memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
            &seed_service,
            memory_mcp::models::IngestRequest {
                source_type: "chat".to_string(),
                source_id: "episode-fallback-single-keyword".to_string(),
                content: "Cobalt is a blue pigment.".to_string(),
                t_ref: Utc::now(),
                t_ingested: None,
                policy_tags: Vec::new(),
            },
            None,
        )
        .await
        .expect("write an unrelated episode through production ingestion");
    let related_episode_id =
        memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
            &seed_service,
            memory_mcp::models::IngestRequest {
                source_type: "chat".to_string(),
                source_id: "episode-fallback-multi-term".to_string(),
                content: "Find cobalt orchids beside the river.".to_string(),
                t_ref: Utc::now(),
                t_ingested: None,
                policy_tags: Vec::new(),
            },
            None,
        )
        .await
        .expect("write a grounded episode through production ingestion");

    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(provider(Arc::new(AtomicUsize::new(0))), 0.7),
    )
    .await
    .expect("valid isolated retrieval service");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("find cobalt orchids beside the river"),
    )
    .await
    .expect("assemble context through the production retrieval path");

    assert_eq!(
        items.len(),
        1,
        "only the episode with sufficient query coverage should survive"
    );
    assert_eq!(items[0].source_episode, related_episode_id);
    assert_ne!(items[0].source_episode, unrelated_episode_id);
}
