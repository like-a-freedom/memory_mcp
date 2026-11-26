#![cfg(feature = "eval-support")]

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
        .find(|sql| {
            sql.to_ascii_lowercase()
                .contains("select source_episode from fact where source_episode in $episode_ids")
        })
        .cloned()
        .expect("observe the grouped active-fact query");
    let normalized = archival_query.to_ascii_lowercase();
    assert!(
        normalized.contains("group by source_episode"),
        "archival existence should group by episode: {archival_query}"
    );
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
    let base = "2026-01-01T00:00:00Z"
        .parse::<chrono::DateTime<Utc>>()
        .expect("fixed episode fixture time");
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
        let episode_id = format!("episode:{source_id}");
        db.create(
            &episode_id,
            json!({
                "episode_id": episode_id,
                "source_type": "document",
                "source_id": source_id,
                "content": content,
                "t_ref": normalize_dt(t_ingested),
                "t_ingested": normalize_dt(t_ingested),
                "policy_tags": if tag.is_empty() { Vec::new() } else { vec![tag] },
            }),
            "org",
            &["t_ref", "t_ingested"],
        )
        .await
        .expect("seed committed facet episode");
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
                t_ingested: None,
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

    let cutoff = normalize_dt(Utc::now() + Duration::seconds(1));
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

// Public storage characterization for late materialization. The independent SQL
// oracle deliberately pins the old query semantics rather than using its builder.
async fn assert_lexical_fact_projection(
    terms: &[&str],
    limit: i32,
    fact_types: &[String],
) -> Vec<Vec<Value>> {
    let (service, db) = common::make_service_with_client().await;
    let valid = chrono::DateTime::parse_from_rfc3339("2026-05-01T00:00:00Z")
        .unwrap()
        .to_utc();
    let quote = "full source payload αβ ".repeat(256);
    for (name, content, kind, valid_days, ingested_days, invalid, invalid_ingested, keys) in [
        (
            "old",
            "falcon location note",
            "note",
            0,
            0,
            None,
            None,
            vec![],
        ),
        (
            "tie-a",
            "falcon location note",
            "note",
            1,
            0,
            None,
            None,
            vec![],
        ),
        (
            "tie-b",
            "falcon location note",
            "note",
            1,
            0,
            None,
            None,
            vec![],
        ),
        (
            "index",
            "unrelated index carrier",
            "note",
            0,
            0,
            None,
            None,
            vec!["falcon"],
        ),
        (
            "both",
            "falcon location note",
            "note",
            0,
            0,
            None,
            None,
            vec!["falcon", "location"],
        ),
        (
            "future-ingestion",
            "falcon location note",
            "note",
            0,
            4,
            None,
            None,
            vec![],
        ),
        (
            "future-valid",
            "falcon location note",
            "note",
            4,
            0,
            None,
            None,
            vec![],
        ),
        (
            "closed",
            "falcon location note",
            "note",
            0,
            0,
            Some(1),
            Some(1),
            vec![],
        ),
        (
            "late-closure",
            "falcon location note",
            "note",
            0,
            0,
            Some(1),
            Some(4),
            vec![],
        ),
        (
            "other-type",
            "falcon location note",
            "decision",
            0,
            0,
            None,
            None,
            vec![],
        ),
    ] {
        let source = format!("episode:projection-{name}");
        let id = service
            .add_fact(
                kind,
                content,
                &quote,
                &source,
                valid + Duration::days(valid_days),
                0.91,
                Vec::new(),
                vec!["lexical".into()],
                Provenance::agent_observation(&source),
            )
            .await
            .unwrap();
        let mut assignments = vec![
            "t_ingested = type::datetime($ingested)",
            "index_keys = $keys",
        ];
        if invalid.is_some() {
            assignments.push("t_invalid = type::datetime($invalid)");
        }
        if invalid_ingested.is_some() {
            assignments.push("t_invalid_ingested = type::datetime($invalid_ingested)");
        }
        db.query(&format!("UPDATE type::record($id) SET {}", assignments.join(", ")), Some(json!({
            "id": id,
            "ingested": normalize_dt(valid + Duration::days(ingested_days)),
            "invalid": invalid.map(|days| normalize_dt(valid + Duration::days(days))),
            "invalid_ingested": invalid_ingested.map(|days| normalize_dt(valid + Duration::days(days))),
            "keys": keys,
        })), "org").await.unwrap();
    }
    let cutoff = "2026-05-04T00:00:00Z";
    const ORACLE: &str = "SELECT fact_id, fact_type, content, quote, source_episode, t_valid, t_ingested, t_invalid, t_invalid_ingested, confidence, index_keys, access_count, last_accessed, entity_links, scope, policy_tags, provenance, search::score(1) AS ft_score FROM fact WHERE t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff)) __TYPES__ AND (content @1@ __TERM__ OR index_keys @1@ __TERM__) ORDER BY ft_score DESC, t_valid DESC, fact_id ASC LIMIT $limit";
    let mut expected = vec![];
    for term in terms {
        let sql = ORACLE
            .replace(
                "__TYPES__",
                if fact_types.is_empty() {
                    ""
                } else {
                    "AND fact_type IN $fact_types"
                },
            )
            .replace("__TERM__", &serde_json::to_string(term).unwrap());
        let rows = db
            .query(
                &sql,
                Some(json!({"cutoff":cutoff,"limit":limit,"fact_types":fact_types})),
                "org",
            )
            .await
            .unwrap();
        expected.push(rows.as_array().unwrap().clone());
    }
    let mut actual = vec![];
    for term in terms {
        let (sql, vars) = memory_mcp::knowledge::queries::build_select_facts_filtered_query(
            cutoff,
            Some(term),
            limit,
            fact_types,
        );
        if std::env::var_os("MEMORY_LEXICAL_PLAN_DIAGNOSTICS").is_some() {
            let plan = db
                .query(&format!("{sql} EXPLAIN"), Some(vars.clone()), "org")
                .await
                .unwrap();
            println!("LEXICAL_QUERY_PLAN {}", json!({"sql":sql,"plan":plan}));
        }
        let rows = db.query(&sql, Some(vars), "org").await.unwrap();
        actual.push(rows.as_array().unwrap().clone());
    }
    assert_eq!(
        actual, expected,
        "exact ordered candidates, score and all source fields must match the independent full-row oracle"
    );
    let sources: Vec<_> = actual[0]
        .iter()
        .map(|row| row["source_episode"].as_str().unwrap())
        .collect();
    assert!(!sources.contains(&"episode:projection-future-ingestion"));
    assert!(!sources.contains(&"episode:projection-future-valid"));
    assert!(!sources.contains(&"episode:projection-closed"));
    assert_eq!(actual[0][0]["quote"], quote);
    actual
}

