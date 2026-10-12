#![cfg(feature = "eval-support")]

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

struct ControlledReranker {
    seen_queries: Arc<Mutex<Vec<String>>>,
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
impl EmbeddingProvider for ControlledReranker {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "eval-controlled-reranker"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; self.dimension()];
        vector[0] = 1.0;
        Ok(vector)
    }

    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        self.seen_queries
            .lock()
            .expect("controlled reranker query lock")
            .push(query.to_owned());
        let query = query.to_ascii_lowercase();
        let scores = documents
            .iter()
            .map(|document| {
                let document = document.to_ascii_lowercase();
                let facet_match = if query == "body measurements" || query == "K measurements" {
                    document.contains("body") && document.contains("measurements")
                        || document.contains("K measurements")
                } else if query == "diagnostics http bounded memory plan" {
                    ["diagnostics", "http", "bounded", "memory", "plan"]
                        .iter()
                        .all(|term| document.contains(term))
                } else if query == "http diagnostics" {
                    document.contains("http") && document.contains("diagnostics")
                } else {
                    false
                };
                if facet_match { 1.0 } else { -1.0 }
            })
            .collect();
        Ok(Some(scores))
    }

    fn reranking_signature(&self) -> Option<&str> {
        Some("reranksig:controlled-query-facets-v1")
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

async fn ingest_source_only_episode(
    service: &MemoryService,
    source_id: &str,
    content: &str,
    t_ref: chrono::DateTime<Utc>,
) -> String {
    memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
        service,
        memory_mcp::models::IngestRequest {
            source_context: None,
            source_type: "chat".into(),
            source_id: source_id.into(),
            content: content.into(),
            t_ref,
            t_ingested: None,
            policy_tags: Vec::new(),
        },
        None,
    )
    .await
    .expect("ingest source-only episode through the production API")
}

async fn ingest_conversation_turn(
    service: &MemoryService,
    source_id: &str,
    group: &str,
    position: u64,
    content: &str,
) -> String {
    memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
        service,
        memory_mcp::models::IngestRequest {
            source_context: Some(memory_mcp::models::SourceContext {
                group: group.to_string(),
                position,
            }),
            source_type: "conversation".into(),
            source_id: source_id.into(),
            content: content.into(),
            t_ref: Utc::now(),
            t_ingested: None,
            policy_tags: Vec::new(),
        },
        None,
    )
    .await
    .expect("ingest conversation turn through the production API")
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
        db.clone() as Arc<dyn DbClient>,
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
async fn lexical_hit_keeps_its_real_ann_similarity_in_verbose_evidence() {
    let db = test_db().await;
    let content = "Cargo nextest runs Rust unit tests before release.";
    let query = "cargo nextest Rust unit tests";
    let mut fact_vector = vec![0.0; 1536];
    fact_vector[0] = 0.9;
    fact_vector[1] = (1.0_f64 - 0.9_f64.powi(2)).sqrt();
    let mut query_vector = vec![0.0; 1536];
    query_vector[0] = 1.0;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(MappedProvider {
                calls: Arc::new(AtomicUsize::new(0)),
                embeddings: HashMap::from([
                    (format!("note\n{content}\n{content}"), fact_vector),
                    (query.to_string(), query_vector),
                ]),
            }),
            0.7,
        ),
    )
    .await
    .expect("component service");
    let fact_id = service
        .add_fact(
            "note",
            content,
            content,
            "episode:fusion",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:fusion"),
        )
        .await
        .expect("write component fact");
    let wire = assemble_context(&service, verbose_tool_params(query))
        .await
        .expect("assembly");
    let item = wire
        .result
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["fact_id"] == fact_id)
        .expect("lexical result");
    let similarity = item["retrieval_evidence"]["semantic_similarity"].as_f64();
    assert!(
        similarity.is_some_and(|score| (score - 0.9).abs() < 0.01),
        "lexical discovery must not suppress the independent ANN score: {item}"
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
        (query.to_string(), query_vector),
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
async fn scoped_secondary_facets_keep_complementary_evidence_and_reject_generic_http_diagnostics() {
    let db = test_db().await;
    let seen_queries = Arc::new(Mutex::new(Vec::new()));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(ControlledReranker {
                seen_queries: seen_queries.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("controlled reranker service");
    let now = Utc::now();
    let body_fact = service
        .add_fact(
            "note",
            "The HTTP request-body measurements record byte count and chunk count.",
            "The HTTP request-body measurements record byte count and chunk count.",
            "episode:http-body-envelope",
            now,
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:http-body-envelope"),
        )
        .await
        .expect("write body-measurement evidence");
    let diagnostics_fact = service
        .add_fact(
            "note",
            "The bounded-memory plan requires HTTP diagnostics for stalled request bodies and streaming backpressure.",
            "The bounded-memory plan requires HTTP diagnostics for stalled request bodies and streaming backpressure.",
            "episode:http-bounded-diagnostics",
            now,
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:http-bounded-diagnostics"),
        )
        .await
        .expect("write scoped diagnostics evidence");
    let distractor_fact = service
        .add_fact(
            "note",
            "HTTP server diagnostics expose request status and log levels for operators.",
            "HTTP server diagnostics expose request status and log levels for operators.",
            "episode:http-server-logging",
            now,
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:http-server-logging"),
        )
        .await
        .expect("write related but unscoped diagnostics evidence");

    let query =
        "Which body measurements and diagnostics does the HTTP bounded memory plan require?";
    let items =
        AssembleContextCapability::assemble_context_from_service(&service, assemble_request(query))
            .await
            .expect("assemble through real retrieval, admission and reranking");

    let returned_ids = items
        .iter()
        .map(|item| item.fact_id.as_str())
        .collect::<Vec<_>>();
    let seen = seen_queries.lock().expect("controlled reranker query lock");
    assert!(
        returned_ids.contains(&body_fact.as_str()),
        "body evidence missing; returned={returned_ids:?}; reranker queries={:?}",
        *seen
    );
    assert!(
        returned_ids.contains(&diagnostics_fact.as_str()),
        "scoped diagnostics missing; returned={returned_ids:?}; reranker queries={:?}",
        *seen
    );
    assert!(
        !returned_ids.contains(&distractor_fact.as_str()),
        "generic diagnostics should be rejected; returned={returned_ids:?}; reranker queries={:?}",
        *seen
    );
    assert!(seen.iter().any(|seen_query| seen_query == query));
    assert!(
        seen.iter()
            .any(|seen_query| seen_query == "body measurements")
    );
    assert!(seen.iter().any(|seen_query| {
        seen_query
            .to_ascii_lowercase()
            .contains("diagnostics http bounded memory plan")
    }));
}

#[tokio::test]
async fn unicode_conjunction_is_assembled_with_its_scoped_http_evidence() {
    let db = test_db().await;
    let seen_queries = Arc::new(Mutex::new(Vec::new()));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(ControlledReranker {
                seen_queries: seen_queries.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("controlled reranker service");
    let content = "The K measurements and HTTP diagnostics are recorded for each request.";
    let fact_id = service
        .add_fact(
            "note",
            content,
            content,
            "episode:unicode-http-measurements",
            Utc::now(),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:unicode-http-measurements"),
        )
        .await
        .expect("write Unicode-scoped evidence");
    let query = "Show K measurements and HTTP diagnostics";

    let items =
        AssembleContextCapability::assemble_context_from_service(&service, assemble_request(query))
            .await
            .expect("Unicode conjunction must survive context assembly");

    assert!(items.iter().any(|item| item.fact_id == fact_id));
    let seen = seen_queries.lock().expect("controlled reranker query lock");
    assert!(seen.iter().any(|seen_query| seen_query == query));
    assert!(seen.iter().any(|seen_query| seen_query == "K measurements"));
    assert!(
        seen.iter()
            .any(|seen_query| seen_query == "HTTP diagnostics")
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
                source_context: None,
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
                source_context: None,
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
#[tokio::test]
async fn semantic_provider_receives_the_original_natural_language_question() {
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
        (query.to_string(), query_vector),
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

struct EpisodeOrderingReranker {
    scored_documents: Arc<Mutex<Vec<String>>>,
}

struct MultiParticipantEpisodeReranker {
    target_passages: Vec<&'static str>,
    scored_documents: Arc<Mutex<Vec<String>>>,
}

#[tokio::test]
async fn episode_fallback_rejects_an_explicit_event_outside_the_requested_month() {
    let db = test_db().await;
    let seed_service = MemoryService::new(
        db.clone() as Arc<dyn DbClient>,
        "main".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service for writing raw episode fixtures");
    let march_reference = chrono::DateTime::parse_from_rfc3339("2026-03-15T09:00:00Z")
        .expect("valid episode reference time")
        .with_timezone(&Utc);
    let episode_id = ingest_source_only_episode(
        &seed_service,
        "atlas-cache-april-update",
        "Atlas cache updates completed in April 2026.",
        march_reference,
    )
    .await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(EpisodeOrderingReranker {
                scored_documents: Arc::new(Mutex::new(Vec::new())),
            }),
            0.7,
        ),
    )
    .await
    .expect("valid isolated retrieval service with a reranker");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Atlas cache updates in March 2026"),
    )
    .await
    .expect("assemble through the production retrieval path");

    assert!(
        !items.iter().any(|item| item.source_episode == episode_id),
        "episode fallback must honor the explicit event month even when its reference time is in March: {items:#?}"
    );
}

#[tokio::test]
async fn episode_fallback_keeps_an_explicit_event_in_the_requested_month_when_referenced_later() {
    let db = test_db().await;
    let seed_service = MemoryService::new(
        db.clone() as Arc<dyn DbClient>,
        "main".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service for writing raw episode fixtures");
    let april_reference = chrono::DateTime::parse_from_rfc3339("2026-04-15T09:00:00Z")
        .expect("valid episode reference time")
        .with_timezone(&Utc);
    let episode_id = ingest_source_only_episode(
        &seed_service,
        "atlas-cache-march-update",
        "Atlas cache updates completed in March 2026.",
        april_reference,
    )
    .await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(provider(Arc::new(AtomicUsize::new(0))), 0.7),
    )
    .await
    .expect("valid isolated retrieval service without a reranker");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Atlas cache updates in March 2026"),
    )
    .await
    .expect("assemble through the production retrieval path");

    assert!(
        items.iter().any(|item| item.source_episode == episode_id),
        "the explicit event date should win over a later episode reference time: {items:#?}"
    );
}

#[async_trait]
impl EmbeddingProvider for EpisodeOrderingReranker {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "controlled-episode-ordering"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; 1536];
        vector[0] = 1.0;
        Ok(vector)
    }

    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        self.scored_documents
            .lock()
            .expect("reranker document capture lock")
            .extend(documents.iter().cloned());
        Ok(Some(
            documents
                .iter()
                .map(|document| {
                    if document.ends_with(
                        "Mara visited Red Rock Canyon State Park. Her hiking trips there were on weekends.",
                    ) || document.ends_with("Mara hikes at Red Rock Canyon State Park.")
                    {
                        9.0
                    } else {
                        0.1
                    }
                })
                .collect(),
        ))
    }

    fn reranking_signature(&self) -> Option<&str> {
        Some("controlled-episode-ordering-v1")
    }
}

#[async_trait]
impl EmbeddingProvider for MultiParticipantEpisodeReranker {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "controlled-multi-participant-episode"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; 1536];
        vector[0] = 1.0;
        Ok(vector)
    }

    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        self.scored_documents
            .lock()
            .expect("reranker document capture lock")
            .extend(documents.iter().cloned());
        Ok(Some(
            documents
                .iter()
                .map(|document| {
                    if self
                        .target_passages
                        .iter()
                        .any(|target| document.ends_with(target))
                    {
                        9.0
                    } else {
                        -1.0
                    }
                })
                .collect(),
        ))
    }

    fn reranking_signature(&self) -> Option<&str> {
        Some("controlled-multi-participant-episode-v1")
    }
}

