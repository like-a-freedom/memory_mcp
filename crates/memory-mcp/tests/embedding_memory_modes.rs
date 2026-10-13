#![cfg(feature = "eval-support")]

#[path = "common/embedding_memory_modes.rs"]
mod embedding_memory_modes;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use embedding_memory_modes::{
    AnnEvidence, ReembedBatchObserverDb, SemanticFixture, fixed_embedding_provider,
    semantic_fixture, semantic_query,
};
use memory_mcp::embedding::providers::DisabledEmbeddingProvider;
use memory_mcp::eval_support::retrieval::{
    EvalRetrievalConfig, new_eval_reembed_service, new_eval_retrieval_service,
};
use memory_mcp::eval_support::{QueryObserver, SqlObservation};
use memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS;
use memory_mcp::models::{AssembleContextRequest, Provenance};
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::service::reembed_options::{ReembedOptions, ReembedOutcome};
use memory_mcp::service::reembed_progress::NoopProgressReporter;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

static NEXT_TEST_DATABASE_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<SqlObservation>>,
}

impl QueryObserver for RecordingObserver {
    fn observe(&self, observation: &SqlObservation) {
        self.observations
            .lock()
            .expect("SQL observation lock")
            .push(observation.clone());
    }
}

impl RecordingObserver {
    fn take(&self) -> Vec<SqlObservation> {
        std::mem::take(&mut *self.observations.lock().expect("SQL observation lock"))
    }
}

struct GatedEmbeddingProvider {
    dimension: usize,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl memory_mcp::embedding::providers::EmbeddingProvider for GatedEmbeddingProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "memory-mode-gated-provider"
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f64>, memory_mcp::MemoryError> {
        self.entered.notify_one();
        self.release.notified().await;
        let mut vector = vec![0.0; self.dimension];
        vector[0] = 1.0;
        Ok(vector)
    }
}

async fn assembled_fact_ids(fixture: &SemanticFixture) -> Vec<String> {
    assembled_fact_ids_result(fixture)
        .await
        .expect("context assembled through the public use case")
}

async fn assembled_fact_ids_result(
    fixture: &SemanticFixture,
) -> Result<Vec<String>, memory_mcp::MemoryError> {
    let items = AssembleContextCapability::assemble_context_from_service(
        &fixture.service,
        AssembleContextRequest {
            query: semantic_query().to_string(),
            as_of: Some(Utc::now() + Duration::seconds(1)),
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            fact_types: Vec::new(),
            compact: true,
        },
    )
    .await?;
    Ok(items.into_iter().map(|item| item.fact_id).collect())
}

async fn assert_one_ann_row_for_fixture(fixture: &SemanticFixture) -> serde_json::Value {
    let query_sql = fixture.observed_query_sql().await;
    let ann_query_count = query_sql
        .iter()
        .filter(|sql| sql.contains("vector::similarity::cosine"))
        .count();
    assert_eq!(
        ann_query_count, 1,
        "the controlled fixture must intercept exactly one ANN query; executed SQL: {query_sql:?}"
    );
    let rows = fixture.observed_ann_rows().await;
    assert_eq!(
        rows.len(),
        1,
        "the controlled ANN query must return its seeded row; executed SQL: {:?}",
        query_sql
    );
    assert_eq!(rows[0]["fact_id"], fixture.fact_id);
    rows[0].clone()
}

#[tokio::test]
async fn semantic_missing_score_uses_embedding_fallback() {
    let fixture = semantic_fixture(AnnEvidence::MissingScoreWithStoredVector, 0.7).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert!(ann_row.get("sem_score").is_none());
    assert!(ann_row["embedding"].is_array());
    assert!(fact_ids.contains(&fixture.fact_id));
}

