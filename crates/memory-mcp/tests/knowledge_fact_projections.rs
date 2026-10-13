#![cfg(feature = "eval-support")]

use chrono::{Duration, Utc};
use memory_mcp::eval_support::{QueryObserver, SqlObservation};
use memory_mcp::knowledge::{KnowledgeGraphStore, KnowledgeStoreClient};
use memory_mcp::models::{AssembleContextRequest, Provenance};
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::storage::table_scope::{KnowledgeTables, ReleaseOwnedTable};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

mod common;

static DIMENSIONAL_DB_COUNTER: AtomicUsize = AtomicUsize::new(1);

#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<SqlObservation>>,
}

impl QueryObserver for RecordingObserver {
    fn observe(&self, observation: &SqlObservation) {
        self.observations
            .lock()
            .expect("observer lock")
            .push(observation.clone());
    }
}

impl RecordingObserver {
    fn observations(&self) -> Vec<SqlObservation> {
        self.observations.lock().expect("observer lock").clone()
    }
}

async fn add_fixture_fact(
    service: &memory_mcp::service::MemoryService,
    content: &str,
    source_episode: &str,
    t_valid: chrono::DateTime<Utc>,
    policy_tags: Vec<String>,
) -> String {
    service
        .add_fact(
            "note",
            content,
            content,
            source_episode,
            t_valid,
            0.9,
            vec![],
            policy_tags,
            Provenance::manual(),
        )
        .await
        .expect("fixture fact added")
}

async fn dimensional_memory(dimension: usize) -> (MemoryService, Arc<SurrealDbClient>) {
    let sequence = DIMENSIONAL_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let database = format!("knowledge_fact_projection_{sequence}");
    let db_client = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(&database, "org", "warn", dimension)
            .await
            .expect("dimension-aware in-memory database connected"),
    );
    db_client
        .apply_migrations("org")
        .await
        .expect("dimension-aware migrations applied");
    let service = MemoryService::new(
        db_client.clone(),
        "org".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("memory service created");

    (service, db_client)
}