#[tokio::test]
async fn episode_reranker_can_recover_a_single_anchor_passage_for_each_participant() {
    let db = test_db().await;
    let seed_service = MemoryService::new(
        db.clone() as Arc<dyn DbClient>,
        "main".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service for writing raw conversation turns");
    let reference_time = chrono::DateTime::parse_from_rfc3339("2023-01-20T16:04:00Z")
        .expect("fixed conversation reference time")
        .with_timezone(&Utc);
    let jon = ingest_source_only_episode(
        &seed_service,
        "locomo:conv-30:D1:6",
        "Jon: I've been into dancing since I was a kid and it's been my passion and escape. I wanna start a dance studio so I can teach others the joy that dancing brings me.",
        reference_time,
    )
    .await;
    let gina = ingest_source_only_episode(
        &seed_service,
        "locomo:conv-30:D1:7",
        "Gina: Wow Jon, same here! Dance is pretty much my go-to for stress relief. Got any fave styles?",
        reference_time,
    )
    .await;
    let lexical_distractor = ingest_source_only_episode(
        &seed_service,
        "locomo:conv-30:D2:11",
        "Gina: Believe in yourself, Jon! The process may be tough, but you got this. Push through and it'll be worth it. Don't forget to take breaks and dance it out when you need to destress!",
        reference_time,
    )
    .await;

    let scored_documents = Arc::new(Mutex::new(Vec::new()));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(MultiParticipantEpisodeReranker {
                target_passages: vec![
                    "Jon: I've been into dancing since I was a kid and it's been my passion and escape. I wanna start a dance studio so I can teach others the joy that dancing brings me.",
                    "Gina: Wow Jon, same here! Dance is pretty much my go-to for stress relief. Got any fave styles?",
                ],
                scored_documents: scored_documents.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("valid isolated retrieval service");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("How do Jon and Gina both like to destress?"),
    )
    .await
    .expect("assemble context through the production retrieval path");

    assert!(
        items.iter().any(|item| item.source_episode == jon),
        "a relevant turn matching only one participant must reach reranking and survive it: {items:#?}"
    );
    assert!(
        items.iter().any(|item| item.source_episode == gina),
        "each participant's answer-bearing turn should be returned: {items:#?}"
    );
    assert!(
        !items
            .iter()
            .any(|item| item.source_episode == lexical_distractor),
        "a stronger lexical overlap must not survive negative reranker evidence: {items:#?}"
    );

    let scored_documents = scored_documents
        .lock()
        .expect("reranker document capture lock");
    assert!(
        scored_documents
            .iter()
            .any(|document| document.ends_with("Jon: I've been into dancing since I was a kid and it's been my passion and escape. I wanna start a dance studio so I can teach others the joy that dancing brings me.")),
        "the single-anchor passage must be scored before evidence selection"
    );
}

#[tokio::test]
async fn episode_reranker_can_recover_a_relevant_passage_beyond_the_lexical_head() {
    let db = test_db().await;
    let seed_service = MemoryService::new(
        db.clone() as Arc<dyn DbClient>,
        "main".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service for writing raw episode fixtures");
    let ingest = |source_id: &'static str, content: &'static str| async {
        memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
            &seed_service,
            memory_mcp::models::IngestRequest {
                source_context: None,
                source_type: "chat".into(),
                source_id: source_id.into(),
                content: content.into(),
                t_ref: Utc::now(),
                t_ingested: None,
                policy_tags: Vec::new(),
            },
            None,
        )
        .await
        .expect("write raw episode through production ingestion")
    };

    let target_content = "Mara hikes at Red Rock Canyon State Park.";
    let target_episode = ingest("episode-rerank-target", target_content).await;
    let distractor_episode = ingest(
        "episode-rerank-lexical-distractor",
        "Mara visits the park to watch the golf course; the park is a favorite place.",
    )
    .await;
    let scored_documents = Arc::new(Mutex::new(Vec::new()));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(EpisodeOrderingReranker {
                scored_documents: scored_documents.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("valid isolated reranking service");
    let mut request = assemble_request("Which park does Mara visit for hiking?");
    request.budget = 1;

    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .expect("assemble through the production retrieval path");

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].source_episode, target_episode,
        "reranking must see beyond the lexical head and choose the answer-bearing episode"
    );
    assert_ne!(items[0].source_episode, distractor_episode);
    let scored_documents = scored_documents
        .lock()
        .expect("reranker document capture lock");
    assert!(
        scored_documents
            .iter()
            .any(|document| document.ends_with(target_content)),
        "the answer-bearing episode must reach the cross-encoder before budget truncation"
    );
}

#[tokio::test]
async fn fact_reranker_scores_candidates_before_response_budget_truncation() {
    let db = test_db().await;
    let seed_service = MemoryService::new(
        db.clone() as Arc<dyn DbClient>,
        "main".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service for writing raw fact fixtures");
    let target_content =
        "Mara visited Red Rock Canyon State Park. Her hiking trips there were on weekends.";
    for (index, content) in [
        "Mara visits the park for hiking several times a week. Her hiking group meets at the park, but she has never named its location.",
        "Mara visits the park to hike with friends each Saturday. The hiking club says the park is scenic, but no source names it.",
        "Mara visits the park for a hike before work. She says the park is a regular hiking spot but never identifies the location.",
        "Mara visits the park and hikes with the group there. The group walks around the park; no destination is named.",
        "Mara visits the park each month for hiking meetups. She calls the park perfect for hiking, but does not say which one.",
    ]
    .into_iter()
    .enumerate()
    {
        seed_service
            .add_fact(
                "note",
                content,
                content,
                &format!("episode:mara-hiking-distractor-{index}"),
                Utc::now(),
                0.9,
                Vec::new(),
                Vec::new(),
                Provenance::manual(),
            )
            .await
            .expect("persist lexical distractor through the production store");
    }
    let target_id = seed_service
        .add_fact(
            "note",
            target_content,
            target_content,
            "episode:mara-red-rock-hiking",
            chrono::DateTime::parse_from_rfc3339("2024-05-05T12:00:00Z")
                .expect("valid historical target time")
                .with_timezone(&Utc),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::manual(),
        )
        .await
        .expect("persist answer-bearing fact through the production store");

    let scored_documents = Arc::new(Mutex::new(Vec::new()));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".to_string(),
        eval_config(
            Arc::new(EpisodeOrderingReranker {
                scored_documents: scored_documents.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("valid isolated reranking service");
    let mut request = assemble_request("Which park does Mara visit for hiking?");
    request.budget = 1;

    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .expect("assemble through the production retrieval path");

    let reranked_documents = scored_documents
        .lock()
        .expect("reranker document capture lock");
    let target_position = reranked_documents
        .iter()
        .position(|document| document.ends_with(target_content))
        .expect("answer-bearing fact reached the reranker");
    assert_eq!(reranked_documents.len(), 6);
    assert!(target_position < reranked_documents.len());
    assert_eq!(
        items.first().map(|item| item.fact_id.as_str()),
        Some(target_id.as_str())
    );
}

struct RejectingReranker;

struct IndependentlyCalibratedReranker {
    scored_documents: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl EmbeddingProvider for IndependentlyCalibratedReranker {
    fn is_enabled(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "controlled-calibrated-boundary"
    }
    fn dimension(&self) -> usize {
        1536
    }
    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        let mut v = vec![0.0; 1536];
        if input.contains("Mara enjoys hiking.") {
            v[1] = 1.0;
        } else {
            v[0] = 1.0;
        }
        Ok(v)
    }
    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        self.scored_documents
            .lock()
            .expect("reranker document lock")
            .extend(documents.iter().cloned());
        Ok(Some(vec![-1.0; documents.len()]))
    }
    fn reranking_signature(&self) -> Option<&str> {
        Some("controlled-negative-logit-reranker")
    }
}

#[tokio::test]
async fn negative_reranker_logit_preserves_grounded_facts_and_rejects_weak_semantics() {
    let db = test_db().await;
    let scored_documents = Arc::new(Mutex::new(Vec::new()));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(IndependentlyCalibratedReranker {
                scored_documents: scored_documents.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("service");
    let id = service
        .add_fact(
            "note",
            "Mara enjoys hiking.",
            "Mara enjoys hiking.",
            "episode:calibrated",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact");
    let weak_id = service
        .add_fact(
            "note",
            "The production HTTP endpoint is awaiting review.",
            "The production HTTP endpoint is awaiting review.",
            "episode:weak-semantic",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("weak semantic fact");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("What activity does Mara enjoy hiking?"),
    )
    .await
    .expect("assembly");
    let scored_documents = scored_documents.lock().expect("reranker document lock");
    assert!(
        scored_documents
            .iter()
            .any(|document| document.ends_with("Mara enjoys hiking.")),
        "the lexical candidate must reach the reranker; scored documents: {scored_documents:#?}"
    );
    assert!(
        items.iter().any(|i| i.fact_id == id),
        "an independent lexical match must survive a negative cross-encoder logit; retrieved items: {items:#?}"
    );
    let grounded_item = items
        .iter()
        .find(|item| item.fact_id == id)
        .expect("lexically admitted fact");
    assert_eq!(
        grounded_item.provenance["retrieval_admission"]["basis"],
        "lexical"
    );
    assert!(
        grounded_item.provenance["retrieval_admission"]["matched_content_terms"]
            .as_array()
            .is_some_and(|terms| !terms.is_empty())
    );
    assert!(
        !items.iter().any(|i| i.fact_id == weak_id),
        "semantic similarity without query grounding must not admit a negative-logit candidate"
    );
}

#[tokio::test]
async fn compound_identifiers_do_not_supply_explicit_subject_support() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(Arc::new(RejectingReranker), 0.7),
    )
    .await
    .expect("service");
    let answer_id = service
        .add_fact(
            "note",
            "Memory MCP log output uses MEMORY_LOG_FORMAT to select text or JSON and MEMORY_LOG_COLOR to select colour behavior.",
            "Memory MCP log output uses MEMORY_LOG_FORMAT to select text or JSON and MEMORY_LOG_COLOR to select colour behavior.",
            "episode:logging-configuration",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("logging configuration fact");
    let unrelated_id = service
        .add_fact(
            "note",
            "A control-plane error envelope carries the correlation id; memory-mcp.log writes one request id on every line.",
            "A control-plane error envelope carries the correlation id; memory-mcp.log writes one request id on every line.",
            "episode:request-correlation",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("request correlation fact");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("What controls the format and colour of Memory MCP logs?"),
    )
    .await
    .expect("assembly");

    assert!(
        items.iter().any(|item| item.fact_id == answer_id),
        "the requested product and logging attributes must survive negative reranker evidence; retrieved items: {items:#?}"
    );
    assert!(
        !items.iter().any(|item| item.fact_id == unrelated_id),
        "hyphenated identifiers must not make unrelated content appear to support the named query subject; retrieved items: {items:#?}"
    );
}

#[tokio::test]
async fn bare_entity_context_includes_a_traceable_one_hop_neighbor() {
    let memory = common::TestMemory::new(false).await;
    common::seed_entity(
        &memory.db_client,
        "org",
        "entity:alice",
        "person",
        "Alice Stone",
        &[],
    )
    .await;
    common::seed_entity(
        &memory.db_client,
        "org",
        "entity:bob",
        "person",
        "Bob Chen",
        &[],
    )
    .await;
    memory
        .service
        .relate(
            "entity:alice",
            "knows",
            "entity:bob",
            memory_mcp::models::EdgeAttributes::inferred(),
        )
        .await
        .expect("edge");
    let id = common::seed_fact_with_links(
        &memory.service,
        "org",
        "Bob Chen owns the Atlas launch checklist.",
        Utc::now(),
        vec!["entity:bob".into()],
    )
    .await;
    let items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        assemble_request("Alice Stone"),
    )
    .await
    .expect("assembly");
    assert!(
        items.iter().any(|i| i.fact_id == id),
        "a bare exact entity query can include qualified neighbor context"
    );
}

#[tokio::test]
async fn broad_prefix_with_an_attribute_does_not_admit_unrelated_entity_traits() {
    let memory = common::TestMemory::new(false).await;
    common::seed_entity(
        &memory.db_client,
        "org",
        "entity:alice",
        "person",
        "Alice Stone",
        &[],
    )
    .await;
    common::seed_fact_with_links(
        &memory.service,
        "org",
        "Alice Stone is a product manager.",
        Utc::now(),
        vec!["entity:alice".into()],
    )
    .await;
    let items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        assemble_request("Tell me about Alice Stone's favourite cuisine"),
    )
    .await
    .expect("assembly");
    assert!(
        items.is_empty(),
        "the whole subject must resolve; a broad prefix does not erase the attribute: {items:#?}"
    );
}

#[tokio::test]
async fn personal_planning_uses_a_self_profile_with_a_matching_activity_anchor() {
    let memory = common::TestMemory::new(false).await;
    let target=common::seed_fact_at(&memory.service,"org",
        "Current user persona: spends weekends experimenting with music software and digital instruments.",Utc::now()).await;
    let unrelated = common::seed_fact_at(
        &memory.service,
        "org",
        "Current user persona: prefers blue decorations in the kitchen.",
        Utc::now(),
    )
    .await;
    let items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        assemble_request("I'm planning a weekend getaway and want something creatively fulfilling"),
    )
    .await
    .expect("assembly");
    assert!(
        items.iter().any(|i| i.fact_id == target),
        "personal planning can use the matching self-profile anchor"
    );
    assert!(
        !items.iter().any(|i| i.fact_id == unrelated),
        "self identity alone does not supply topical evidence"
    );
}

#[tokio::test]
async fn semantic_admission_survives_a_lexical_channel_with_sparse_word_overlap() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(provider(Arc::new(AtomicUsize::new(0))), 0.7),
    )
    .await
    .expect("component service");
    let content = "Morgan: I donated it yesterday.";
    let id = service
        .add_fact(
            "note",
            content,
            content,
            "episode:donation",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("When did Morgan donate the family vehicle to a local charity?"),
    )
    .await
    .expect("assembly");
    assert!(
        items.iter().any(|item| item.fact_id == id),
        "the observed ANN evidence must survive final lexical grounding checks"
    );
}

struct OrderedReranker;
#[async_trait]
impl EmbeddingProvider for OrderedReranker {
    fn is_enabled(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "controlled-ordering"
    }
    fn dimension(&self) -> usize {
        1536
    }
    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut v = vec![0.0; 1536];
        v[0] = 1.0;
        Ok(v)
    }
    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        Ok(Some(
            documents
                .iter()
                .map(|d| {
                    if d.ends_with("Mara enjoys hiking.") {
                        9.0
                    } else {
                        0.1
                    }
                })
                .collect(),
        ))
    }
    fn reranking_signature(&self) -> Option<&str> {
        Some("controlled-ordering-v1")
    }
}

