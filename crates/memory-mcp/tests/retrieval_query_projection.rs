mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use memory_mcp::eval_support::{QueryObserver, SqlObservation};
use memory_mcp::knowledge::graph_store::KnowledgeGraphStore;
use memory_mcp::memory::episode_context_store::EpisodeContextStore;
use memory_mcp::models::{IngestRequest, Provenance, SourceContext};
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::service::{MemoryService, normalize_dt};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::{Map, Value, json};
use std::sync::Mutex;

async fn service_with_dimension(
    dimension: usize,
    database: &str,
) -> (MemoryService, Arc<SurrealDbClient>) {
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_dimension(database, "org", "warn", dimension)
            .await
            .expect("connect dimension-specific embedded database"),
    );
    db.apply_migrations("org")
        .await
        .expect("apply current schema migrations");
    let service = MemoryService::new(db.clone(), "org".to_string(), "warn".to_string(), 50, 100)
        .expect("construct service with embeddings disabled");
    (service, db)
}

async fn assert_source_fact_projection(dimension: usize, persist_embedding: bool) {
    let database = format!("query_projection_{dimension}_{persist_embedding}");
    let (service, db) = service_with_dimension(dimension, &database).await;
    let t_valid = Utc::now() - Duration::seconds(2);
    let fact_id = service
        .add_fact(
            "note",
            "projection source fact",
            "literal source quote",
            "episode:projection-source",
            t_valid,
            0.91,
            Vec::new(),
            vec!["quality".to_string()],
            Provenance::agent_observation("episode:projection-source"),
        )
        .await
        .expect("seed source fact");

    if persist_embedding {
        db.update(
            &fact_id,
            json!({"embedding": vec![0.125_f32; dimension]}),
            "org",
            &[],
        )
        .await
        .expect("seed stored vector at the schema dimension");
    }

    let cutoff = normalize_dt(Utc::now() + Duration::seconds(2));
    let baseline = db
        .query(
            "SELECT * FROM fact WHERE source_episode = $episode_id \
             AND t_valid <= type::datetime($cutoff) \
             AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) \
             AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) \
                  OR t_invalid_ingested > type::datetime($cutoff)) \
             LIMIT $limit",
            Some(json!({
                "episode_id": "episode:projection-source",
                "cutoff": cutoff,
                "limit": 10,
            })),
            "org",
        )
        .await
        .expect("read the pre-change full-row oracle");
    let records = KnowledgeGraphStore::new(db, "org")
        .select_active_facts_by_episode("episode:projection-source", &cutoff, 10)
        .await
        .expect("select active source facts");

    let baseline_rows = baseline.as_array().expect("baseline result rows");
    assert_eq!(records.len(), 1, "the source fact remains retrievable");
    assert_eq!(baseline_rows.len(), records.len());
    assert_eq!(
        logical_fact_fields(&baseline_rows[0]),
        logical_fact_fields(&records[0]),
        "the projection must preserve every domain field consumed by retrieval"
    );
    let fact = &records[0];
    assert_eq!(fact["fact_id"], fact_id);
    assert_eq!(fact["content"], "projection source fact");
    assert_eq!(fact["quote"], "literal source quote");
    assert_eq!(fact["source_episode"], "episode:projection-source");
    assert!(fact["provenance"].is_object());
    assert!(
        !fact
            .as_object()
            .is_some_and(|record| record.contains_key("embedding")),
        "source sibling reads must not transfer a stored vector"
    );
    assert!(
        fact.get("id").is_none(),
        "the owner projection needs no record ID"
    );
    let response_bytes = serde_json::to_vec(&records)
        .expect("serialize projected source rows")
        .len();
    assert!(
        response_bytes < 2_048,
        "retrieval projection unexpectedly returned {response_bytes} bytes"
    );
}

fn logical_fact_fields(record: &Value) -> Map<String, Value> {
    const REQUIRED_FIELDS: &[&str] = &[
        "fact_id",
        "fact_type",
        "content",
        "quote",
        "source_episode",
        "t_valid",
        "t_ingested",
        "confidence",
        "index_keys",
        "access_count",
        "entity_links",
        "policy_tags",
        "provenance",
    ];
    const OPTIONAL_FIELDS: &[&str] = &["t_invalid", "t_invalid_ingested", "last_accessed", "scope"];

    let mut fields = Map::new();
    for field in REQUIRED_FIELDS {
        fields.insert(
            (*field).to_string(),
            record
                .get(*field)
                .unwrap_or_else(|| panic!("required fact field `{field}` is missing"))
                .clone(),
        );
    }
    for field in OPTIONAL_FIELDS {
        fields.insert(
            (*field).to_string(),
            record.get(*field).cloned().unwrap_or(Value::Null),
        );
    }
    fields
}