async fn declared_index_dimension(db_client: &SurrealDbClient) -> Option<usize> {
    let raw = DbClient::query(db_client, "INFO FOR TABLE fact", None, "org")
        .await
        .expect("fact index definition read through public DbClient");
    let define = raw
        .as_array()?
        .first()?
        .get("indexes")?
        .get("fact_embedding_hnsw")?
        .as_str()?;
    let width = &define[define.find("DIMENSION ")? + "DIMENSION ".len()..];
    width
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

async fn assert_stored_vector_dimension(
    db_client: &SurrealDbClient,
    fact_id: &str,
    expected_dimension: usize,
) {
    assert_eq!(
        stored_vector_dimension(db_client, fact_id).await,
        Some(expected_dimension),
        "fixture vector width is independent evidence from the declared index"
    );
    assert_eq!(
        declared_index_dimension(db_client).await,
        Some(expected_dimension),
        "the public schema reports the requested HNSW width"
    );
}

async fn stored_vector_dimension(db_client: &SurrealDbClient, fact_id: &str) -> Option<usize> {
    let record = DbClient::select_one(db_client, fact_id, "org")
        .await
        .expect("stored vector readback succeeds")
        .expect("stored vector fact exists");
    record["embedding"].as_array().map(Vec::len)
}

async fn assert_active_projection_at_dimension(dimension: usize) {
    let (service, db_client) = dimensional_memory(dimension).await;
    let fact_id = add_fixture_fact(
        &service,
        "dimension-aware active projection fixture",
        "episode:dimension-active-projection",
        Utc::now(),
        vec!["dimension-matrix".to_string()],
    )
    .await;
    DbClient::update(
        db_client.as_ref(),
        &fact_id,
        json!({"embedding": vec![0.25_f32; dimension]}),
        "org",
        &[],
    )
    .await
    .expect("dimension-specific vector stored");
    let rows = KnowledgeGraphStore::new(db_client.clone(), "org")
        .select_active_facts(10)
        .await
        .expect("active facts selected by owning store");
    let row = rows
        .iter()
        .find(|row| row["fact_id"].as_str() == Some(fact_id.as_str()))
        .expect("active fixture returned by owning store");

    assert_eq!(row["content"], "dimension-aware active projection fixture");
    assert_eq!(row["policy_tags"], json!(["dimension-matrix"]));
    assert!(row.get("embedding").is_none());
    assert_stored_vector_dimension(db_client.as_ref(), &fact_id, dimension).await;
}

async fn assert_decay_projection_at_dimension(dimension: usize) {
    let (service, db_client) = dimensional_memory(dimension).await;
    let fact_id = add_fixture_fact(
        &service,
        "dimension-aware decay projection fixture",
        "episode:dimension-decay-projection",
        Utc::now() - Duration::days(400),
        vec![],
    )
    .await;
    DbClient::update(
        db_client.as_ref(),
        &fact_id,
        json!({"embedding": vec![0.25_f32; dimension]}),
        "org",
        &[],
    )
    .await
    .expect("dimension-specific vector stored");
    let rows = KnowledgeGraphStore::new(db_client.clone(), "org")
        .select_decay_candidates(10)
        .await
        .expect("decay candidates selected by owning store");
    let row = rows
        .iter()
        .find(|row| row["fact_id"].as_str() == Some(fact_id.as_str()))
        .expect("decay fixture returned by owning store");

    assert_eq!(row["confidence"], 0.9);
    assert!(row.get("content").is_none());
    assert!(row.get("embedding").is_none());
    assert_stored_vector_dimension(db_client.as_ref(), &fact_id, dimension).await;
}

async fn assert_scan_projection_at_dimension(dimension: usize) {
    let (service, db_client) = dimensional_memory(dimension).await;
    let fact_id = add_fixture_fact(
        &service,
        "dimension-aware nonsemantic scan fixture",
        "episode:dimension-scan-projection",
        Utc::now(),
        vec![],
    )
    .await;
    DbClient::update(
        db_client.as_ref(),
        &fact_id,
        json!({"embedding": vec![0.25_f32; dimension]}),
        "org",
        &[],
    )
    .await
    .expect("dimension-specific vector stored");
    let observer = Arc::new(RecordingObserver::default());
    db_client.set_eval_query_observer(observer.clone() as Arc<dyn QueryObserver>);
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request(
            "ignored by wake-up view",
            Utc::now() + Duration::hours(1),
            10,
            Some("wake_up"),
            None,
        ),
    )
    .await
    .expect("nonsemantic scan completes with embeddings stored");

    assert!(items.iter().any(|item| item.fact_id == fact_id));
    let scan = observer
        .observations()
        .into_iter()
        .find(|observation| observation.sql.contains("FROM fact"))
        .expect("actual fact scan observed");
    assert!(scan.sql.to_ascii_lowercase().contains("select id, fact_id"));
    assert!(!scan.sql.to_ascii_lowercase().contains("embedding"));
    assert_stored_vector_dimension(db_client.as_ref(), &fact_id, dimension).await;
}

async fn assert_ann_projection_at_dimension(dimension: usize) {
    let (service, db_client) = dimensional_memory(dimension).await;
    let now = Utc::now();
    let e0_id = add_fixture_fact(
        &service,
        "semantic basis e0 dimension matrix",
        "episode:semantic-e0-dimension-matrix",
        now,
        vec![],
    )
    .await;
    let e1_id = add_fixture_fact(
        &service,
        "semantic basis e1 dimension matrix",
        "episode:semantic-e1-dimension-matrix",
        now,
        vec![],
    )
    .await;
    let mut e0 = vec![0.0_f32; dimension];
    e0[0] = 1.0;
    let mut e1 = vec![0.0_f32; dimension];
    e1[1] = 1.0;
    for (fact_id, embedding) in [(&e0_id, e0), (&e1_id, e1)] {
        DbClient::update(
            db_client.as_ref(),
            fact_id,
            json!({"embedding": embedding}),
            "org",
            &[],
        )
        .await
        .expect("dimension-specific ANN vector stored");
    }
    let store = KnowledgeStoreClient::new(db_client.clone(), "org");
    let mut query = vec![0.0; dimension];
    query[0] = 1.0;

    let rows = store
        .select_facts_ann(&(now + Duration::seconds(1)).to_rfc3339(), &query, 2)
        .await
        .expect("ANN query uses the dimension-specific public store path");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["fact_id"].as_str(), Some(e0_id.as_str()));
    assert_eq!(rows[0]["sem_score"].as_f64(), Some(1.0));
    assert_eq!(rows[1]["fact_id"].as_str(), Some(e1_id.as_str()));
    assert_eq!(rows[1]["sem_score"].as_f64(), Some(0.0));
    assert!(rows.iter().all(|row| row.get("embedding").is_none()));
    assert_stored_vector_dimension(db_client.as_ref(), &e0_id, dimension).await;
    assert_stored_vector_dimension(db_client.as_ref(), &e1_id, dimension).await;
}