#[tokio::test]
async fn reranker_order_controls_budget_selection_over_lexical_term_frequency() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(Arc::new(OrderedReranker), 0.7),
    )
    .await
    .expect("service");
    let target = service
        .add_fact(
            "note",
            "Mara enjoys hiking.",
            "Mara enjoys hiking.",
            "episode:preference",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("target");
    service
        .add_fact(
            "note",
            "Mara hobbies: hobbies and favourite hobbies include music hobbies.",
            "Mara hobbies",
            "episode:lexical",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("lexical");
    let mut request = assemble_request("What are Mara's favourite hobbies?");
    request.budget = 1;
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .expect("assembly");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].fact_id, target,
        "higher cross-encoder evidence must determine the bounded selection"
    );
}

#[async_trait]
impl EmbeddingProvider for RejectingReranker {
    fn is_enabled(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "controlled-rerank-component"
    }
    fn dimension(&self) -> usize {
        1536
    }
    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; 1536];
        vector[0] = 1.0;
        Ok(vector)
    }
    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        Ok(Some(vec![-4.0; documents.len()]))
    }
    fn reranking_signature(&self) -> Option<&str> {
        Some("controlled-component-v1")
    }
}

#[tokio::test]
async fn negative_cross_encoder_evidence_rejects_a_high_cosine_lexical_fact() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(Arc::new(RejectingReranker), 0.7),
    )
    .await
    .expect("component service");
    service
        .add_fact(
            "note",
            "Approved migrations use a logging checklist.",
            "Approved migrations use a logging checklist.",
            "episode:component",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Who approved the database migration?"),
    )
    .await
    .expect("assembly");
    assert!(
        items.is_empty(),
        "negative independent relevance evidence must reject the lexical/ANN hit; retrieved items: {items:#?}"
    );
}