macro_rules! source_projection_dimension_case {
    ($name:ident, $dimension:literal) => {
        #[tokio::test]
        async fn $name() {
            assert_source_fact_projection($dimension, true).await;
        }
    };
}

source_projection_dimension_case!(source_siblings_do_not_materialize_17d_vectors, 17);
source_projection_dimension_case!(source_siblings_do_not_materialize_384d_vectors, 384);
source_projection_dimension_case!(source_siblings_do_not_materialize_768d_vectors, 768);
source_projection_dimension_case!(source_siblings_do_not_materialize_1536d_vectors, 1536);
source_projection_dimension_case!(source_siblings_do_not_materialize_2048d_vectors, 2048);
source_projection_dimension_case!(source_siblings_do_not_materialize_3072d_vectors, 3072);

#[tokio::test]
async fn source_siblings_preserve_no_embedding_mode() {
    assert_source_fact_projection(17, false).await;
}

#[derive(Default)]
struct SqlRecorder(Mutex<Vec<String>>);

impl QueryObserver for SqlRecorder {
    fn observe(&self, observation: &SqlObservation) {
        self.0
            .lock()
            .expect("query recorder lock")
            .push(observation.sql.clone());
    }
}

#[tokio::test]
async fn archival_existence_does_not_read_fact_payload() {
    let (service, db) = common::make_service_with_client().await;
    let recorder = Arc::new(SqlRecorder::default());
    db.set_eval_query_observer(recorder.clone());
    let old_date = Utc::now() - Duration::days(200);
    let episode_id = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
            source_type: "meeting".to_string(),
            source_id: "projection-archival-active-fact".to_string(),
            content: "An old episode protected by an active fact".to_string(),
            t_ref: old_date,
            t_ingested: Some(old_date),
            policy_tags: Vec::new(),
        },
        None,
    )
    .await
    .expect("seed old episode");
    let fact_id = service
        .add_fact(
            "note",
            "active archival guard fact",
            "active archival guard quote",
            &episode_id,
            old_date,
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation(&episode_id),
        )
        .await
        .expect("seed active fact");
    db.update(
        &fact_id,
        json!({"embedding": vec![0.25_f32; 1536]}),
        "org",
        &[],
    )
    .await
    .expect("seed stored vector");

    let archived = memory_mcp::platform::lifecycle_runtime::archival_pass(&service, 90)
        .await
        .expect("run archival pass");
    assert_eq!(archived, 0, "an active fact still prevents archival");

    let archival_query = recorder
        .0
        .lock()
        .expect("query recorder lock")
        .iter()
        .find(|sql| sql.contains("source_episode = $episode_id"))
        .cloned()
        .expect("observe the active-fact existence query");
    let normalized = archival_query.to_ascii_lowercase();
    assert!(
        !normalized.contains("select *"),
        "archival only needs existence, not the fact payload: {archival_query}"
    );
    assert!(
        !normalized.contains("embedding") && !normalized.contains("content"),
        "archival existence query must not read vectors or text: {archival_query}"
    );
}

#[tokio::test]
async fn active_fact_existence_preserves_bitemporal_invalidation_boundaries() {
    let (service, db) = service_with_dimension(1536, "query_projection_existence_boundaries").await;
    let t_valid = Utc::now() - Duration::seconds(5);
    let fact_id = service
        .add_fact(
            "note",
            "bitemporal existence fact",
            "bitemporal quote",
            "episode:existence-boundary",
            t_valid,
            0.8,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:existence-boundary"),
        )
        .await
        .expect("seed bitemporal fact");
    let pre_ingestion_cutoff = normalize_dt(t_valid);
    let visible_cutoff = Utc::now() + Duration::seconds(2);
    let store = KnowledgeGraphStore::new(db.clone(), "org");
    assert!(
        !store
            .has_active_facts_by_episode("episode:existence-boundary", &pre_ingestion_cutoff)
            .await
            .expect("check before ingestion time"),
        "a fact is not visible before its transaction time"
    );
    assert!(
        store
            .has_active_facts_by_episode(
                "episode:existence-boundary",
                &normalize_dt(visible_cutoff),
            )
            .await
            .expect("check active fact")
    );

    let t_invalid = visible_cutoff + Duration::seconds(10);
    let t_invalid_ingested = visible_cutoff + Duration::seconds(20);
    db.update(
        &fact_id,
        json!({
            "t_invalid": t_invalid,
            "t_invalid_ingested": t_invalid_ingested,
        }),
        "org",
        &["t_invalid", "t_invalid_ingested"],
    )
    .await
    .expect("set future bitemporal invalidation");

    assert!(
        store
            .has_active_facts_by_episode(
                "episode:existence-boundary",
                &normalize_dt(visible_cutoff + Duration::seconds(15)),
            )
            .await
            .expect("check invalidation event not yet ingested"),
        "an invalidation remains invisible before its transaction time"
    );
    assert!(
        !store
            .has_active_facts_by_episode(
                "episode:existence-boundary",
                &normalize_dt(visible_cutoff + Duration::seconds(21)),
            )
            .await
            .expect("check after invalidation ingestion"),
        "the fact is inactive after both invalidation times"
    );
}