async fn remove_embedding_index(db_client: &SurrealDbClient) {
    DbClient::query(
        db_client,
        "REMOVE INDEX fact_embedding_hnsw ON TABLE fact",
        None,
        "org",
    )
    .await
    .expect("embedding index removed through public DbClient");
}

#[tokio::test]
async fn active_projection_keeps_results_when_embedding_index_is_absent() {
    const DIMENSION: usize = 3072;
    let (service, db_client) = dimensional_memory(DIMENSION).await;
    let fact_id = add_fixture_fact(
        &service,
        "active projection without embedding index",
        "episode:active-without-index",
        Utc::now(),
        vec![],
    )
    .await;
    DbClient::update(
        db_client.as_ref(),
        &fact_id,
        json!({"embedding": vec![0.25_f32; DIMENSION]}),
        "org",
        &[],
    )
    .await
    .expect("stored vector written before dropping the index");
    remove_embedding_index(db_client.as_ref()).await;

    let rows = KnowledgeGraphStore::new(db_client.clone(), "org")
        .select_active_facts(10)
        .await
        .expect("active read does not depend on the vector index");
    let row = rows
        .iter()
        .find(|row| row["fact_id"].as_str() == Some(fact_id.as_str()))
        .expect("active fact remains available");

    assert_eq!(row["content"], "active projection without embedding index");
    assert!(row.get("embedding").is_none());
    assert_eq!(declared_index_dimension(db_client.as_ref()).await, None);
    assert_eq!(
        stored_vector_dimension(db_client.as_ref(), &fact_id).await,
        Some(DIMENSION_WITHOUT_INDEX)
    );
}

#[tokio::test]
async fn decay_projection_keeps_results_when_embedding_index_is_absent() {
    const DIMENSION: usize = 3072;
    let (service, db_client) = dimensional_memory(DIMENSION).await;
    let fact_id = add_fixture_fact(
        &service,
        "decay projection without embedding index",
        "episode:decay-without-index",
        Utc::now() - Duration::days(400),
        vec![],
    )
    .await;
    DbClient::update(
        db_client.as_ref(),
        &fact_id,
        json!({"embedding": vec![0.25_f32; DIMENSION]}),
        "org",
        &[],
    )
    .await
    .expect("stored vector written before dropping the index");
    remove_embedding_index(db_client.as_ref()).await;

    let rows = KnowledgeGraphStore::new(db_client.clone(), "org")
        .select_decay_candidates(10)
        .await
        .expect("decay read does not depend on the vector index");
    let row = rows
        .iter()
        .find(|row| row["fact_id"].as_str() == Some(fact_id.as_str()))
        .expect("decay candidate remains available");

    assert_eq!(row["confidence"], 0.9);
    assert!(row.get("content").is_none());
    assert!(row.get("embedding").is_none());
    assert_eq!(declared_index_dimension(db_client.as_ref()).await, None);
    assert_eq!(
        stored_vector_dimension(db_client.as_ref(), &fact_id).await,
        Some(DIMENSION_WITHOUT_INDEX)
    );
}