#[tokio::test]
async fn qualified_reference_with_topic_support_survives_a_below_floor_reranker_score() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(Arc::new(RejectingReranker), 0.7),
    )
    .await
    .expect("component service");
    let content = "Task 16 adds HTTP memory preflight metrics for request bodies.";
    let id = service
        .add_fact(
            "note",
            content,
            content,
            "episode:qualified-reference-reranking",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Task 16 HTTP memory preflight metrics"),
    )
    .await
    .expect("assembly");

    assert!(
        items.iter().any(|item| item.fact_id == id),
        "an exact qualified reference with topical support is already independent admission evidence"
    );
    let item = items
        .iter()
        .find(|item| item.fact_id == id)
        .expect("qualified reference result");
    assert_eq!(item.provenance["retrieval_admission"]["basis"], "reference");
    assert_eq!(item.provenance["reranking"]["relevance_logit"], -4.0);
}

#[tokio::test]
async fn episode_fallback_cannot_bypass_negative_cross_encoder_evidence() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(Arc::new(RejectingReranker), 0.7),
    )
    .await
    .expect("component service");
    memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
        &service, memory_mcp::models::IngestRequest {
            source_context: None,
            source_type: "chat".into(), source_id: "raw-episode-reranking".into(),
            content: "Approved migrations use a logging checklist.".into(), t_ref: Utc::now(),
            t_ingested: None, policy_tags: vec![],
        }, None).await.expect("raw episode without extracted facts");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Who approved the database migration?"),
    )
    .await
    .expect("assembly");
    assert!(
        items.is_empty(),
        "episode fallback needs the same relevance gate as facts"
    );
}

