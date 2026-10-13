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
    const DIMENSION: usize = 2048;
    let (service, db_client) = dimensional_memory(DIMENSION).await;
    let now = Utc::now();
    let e0_id = add_fixture_fact(
        &service,
        "semantic basis e0 fixture",
        "episode:semantic-e0",
        now,
        vec![],
    )
    .await;
    let e1_id = add_fixture_fact(
        &service,
        "semantic basis e1 fixture",
        "episode:semantic-e1",
        now,
        vec![],
    )
    .await;
    let mut e0 = vec![0.0_f32; DIMENSION];
    e0[0] = 1.0;
    let mut e1 = vec![0.0_f32; DIMENSION];
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
        .expect("fixture embedding stored");
    }
    let store = KnowledgeStoreClient::new(db_client, "org");
    let mut query = vec![0.0; DIMENSION];
    query[0] = 1.0;

    let rows = store
        .select_facts_ann(&(now + Duration::seconds(1)).to_rfc3339(), &query, 1)
        .await
        .expect("ANN facts selected");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["fact_id"].as_str(), Some(e0_id.as_str()));
    assert_eq!(rows[0]["sem_score"].as_f64(), Some(1.0));
    assert!(rows[0].get("embedding").is_none());
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