#[tokio::test]
async fn nonsemantic_scan_keeps_results_when_embedding_index_is_absent() {
    const DIMENSION: usize = 3072;
    let (service, db_client) = dimensional_memory(DIMENSION).await;
    let fact_id = add_fixture_fact(
        &service,
        "nonsemantic scan without embedding index",
        "episode:scan-without-index",
        Utc::now(),
        vec![],
    )
    .await;
    DbClient::update(
        db_client.as_ref(),
        &fact_id,
        json!({"embedding": vec![0.25_f32; DIMENSION]}),
        "org",
        &[],
    )
    .await
    .expect("stored vector written before dropping the index");
    remove_embedding_index(db_client.as_ref()).await;
    let observer = Arc::new(RecordingObserver::default());
    db_client.set_eval_query_observer(observer.clone() as Arc<dyn QueryObserver>);

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        assemble_request(
            "ignored by wake-up view",
            Utc::now() + Duration::hours(1),
            10,
            Some("wake_up"),
            None,
        ),
    )
    .await
    .expect("nonsemantic scan does not depend on the vector index");

    assert!(items.iter().any(|item| item.fact_id == fact_id));
    let scan = observer
        .observations()
        .into_iter()
        .find(|observation| observation.sql.contains("FROM fact"))
        .expect("actual full fact scan observed");
    assert!(!scan.sql.to_ascii_lowercase().contains("embedding"));
    assert_eq!(declared_index_dimension(db_client.as_ref()).await, None);
    assert_eq!(
        stored_vector_dimension(db_client.as_ref(), &fact_id).await,
        Some(DIMENSION_WITHOUT_INDEX)
    );
}

const DIMENSION_WITHOUT_INDEX: usize = 3072;

fn assemble_request(
    query: &str,
    as_of: chrono::DateTime<Utc>,
    budget: i32,
    view_mode: Option<&str>,
    access: Option<memory_mcp::models::AccessPayload>,
) -> AssembleContextRequest {
    AssembleContextRequest {
        query: query.to_string(),
        as_of: Some(as_of),
        budget,
        view_mode: view_mode.map(str::to_string),
        window_start: None,
        window_end: None,
        access,
        fact_types: vec![],
        compact: false,
    }
}

#[tokio::test]
async fn decay_projection_omits_content_and_embedding() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = memory
        .service
        .add_fact(
            "metric",
            "fact body that decay does not need",
            "fact body that decay does not need",
            "episode:decay_projection_fixture",
            Utc::now() - Duration::days(400),
            0.4,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");
    let embedding = vec![0.125_f32; memory_mcp::config::DEFAULT_EMBEDDING_DIMENSION];
    DbClient::update(
        memory.db_client.as_ref(),
        &fact_id,
        json!({"embedding": embedding}),
        "org",
        &[],
    )
    .await
    .expect("embedding stored");
    let store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");

    let rows = store
        .select_decay_candidates(10)
        .await
        .expect("decay candidates selected");

    let row = rows
        .iter()
        .find(|row| row["fact_id"].as_str() == Some(fact_id.as_str()))
        .expect("fixture fact selected");
    assert_eq!(row["fact_id"].as_str(), Some(fact_id.as_str()));
    assert!(row.get("embedding").is_none());
    assert!(row.get("content").is_none());
    assert_eq!(row["confidence"], 0.4);
}

#[tokio::test]
async fn active_fact_reads_omit_embeddings_and_keep_nonsemantic_fields() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = memory
        .service
        .add_fact(
            "note",
            "synthetic active fact",
            "synthetic active fact",
            "episode:active-fixture",
            Utc::now(),
            0.8,
            vec!["active-fixture".to_string()],
            vec!["persona".to_string()],
            Provenance::manual(),
        )
        .await
        .expect("fact added");
    let embedding = vec![0.125_f32; memory_mcp::config::DEFAULT_EMBEDDING_DIMENSION];
    DbClient::update(
        memory.db_client.as_ref(),
        &fact_id,
        json!({"embedding": embedding}),
        "org",
        &[],
    )
    .await
    .expect("embedding stored");
    let store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");

    let rows = store
        .select_active_facts(10)
        .await
        .expect("active facts selected");

    let row = rows
        .iter()
        .find(|row| row["fact_id"].as_str() == Some(fact_id.as_str()))
        .expect("fixture fact selected");
    assert!(row.get("embedding").is_none());
    assert_eq!(row["content"], "synthetic active fact");
    assert_eq!(row["source_episode"], "episode:active-fixture");
    assert_eq!(row["policy_tags"], json!(["persona"]));
    assert!(row["index_keys"].is_array());
    assert_eq!(row["confidence"], 0.8);
}

#[tokio::test]
async fn ann_returns_score_without_vector_payload() {
    assert_ann_projection_at_dimension(2048).await;
}