#[tokio::test]
async fn facets_preserve_group_order_while_projecting_episode_inputs() {
    let (service, db) = common::make_service_with_client().await;
    let base = Utc::now() - Duration::minutes(10);
    for (source_id, content, tag, t_ingested) in [
        ("facet-alpha-old", "old alpha content", "alpha", base),
        (
            "facet-alpha-new",
            "new alpha content",
            "alpha",
            base + Duration::minutes(2),
        ),
        (
            "facet-zeta",
            "zeta content",
            "zeta",
            base + Duration::minutes(3),
        ),
        (
            "facet-uncategorized",
            "uncategorized content",
            "",
            base + Duration::minutes(4),
        ),
    ] {
        IngestCapability::ingest_from_service(
            &service,
            IngestRequest {
                source_context: None,
                source_type: "document".to_string(),
                source_id: source_id.to_string(),
                content: content.to_string(),
                t_ref: t_ingested,
                t_ingested: Some(t_ingested),
                policy_tags: if tag.is_empty() {
                    Vec::new()
                } else {
                    vec![tag.to_string()]
                },
            },
            None,
        )
        .await
        .expect("seed facet episode");
    }

    let rows = EpisodeContextStore::new(db.clone(), "org")
        .select_facet_inputs()
        .await
        .expect("read facet input rows");
    assert!(
        rows.iter().all(|row| row.get("id").is_none()),
        "facet input projection does not need Surreal record IDs"
    );

    let items = memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
        &service,
        memory_mcp::models::AssembleContextRequest {
            query: String::new(),
            as_of: Some(base + Duration::minutes(5)),
            budget: 10,
            view_mode: Some("facets".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            fact_types: Vec::new(),
            compact: false,
        },
    )
    .await
    .expect("assemble facets view");

    let labels = items
        .iter()
        .map(|item| item.content.as_str())
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["uncategorized", "zeta", "alpha"]);
    let alpha = items
        .iter()
        .find(|item| item.content == "alpha")
        .expect("alpha facet");
    assert_eq!(alpha.provenance["count"], 2);
}

#[tokio::test]
async fn source_neighborhood_projection_preserves_order_and_content() {
    let (service, db) = common::make_service_with_client().await;
    let base = Utc::now() - Duration::minutes(5);
    let mut middle_episode_id = String::new();
    for (position, source_id, content) in [
        (1, "projection-source-1", "first source turn"),
        (2, "projection-source-2", "middle source turn"),
        (3, "projection-source-3", "last source turn"),
    ] {
        let episode_id = IngestCapability::ingest_from_service(
            &service,
            IngestRequest {
                source_context: Some(SourceContext {
                    group: "projection-source-group".to_string(),
                    position,
                }),
                source_type: "conversation".to_string(),
                source_id: source_id.to_string(),
                content: content.to_string(),
                t_ref: base + Duration::seconds(position as i64),
                t_ingested: Some(base + Duration::seconds(position as i64)),
                policy_tags: vec!["source-visible".to_string()],
            },
            None,
        )
        .await
        .expect("seed source-context episode");
        if position == 2 {
            middle_episode_id = episode_id;
        }
    }

    let cutoff = normalize_dt(base + Duration::minutes(1));
    let rows = EpisodeContextStore::new(db, "org")
        .select_source_neighborhoods_for_episodes(&[middle_episode_id], &cutoff, None, None)
        .await
        .expect("read source neighborhood");
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter()
            .map(|row| row["content"].as_str().expect("source content"))
            .collect::<Vec<_>>(),
        vec![
            "first source turn",
            "middle source turn",
            "last source turn"
        ]
    );
    assert!(rows.iter().all(|row| row.get("id").is_none()));
    assert!(rows.iter().all(|row| row.get("policy_tags").is_some()));
}