struct SelectiveReranker {
    accepted: String,
}
#[async_trait]
impl EmbeddingProvider for SelectiveReranker {
    fn is_enabled(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "controlled-selective-rerank"
    }
    fn dimension(&self) -> usize {
        1536
    }
    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; 1536];
        vector[0] = 1.0;
        Ok(vector)
    }
    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        Ok(Some(
            documents
                .iter()
                .map(|d| {
                    if d.ends_with(&self.accepted) {
                        4.0
                    } else {
                        -4.0
                    }
                })
                .collect(),
        ))
    }
    fn reranking_signature(&self) -> Option<&str> {
        Some("controlled-selective-v1")
    }
}

#[tokio::test]
async fn irrelevant_episode_cannot_replace_a_reranked_relevant_fact() {
    let db = test_db().await;
    let content = "Database migration was approved by Alice.";
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(SelectiveReranker {
                accepted: content.into(),
            }),
            0.7,
        ),
    )
    .await
    .expect("service");
    let id = service
        .add_fact(
            "note",
            content,
            content,
            "episode:good",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact");
    memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
        &service, memory_mcp::models::IngestRequest {
            source_context: None,
            source_type: "chat".into(), source_id: "strong-overlap-irrelevant-episode".into(),
            content: "The database migration warnings are a draft; nobody has approved the request.".into(),
            t_ref: Utc::now(), t_ingested: None, policy_tags: vec![],
        }, None).await.expect("raw episode");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Who approved database migration warnings?"),
    )
    .await
    .expect("assembly");
    assert_eq!(
        items
            .iter()
            .map(|item| item.fact_id.as_str())
            .collect::<Vec<_>>(),
        vec![id.as_str()]
    );
    assert_eq!(
        items[0].provenance["reranking"]["model_signature"],
        "controlled-selective-v1"
    );
}

struct CalendarReranker;
#[async_trait]
impl EmbeddingProvider for CalendarReranker {
    fn is_enabled(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "calendar-relevance-component"
    }
    fn dimension(&self) -> usize {
        1536
    }
    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut v = vec![0.0; 1536];
        v[0] = 1.0;
        Ok(v)
    }
    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        Ok(Some(
            documents
                .iter()
                .map(|d| if d.contains("2022-03-16") { 4.0 } else { -4.0 })
                .collect(),
        ))
    }
    fn reranking_signature(&self) -> Option<&str> {
        Some("calendar-component-v1")
    }
}

#[tokio::test]
async fn calendar_query_reranking_receives_the_persisted_fact_valid_time() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(Arc::new(CalendarReranker), 0.7),
    )
    .await
    .expect("service");
    let id = service
        .add_fact(
            "note",
            "James went hiking in the mountains.",
            "James went hiking in the mountains.",
            "episode:calendar",
            chrono::DateTime::parse_from_rfc3339("2022-03-16T12:00:00Z")
                .expect("date")
                .with_timezone(&Utc),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("What did James do while hiking on March 16, 2022?"),
    )
    .await
    .expect("assembly");
    assert!(
        items.iter().any(|item| item.fact_id == id),
        "date evidence is persisted metadata, not necessarily repeated in fact text"
    );
}

struct TailGoldProvider {
    accepted: String,
}
#[async_trait]
impl EmbeddingProvider for TailGoldProvider {
    fn is_enabled(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "tail-candidate-component"
    }
    fn dimension(&self) -> usize {
        1536
    }
    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        let similarity: f64 = if input.starts_with("note\n") {
            if input.contains(&self.accepted) {
                0.72
            } else {
                let index = input
                    .split("glacier number ")
                    .nth(1)
                    .and_then(|tail| tail.split_whitespace().next())
                    .and_then(|n| n.parse::<f64>().ok())
                    .expect("controlled noise index");
                0.9 + index * 0.001
            }
        } else {
            1.0
        };
        let mut v = vec![0.0; 1536];
        v[0] = similarity;
        v[1] = (1.0 - similarity * similarity).sqrt();
        Ok(v)
    }
    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        Ok(Some(
            documents
                .iter()
                .map(|d| {
                    if d.ends_with(&self.accepted) {
                        4.0
                    } else {
                        -4.0
                    }
                })
                .collect(),
        ))
    }
    fn reranking_signature(&self) -> Option<&str> {
        Some("tail-component-v1")
    }
}

#[tokio::test]
async fn reranking_can_recover_the_relevant_candidate_below_the_first_twenty_ann_hits() {
    let db = test_db().await;
    let content = "HNSW vectors retrieve stored facts by cosine similarity.";
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(TailGoldProvider {
                accepted: content.into(),
            }),
            0.7,
        ),
    )
    .await
    .expect("service");
    // Controlled vectors establish a realistic first-stage ranking error. This
    // tests candidate availability, not model quality; native gates stay separate.
    for index in 0..25 {
        let noise = format!("A purple marmot crossed glacier number {index} yesterday.");
        service
            .add_fact(
                "note",
                &noise,
                &noise,
                &format!("episode:noise{index}"),
                Utc::now(),
                0.9,
                vec![],
                vec![],
                Provenance::manual(),
            )
            .await
            .expect("distractor fact");
    }
    let id = service
        .add_fact(
            "note",
            content,
            content,
            "episode:answer",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("answer fact");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("How does the service find conceptually related recollections?"),
    )
    .await
    .expect("assembly");
    assert_eq!(
        items.len(),
        1,
        "the relevant ANN candidate must be available to reranking"
    );
    assert!(
        items[0]
            .retrieval_evidence
            .as_ref()
            .and_then(|e| e.semantic_similarity)
            .is_some_and(|score| (score - 0.72).abs() < 0.01),
        "the regression must exercise actual ANN, not another channel"
    );
    assert_eq!(
        items
            .iter()
            .map(|item| item.fact_id.as_str())
            .collect::<Vec<_>>(),
        vec![id.as_str()]
    );
}