#[tokio::test]
async fn nonsemantic_scan_omits_vectors_without_truncating_history() {
    let memory = common::TestMemory::new(false).await;
    let now = Utc::now();
    let historical_cutoff = now + Duration::hours(1);
    let closed_at = now + Duration::hours(2);
    let after_close_cutoff = now + Duration::hours(3);
    let fact_time = now - Duration::minutes(1);
    let vector = vec![0.125_f32; memory_mcp::config::DEFAULT_EMBEDDING_DIMENSION];

    let active_id = memory
        .service
        .add_fact(
            "note",
            "active scan fixture",
            "active scan fixture",
            "episode:scan-active",
            fact_time,
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("active fact added");
    let closed_id = memory
        .service
        .add_fact(
            "note",
            "historical predecessor scan fixture",
            "historical predecessor scan fixture",
            "episode:scan-closed",
            fact_time,
            0.8,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("historical fact added");
    let newest_id = memory
        .service
        .add_fact(
            "note",
            "newest scan fixture",
            "newest scan fixture",
            "episode:scan-newest",
            fact_time,
            0.95,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("newest fact added");

    for fact_id in [&active_id, &closed_id, &newest_id] {
        DbClient::update(
            memory.db_client.as_ref(),
            fact_id,
            json!({"embedding": vector}),
            "org",
            &[],
        )
        .await
        .expect("fixture vector stored");
    }
    DbClient::update(
        memory.db_client.as_ref(),
        &closed_id,
        json!({"t_invalid": closed_at.to_rfc3339()}),
        "org",
        &["t_invalid"],
    )
    .await
    .expect("historical closure stored");

    let observer = Arc::new(RecordingObserver::default());
    memory
        .db_client
        .set_eval_query_observer(observer.clone() as Arc<dyn QueryObserver>);

    let earlier_items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        AssembleContextRequest {
            query: "ignored by wake-up view".to_string(),
            as_of: Some(historical_cutoff),
            budget: 10,
            view_mode: Some("wake_up".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            fact_types: vec![],
            compact: false,
        },
    )
    .await
    .expect("historical wake-up context assembled");

    let mut returned_ids = earlier_items
        .iter()
        .map(|item| item.fact_id.clone())
        .collect::<Vec<_>>();
    let mut expected_ids = vec![active_id.clone(), closed_id.clone(), newest_id.clone()];
    returned_ids.sort();
    expected_ids.sort();
    assert_eq!(returned_ids, expected_ids);
    assert!(
        earlier_items.iter().any(|item| item.fact_id == closed_id
            && item.content == "historical predecessor scan fixture")
    );

    let later_items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        AssembleContextRequest {
            query: "ignored by wake-up view".to_string(),
            as_of: Some(after_close_cutoff),
            budget: 10,
            view_mode: Some("wake_up".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            fact_types: vec![],
            compact: false,
        },
    )
    .await
    .expect("later wake-up context assembled");
    let later_ids = later_items
        .iter()
        .map(|item| item.fact_id.as_str())
        .collect::<Vec<_>>();
    assert!(later_ids.contains(&active_id.as_str()));
    assert!(later_ids.contains(&newest_id.as_str()));
    assert!(!later_ids.contains(&closed_id.as_str()));

    let scan_queries = observer
        .observations()
        .into_iter()
        .filter(|observation| observation.sql.contains("FROM fact"))
        .collect::<Vec<_>>();
    assert_eq!(
        scan_queries.len(),
        2,
        "each wake-up read must scan fact once"
    );
    for observation in scan_queries {
        let normalized_sql = observation.sql.to_ascii_lowercase();
        assert!(
            normalized_sql.contains("select id, fact_id"),
            "the scan must select its ID plus nonsemantic projection: {}",
            observation.sql
        );
        assert!(!normalized_sql.contains("embedding"), "{}", observation.sql);
        assert!(!normalized_sql.contains(" where "), "{}", observation.sql);
        assert!(!normalized_sql.contains(" limit "), "{}", observation.sql);
    }
}

#[tokio::test]
async fn temporal_scan_keeps_relative_event_date_match() {
    let memory = common::TestMemory::new(false).await;
    let cutoff = Utc::now() + Duration::hours(1);
    let fact_id = add_fixture_fact(
        &memory.service,
        "Atlas approved the release yesterday.",
        "episode:relative-temporal-event",
        cutoff,
        vec![],
    )
    .await;
    assert_ne!(
        cutoff.date_naive(),
        (cutoff - Duration::days(1)).date_naive()
    );

    let observer = Arc::new(RecordingObserver::default());
    memory
        .db_client
        .set_eval_query_observer(observer.clone() as Arc<dyn QueryObserver>);
    let items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        assemble_request("yesterday", cutoff, 5, None, None),
    )
    .await
    .expect("relative temporal context assembled");

    let item = items
        .iter()
        .find(|item| item.fact_id == fact_id)
        .expect("a source-time fact with a relative event date inside yesterday is retained");
    assert_eq!(item.retrieval_tier.as_deref(), Some("temporal"));
    assert!(observer.observations().iter().any(|observation| {
        observation.sql.starts_with("SELECT id, fact_id") && observation.sql.contains("FROM fact")
    }));
}

#[tokio::test]
async fn projected_scan_preserves_policy_restrictions() {
    let memory = common::TestMemory::new(false).await;
    let cutoff = Utc::now() + Duration::hours(1);
    let allowed_id = add_fixture_fact(
        &memory.service,
        "Public release decision",
        "episode:public-scan-fact",
        cutoff,
        vec!["public".to_string()],
    )
    .await;
    let protected_id = add_fixture_fact(
        &memory.service,
        "Restricted salary decision",
        "episode:restricted-scan-fact",
        cutoff,
        vec!["hr.salary".to_string()],
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        assemble_request(
            "ignored by wake-up view",
            cutoff,
            10,
            Some("wake_up"),
            Some(memory_mcp::models::AccessPayload {
                allowed_tags: Some(vec!["public".to_string()]),
                caller_id: None,
                session_vars: None,
                transport: None,
                content_type: None,
            }),
        ),
    )
    .await
    .expect("filtered wake-up context assembled");

    let returned_ids = items
        .iter()
        .map(|item| item.fact_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(returned_ids, [allowed_id.as_str()]);
    assert!(!returned_ids.contains(&protected_id.as_str()));
}

#[tokio::test]
async fn lexical_fallback_runs_the_nonsemantic_fact_scan() {
    let memory = common::TestMemory::new(false).await;
    let cutoff = Utc::now() + Duration::hours(1);
    let fact_id = add_fixture_fact(
        &memory.service,
        "Atlas migration review approved the Seldon protocol after Qzvortex validation.",
        "episode:lexical-scan-match",
        cutoff,
        vec![],
    )
    .await;

    let observer = Arc::new(RecordingObserver::default());
    memory
        .db_client
        .set_eval_query_observer(observer.clone() as Arc<dyn QueryObserver>);
    let items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        assemble_request("Atlas Seldon Qzvortex", cutoff, 5, None, None),
    )
    .await
    .expect("lexical fallback context assembled");

    assert_eq!(
        items.first().map(|item| item.fact_id.as_str()),
        Some(fact_id.as_str())
    );
    assert!(observer.observations().iter().any(|observation| {
        let sql = observation.sql.to_ascii_lowercase();
        sql.starts_with("select id, fact_id")
            && sql.contains("from fact")
            && !sql.contains(" where ")
    }));
}

#[tokio::test]
async fn full_scan_keeps_rows_after_the_first_1000_records() {
    let memory = common::TestMemory::new(false).await;
    let cutoff = Utc::now() + Duration::hours(1);
    let fact_time = cutoff - Duration::minutes(30);
    let mut expected_ids = Vec::with_capacity(1002);

    for index in 0..1000 {
        let content = format!("unrelated full scan fixture {index}");
        let source_episode = format!("episode:full-scan-row-{index}");
        expected_ids.push(
            add_fixture_fact(
                &memory.service,
                &content,
                &source_episode,
                fact_time,
                vec![],
            )
            .await,
        );
    }
    let inserted_late_id = add_fixture_fact(
        &memory.service,
        "deterministic late full scan fixture",
        "episode:full-scan-late-row",
        fact_time,
        vec![],
    )
    .await;
    expected_ids.push(inserted_late_id.clone());

    let late_id = "fact:zzzz_late_full_scan_row";
    let mut late_record = DbClient::select_one(memory.db_client.as_ref(), &inserted_late_id, "org")
        .await
        .expect("inserted late row read back")
        .expect("inserted late row exists")
        .as_object()
        .cloned()
        .expect("late row is an object");
    late_record.remove("id");
    late_record.insert("fact_id".to_string(), json!(late_id));
    DbClient::create(
        memory.db_client.as_ref(),
        late_id,
        Value::Object(late_record),
        "org",
        memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS,
    )
    .await
    .expect("fixed late row created");
    expected_ids.push(late_id.to_string());

    let persisted_facts = DbClient::select_table(
        memory.db_client.as_ref(),
        KnowledgeTables::table("fact"),
        "org",
    )
    .await
    .expect("all persisted facts selected");
    assert_eq!(persisted_facts.len(), expected_ids.len());
    let late_position = persisted_facts
        .iter()
        .position(|fact| fact.get("fact_id").and_then(Value::as_str) == Some(late_id))
        .expect("fixed late row is in the persisted fact table");
    assert!(
        late_position >= 1000,
        "fixture must sit after the first 1000 table rows, got position {late_position}"
    );

    let observer = Arc::new(RecordingObserver::default());
    memory
        .db_client
        .set_eval_query_observer(observer.clone() as Arc<dyn QueryObserver>);
    let items = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        assemble_request(
            "ignored by wake-up view",
            cutoff,
            i32::try_from(expected_ids.len()).expect("fixture count fits request budget"),
            Some("wake_up"),
            None,
        ),
    )
    .await
    .expect("complete wake-up scan assembled");

    let mut returned_ids = items
        .iter()
        .map(|item| item.fact_id.clone())
        .collect::<Vec<_>>();
    returned_ids.sort();
    expected_ids.sort();
    assert_eq!(returned_ids, expected_ids);
    assert!(observer.observations().iter().any(|observation| {
        let sql = observation.sql.to_ascii_lowercase();
        sql.starts_with("select id, fact_id")
            && sql.contains("from fact")
            && !sql.contains(" where ")
            && !sql.contains(" limit ")
    }));
}

macro_rules! active_projection_dimension_case {
    ($name:ident, $dimension:literal) => {
        #[tokio::test]
        async fn $name() {
            assert_active_projection_at_dimension($dimension).await;
        }
    };
}

macro_rules! decay_projection_dimension_case {
    ($name:ident, $dimension:literal) => {
        #[tokio::test]
        async fn $name() {
            assert_decay_projection_at_dimension($dimension).await;
        }
    };
}

macro_rules! scan_projection_dimension_case {
    ($name:ident, $dimension:literal) => {
        #[tokio::test]
        async fn $name() {
            assert_scan_projection_at_dimension($dimension).await;
        }
    };
}

macro_rules! ann_projection_dimension_case {
    ($name:ident, $dimension:literal) => {
        #[tokio::test]
        async fn $name() {
            assert_ann_projection_at_dimension($dimension).await;
        }
    };
}

active_projection_dimension_case!(active_projection_dimension_17, 17);
active_projection_dimension_case!(active_projection_dimension_384, 384);
active_projection_dimension_case!(active_projection_dimension_768, 768);
active_projection_dimension_case!(active_projection_dimension_1536, 1536);
active_projection_dimension_case!(active_projection_dimension_2048, 2048);
active_projection_dimension_case!(active_projection_dimension_3072, 3072);

decay_projection_dimension_case!(decay_projection_dimension_17, 17);
decay_projection_dimension_case!(decay_projection_dimension_384, 384);
decay_projection_dimension_case!(decay_projection_dimension_768, 768);
decay_projection_dimension_case!(decay_projection_dimension_1536, 1536);
decay_projection_dimension_case!(decay_projection_dimension_2048, 2048);
decay_projection_dimension_case!(decay_projection_dimension_3072, 3072);

scan_projection_dimension_case!(scan_projection_dimension_17, 17);
scan_projection_dimension_case!(scan_projection_dimension_384, 384);
scan_projection_dimension_case!(scan_projection_dimension_768, 768);
scan_projection_dimension_case!(scan_projection_dimension_1536, 1536);
scan_projection_dimension_case!(scan_projection_dimension_2048, 2048);
scan_projection_dimension_case!(scan_projection_dimension_3072, 3072);

ann_projection_dimension_case!(ann_projection_dimension_17, 17);
ann_projection_dimension_case!(ann_projection_dimension_384, 384);
ann_projection_dimension_case!(ann_projection_dimension_768, 768);
ann_projection_dimension_case!(ann_projection_dimension_1536, 1536);
ann_projection_dimension_case!(ann_projection_dimension_3072, 3072);