#[tokio::test]
async fn semantic_candidate_missing_evidence_is_rejected_at_zero_threshold() {
    let fixture = semantic_fixture(AnnEvidence::Missing, 0.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert!(ann_row.get("sem_score").is_none());
    assert!(ann_row.get("embedding").is_none());
    assert!(
        !fact_ids.contains(&fixture.fact_id),
        "the ANN row has no score or usable fallback vector and must not be admitted"
    );
}

#[tokio::test]
async fn semantic_candidate_missing_evidence_is_rejected_at_negative_threshold() {
    let fixture = semantic_fixture(AnnEvidence::Missing, -1.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert!(ann_row.get("sem_score").is_none());
    assert!(ann_row.get("embedding").is_none());
    assert!(
        !fact_ids.contains(&fixture.fact_id),
        "the ANN row has no score or usable fallback vector and must not be admitted"
    );
}

#[tokio::test]
async fn semantic_candidate_wrong_width_fallback_is_rejected() {
    let fixture = semantic_fixture(AnnEvidence::WrongWidthFallback, -1.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert!(ann_row.get("sem_score").is_none());
    assert_eq!(ann_row["embedding"].as_array().map(Vec::len), Some(1));
    assert!(
        !fact_ids.contains(&fixture.fact_id),
        "a fallback vector with the wrong dimension is not semantic evidence"
    );
}

#[tokio::test]
async fn semantic_candidate_malformed_score_without_vector_is_rejected() {
    let fixture = semantic_fixture(AnnEvidence::MalformedScore, -1.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"], "not-a-number");
    assert!(ann_row.get("embedding").is_none());
    assert!(
        !fact_ids.contains(&fixture.fact_id),
        "a malformed score with no fallback vector is not semantic evidence"
    );
}

#[tokio::test]
async fn semantic_candidate_out_of_range_numeric_score_remains_a_storage_error() {
    let fixture = semantic_fixture(AnnEvidence::ExplicitScore(1.01), 0.0).await;
    let result = assembled_fact_ids_result(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"].as_f64(), Some(1.01));
    assert!(matches!(
        result,
        Err(memory_mcp::MemoryError::Storage(message))
            if message.contains("semantic score") && message.contains("outside the cosine range")
    ));
}

#[tokio::test]
async fn semantic_score_without_embedding_keeps_candidate() {
    let fixture = semantic_fixture(AnnEvidence::ExplicitScore(1.0), 0.7).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"].as_f64(), Some(1.0));
    assert!(ann_row.get("embedding").is_none());
    assert!(fact_ids.contains(&fixture.fact_id));
}

#[tokio::test]
async fn semantic_candidate_with_explicit_zero_score_is_admitted_at_zero_threshold() {
    let fixture = semantic_fixture(AnnEvidence::ExplicitScore(0.0), 0.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"].as_f64(), Some(0.0));
    assert!(ann_row.get("embedding").is_none());
    assert!(fact_ids.contains(&fixture.fact_id));
}

#[tokio::test]
async fn semantic_candidate_with_explicit_negative_one_score_is_admitted_at_negative_threshold() {
    let fixture = semantic_fixture(AnnEvidence::ExplicitScore(-1.0), -1.0).await;
    let fact_ids = assembled_fact_ids(&fixture).await;

    let ann_row = assert_one_ann_row_for_fixture(&fixture).await;
    assert_eq!(ann_row["sem_score"].as_f64(), Some(-1.0));
    assert!(ann_row.get("embedding").is_none());
    assert!(fact_ids.contains(&fixture.fact_id));
}

#[tokio::test]
async fn disabled_embeddings_keep_lexical_retrieval_without_reading_stored_vectors() {
    const STORED_DIMENSION: usize = 3072;
    let database = format!(
        "embedding_modes_disabled_stored_{}",
        NEXT_TEST_DATABASE_ID.fetch_add(1, Ordering::Relaxed)
    );
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(
            &database,
            "main",
            "warn",
            STORED_DIMENSION,
        )
        .await
        .expect("3072-dimensional disabled-mode database connected"),
    );
    db.apply_migrations("main")
        .await
        .expect("disabled-mode fixture migrations applied");
    let observer = Arc::new(RecordingObserver::default());
    db.set_eval_query_observer(observer.clone());

    let service = new_eval_retrieval_service(
        db.clone(),
        "main".to_string(),
        EvalRetrievalConfig {
            embedding_provider: Arc::new(DisabledEmbeddingProvider::new(STORED_DIMENSION)),
            semantic_threshold: 0.7,
            claim_stage: "disabled".to_string(),
        },
    )
    .await
    .expect("disabled-mode retrieval service created");

    let fact_id = service
        .add_fact(
            "note",
            "Rhea keeps the glacier maps inside a carved cedar cabinet.",
            "Rhea keeps the glacier maps inside a carved cedar cabinet.",
            "episode:disabled-stored-vector",
            Utc::now(),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:disabled-stored-vector"),
        )
        .await
        .expect("fact ingested while embeddings are disabled");
    let stored_vector = vec![0.25_f64; STORED_DIMENSION];
    DbClient::update(
        db.as_ref(),
        &fact_id,
        json!({
            "embedding": stored_vector,
            "embedding_provider": "legacy-provider",
            "embedding_model": "legacy-model",
            "embedding_dimension": STORED_DIMENSION,
            "embedding_signature": "legacy-signature",
        }),
        "main",
        &[],
    )
    .await
    .expect("legacy vector attached to the disabled-mode fixture fact");
    let before_retrieval = DbClient::select_one(db.as_ref(), &fact_id, "main")
        .await
        .expect("stored fact read")
        .expect("fact exists");
    assert_eq!(
        before_retrieval["embedding"].as_array().map(Vec::len),
        Some(STORED_DIMENSION)
    );

    observer.take();
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "Where does Rhea keep the glacier maps?".to_string(),
            as_of: Some(Utc::now() + Duration::seconds(1)),
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
    .expect("lexical context succeeds while embeddings are disabled");

    assert_eq!(
        items
            .iter()
            .map(|item| item.fact_id.as_str())
            .collect::<Vec<_>>(),
        vec![fact_id.as_str()]
    );
    let retrieval_sql = observer.take();
    assert!(
        retrieval_sql
            .iter()
            .all(|observation| !observation.sql.contains("vector::similarity::cosine")),
        "disabled mode must not issue an ANN query: {retrieval_sql:?}"
    );

    let after_retrieval = DbClient::select_one(db.as_ref(), &fact_id, "main")
        .await
        .expect("stored fact re-read")
        .expect("fact still exists");
    assert_eq!(after_retrieval["embedding"], before_retrieval["embedding"]);
    assert_eq!(after_retrieval["embedding_dimension"], STORED_DIMENSION);
    assert_eq!(after_retrieval["embedding_signature"], "legacy-signature");
}

#[tokio::test]
async fn disabled_empty_mode_keeps_lexical_retrieval_without_ann() {
    const CONFIGURED_DIMENSION: usize = 3072;
    let database = format!(
        "embedding_modes_disabled_empty_{}",
        NEXT_TEST_DATABASE_ID.fetch_add(1, Ordering::Relaxed)
    );
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(
            &database,
            "main",
            "warn",
            CONFIGURED_DIMENSION,
        )
        .await
        .expect("disabled-empty database connected"),
    );
    db.apply_migrations("main")
        .await
        .expect("disabled-empty fixture migrations applied");
    let observer = Arc::new(RecordingObserver::default());
    db.set_eval_query_observer(observer.clone());
    let service = new_eval_retrieval_service(
        db.clone(),
        "main".to_string(),
        EvalRetrievalConfig {
            embedding_provider: Arc::new(DisabledEmbeddingProvider::new(CONFIGURED_DIMENSION)),
            semantic_threshold: 0.7,
            claim_stage: "disabled".to_string(),
        },
    )
    .await
    .expect("disabled-empty retrieval service created");
    let fact_id = service
        .add_fact(
            "note",
            "Mira stores the winter field journal in a blue canvas case.",
            "Mira stores the winter field journal in a blue canvas case.",
            "episode:disabled-empty",
            Utc::now(),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:disabled-empty"),
        )
        .await
        .expect("fact ingested without embeddings");
    let before_retrieval = DbClient::select_one(db.as_ref(), &fact_id, "main")
        .await
        .expect("unembedded fact read")
        .expect("fact exists");
    assert!(
        before_retrieval
            .get("embedding")
            .is_none_or(serde_json::Value::is_null),
        "disabled-empty mode must have no stored vector: {before_retrieval:?}"
    );

    observer.take();
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "Where does Mira keep the winter field journal?".to_string(),
            as_of: Some(Utc::now() + Duration::seconds(1)),
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
    .expect("lexical retrieval succeeds without embeddings");

    assert_eq!(
        items
            .iter()
            .map(|item| item.fact_id.as_str())
            .collect::<Vec<_>>(),
        vec![fact_id.as_str()]
    );
    let retrieval_sql = observer.take();
    assert!(
        retrieval_sql
            .iter()
            .all(|observation| !observation.sql.contains("vector::similarity::cosine")),
        "disabled-empty mode must not issue an ANN query: {retrieval_sql:?}"
    );
    let after_retrieval = DbClient::select_one(db.as_ref(), &fact_id, "main")
        .await
        .expect("unembedded fact re-read")
        .expect("fact still exists");
    assert!(
        after_retrieval
            .get("embedding")
            .is_none_or(serde_json::Value::is_null),
        "lexical retrieval must not create a vector: {after_retrieval:?}"
    );
}

#[tokio::test]
async fn reembed_preserves_metadata_written_after_its_batch_read() {
    const OLD_DIMENSION: usize = 3072;
    const TARGET_DIMENSION: usize = 384;
    let database = format!(
        "embedding_modes_concurrent_metadata_{}",
        NEXT_TEST_DATABASE_ID.fetch_add(1, Ordering::Relaxed)
    );
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(&database, "main", "warn", OLD_DIMENSION)
            .await
            .expect("3072-dimensional concurrent metadata database connected"),
    );
    db.apply_migrations("main")
        .await
        .expect("concurrent metadata fixture migrations applied");
    let disabled_service = new_eval_retrieval_service(
        db.clone(),
        "main".to_string(),
        EvalRetrievalConfig {
            embedding_provider: Arc::new(DisabledEmbeddingProvider::new(OLD_DIMENSION)),
            semantic_threshold: 0.7,
            claim_stage: "disabled".to_string(),
        },
    )
    .await
    .expect("seed service created without an embedding model");
    let fact_id = disabled_service
        .add_fact(
            "note",
            "Aster keeps the signed field report in the eastern archive.",
            "Aster keeps the signed field report in the eastern archive.",
            "episode:concurrent-reembed-metadata",
            Utc::now(),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:concurrent-reembed-metadata"),
        )
        .await
        .expect("stale fact created");
    let original_vector = vec![0.125_f64; OLD_DIMENSION];
    let original_valid_at = Utc::now() - Duration::days(1);
    DbClient::update(
        db.as_ref(),
        &fact_id,
        json!({
            "embedding": original_vector,
            "embedding_provider": "legacy-provider",
            "embedding_model": "legacy-model",
            "embedding_dimension": OLD_DIMENSION,
            "embedding_signature": "legacy-signature",
            "access_count": 17,
            "t_valid": original_valid_at,
        }),
        "main",
        FACT_TEMPORAL_FIELDS,
    )
    .await
    .expect("stale vector and initial metadata persisted");

    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let provider = Arc::new(GatedEmbeddingProvider {
        dimension: TARGET_DIMENSION,
        entered: entered.clone(),
        release: release.clone(),
    });
    let service = new_eval_reembed_service(
        db.clone(),
        "main".to_string(),
        EvalRetrievalConfig {
            embedding_provider: provider,
            semantic_threshold: 0.7,
            claim_stage: "disabled".to_string(),
        },
        "target-signature-v1".to_string(),
        Some("target-model".to_string()),
    )
    .await
    .expect("target-width reembed service created");

    let entered_before_reembed = entered.notified();
    let task = tokio::spawn(async move {
        service
            .reembed_all_facts(
                &ReembedOptions::default(),
                &NoopProgressReporter,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered_before_reembed)
        .await
        .expect("reembed read its batch and reached the provider");

    let concurrent_valid_at = Utc::now() + Duration::days(1);
    DbClient::update(
        db.as_ref(),
        &fact_id,
        json!({
            "access_count": 999,
            "t_valid": concurrent_valid_at,
        }),
        "main",
        FACT_TEMPORAL_FIELDS,
    )
    .await
    .expect("concurrent lifecycle metadata update persisted while provider was blocked");
    release.notify_one();
    let (_, outcome) = task
        .await
        .expect("reembed task joined")
        .expect("reembed completed successfully");

    assert_eq!(outcome, ReembedOutcome::Completed);
    let rewritten = DbClient::select_one(db.as_ref(), &fact_id, "main")
        .await
        .expect("rewritten fact read")
        .expect("fact remains durable");
    assert_eq!(
        rewritten["embedding"].as_array().map(Vec::len),
        Some(TARGET_DIMENSION)
    );
    assert_eq!(rewritten["embedding_signature"], "target-signature-v1");
    assert_eq!(rewritten["access_count"], 999);
    let persisted_valid_at = rewritten["t_valid"]
        .as_str()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .expect("concurrent temporal update remains a datetime")
        .with_timezone(&Utc);
    assert_eq!(persisted_valid_at, concurrent_valid_at);
}

#[tokio::test]
async fn reembed_batch_does_not_read_old_vectors() {
    const OLD_DIMENSION: usize = 3072;
    const TARGET_DIMENSION: usize = 384;
    const FACT_COUNT: usize = 205;
    let database = format!(
        "embedding_modes_reembed_{}",
        NEXT_TEST_DATABASE_ID.fetch_add(1, Ordering::Relaxed)
    );
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(&database, "org", "warn", OLD_DIMENSION)
            .await
            .expect("3072-dimensional reembed database connected"),
    );
    db.apply_migrations("org")
        .await
        .expect("reembed fixture migrations applied");

    for index in 0..FACT_COUNT {
        let fact_id = format!("fact:reembed_projection_{index:03}");
        let content = format!("Projection batch payload {index:03}");
        let quote = format!("Quoted projection payload {index:03}");
        let source_episode = format!("episode:reembed_projection_{index:03}");
        let now = Utc::now();
        let index_keys = (0..24)
            .map(|key| format!("projection-metadata-{index:03}-{key:02}"))
            .collect::<Vec<_>>();
        DbClient::create(
            db.as_ref(),
            &fact_id,
            json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": content,
                "quote": quote,
                "source_episode": source_episode.clone(),
                "t_valid": now,
                "t_ingested": now,
                "confidence": 0.9,
                "index_keys": index_keys,
                "access_count": index,
                "entity_links": ["entity:projection-fixture"],
                "scope": "org",
                "policy_tags": ["synthetic"],
                "provenance": {"source_episode": source_episode},
                "embedding": vec![0.125_f64; OLD_DIMENSION],
                "embedding_provider": "legacy-provider",
                "embedding_model": "legacy-model",
                "embedding_dimension": OLD_DIMENSION,
                "embedding_signature": "legacy-signature",
                "embedding_updated_at": now,
            }),
            "org",
            FACT_TEMPORAL_FIELDS,
        )
        .await
        .expect("stale 3072-dimensional fact inserted");
    }

    let (observed_db, observer) = ReembedBatchObserverDb::new(db.clone());
    let service = new_eval_reembed_service(
        Arc::new(observed_db),
        "org".to_string(),
        EvalRetrievalConfig {
            embedding_provider: fixed_embedding_provider(TARGET_DIMENSION),
            semantic_threshold: 0.7,
            claim_stage: "disabled".to_string(),
        },
        "memory-mode-reembed-fixture-v1".to_string(),
        Some("fixture-model".to_string()),
    )
    .await
    .expect("384-dimensional reembed service created");
    let (_, outcome) = service
        .reembed_all_facts(
            &ReembedOptions::default(),
            &NoopProgressReporter,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("public reembed use case completes");

    assert_eq!(outcome, ReembedOutcome::Completed);
    let batches = observer.batches().await;
    assert_eq!(
        batches
            .iter()
            .map(|batch| batch.rows.len())
            .collect::<Vec<_>>(),
        vec![100, 100, 5, 0],
        "reembed processes three populated batches and performs one empty cursor read to finish"
    );
    assert!(batches[0].sql.contains("ORDER BY fact_id ASC LIMIT $limit"));
    assert!(!batches[0].sql.contains("fact_id > $last_completed_fact_id"));
    assert!(batches[1].sql.contains("fact_id > $last_completed_fact_id"));
    assert!(
        batches[1]
            .sql
            .contains("FROM (SELECT fact_id, fact_type, content, quote FROM fact")
    );
    assert!(batches[2].sql.contains("fact_id > $last_completed_fact_id"));

    for (index, row) in batches.iter().flat_map(|batch| &batch.rows).enumerate() {
        let expected_id = format!("fact:reembed_projection_{index:03}");
        let expected_content = format!("Projection batch payload {index:03}");
        let expected_quote = format!("Quoted projection payload {index:03}");
        assert_eq!(row.fact_id.as_deref(), Some(expected_id.as_str()));
        assert_eq!(row.fact_type.as_deref(), Some("note"));
        assert_eq!(row.content.as_deref(), Some(expected_content.as_str()));
        assert_eq!(row.quote.as_deref(), Some(expected_quote.as_str()));
        assert!(
            !row.columns.iter().any(|column| column == "embedding"),
            "old vector must not cross the reembed batch boundary for {expected_id}; columns: {:?}",
            row.columns
        );
        assert!(
            row.columns.iter().all(|column| {
                ["id", "fact_id", "fact_type", "content", "quote"].contains(&column.as_str())
            }),
            "reembed input row carried unrelated metadata for {expected_id}: {:?}",
            row.columns
        );
    }
}

#[test]
fn frozen_storage_quality_corpus_has_grounded_mode_and_embedding_coverage() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/storage_quality/modes-v2.json"))
            .expect("frozen storage quality corpus is valid JSON");
    assert_eq!(
        corpus["schema_version"],
        "memory-mcp-storage-quality/modes-v2"
    );
    let as_of = corpus["as_of"]
        .as_str()
        .expect("the corpus has one fixed as_of instant");
    let sources = corpus["sources"]
        .as_array()
        .expect("source fixtures are present");
    let source_keys = sources
        .iter()
        .map(|source| {
            source["key"]
                .as_str()
                .expect("each source has a literal source key")
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(source_keys.len(), sources.len(), "source keys are unique");
    assert!(sources.iter().all(|source| {
        source["content"]
            .as_str()
            .is_some_and(|content| !content.is_empty())
            && source["t_valid"].as_str().is_some()
            && (source["t_invalid"].is_null() || source["t_invalid"].as_str().is_some())
    }));

    let cases = corpus["cases"]
        .as_array()
        .expect("retrieval cases are present");
    let case_ids = cases
        .iter()
        .map(|case| case["id"].as_str().expect("each case has an id"))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(case_ids.len(), cases.len(), "case ids are unique");
    let count_class = |class: &str| {
        cases
            .iter()
            .filter(|case| case["class"].as_str() == Some(class))
            .count()
    };
    assert!(count_class("answerable") >= 20);
    assert!(count_class("unknown") >= 5);
    assert!(count_class("protected_or_historical") >= 5);

    assert!(cases.iter().all(|case| {
        let expected = case["expected_ordered_source_keys"]
            .as_array()
            .expect("every case has literal ordered expected keys");
        let required = case["required_evidence"]
            .as_array()
            .expect("every case has required evidence keys");
        let relevance = case["graded_relevance"]
            .as_object()
            .expect("every case has graded relevance labels");
        expected
            .iter()
            .all(|key| key.as_str().is_some_and(|key| source_keys.contains(key)))
            && required.iter().all(|key| expected.contains(key))
            && expected
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == expected.len()
            && relevance
                .values()
                .all(|grade| grade.as_u64().is_some_and(|grade| (0..=2).contains(&grade)))
            && match case["embedding_mode"].as_str() {
                Some("enabled") => [384, 768, 1536, 2048, 3072]
                    .contains(&case["dimension"].as_u64().unwrap_or_default()),
                Some("disabled_empty") => case["dimension"].is_null(),
                Some("disabled_stored_3072") => case["dimension"] == 3072,
                _ => false,
            }
    }));
    assert!(
        cases
            .iter()
            .flat_map(|case| case["graded_relevance"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.values()))
            .any(|grade| grade == 0)
            && cases
                .iter()
                .flat_map(|case| case["graded_relevance"]
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.values()))
                .any(|grade| grade == 1)
            && cases
                .iter()
                .flat_map(|case| case["graded_relevance"]
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.values()))
                .any(|grade| grade == 2),
        "the frozen corpus uses each relevance grade 0, 1, and 2"
    );

    assert!(cases.iter().all(|case| match case["class"].as_str() {
        Some("answerable") => {
            !case["expected_ordered_source_keys"]
                .as_array()
                .is_none_or(Vec::is_empty)
                && !case["required_evidence"]
                    .as_array()
                    .is_none_or(Vec::is_empty)
        }
        Some("unknown") => {
            case["expected_ordered_source_keys"]
                .as_array()
                .is_some_and(Vec::is_empty)
                && case["required_evidence"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
        }
        Some("protected_or_historical") => {
            let prohibited = case["prohibited_source_keys"].as_array();
            prohibited.is_some_and(|keys| {
                !keys.is_empty()
                    && keys.iter().all(|key| {
                        key.as_str().is_some_and(|key| {
                            !case["expected_ordered_source_keys"].as_array().is_some_and(
                                |expected| expected.iter().any(|expected| expected == key),
                            ) && sources.iter().any(|source| {
                                source["key"].as_str() == Some(key)
                                    && (source["policy_tags"]
                                        .as_array()
                                        .is_some_and(|tags| !tags.is_empty())
                                        || source["t_invalid"]
                                            .as_str()
                                            .is_some_and(|end| end <= as_of)
                                        || source["t_valid"]
                                            .as_str()
                                            .is_some_and(|start| start > as_of))
                            })
                        })
                    })
            })
        }
        _ => false,
    }));
}