struct SourceAwareReranker;
#[async_trait]
impl EmbeddingProvider for SourceAwareReranker {
    fn is_enabled(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "source-aware-component"
    }
    fn dimension(&self) -> usize {
        1536
    }
    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut v = vec![0.0; 1536];
        v[0] = 1.0;
        Ok(v)
    }
    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        Ok(Some(
            documents
                .iter()
                .map(|d| {
                    if d.contains("docs/operations/LOGGING.md") {
                        4.0
                    } else {
                        -4.0
                    }
                })
                .collect(),
        ))
    }
    fn reranking_signature(&self) -> Option<&str> {
        Some("source-aware-component-v1")
    }
}

#[tokio::test]
async fn reranking_retains_the_exact_persisted_source_identity_for_a_document_fragment() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(Arc::new(SourceAwareReranker), 0.7),
    )
    .await
    .expect("service");
    let mut provenance = Provenance::manual();
    provenance.source_id = Some("docs/operations/LOGGING.md".into());
    let id = service
        .add_fact(
            "note",
            "ERROR marks failed work and WARN reports handled degradation.",
            "ERROR marks failed work and WARN reports handled degradation.",
            "episode:document",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            provenance,
        )
        .await
        .expect("fact");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("How does the service classify log levels?"),
    )
    .await
    .expect("assembly");
    assert!(
        items.iter().any(|item| item.fact_id == id),
        "document origin must survive in the relevance input"
    );
}

#[tokio::test]
async fn calendar_query_reranking_receives_the_persisted_source_episode_reference_time() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(Arc::new(CalendarReranker), 0.7),
    )
    .await
    .expect("service");
    let id=memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
        &service,memory_mcp::models::IngestRequest {
            source_context: None,
            source_type:"chat".into(),source_id:"calendar-episode".into(),
            content:"James went hiking in the mountains.".into(),
            t_ref:chrono::DateTime::parse_from_rfc3339("2022-03-16T12:00:00Z").expect("date").with_timezone(&Utc),
            t_ingested:None,policy_tags:vec![],
        },None).await.expect("episode");
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("James hiking mountains March 16 2022"),
    )
    .await
    .expect("assembly");
    assert!(
        items.iter().any(|item| item.source_episode == id),
        "episode temporal metadata must reach the scorer"
    );
}

#[tokio::test]
async fn calendar_admission_keeps_speaker_attribution_across_sentences_in_one_turn() {
    let db = test_db().await;
    let service = new_eval_retrieval_service(
        db.clone() as Arc<dyn DbClient>,
        "main".into(),
        eval_config(provider(Arc::new(AtomicUsize::new(0))), 0.7),
    )
    .await
    .expect("service");
    db.create(
        "entity:james",
        serde_json::json!({
            "entity_id": "entity:james",
            "entity_type": "person",
            "canonical_name": "James",
            "canonical_name_normalized": "james",
            "aliases": [],
        }),
        "main",
        memory_mcp::knowledge::queries::ENTITY_TEMPORAL_FIELDS,
    )
    .await
    .expect("persist the named speaker so subject-term filtering runs");
    let content = "James: I'll be looking forward to it. By the way, yesterday I went bowling and got 2 strikes. I love bowling!";
    let fact_id = service
        .add_fact(
            "experience",
            content,
            content,
            "episode:james-bowling",
            chrono::DateTime::parse_from_rfc3339("2022-03-17T15:47:00Z")
                .expect("source reference time")
                .with_timezone(&Utc),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("persist the source-grounded fact");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Which recreational activity was James pursuing on March 16, 2022?"),
    )
    .await
    .expect("assembly");

    assert!(
        items.iter().any(|item| item.fact_id == fact_id),
        "the source speaker should remain attributable across sentences in the same turn"
    );
}

struct NegativeCalendarReranker {
    calls: Arc<AtomicUsize>,
}

struct NegativeRerankerWithProductionEmbedding {
    inner: Arc<dyn EmbeddingProvider>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl EmbeddingProvider for NegativeRerankerWithProductionEmbedding {
    fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }

    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        self.inner.embed(input).await
    }

    async fn embed_query(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        self.inner.embed_query(input).await
    }

    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(Some(vec![-2.0; documents.len()]))
    }

    fn reranking_signature(&self) -> Option<&str> {
        Some("controlled-negative-calendar-reranker-v1")
    }
}

#[async_trait]
impl EmbeddingProvider for NegativeCalendarReranker {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "controlled-negative-calendar-reranker"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; 1536];
        vector[0] = 1.0;
        Ok(vector)
    }

    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(Some(vec![-2.0; documents.len()]))
    }

    fn reranking_signature(&self) -> Option<&str> {
        Some("controlled-negative-calendar-reranker-v1")
    }
}