#[tokio::test]
async fn lexical_projection_preserves_complete_sources_and_temporal_visibility() {
    let actual = assert_lexical_fact_projection(&["falcon"], 20, &[]).await;
    let sources = actual[0]
        .iter()
        .map(|row| row["source_episode"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        sources,
        [
            "episode:projection-both",
            "episode:projection-old",
            "episode:projection-tie-a",
            "episode:projection-tie-b",
            "episode:projection-index",
            "episode:projection-late-closure",
            "episode:projection-other-type"
        ]
        .into()
    );
}

#[tokio::test]
async fn lexical_projection_preserves_each_limited_group_and_repeated_term() {
    let actual = assert_lexical_fact_projection(&["falcon", "absent", "falcon"], 2, &[]).await;
    assert_eq!(actual[0].len(), 2);
    assert!(actual[1].is_empty());
    assert_eq!(actual[0], actual[2]);
}

#[tokio::test]
async fn lexical_projection_preserves_type_filter_and_index_key_match() {
    let actual = assert_lexical_fact_projection(&["falcon"], 20, &["note".into()]).await;
    assert_eq!(actual[0].len(), 6);
    assert!(
        actual[0]
            .iter()
            .any(|row| row["source_episode"] == "episode:projection-index")
    );
    assert!(actual[0].iter().all(|row| row["fact_type"] == "note"));
}

#[tokio::test]
async fn lexical_sort_receives_only_candidate_identity_and_rank_fields() {
    let (_, db) = common::make_service_with_client().await;
    let (sql, vars) = memory_mcp::knowledge::queries::build_select_facts_filtered_query(
        "2026-05-04T00:00:00Z",
        Some("falcon"),
        2,
        &[],
    );
    let plan = db
        .query(&format!("{sql} EXPLAIN"), Some(vars), "org")
        .await
        .unwrap();
    fn find_sort(node: &Value) -> Option<&Value> {
        if node["operator"] == "Sort" {
            return Some(node);
        }
        node.get("children")
            .and_then(Value::as_array)?
            .iter()
            .find_map(find_sort)
    }
    let sort = plan
        .as_array()
        .unwrap()
        .iter()
        .find_map(find_sort)
        .expect("an actual FTS sort");
    let mut input = &sort["children"][0];
    while input["operator"] == "Subquery" {
        input = &input["children"][0];
    }
    assert_eq!(
        input["operator"], "SelectProject",
        "the actual engine must discard full source payload before sorting candidates"
    );
    assert_eq!(
        input["attributes"]["projections"],
        "record, fact_id, t_valid, ft_score"
    );
    let hydration = &plan.as_array().unwrap()[0];
    assert_eq!(hydration["operator"], "SelectProject");
    let limited = &hydration["children"][0];
    assert_eq!(limited["operator"], "Limit", "only winners may be hydrated");
    assert_eq!(&limited["children"][0], sort);
}

#[tokio::test]
async fn lexical_projection_preserves_multiword_matching_on_both_indexes() {
    let actual = assert_lexical_fact_projection(&["falcon location"], 20, &[]).await;
    assert!(
        actual[0]
            .iter()
            .any(|row| row["source_episode"] == "episode:projection-both")
    );
    assert!(
        actual[0]
            .iter()
            .all(|row| row["source_episode"] != "episode:projection-index")
    );
}

#[tokio::test]
async fn batched_lexical_sort_discards_full_sources_before_each_term_limit() {
    let (_, db) = common::make_service_with_client().await;
    let recorder = Arc::new(SqlRecorder::default());
    db.set_eval_query_observer(recorder.clone());
    memory_mcp::knowledge::KnowledgeStoreClient::new(db.clone(), "org")
        .select_lexical_fact_candidates("2026-05-04T00:00:00Z", &["falcon".into()], 2, &[])
        .await
        .unwrap();
    let sql = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .find(|sql| sql.contains("array::flatten(["))
        .expect("actual public batched read")
        .clone();
    // This fixed one-term fixture extracts the actual bounded term statement,
    // including nested projections, rather than constructing a second builder.
    let term_sql = sql
        .split_once("array::flatten([(")
        .unwrap()
        .1
        .split_once(")]) GROUP BY fact_id")
        .unwrap()
        .0;
    let plan = db
        .query(
            &format!("{term_sql} EXPLAIN"),
            Some(json!({"cutoff":"2026-05-04T00:00:00Z","limit":2})),
            "org",
        )
        .await
        .unwrap();
    fn find_sort(node: &Value) -> Option<&Value> {
        if node["operator"] == "Sort" {
            return Some(node);
        }
        node.get("children")
            .and_then(Value::as_array)?
            .iter()
            .find_map(find_sort)
    }
    let sort = plan
        .as_array()
        .unwrap()
        .iter()
        .find_map(find_sort)
        .expect("actual per-term sort");
    let mut input = &sort["children"][0];
    while input["operator"] == "Subquery" {
        input = &input["children"][0];
    }
    assert_eq!(
        input["operator"], "SelectProject",
        "each term must discard full source payload before sorting, not merely before union hydration"
    );
    assert_eq!(
        input["attributes"]["projections"],
        "record, fact_id, t_valid, ft_score, term_rank"
    );
}

#[tokio::test]
async fn batched_episode_sort_discards_full_sources_before_each_term_limit() {
    let (_, db) = common::make_service_with_client().await;
    db.create("episode:thin_source", json!({"episode_id":"episode:thin_source","source_type":"document","source_id":"thin-source","content":"Falcon literal source payload","t_ref":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","scope":"org","policy_tags":[]}), "org", &["t_ref","t_ingested"]).await.unwrap();
    let recorder = Arc::new(SqlRecorder::default());
    db.set_eval_query_observer(recorder.clone());
    EpisodeContextStore::new(db.clone(), "org")
        .select_episode_candidates("2026-05-04T00:00:00Z", &["falcon".into()], 2, &[])
        .await
        .unwrap();
    let sql = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .find(|sql| sql.contains("LET $pool = "))
        .expect("actual public batched read")
        .clone();
    // Execute the actual pre-sort pool separately: it must discard raw and
    // normalized source text, retaining only identity, ordering and membership.
    let pool_sql = sql
        .split_once("LET $pool = ")
        .unwrap()
        .1
        .split_once("; RETURN (")
        .unwrap()
        .0;
    let pool = db
        .query(
            &format!("RETURN {pool_sql}"),
            Some(json!({"cutoff":"2026-05-04T00:00:00Z","queries":["falcon"]})),
            "org",
        )
        .await
        .unwrap();
    let rows = pool.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["episode_id", "matched_terms", "record", "t_ref"]
    );
    assert_eq!(rows[0]["episode_id"], "episode:thin_source");
    assert_eq!(rows[0]["matched_terms"], json!([true]));
    let term_sql = sql
        .split_once("|$term_rank| (")
        .unwrap()
        .1
        .split_once("))) GROUP BY episode_id")
        .unwrap()
        .0;
    let plan = db
        .query(
            &format!("{term_sql} EXPLAIN"),
            Some(json!({"pool":pool,"limit":2,"term_rank":0})),
            "org",
        )
        .await
        .unwrap();
    fn find_sort(node: &Value) -> Option<&Value> {
        if node["operator"] == "Sort" {
            return Some(node);
        }
        node.get("children")
            .and_then(Value::as_array)?
            .iter()
            .find_map(find_sort)
    }
    let sort = plan
        .as_array()
        .unwrap()
        .iter()
        .find_map(find_sort)
        .expect("actual per-term sort");
    assert_eq!(
        sort["operator"], "Sort",
        "actual bounded term statement sorts the verified thin pool"
    );
}