#[tokio::test]
#[ignore = "diagnostic requires the pinned multilingual-e5-small bundle"]
async fn reranker_cannot_veto_fact_with_exact_date_subject_and_event_support() {
    let id = NEXT_DB_ID.fetch_add(1, Ordering::Relaxed);
    let database = format!("retrieval_quality_real_e5_{id}");
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(&database, "main", "warn", 384)
            .await
            .expect("connect isolated eval database with the pinned E5 dimension"),
    );
    db.apply_migrations("main")
        .await
        .expect("apply production migrations to eval database");
    let reranker_calls = Arc::new(AtomicUsize::new(0));
    let model_dir = std::env::var("EMBEDDINGS_MODEL_DIR").expect("verified E5 model directory");
    let embedding_config = memory_mcp::config::EmbeddingConfig {
        provider: memory_mcp::config::EmbeddingProviderKind::LocalCandle,
        model: Some("intfloat/multilingual-e5-small".into()),
        model_dir: Some(model_dir),
        ..Default::default()
    };
    let resolved = memory_mcp::eval_support::retrieval::create_eval_embedding_provider(
        &embedding_config,
        "/tmp",
    )
    .await
    .expect("load the pinned production embedding provider");
    let query_provider = resolved.provider.clone();
    let provider = Arc::new(NegativeRerankerWithProductionEmbedding {
        inner: resolved.provider,
        calls: reranker_calls.clone(),
    });
    let service = new_eval_retrieval_service(
        db.clone() as Arc<dyn DbClient>,
        "main".into(),
        eval_config(provider.clone(), 0.7),
    )
    .await
    .expect("service");
    let corpus = serde_json::from_str::<serde_json::Value>(include_str!(
        "../../../evals/corpora/retrieval-quality-v1/retrieval-quality.json"
    ))
    .expect("frozen reviewed retrieval-quality corpus");
    let case = corpus["cases"]
        .as_array()
        .expect("case list")
        .iter()
        .find(|case| case["id"].as_str() == Some("locomo-conv-47-001"))
        .expect("frozen LoCoMo source-grounded case");
    let query = case["query"].as_str().expect("reviewed natural query");
    let sources = case["sources"].as_array().expect("complete source turns");
    let mut target_episode_id = None;
    let mut extracted_fact_ids = Vec::new();
    for source in sources {
        let key = source["key"].as_str().expect("source turn key");
        let episode_id = memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(
            &service,
            memory_mcp::models::IngestRequest {
                source_context: None,
                source_type: source["source_type"].as_str().expect("source type").into(),
                source_id: source["source_id"].as_str().expect("source identity").into(),
                content: source["content"].as_str().expect("source text").into(),
                t_ref: chrono::DateTime::parse_from_rfc3339(
                    source["t_ref"].as_str().expect("source reference time"),
                )
                .expect("valid source reference time")
                .with_timezone(&Utc),
                t_ingested: None,
                policy_tags: Vec::new(),
            },
            None,
        )
        .await
        .expect("ingest the frozen source turn through the production API");
        let extracted = memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability::extract_from_service(
            &service,
            &episode_id,
            None,
            None,
        )
        .await
        .expect("extract entities and facts through the production API");
        if key == "D1:26" {
            target_episode_id = Some(episode_id);
            extracted_fact_ids = extracted
                .facts
                .iter()
                .map(|fact| fact.fact_id.clone())
                .collect();
        }
    }
    assert!(
        !extracted_fact_ids.is_empty(),
        "production extraction must persist the reviewed target turn's facts"
    );

    use memory_mcp::knowledge::api::{KnowledgeReadPort, KnowledgeReadScope};
    use memory_mcp::knowledge::infra::KnowledgeReadAdapter;
    let fact_rows = KnowledgeReadAdapter::new(db.clone() as Arc<dyn DbClient>, "main")
        .read_scope(KnowledgeReadScope::Facts)
        .await
        .expect("read back facts after production extraction");
    let target_fact_contents = fact_rows
        .iter()
        .filter_map(memory_mcp::knowledge::fact_parsing::fact_from_value_or_wrapper)
        .filter(|fact| extracted_fact_ids.contains(&fact.fact_id))
        .map(|fact| fact.content)
        .collect::<Vec<_>>();

    let query_vector = query_provider
        .embed_query(query)
        .await
        .expect("embed the source-grounded query with the real provider");
    let ann_cutoff = memory_mcp::shared::temporal::normalize_dt(Utc::now());
    let (ann_sql, ann_vars) = memory_mcp::knowledge::queries::build_select_facts_ann_query(
        &ann_cutoff,
        &query_vector,
        50,
    );
    let ann_rows = db
        .query(&ann_sql, Some(ann_vars), "main")
        .await
        .expect("run the production ANN query against the persisted source facts");
    let ann_ids = ann_rows
        .as_array()
        .expect("ANN returns a row array")
        .iter()
        .filter_map(|row| row.get("Object").unwrap_or(row).get("fact_id"))
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let items =
        AssembleContextCapability::assemble_context_from_service(&service, assemble_request(query))
            .await
            .expect("assembly");

    let mut target_similarities = Vec::with_capacity(target_fact_contents.len());
    for content in &target_fact_contents {
        let passage = query_provider
            .embed(content)
            .await
            .expect("embed the extracted source-grounded passage");
        let dot = query_vector
            .iter()
            .zip(&passage)
            .map(|(left, right)| left * right)
            .sum::<f64>();
        let query_norm = query_vector
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        let passage_norm = passage
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        target_similarities.push(dot / (query_norm * passage_norm));
    }
    eprintln!(
        "real E5 target similarity={target_similarities:?}; target_fact_ids={extracted_fact_ids:?}; target_in_ann={}; ann_count={}; reranker_calls={}",
        extracted_fact_ids.iter().any(|id| ann_ids.contains(id)),
        ann_ids.len(),
        reranker_calls.load(Ordering::Relaxed),
    );

    assert!(
        reranker_calls.load(Ordering::Relaxed) > 0,
        "the production fact candidate pool should reach the configured reranker; target episode={target_episode_id:?}, target facts={extracted_fact_ids:?}, items={items:#?}"
    );
    assert!(
        items.iter().any(|item| {
            (extracted_fact_ids.contains(&item.fact_id)
                || target_episode_id.as_deref() == Some(item.source_episode.as_str()))
                && item.content.to_lowercase().contains("bowl")
        }),
        "model disagreement must not veto the extracted exact-date, subject, and event evidence: {items:#?}"
    );
}

#[tokio::test]
async fn reranker_floor_still_applies_when_only_a_month_period_supports_the_fact() {
    let db = test_db().await;
    let reranker_calls = Arc::new(AtomicUsize::new(0));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(NegativeCalendarReranker {
                calls: reranker_calls.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("service");
    let fact_id = service
        .add_fact(
            "experience",
            "James went bowling on March 16, 2022.",
            "James went bowling on March 16, 2022.",
            "episode:james-bowling-month-query",
            chrono::DateTime::parse_from_rfc3339("2022-03-16T15:47:00Z")
                .expect("source reference time")
                .with_timezone(&Utc),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("persist the source-grounded fact");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Which activity did James do in March 2022?"),
    )
    .await
    .expect("assembly");

    assert!(reranker_calls.load(Ordering::Relaxed) > 0);
    assert!(
        items.iter().all(|item| item.fact_id != fact_id),
        "a broad month match must not bypass a negative reranker score: {items:#?}"
    );
}

struct ComplementaryEvidenceReranker {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl EmbeddingProvider for ComplementaryEvidenceReranker {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "complementary-evidence-component"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; 1536];
        vector[0] = 1.0;
        Ok(vector)
    }

    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(Some(
            documents
                .iter()
                .map(|document| {
                    if document.contains("Jon owns the deployment status") {
                        9.0
                    } else {
                        8.0
                    }
                })
                .collect(),
        ))
    }

    fn reranking_signature(&self) -> Option<&str> {
        Some("complementary-evidence-v1")
    }
}

#[tokio::test]
async fn higher_scoring_partial_episode_does_not_replace_a_complementary_fact() {
    let db = test_db().await;
    let rerank_calls = Arc::new(AtomicUsize::new(0));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(ComplementaryEvidenceReranker {
                calls: rerank_calls.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("service");
    ingest_source_only_episode(
        &service,
        "jon-deployment-status",
        "Jon owns the deployment status.",
        Utc::now(),
    )
    .await;
    let gina_fact_id = service
        .add_fact(
            "note",
            "Gina owns the billing status.",
            "Gina owns the billing status.",
            "episode:gina-billing-status",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("persist Gina's independent fact");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Jon deployment and Gina billing status"),
    )
    .await
    .expect("assemble complementary evidence");

    assert!(rerank_calls.load(Ordering::Relaxed) > 0);
    assert!(
        items.iter().any(|item| item.fact_id == gina_fact_id),
        "a higher-scoring episode about Jon must not discard the selected fact about Gina"
    );
}

struct AlwaysPositiveReranker {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl EmbeddingProvider for AlwaysPositiveReranker {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "always-positive-component"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; 1536];
        vector[0] = 1.0;
        Ok(vector)
    }

    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(Some(vec![10.0; documents.len()]))
    }

    fn reranking_signature(&self) -> Option<&str> {
        Some("always-positive-v1")
    }
}

#[tokio::test]
async fn positive_reranker_cannot_rescue_episode_with_explicitly_excluded_project_membership() {
    let db = test_db().await;
    let rerank_calls = Arc::new(AtomicUsize::new(0));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(AlwaysPositiveReranker {
                calls: rerank_calls.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("service");
    let episode_id = ingest_source_only_episode(
        &service,
        "release-notes-project-exclusion",
        "Release notes are part of Trackly, not Memory MCP.",
        Utc::now(),
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Memory MCP release notes"),
    )
    .await
    .expect("assemble project-scoped query");

    assert!(
        items.iter().all(|item| item.source_episode != episode_id),
        "explicitly excluded project membership must veto episode fallback before reranking"
    );
    assert_eq!(
        rerank_calls.load(Ordering::Relaxed),
        0,
        "a hard project-membership contradiction must be rejected before model scoring"
    );
}

#[tokio::test]
async fn lexical_episode_admission_rejects_explicitly_excluded_project_membership() {
    let db = test_db().await;
    let embedding_calls = Arc::new(AtomicUsize::new(0));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(provider(embedding_calls), 0.7),
    )
    .await
    .expect("service without a reranker");
    let episode_id = ingest_source_only_episode(
        &service,
        "release-notes-project-exclusion-no-reranker",
        "Release notes are part of Trackly, not Memory MCP.",
        Utc::now(),
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("Memory MCP release notes"),
    )
    .await
    .expect("assemble project-scoped query");

    assert!(
        items.iter().all(|item| item.source_episode != episode_id),
        "lexical episode admission must honor explicit negative project membership"
    );
}

struct CapturingContextReranker {
    documents: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl EmbeddingProvider for CapturingContextReranker {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "capturing-source-context-reranker"
    }

    fn dimension(&self) -> usize {
        1536
    }

    async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
        let mut vector = vec![0.0; 1536];
        vector[0] = 1.0;
        Ok(vector)
    }

    async fn rerank(&self, _: &str, documents: &[String]) -> Result<Option<Vec<f64>>, MemoryError> {
        self.documents
            .lock()
            .expect("captured reranker documents are not poisoned")
            .extend(documents.iter().cloned());
        Ok(Some(vec![10.0; documents.len()]))
    }

    fn reranking_signature(&self) -> Option<&str> {
        Some("capturing-source-context-reranker-v1")
    }
}

#[tokio::test]
async fn candidate_reranking_document_includes_only_its_immediate_source_question() {
    let db = test_db().await;
    let documents = Arc::new(Mutex::new(Vec::new()));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(CapturingContextReranker {
                documents: documents.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("service");
    ingest_conversation_turn(
        &service,
        "conversation:company-question",
        "conversation:company",
        2,
        "Owen: Which company did you select?",
    )
    .await;
    let answer_episode = ingest_conversation_turn(
        &service,
        "conversation:company-answer",
        "conversation:company",
        3,
        "Mira: I selected Acme as my company.",
    )
    .await;
    ingest_conversation_turn(
        &service,
        "conversation:company-follow-up",
        "conversation:company",
        4,
        "Owen: Which bank did Mira choose?",
    )
    .await;
    let fact_id = service
        .add_fact(
            "note",
            "Mira selected Acme as her company.",
            "Mira selected Acme as my company.",
            &answer_episode,
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("persist the answer fact");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("What company did Mira select?"),
    )
    .await
    .expect("assemble context");

    assert!(items.iter().any(|item| item.fact_id == fact_id));
    let documents = documents
        .lock()
        .expect("captured reranker documents are not poisoned");
    let candidate_document = documents
        .iter()
        .find(|document| document.contains("Mira selected Acme as her company."))
        .expect("the selected answer fact must be reranked");
    assert!(
        candidate_document.contains("Which company did you select?"),
        "the answer candidate should be scored with its immediate source question: {candidate_document:#?}"
    );
    assert!(
        !candidate_document.contains("Which bank did Mira choose?"),
        "unrelated later turns must not leak into the candidate's reranking context: {candidate_document:#?}"
    );
}

#[tokio::test]
async fn candidate_reranking_document_includes_the_same_speaker_context_of_a_follow_up_answer() {
    let db = test_db().await;
    let documents = Arc::new(Mutex::new(Vec::new()));
    let service = new_eval_retrieval_service(
        db as Arc<dyn DbClient>,
        "main".into(),
        eval_config(
            Arc::new(CapturingContextReranker {
                documents: documents.clone(),
            }),
            0.7,
        ),
    )
    .await
    .expect("service");
    ingest_conversation_turn(
        &service,
        "locomo:conv-41:D1:6",
        "locomo:conv-41:session_1",
        6,
        "John: Yeah Maria, I'm really hoping to get into local politics. I love helping the community and making it a better place.",
    )
    .await;
    ingest_conversation_turn(
        &service,
        "locomo:conv-41:D1:7",
        "locomo:conv-41:session_1",
        7,
        "Maria: Woohoo, John! That's awesome! Any specific areas do you want to tackle?",
    )
    .await;
    let answer_episode = ingest_conversation_turn(
        &service,
        "locomo:conv-41:D1:8",
        "locomo:conv-41:session_1",
        8,
        "John: I'm passionate about improving education and infrastructure in our community. Those are my main focuses.",
    )
    .await;
    ingest_conversation_turn(
        &service,
        "locomo:conv-41:D1:9",
        "locomo:conv-41:session_1",
        9,
        "Maria: Could you tell me what got you interested in those topics?",
    )
    .await;
    let fact_id = service
        .add_fact(
            "note",
            "John is passionate about improving education and infrastructure in his community.",
            "John is passionate about improving education and infrastructure in his community.",
            &answer_episode,
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("persist the answer fact");

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request("What is John's main focus in local politics?"),
    )
    .await
    .expect("assemble context");

    assert!(items.iter().any(|item| item.fact_id == fact_id));
    let documents = documents
        .lock()
        .expect("captured reranker documents are not poisoned");
    let candidate_document = documents
        .iter()
        .find(|document| {
            document.contains("John is passionate about improving education and infrastructure")
        })
        .expect("the selected answer fact must be reranked");
    assert!(
        candidate_document
            .contains("John: Yeah Maria, I'm really hoping to get into local politics."),
        "a follow-up answer should be scored with the same speaker's immediately prior topic: {candidate_document:#?}"
    );
    assert!(
        candidate_document.contains(
            "Maria: Woohoo, John! That's awesome! Any specific areas do you want to tackle?"
        ),
        "the direct question should remain in the candidate's source context: {candidate_document:#?}"
    );
    assert!(
        !candidate_document.contains("Could you tell me what got you interested"),
        "the candidate must not inherit later source turns: {candidate_document:#?}"
    );
}
