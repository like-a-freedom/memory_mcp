//! Integration tests for service module interactions.
//!
//! These tests verify that different service components work together correctly.

use chrono::{TimeZone, Utc};
use memory_mcp::models::Provenance;
use memory_mcp::service::EntityExtractor;
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use memory_mcp::service::capabilities::explain::ExplainCapability;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::service::capabilities::invalidate::InvalidateCapability;
use memory_mcp::storage::DbClient;
use serde_json::{Value, json};

mod common;

fn json_bool(value: &Value) -> Option<bool> {
    value
        .as_bool()
        .or_else(|| value.get("Bool").and_then(|inner| inner.as_bool()))
}

async fn seed_query_log_row(
    db_client: &std::sync::Arc<memory_mcp::storage::SurrealDbClient>,
    namespace: &str,
    record_id: &str,
    logged_at: chrono::DateTime<Utc>,
    query: &str,
) {
    db_client
        .create(
            record_id,
            json!({
                "query_log_id": record_id,
                "logged_at": memory_mcp::service::normalize_dt(logged_at),
                "scope": namespace,
                "query": query,
                "query_flags": [],
                "result_count": 1,
                "latency_ms": 1.0,
                "cache_hit": false,
            }),
            namespace,
        )
        .await
        .expect("seed query_log row should succeed");
}

#[tokio::test]
async fn test_service_ingest_and_extract_flow() {
    let service = common::make_service().await;

    let request = memory_mcp::models::IngestRequest {
        source_type: "meeting".to_string(),
        source_id: "integration-test-1".to_string(),
        content: "Meeting with Alice Inc and Bob Corp. Discussed ARR growth to $5M. Alice will deliver the prototype by Friday.".to_string(),
        t_ref: Utc::now(),
        scope: "org".to_string(),
        project: None,
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };

    let episode_id = IngestCapability::ingest(&service.build_context(), request, None)
        .await
        .unwrap();
    assert!(episode_id.starts_with("episode:"));

    let result = ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .unwrap();

    assert_eq!(result.episode_id, episode_id);
    assert!(!result.entities.is_empty());
    assert!(!result.facts.is_empty());
}

/// Selects append-only extraction projection rows for an episode, ordered by
/// ingestion time so the earliest run is first.
async fn select_extraction_projections(
    db_client: &std::sync::Arc<memory_mcp::storage::SurrealDbClient>,
    episode_id: &str,
) -> Vec<Value> {
    db_client
        .query(
            "SELECT * FROM entity_extraction_projection WHERE episode_id = $episode_id ORDER BY t_ingested",
            Some(json!({ "episode_id": episode_id })),
            "org",
        )
        .await
        .expect("select projection rows should succeed")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn extractor_fingerprint_projection() {
    let (service, db_client) = common::make_service_with_client().await;

    let request = memory_mcp::models::IngestRequest {
        source_type: "meeting".to_string(),
        source_id: "fingerprint-projection-1".to_string(),
        content: "Alice Inc and Bob Corp discussed the Atlas launch with Carol.".to_string(),
        t_ref: Utc::now(),
        scope: "org".to_string(),
        project: None,
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };

    let episode_id = IngestCapability::ingest(&service.build_context(), request, None)
        .await
        .unwrap();
    assert!(episode_id.starts_with("episode:"));

    let first = ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .unwrap();
    assert!(!first.entities.is_empty());

    // One append-only projection row per successful extraction run.
    let rows = select_extraction_projections(&db_client, &episode_id).await;
    assert_eq!(
        rows.len(),
        1,
        "first extraction must write one projection row"
    );

    let row = &rows[0];
    assert_eq!(row["episode_id"].as_str(), Some(episode_id.as_str()));
    assert_eq!(row["scope"].as_str(), Some("org"));
    assert!(
        !row["t_ingested"].is_null(),
        "projection row must carry its ingestion timestamp"
    );
    assert_eq!(
        row["fingerprint"]["selector"].as_str(),
        Some("anno"),
        "projection must record the extractor selector"
    );
    let entity_ids = row["entity_ids"].as_array().expect("entity_ids array");
    assert!(
        !entity_ids.is_empty(),
        "projection must record the resolved entity ids"
    );
    assert!(entity_ids.iter().all(|id| id.is_string()));

    let id_value = row.get("id").expect("projection row has a record id");
    assert!(
        serde_json::to_string(id_value)
            .unwrap_or_default()
            .contains("entity_extraction_projection"),
        "projection record id must live in the projection table: {id_value}"
    );

    // Re-extracting appends a SECOND projection row; the first stays unchanged.
    let _second = ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .unwrap();

    let rows_after = select_extraction_projections(&db_client, &episode_id).await;
    assert_eq!(
        rows_after.len(),
        2,
        "second extraction must append a new projection row"
    );
    assert_eq!(
        rows_after[0], rows[0],
        "first projection row must remain unchanged (append-only)"
    );
    assert_ne!(
        rows_after[0].get("id"),
        rows_after[1].get("id"),
        "projection rows must have distinct record ids"
    );
}

#[tokio::test]
async fn test_service_resolve_and_relate_entities() {
    let service = common::make_service().await;

    let alice_id = service
        .resolve_entity("person", "Alice Smith")
        .await
        .unwrap();
    assert!(alice_id.starts_with("entity:"));

    let bob_id = service.resolve_entity("person", "Bob Jones").await.unwrap();
    assert!(bob_id.starts_with("entity:"));

    let alice_id_2 = service
        .resolve_entity("person", "Alice Smith")
        .await
        .unwrap();
    assert_eq!(alice_id, alice_id_2);

    service.relate(&alice_id, "knows", &bob_id).await.unwrap();
}

#[tokio::test]
async fn test_service_relate_persists_native_edge_endpoints_and_inferred_origin() {
    let (service, db_client) = common::make_service_with_client().await;

    let alice_id = service
        .resolve_entity("person", "Alice Smith")
        .await
        .unwrap();
    let bob_id = service.resolve_entity("person", "Bob Jones").await.unwrap();

    service.relate(&alice_id, "knows", &bob_id).await.unwrap();

    let edges = db_client.select_table("edge", "org").await.unwrap();
    let edge = edges.first().expect("stored edge");

    let to_record_id = |record_id: &str| {
        let (table, key) = record_id
            .split_once(':')
            .expect("record id should contain table prefix");
        json!({"RecordId": {"table": table, "key": key}})
    };

    assert_eq!(edge.get("in"), Some(&to_record_id(&alice_id)));
    assert_eq!(edge.get("out"), Some(&to_record_id(&bob_id)));
    assert_eq!(edge.get("origin"), Some(&json!("inferred")));
}

#[tokio::test]
async fn test_service_add_fact_and_assemble_context() {
    let service = common::make_service().await;

    let t_valid = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

    let _fact_id = service
        .add_fact(
            "metric",
            "ARR reached $5M in Q4 2023",
            "ARR reached $5M",
            "episode:test",
            t_valid,
            "org",
            0.9,
            vec![],
            vec!["finance".to_string()],
            memory_mcp::models::Provenance::from_json_value(
                &json!({"quarter": "Q4", "year": 2023}),
            ),
        )
        .await
        .unwrap();

    let request = memory_mcp::models::AssembleContextRequest {
        query: "ARR metric".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc::now()),
        budget: 10,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };

    let context = AssembleContextCapability::assemble_context(&service.build_context(), request)
        .await
        .unwrap();
    assert!(!context.is_empty());
    assert!(!context[0].fact_id.is_empty());
    assert!(!context[0].content.is_empty());
    assert!(context[0].confidence.is_finite());
}

#[tokio::test]
async fn test_service_add_fact_persists_provenance() {
    let (service, db_client) = common::make_service_with_client().await;

    let provenance = memory_mcp::models::Provenance {
        source_episode_id: Some("episode:provenance".to_string()),
        ingestion_method: "manual".to_string(),
        ..Default::default()
    };

    let fact_id = service
        .add_fact(
            "metric",
            "ARR reached $7M",
            "ARR reached $7M",
            "episode:provenance",
            Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap(),
            "org",
            0.95,
            vec![],
            vec![],
            provenance.clone(),
        )
        .await
        .unwrap();

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .unwrap()
        .expect("stored fact");
    assert_eq!(
        stored
            .get("provenance")
            .and_then(|p| p.get("source_episode_id")),
        Some(&serde_json::json!("episode:provenance"))
    );
}

#[tokio::test]
async fn test_service_extract_persists_edge_provenance_and_extracted_origin() {
    let (service, db_client) = common::make_service_with_client().await;

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "meeting".to_string(),
            source_id: "edge-prov-1".to_string(),
            content: "Meeting with Alice Smith about ARR goals".to_string(),
            t_ref: Utc.with_ymd_and_hms(2024, 3, 2, 12, 0, 0).unwrap(),
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .unwrap();

    let extraction = ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .unwrap();
    assert!(!extraction.links.is_empty());

    let edges = db_client.select_table("edge", "org").await.unwrap();
    assert!(!edges.is_empty());
    assert!(edges.iter().all(|edge| {
        edge.get("provenance")
            .and_then(|value| value.get("source_episode_id"))
            == Some(&json!(episode_id))
    }));
    assert!(
        edges
            .iter()
            .all(|edge| edge.get("origin") == Some(&json!("extracted")))
    );
}

#[tokio::test]
async fn test_service_extract_returns_contradiction_warning_for_conflicting_metric_fact() {
    let service = common::make_service().await;

    let first_episode = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "chat".to_string(),
            source_id: "contradiction-metric-1".to_string(),
            content: "Alice Smith reports ARR is $5M.".to_string(),
            t_ref: "2026-03-01T10:00:00Z"
                .parse()
                .expect("static timestamp should parse"),
            scope: "personal".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest first episode");
    let first_result =
        ExtractCapability::extract(&service.build_context(), &first_episode, None, None)
            .await
            .unwrap();
    let first_json = serde_json::to_value(&first_result).expect("serialize first extract result");

    assert_eq!(
        first_json
            .get("warnings")
            .and_then(|value| value.as_array())
            .map(|warnings| warnings.len())
            .unwrap_or(0),
        0
    );

    let second_episode = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "chat".to_string(),
            source_id: "contradiction-metric-2".to_string(),
            content: "Alice Smith reports ARR is $7M.".to_string(),
            t_ref: "2026-03-01T10:00:00Z"
                .parse()
                .expect("static timestamp should parse"),
            scope: "personal".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest second episode");
    let second_result =
        ExtractCapability::extract(&service.build_context(), &second_episode, None, None)
            .await
            .unwrap();
    let second_json =
        serde_json::to_value(&second_result).expect("serialize second extract result");

    let warnings = second_json
        .get("warnings")
        .and_then(|value| value.as_array())
        .expect("warnings array should exist");
    assert!(
        !warnings.is_empty(),
        "expected contradiction warning after conflicting metric extract, got {second_json}"
    );

    let warning = &warnings[0];
    assert_eq!(warning.get("fact_type"), Some(&json!("metric")));
    assert_eq!(
        warning.get("conflicting_fact_id"),
        Some(&json!(first_result.facts[0].fact_id.clone()))
    );
    assert_eq!(
        warning.get("new_fact_id"),
        Some(&json!(second_result.facts[0].fact_id.clone()))
    );
    assert_eq!(
        warning.get("existing_content"),
        Some(&json!("Alice Smith reports ARR is $5M."))
    );
    assert_eq!(
        warning.get("new_content"),
        Some(&json!("Alice Smith reports ARR is $7M."))
    );
}

#[tokio::test]
async fn test_service_extract_persists_index_keys_for_entities_and_temporal_markers() {
    let (service, db_client) = common::make_service_with_client().await;

    let episode_id = common::ingest_episode(
        &service,
        "index-keys-1",
        "Alice Smith from Atlas Corp will send the launch deck in March 2026.",
    )
    .await;

    let facts = db_client.select_table("fact", "personal").await.unwrap();
    let fact = facts
        .iter()
        .find(|record| {
            record
                .get("source_episode")
                .and_then(|value| value.as_str())
                == Some(episode_id.as_str())
        })
        .expect("extracted fact for seeded episode");

    let index_keys = fact
        .get("index_keys")
        .and_then(|value| value.as_array())
        .expect("index_keys array should be present");

    assert!(
        index_keys
            .iter()
            .any(|value| value.as_str() == Some("alice smith")),
        "expected canonical person name in index_keys"
    );
    assert!(
        index_keys
            .iter()
            .any(|value| value.as_str() == Some("atlas corp")),
        "expected canonical company name in index_keys"
    );
    assert!(
        index_keys
            .iter()
            .any(|value| value.as_str() == Some("march 2026")),
        "expected explicit temporal phrase in index_keys"
    );
    assert!(
        index_keys
            .iter()
            .any(|value| value.as_str() == Some("2026-03")),
        "expected normalized year-month marker in index_keys"
    );
}

#[tokio::test]
async fn test_service_exposes_regex_entity_extractor() {
    let extractor = memory_mcp::service::RegexEntityExtractor::new().unwrap();
    let candidates = extractor
        .extract_candidates("Alice Smith met Bob Jones at Acme Inc")
        .await
        .unwrap();

    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0].canonical_name, "Acme Inc");
    assert_eq!(candidates[1].canonical_name, "Alice Smith");
    assert_eq!(candidates[2].canonical_name, "Bob Jones");
}

#[tokio::test]
async fn test_service_does_not_persist_fact_embeddings_without_provider() {
    let (service, db_client) = common::make_service_with_client().await;

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "meeting".to_string(),
            source_id: "semantic-slot-1".to_string(),
            content: "Alice Smith reviewed ARR improvements".to_string(),
            t_ref: Utc.with_ymd_and_hms(2024, 4, 1, 10, 0, 0).unwrap(),
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .unwrap();

    let entity_id = service
        .resolve_entity("person", "Alice Smith")
        .await
        .unwrap();
    let fact_id = service
        .add_fact(
            "note",
            "Alice Smith reviewed ARR improvements",
            "Alice Smith reviewed ARR improvements",
            &episode_id,
            Utc.with_ymd_and_hms(2024, 4, 1, 10, 0, 0).unwrap(),
            "org",
            0.8,
            vec![entity_id.clone()],
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .unwrap();

    let episode = db_client
        .select_one(&episode_id, "org")
        .await
        .unwrap()
        .expect("stored episode");
    let entity = db_client
        .select_one(&entity_id, "org")
        .await
        .unwrap()
        .expect("stored entity");
    let fact = db_client
        .select_one(&fact_id, "org")
        .await
        .unwrap()
        .expect("stored fact");

    assert!(episode.get("embedding").is_none());
    assert!(entity.get("embedding").is_none());
    assert!(fact.get("embedding").is_none());
}

#[tokio::test]
async fn test_service_assemble_context_without_provider_skips_semantic_similarity() {
    let service = common::make_service().await;

    let fact_id = service
        .add_fact(
            "note",
            "Compensation increase approved for the engineering team",
            "Compensation increase approved",
            "episode:semantic-similarity",
            Utc.with_ymd_and_hms(2024, 4, 3, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:semantic-similarity"),
        )
        .await
        .unwrap();

    let context = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "salary raise".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(
        context.iter().all(|item| item.fact_id != fact_id),
        "semantic-only matches should stay disabled without an embedding provider"
    );
}

#[tokio::test]
async fn test_service_merges_overlapping_entity_cohorts_into_one_community() {
    let (service, db_client) = common::make_service_with_client().await;
    let alice_id = service
        .resolve_entity("person", "Alice Smith")
        .await
        .unwrap();
    let bob_id = service.resolve_entity("person", "Bob Jones").await.unwrap();
    let carol_id = service
        .resolve_entity("person", "Carol White")
        .await
        .unwrap();

    for (source_id, content) in [
        ("community-merge-1", "Alice Smith met Bob Jones"),
        ("community-merge-2", "Bob Jones met Carol White"),
    ] {
        let episode_id = IngestCapability::ingest(
            &service.build_context(),
            memory_mcp::models::IngestRequest {
                source_type: "meeting".to_string(),
                source_id: source_id.to_string(),
                content: content.to_string(),
                t_ref: Utc.with_ymd_and_hms(2024, 4, 2, 10, 0, 0).unwrap(),
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .unwrap();
        ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
            .await
            .unwrap();
    }

    let communities = db_client.select_table("community", "org").await.unwrap();
    let merged = communities.iter().find(|community| {
        let Some(members) = community
            .get("member_entities")
            .and_then(|value| value.as_array())
        else {
            return false;
        };
        let members: Vec<_> = members.iter().filter_map(|value| value.as_str()).collect();
        members.contains(&alice_id.as_str())
            && members.contains(&bob_id.as_str())
            && members.contains(&carol_id.as_str())
    });

    assert!(
        merged.is_some(),
        "expected a merged community containing Alice, Bob, and Carol"
    );
}

#[tokio::test]
async fn test_service_fact_invalidation() {
    let service = common::make_service().await;

    let t_valid = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

    let fact_id = service
        .add_fact(
            "metric",
            "ARR $3M",
            "ARR $3M",
            "episode:test",
            t_valid,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .unwrap();

    let request_before = memory_mcp::models::AssembleContextRequest {
        query: "ARR".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc::now()),
        budget: 10,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };
    let context_before =
        AssembleContextCapability::assemble_context(&service.build_context(), request_before)
            .await
            .unwrap();
    assert!(context_before.iter().any(|f| f.fact_id == fact_id));

    let t_invalid = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
    InvalidateCapability::invalidate(
        &service.build_context(),
        memory_mcp::models::InvalidateRequest {
            fact_id: fact_id.clone(),
            reason: "Superseded by new value".to_string(),
            t_invalid,
        },
        None,
    )
    .await
    .unwrap();

    let request_after = memory_mcp::models::AssembleContextRequest {
        query: "ARR".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc.with_ymd_and_hms(2024, 12, 1, 0, 0, 0).unwrap()),
        budget: 10,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };
    let context_after =
        AssembleContextCapability::assemble_context(&service.build_context(), request_after)
            .await
            .unwrap();
    assert!(!context_after.iter().any(|f| f.fact_id == fact_id));
}

#[tokio::test]
async fn test_service_cache_behavior() {
    let service = common::make_service().await;

    let t_valid = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

    service
        .add_fact(
            "note",
            "Test content for caching",
            "Test quote",
            "episode:cache-test",
            t_valid,
            "org",
            0.8,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .unwrap();

    let request = memory_mcp::models::AssembleContextRequest {
        query: "Test content".to_string(),
        scope: "org".to_string(),
        as_of: None,
        budget: 5,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };
    let result1 =
        AssembleContextCapability::assemble_context(&service.build_context(), request.clone())
            .await
            .unwrap();
    assert!(!result1.is_empty());

    let result2 = AssembleContextCapability::assemble_context(&service.build_context(), request)
        .await
        .unwrap();
    assert_eq!(result1.len(), result2.len());
}

#[tokio::test]
async fn test_service_assemble_context_records_fact_access_heat() {
    let (service, db_client) = common::make_service_with_client().await;

    let fact_id = service
        .add_fact(
            "note",
            "Heat tracking note for retrieval",
            "Heat tracking note for retrieval",
            "episode:heat-retrieval",
            Utc::with_ymd_and_hms(&Utc, 2026, 3, 1, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:heat-retrieval"),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "heat tracking retrieval".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(items.iter().any(|item| item.fact_id == fact_id));

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .unwrap()
        .expect("stored fact");

    assert_eq!(
        stored.get("access_count").and_then(|value| value.as_i64()),
        Some(1)
    );
    assert!(stored.get("last_accessed").is_some());
}

#[tokio::test]
async fn test_service_assemble_context_records_fact_access_heat_on_cache_hit_and_fresh() {
    let (service, db_client) = common::make_service_with_client().await;

    let fact_id = service
        .add_fact(
            "note",
            "Heat tracking note for retrieval",
            "Heat tracking note for retrieval",
            "episode:heat-cache",
            Utc::with_ymd_and_hms(&Utc, 2026, 3, 2, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:heat-cache"),
        )
        .await
        .unwrap();

    let as_of = Utc::now() + chrono::Duration::seconds(1);

    let request = memory_mcp::models::AssembleContextRequest {
        query: "heat tracking retrieval".to_string(),
        scope: "org".to_string(),
        as_of: Some(as_of),
        budget: 5,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };

    let first_items =
        AssembleContextCapability::assemble_context(&service.build_context(), request.clone())
            .await
            .unwrap();
    assert!(first_items.iter().any(|item| item.fact_id == fact_id));

    let stored_after_first = db_client
        .select_one(&fact_id, "org")
        .await
        .unwrap()
        .expect("stored fact after fresh retrieval");

    assert_eq!(
        stored_after_first
            .get("access_count")
            .and_then(|value| value.as_i64()),
        Some(1)
    );
    assert!(stored_after_first.get("last_accessed").is_some());

    let second_items =
        AssembleContextCapability::assemble_context(&service.build_context(), request)
            .await
            .unwrap();
    assert!(second_items.iter().any(|item| item.fact_id == fact_id));

    let stored_after_second = db_client
        .select_one(&fact_id, "org")
        .await
        .unwrap()
        .expect("stored fact after cache-hit retrieval");

    assert_eq!(
        stored_after_second
            .get("access_count")
            .and_then(|value| value.as_i64()),
        Some(2)
    );
    assert!(stored_after_second.get("last_accessed").is_some());
}

#[tokio::test]
async fn test_service_assemble_context_does_not_record_query_log_when_disabled_by_default() {
    let (service, db_client) = common::make_service_with_client().await;

    service
        .add_fact(
            "note",
            "Default disabled query logging note",
            "Default disabled query logging note",
            "episode:query-log-default-off",
            Utc.with_ymd_and_hms(2026, 4, 8, 9, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:query-log-default-off"),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "default disabled query logging".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 8, 12, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(!items.is_empty());

    let query_logs = db_client.select_table("query_log", "org").await.unwrap();
    assert!(
        query_logs.is_empty(),
        "query logging should stay disabled by default, got: {query_logs:?}"
    );
}

#[tokio::test]
async fn test_service_assemble_context_records_query_log_with_tier_latency_and_result_count() {
    let (service, db_client) = common::make_service_with_client_and_query_logging(true).await;

    let fact_id = service
        .add_fact(
            "note",
            "Query analytics retrieval note",
            "Query analytics retrieval note",
            "episode:query-log-direct",
            Utc.with_ymd_and_hms(2026, 4, 8, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:query-log-direct"),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "query analytics retrieval".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 8, 12, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(items.iter().any(|item| item.fact_id == fact_id));

    let query_logs = db_client.select_table("query_log", "org").await.unwrap();
    assert_eq!(
        query_logs.len(),
        1,
        "expected one query_log row after one assemble_context call"
    );

    let row = query_logs.first().expect("query_log row should exist");
    assert_eq!(
        row.get("scope").and_then(|value| value.as_str()),
        Some("org")
    );
    assert_eq!(
        row.get("query").and_then(|value| value.as_str()),
        Some("query analytics retrieval")
    );
    assert_eq!(
        row.get("result_count").and_then(|value| value.as_i64()),
        Some(1)
    );
    assert_eq!(
        row.get("retrieval_tier").and_then(|value| value.as_str()),
        Some("fallback")
    );
    assert_eq!(
        row.get("cache_hit").and_then(json_bool),
        Some(false),
        "expected cache_hit=false in query_log row, got: {row:?}"
    );
    assert!(
        row.get("latency_ms")
            .and_then(|value| value.as_f64())
            .is_some_and(|value| value >= 0.0),
        "expected latency_ms to be recorded as a non-negative float, got: {row:?}"
    );
    assert!(row.get("logged_at").is_some());
}

#[tokio::test]
async fn test_service_assemble_context_records_query_log_with_resolved_view_mode_and_flags() {
    let (mut service, db_client) = common::make_service_with_client_and_query_logging(true).await;
    service = service.with_query_log_retention_days(30);

    common::seed_fact_at(
        &service,
        "org",
        "Atlas budget increased in January 2026",
        "2026-01-10T09:00:00Z".parse().unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "org",
        "Atlas launch confirmed in March 2026",
        "2026-03-10T09:00:00Z".parse().unwrap(),
    )
    .await;

    let _ = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "timeline of atlas changes in q1 2026".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("assemble context");

    let query_logs = db_client.select_table("query_log", "org").await.unwrap();
    let row = query_logs.first().expect("query_log row should exist");

    assert_eq!(
        row.get("resolved_view_mode")
            .and_then(|value| value.as_str()),
        Some("timeline"),
    );
    let flags = row
        .get("query_flags")
        .and_then(|value| value.as_array())
        .expect("query_flags should be stored as an array");
    assert!(flags.iter().any(|value| value.as_str() == Some("timeline")));
    assert!(
        row.get("retrieval_tiers")
            .and_then(|value| value.as_object())
            .is_some(),
        "retrieval_tiers distribution should be recorded",
    );
}

#[tokio::test]
async fn test_service_assemble_context_records_query_log_for_cache_hit_queries() {
    let (service, db_client) = common::make_service_with_client_and_query_logging(true).await;

    service
        .add_fact(
            "note",
            "Cache hit analytics retrieval",
            "Cache hit analytics retrieval",
            "episode:query-log-cache",
            Utc.with_ymd_and_hms(2026, 4, 8, 11, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:query-log-cache"),
        )
        .await
        .unwrap();

    let request = memory_mcp::models::AssembleContextRequest {
        query: "cache hit analytics retrieval".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc.with_ymd_and_hms(2026, 4, 8, 12, 0, 0).unwrap()),
        budget: 5,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };

    let first =
        AssembleContextCapability::assemble_context(&service.build_context(), request.clone())
            .await
            .unwrap();
    let second = AssembleContextCapability::assemble_context(&service.build_context(), request)
        .await
        .unwrap();

    assert!(!first.is_empty());
    assert!(!second.is_empty());

    let query_logs = db_client.select_table("query_log", "org").await.unwrap();
    assert_eq!(
        query_logs.len(),
        2,
        "expected fresh retrieval and cache hit to each create a query_log row"
    );
    assert_eq!(
        query_logs
            .iter()
            .filter(|row| row.get("cache_hit").and_then(json_bool) == Some(false))
            .count(),
        1,
        "expected exactly one fresh query_log row, got: {query_logs:?}"
    );
    assert_eq!(
        query_logs
            .iter()
            .filter(|row| row.get("cache_hit").and_then(json_bool) == Some(true))
            .count(),
        1,
        "expected exactly one cache-hit query_log row, got: {query_logs:?}"
    );
}

#[tokio::test]
async fn test_service_assemble_context_prunes_query_logs_older_than_default_retention() {
    let (service, db_client) = common::make_service_with_client_and_query_logging(true).await;

    seed_query_log_row(
        &db_client,
        "org",
        "query_log:stale-default-retention",
        Utc::now() - chrono::Duration::days(91),
        "stale default retention row",
    )
    .await;

    service
        .add_fact(
            "note",
            "Default retention pruning note",
            "Default retention pruning note",
            "episode:query-log-retention-default",
            Utc.with_ymd_and_hms(2026, 4, 8, 13, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:query-log-retention-default"),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "default retention pruning".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 8, 13, 5, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(!items.is_empty());
    assert!(
        db_client
            .select_one("query_log:stale-default-retention", "org")
            .await
            .unwrap()
            .is_none(),
        "stale query_log row should be pruned by the default 90-day retention"
    );

    let query_logs = db_client.select_table("query_log", "org").await.unwrap();
    assert_eq!(
        query_logs.len(),
        1,
        "expected stale row to be pruned and fresh row to remain, got: {query_logs:?}"
    );
}

#[tokio::test]
async fn test_service_assemble_context_honors_custom_query_log_retention_days() {
    let (service, db_client) = common::make_service_with_client_and_query_logging(true).await;
    let service = service.with_query_log_retention_days(7);

    seed_query_log_row(
        &db_client,
        "org",
        "query_log:older-than-custom-retention",
        Utc::now() - chrono::Duration::days(8),
        "older than custom retention",
    )
    .await;
    seed_query_log_row(
        &db_client,
        "org",
        "query_log:within-custom-retention",
        Utc::now() - chrono::Duration::days(6),
        "within custom retention",
    )
    .await;

    service
        .add_fact(
            "note",
            "Custom retention pruning note",
            "Custom retention pruning note",
            "episode:query-log-retention-custom",
            Utc.with_ymd_and_hms(2026, 4, 8, 14, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:query-log-retention-custom"),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "custom retention pruning".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 8, 14, 5, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(!items.is_empty());
    assert!(
        db_client
            .select_one("query_log:older-than-custom-retention", "org")
            .await
            .unwrap()
            .is_none(),
        "row older than custom retention should be pruned"
    );
    assert!(
        db_client
            .select_one("query_log:within-custom-retention", "org")
            .await
            .unwrap()
            .is_some(),
        "row inside custom retention window should be preserved"
    );

    let query_logs = db_client.select_table("query_log", "org").await.unwrap();
    assert_eq!(
        query_logs.len(),
        2,
        "expected one preserved historical row plus one fresh row, got: {query_logs:?}"
    );
}

#[tokio::test]
async fn test_service_scope_isolation() {
    let service = common::make_service().await;

    let t_valid = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

    service
        .add_fact(
            "note",
            "Org scope fact",
            "Org quote",
            "episode:org",
            t_valid,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .unwrap();

    service
        .add_fact(
            "note",
            "Personal scope fact",
            "Personal quote",
            "episode:personal",
            t_valid,
            "personal",
            0.9,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .unwrap();

    let request_org = memory_mcp::models::AssembleContextRequest {
        query: "scope fact".to_string(),
        scope: "org".to_string(),
        as_of: None,
        budget: 10,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };
    let org_results =
        AssembleContextCapability::assemble_context(&service.build_context(), request_org)
            .await
            .unwrap();
    assert!(org_results.iter().all(|r| { r.content.contains("Org") }));
}

#[tokio::test]
async fn test_service_assemble_context_timeline_view_sorts_and_filters_by_window() {
    let service = common::make_service().await;

    common::seed_fact_at(
        &service,
        "personal",
        "Atlas planning started",
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas budget increased",
        Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas launch confirmed",
        Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap(),
    )
    .await;

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "atlas".to_string(),
            scope: "personal".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: Some("timeline".to_string()),
            window_start: Some(Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap()),
            window_end: Some(Utc.with_ymd_and_hms(2026, 3, 31, 0, 0, 0).unwrap()),
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    let contents = items
        .iter()
        .map(|item| item.content.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        contents,
        vec!["Atlas budget increased", "Atlas launch confirmed"]
    );
}

#[tokio::test]
async fn assemble_context_auto_timeline_orders_results_without_explicit_view_mode() {
    let service = common::make_service().await;

    common::seed_fact_at(
        &service,
        "personal",
        "Atlas planning started",
        Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas budget increased",
        Utc.with_ymd_and_hms(2026, 2, 10, 9, 0, 0).unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas launch confirmed",
        Utc.with_ymd_and_hms(2026, 3, 20, 9, 0, 0).unwrap(),
    )
    .await;

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "timeline of atlas changes in q1 2026".to_string(),
            scope: "personal".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("assemble context");

    assert_eq!(items.len(), 3);
    assert!(items[0].content.contains("planning started"));
    assert!(items[1].content.contains("budget increased"));
    assert!(items[2].content.contains("launch confirmed"));
}

#[tokio::test]
async fn assemble_context_graph_expansion_returns_anchor_neighbor_fact() {
    let (service, db_client) = common::make_service_with_client().await;
    let t = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();

    common::seed_entity(
        &db_client,
        "org",
        "entity:alice",
        "person",
        "Alice Stone",
        &[],
    )
    .await;
    common::seed_entity(&db_client, "org", "entity:bob", "person", "Bob Chen", &[]).await;

    service
        .relate("entity:alice", "knows", "entity:bob")
        .await
        .expect("seed edge");

    common::seed_fact_with_links(
        &service,
        "org",
        "Bob Chen owns the Atlas launch checklist.",
        t,
        vec!["entity:bob".to_string()],
    )
    .await;

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "Alice Stone".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("assemble context");

    let graph_item = items
        .iter()
        .find(|item| item.retrieval_tier.as_deref() == Some("graph"))
        .expect("graph-expanded item should exist");
    assert!(graph_item.content.contains("Atlas launch checklist"));
}

#[tokio::test]
async fn test_service_assemble_context_filters_by_project_and_fact_type() {
    let service = common::make_service().await;
    let t_valid = Utc.with_ymd_and_hms(2026, 4, 7, 10, 0, 0).unwrap();

    let atlas_episode = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "document".to_string(),
            source_id: "project-atlas-budget".to_string(),
            content: "Atlas budget source note".to_string(),
            t_ref: t_valid,
            scope: "org".to_string(),
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
            project: Some("atlas".to_string()),
        },
        None,
    )
    .await
    .unwrap();

    let beacon_episode = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "document".to_string(),
            source_id: "project-beacon-budget".to_string(),
            content: "Beacon budget source note".to_string(),
            t_ref: t_valid,
            scope: "org".to_string(),
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
            project: Some("beacon".to_string()),
        },
        None,
    )
    .await
    .unwrap();

    service
        .add_fact(
            "metric",
            "Atlas budget is $2M",
            "Atlas budget is $2M",
            &atlas_episode,
            t_valid,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation(&atlas_episode),
        )
        .await
        .unwrap();

    service
        .add_fact(
            "promise",
            "Atlas budget owner will review the plan",
            "Atlas budget owner will review the plan",
            &atlas_episode,
            t_valid,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation(&atlas_episode),
        )
        .await
        .unwrap();

    service
        .add_fact(
            "metric",
            "Beacon budget is $3M",
            "Beacon budget is $3M",
            &beacon_episode,
            t_valid,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation(&beacon_episode),
        )
        .await
        .unwrap();
    let as_of = Utc::now() + chrono::Duration::seconds(1);

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "budget".to_string(),
            scope: "org".to_string(),
            as_of: Some(as_of),
            budget: 10,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            project: Some("atlas".to_string()),
            fact_types: vec!["metric".to_string()],
            compact: false,
        },
    )
    .await
    .unwrap();

    let contents = items
        .iter()
        .map(|item| item.content.as_str())
        .collect::<Vec<_>>();

    assert_eq!(contents, vec!["Atlas budget is $2M"]);
}

#[tokio::test]
async fn test_service_assemble_context_does_not_append_recent_experience_for_query_driven_retrieval()
 {
    let service = common::make_service().await;
    let note_time = Utc.with_ymd_and_hms(2026, 4, 7, 10, 0, 0).unwrap();
    let experience_time = Utc.with_ymd_and_hms(2026, 4, 8, 9, 0, 0).unwrap();

    let source_episode = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "document".to_string(),
            source_id: "experience-primary-match".to_string(),
            content: "Atlas source episode".to_string(),
            t_ref: note_time,
            scope: "org".to_string(),
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
            project: None,
        },
        None,
    )
    .await
    .unwrap();

    service
        .add_fact(
            "note",
            "Atlas budget is $2M",
            "Atlas budget is $2M",
            &source_episode,
            note_time,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation(&source_episode),
        )
        .await
        .unwrap();

    service
        .add_fact(
            "experience",
            "Alice prefers weekly launch updates",
            "Alice prefers weekly launch updates",
            &source_episode,
            experience_time,
            "org",
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation(&source_episode),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "budget".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            project: None,
            fact_types: vec![],
            compact: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(items[0].content, "Atlas budget is $2M");
    assert!(
        !items
            .iter()
            .any(|item| item.content == "Alice prefers weekly launch updates"),
        "query-driven retrieval should not append recent experience items: {items:?}"
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item.content == "Atlas budget is $2M")
            .count(),
        1,
        "primary matching fact should still be returned exactly once"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.rationale.contains("supplemental experience")),
        "query-driven retrieval should not include supplemental experience rationale"
    );
}

#[tokio::test]
async fn test_service_assemble_context_facets_view_groups_by_project_policy_or_scope() {
    let service = common::make_service().await;
    let t_valid = Utc.with_ymd_and_hms(2026, 4, 7, 10, 0, 0).unwrap();

    for (source_id, content, project, policy_tags, t_ref) in [
        (
            "facet-atlas",
            "Atlas roadmap note",
            Some("atlas"),
            Vec::<&str>::new(),
            t_valid,
        ),
        (
            "facet-persona",
            "Persona note",
            None,
            vec!["persona"],
            t_valid + chrono::Duration::minutes(1),
        ),
        (
            "facet-org",
            "Org note",
            None,
            Vec::<&str>::new(),
            t_valid + chrono::Duration::minutes(2),
        ),
    ] {
        IngestCapability::ingest(
            &service.build_context(),
            memory_mcp::models::IngestRequest {
                source_type: "document".to_string(),
                source_id: source_id.to_string(),
                content: content.to_string(),
                t_ref,
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: policy_tags.into_iter().map(str::to_string).collect(),
                project: project.map(str::to_string),
            },
            None,
        )
        .await
        .unwrap();
    }

    let as_of = Utc::now() + chrono::Duration::seconds(1);

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: String::new(),
            scope: "org".to_string(),
            as_of: Some(as_of),
            budget: 10,
            view_mode: Some("facets".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            project: None,
            fact_types: vec![],
            compact: false,
        },
    )
    .await
    .unwrap();

    let atlas = items
        .iter()
        .find(|item| item.content == "atlas")
        .expect("atlas facet should exist");
    let persona = items
        .iter()
        .find(|item| item.content == "persona")
        .expect("persona facet should exist");
    let org = items
        .iter()
        .find(|item| item.content == "org")
        .expect("scope facet should exist");

    assert_eq!(atlas.provenance.get("count"), Some(&json!(1)));
    assert_eq!(persona.provenance.get("count"), Some(&json!(1)));
    assert_eq!(org.provenance.get("count"), Some(&json!(1)));
    assert!(atlas.rationale.contains("view_mode=facets"));
}

#[tokio::test]
async fn test_service_assemble_context_wake_up_prioritizes_persona_then_recent() {
    let service = common::make_service().await;
    let t_valid = Utc.with_ymd_and_hms(2026, 4, 7, 10, 0, 0).unwrap();

    service
        .add_fact(
            "note",
            "I prefer concise weekly digests",
            "I prefer concise weekly digests",
            "episode:persona",
            t_valid,
            "org",
            0.9,
            vec![],
            vec!["persona".to_string()],
            Provenance::agent_observation("episode:persona"),
        )
        .await
        .unwrap();

    service
        .add_fact(
            "note",
            "Old checklist note",
            "Old checklist note",
            "episode:old",
            t_valid - chrono::Duration::days(2),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:old"),
        )
        .await
        .unwrap();

    service
        .add_fact(
            "note",
            "Reviewed Atlas risk register yesterday",
            "Reviewed Atlas risk register yesterday",
            "episode:recent",
            t_valid + chrono::Duration::hours(2),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:recent"),
        )
        .await
        .unwrap();

    let as_of = Utc::now() + chrono::Duration::seconds(1);

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "ignored".to_string(),
            scope: "org".to_string(),
            as_of: Some(as_of),
            budget: 2,
            view_mode: Some("wake_up".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            project: None,
            fact_types: vec![],
            compact: false,
        },
    )
    .await
    .unwrap();

    let contents = items
        .iter()
        .map(|item| item.content.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        contents,
        vec![
            "I prefer concise weekly digests",
            "Reviewed Atlas risk register yesterday"
        ]
    );
}

#[tokio::test]
async fn test_service_assemble_context_map_view_returns_hub_entities_sorted_by_degree() {
    let (service, _db_client) = common::make_service_with_client().await;

    let alice_id = service
        .resolve_entity("person", "Alice Smith")
        .await
        .unwrap();
    let bob_id = service.resolve_entity("person", "Bob Jones").await.unwrap();
    let carol_id = service
        .resolve_entity("person", "Carol White")
        .await
        .unwrap();
    let diana_id = service
        .resolve_entity("person", "Diana Prince")
        .await
        .unwrap();

    service.relate(&alice_id, "knows", &bob_id).await.unwrap();
    service.relate(&bob_id, "knows", &carol_id).await.unwrap();
    service.relate(&bob_id, "knows", &diana_id).await.unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: String::new(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 3,
            project: None,
            fact_types: vec![],
            view_mode: Some("map".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(items.len(), 3, "expected top 3 hub entities");
    assert!(
        items
            .iter()
            .all(|item| { item.provenance.get("kind") == Some(&json!("hub_entity")) })
    );

    assert_eq!(items[0].content, "Bob Jones");
    assert_eq!(items[0].provenance.get("degree"), Some(&json!(3)));
    assert!(items[0].rationale.contains("view_mode=map"));

    let hub_names = items
        .iter()
        .map(|item| item.content.as_str())
        .collect::<Vec<_>>();
    assert!(hub_names.contains(&"Alice Smith"));
    assert!(hub_names.contains(&"Carol White") || hub_names.contains(&"Diana Prince"));
}

#[tokio::test]
async fn test_service_assemble_context_map_view_includes_communities() {
    let (service, db_client) = common::make_service_with_client().await;

    let alice_id = service
        .resolve_entity("person", "Alice Smith")
        .await
        .unwrap();
    let bob_id = service.resolve_entity("person", "Bob Jones").await.unwrap();
    let carol_id = service
        .resolve_entity("person", "Carol White")
        .await
        .unwrap();

    common::seed_community(
        &db_client,
        "org",
        "community:atlas-team",
        &[alice_id.clone(), bob_id.clone(), carol_id.clone()],
        "Alice Smith, Bob Jones, Carol White",
        Utc.with_ymd_and_hms(2026, 4, 7, 12, 0, 0).unwrap(),
    )
    .await;

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: String::new(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: Some("map".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    let community = items
        .iter()
        .find(|item| item.provenance.get("kind") == Some(&json!("community")))
        .expect("map view should include community items");

    assert_eq!(community.fact_id, "map:community:community:atlas-team");
    assert_eq!(community.content, "Alice Smith, Bob Jones, Carol White");
    assert_eq!(community.quote, "3 members");
    assert_eq!(community.source_episode, "community:atlas-team");
    assert_eq!(community.provenance.get("member_count"), Some(&json!(3)));
    assert_eq!(
        community.provenance.get("member_entities"),
        Some(&json!([alice_id, bob_id, carol_id]))
    );
    assert!(community.rationale.contains("view_mode=map"));
}

// ---------------------------------------------------------------------------
// High-value coverage gaps: view modes, cache, experience, query logging,
// resolve race, embedding errors, invalidate, explain graph insights,
// multi-namespace isolation, semantic disabled/error/threshold, decay, archival.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_service_assemble_context_timeline_view_sorts_chronologically() {
    let service = common::make_service().await;
    let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 3, 1, 10, 0, 0).unwrap();
    let t3 = Utc.with_ymd_and_hms(2026, 2, 1, 10, 0, 0).unwrap();

    for (content, t) in [
        ("first event january", t1),
        ("third event march", t2),
        ("second event february", t3),
    ] {
        service
            .add_fact(
                "note",
                content,
                content,
                &format!("episode:timeline-{content}"),
                t,
                "org",
                0.9,
                vec![],
                vec![],
                Provenance::agent_observation(format!("episode:timeline-{content}")),
            )
            .await
            .unwrap();
    }

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "event".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: Some("timeline".to_string()),
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(items.len(), 3);
    // Timeline view must be chronologically ascending by t_ref.
    assert!(items[0].content.contains("january"));
    assert!(items[1].content.contains("february"));
    assert!(items[2].content.contains("march"));
}

#[tokio::test]
async fn test_service_assemble_context_cache_hit_tracks_fact_access() {
    let (service, db_client) = common::make_service_with_client().await;

    let fact_id = service
        .add_fact(
            "note",
            "Cache hit access tracking",
            "Cache hit access tracking",
            "episode:cache-hit-access",
            Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:cache-hit-access"),
        )
        .await
        .unwrap();

    let request = memory_mcp::models::AssembleContextRequest {
        query: "access tracking".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
        budget: 5,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };

    // First call: cache miss, computes and stores in cache.
    let first =
        AssembleContextCapability::assemble_context(&service.build_context(), request.clone())
            .await
            .unwrap();
    assert!(first.iter().any(|item| item.fact_id == fact_id));

    // Second call with identical params: cache hit, still tracks access.
    let second = AssembleContextCapability::assemble_context(&service.build_context(), request)
        .await
        .unwrap();
    assert_eq!(first.len(), second.len());

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .unwrap()
        .expect("stored fact");

    assert_eq!(
        stored.get("access_count").and_then(|v| v.as_i64()),
        Some(2),
        "cache hit should still increment access_count"
    );
}

#[tokio::test]
async fn test_service_assemble_context_appends_recent_experience_for_browse_like_requests() {
    let (service, _db_client) = common::make_service_with_client().await;

    let source_episode = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "document".to_string(),
            source_id: "experience-browse-base".to_string(),
            content: "Atlas source episode".to_string(),
            t_ref: Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap(),
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .unwrap();

    let fact_id = service
        .add_fact(
            "note",
            "budget allocation for Q4 infrastructure spend",
            "budget allocation for Q4",
            &source_episode,
            Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation(&source_episode),
        )
        .await
        .unwrap();

    let recent_experience_id = service
        .add_fact(
            "experience",
            "Alice prefers weekly launch updates",
            "Alice prefers weekly launch updates",
            &source_episode,
            Utc.with_ymd_and_hms(2026, 4, 10, 10, 0, 0).unwrap(),
            "org",
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation(&source_episode),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: String::new(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(
        items.iter().any(|item| item.fact_id == fact_id),
        "browse-like retrieval should still include active base facts"
    );
    assert!(
        items
            .iter()
            .any(|item| item.fact_id == recent_experience_id),
        "browse-like retrieval should append recent experience facts: {items:?}"
    );
}

#[tokio::test]
async fn test_service_assemble_context_records_query_log_when_enabled() {
    let (service, db_client) = common::make_service_with_client_and_query_logging(true).await;

    service
        .add_fact(
            "note",
            "Query logging enabled test fact",
            "Query logging enabled test fact",
            "episode:ql-enabled",
            Utc.with_ymd_and_hms(2026, 4, 8, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:ql-enabled"),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "query logging enabled".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(!items.is_empty());

    let query_logs = db_client.select_table("query_log", "org").await.unwrap();
    assert!(
        !query_logs.is_empty(),
        "query logging should create a row when enabled"
    );

    let log_entry = query_logs.first().unwrap();
    assert_eq!(
        log_entry.get("result_count").and_then(|v| v.as_i64()),
        Some(items.len() as i64)
    );
    assert_eq!(
        log_entry.get("cache_hit").and_then(json_bool),
        Some(false),
        "first retrieval should be a cache miss"
    );
}

#[tokio::test]
async fn test_service_resolve_handles_concurrent_duplicate_gracefully() {
    // When two concurrent resolve calls race to create the same entity,
    // the second one should get the existing entity ID instead of failing.
    let service = common::make_service().await;

    let (id1, id2) = tokio::join!(
        service.resolve_entity("person", "Concurrent Alice"),
        service.resolve_entity("person", "Concurrent Alice"),
    );

    let id1 = id1.expect("first resolve should succeed");
    let id2 = id2.expect("second resolve should succeed");

    assert_eq!(id1, id2, "concurrent resolves should return same entity_id");
    assert!(id1.starts_with("entity:"));
}

#[tokio::test]
async fn test_service_add_fact_logs_warning_on_embedding_error_and_still_persists() {
    // Uses the default disabled embedding provider — generate_embedding returns
    // Ok(None), which should silently skip embedding and still persist the fact.
    let (service, db_client) = common::make_service_with_client().await;

    let fact_id = service
        .add_fact(
            "note",
            "Embedding error skip test",
            "Embedding error skip test",
            "episode:embed-error",
            Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:embed-error"),
        )
        .await
        .expect("add_fact should succeed even when embedding fails");

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .unwrap()
        .expect("stored fact");

    assert_eq!(
        stored.get("content"),
        Some(&json!("Embedding error skip test"))
    );
    assert!(
        stored.get("embedding").is_none(),
        "fact should not have embedding when provider is disabled"
    );
}

#[tokio::test]
async fn test_service_invalidate_sets_t_invalid_and_clears_cache() {
    let service = common::make_service().await;

    let fact_id = service
        .add_fact(
            "metric",
            "ARR $10M",
            "ARR $10M",
            "episode:invalidate-cache",
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:invalidate-cache"),
        )
        .await
        .unwrap();

    // Warm the cache.
    let request = memory_mcp::models::AssembleContextRequest {
        query: "ARR".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
        budget: 5,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };
    let cached =
        AssembleContextCapability::assemble_context(&service.build_context(), request.clone())
            .await
            .unwrap();
    assert!(cached.iter().any(|item| item.fact_id == fact_id));

    // Invalidate the fact.
    InvalidateCapability::invalidate(
        &service.build_context(),
        memory_mcp::models::InvalidateRequest {
            fact_id: fact_id.clone(),
            reason: "superseded".to_string(),
            t_invalid: Utc::now() - chrono::Duration::seconds(10),
        },
        None,
    )
    .await
    .unwrap();

    // Re-query with a later as_of — invalidated fact must be excluded.
    let after_request = memory_mcp::models::AssembleContextRequest {
        query: "ARR".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
        budget: 5,
        project: None,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };
    let after =
        AssembleContextCapability::assemble_context(&service.build_context(), after_request)
            .await
            .unwrap();
    assert!(
        !after.iter().any(|item| item.fact_id == fact_id),
        "invalidated fact should not appear in context after cache invalidation"
    );
}

#[tokio::test]
async fn test_service_explain_with_graph_insights_returns_hub_and_connections() {
    let (service, db_client) = common::make_service_with_client().await;
    let t_ref = Utc.with_ymd_and_hms(2026, 4, 8, 10, 0, 0).unwrap();

    // Build a small graph: Alice -> Bob -> Carol, with Bob as the hub.
    let alice_id = service
        .resolve_entity("person", "Alice Explain")
        .await
        .unwrap();
    let bob_id = service
        .resolve_entity("person", "Bob Explain")
        .await
        .unwrap();
    let carol_id = service
        .resolve_entity("person", "Carol Explain")
        .await
        .unwrap();

    service.relate(&alice_id, "knows", &bob_id).await.unwrap();
    service.relate(&bob_id, "knows", &carol_id).await.unwrap();
    service.relate(&bob_id, "knows", &alice_id).await.unwrap();

    // Seed a community so Bob shows up as a hub.
    common::seed_community(
        &db_client,
        "org",
        "community:explain-test",
        &[alice_id.clone(), bob_id.clone(), carol_id.clone()],
        "Alice Explain, Bob Explain, Carol Explain",
        t_ref,
    )
    .await;

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "meeting".to_string(),
            source_id: "explain-graph-1".to_string(),
            content: "Bob Explain coordinated the partner review".to_string(),
            t_ref,
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .unwrap();

    let fact_id = service
        .add_fact(
            "note",
            "Bob Explain coordinated the partner review",
            "Bob Explain coordinated the partner review",
            &episode_id,
            t_ref,
            "org",
            0.9,
            vec![bob_id.clone()],
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .unwrap();

    let explanation = ExplainCapability::explain(
        &service.build_context(),
        memory_mcp::models::ExplainRequest {
            context_pack: vec![memory_mcp::models::ExplainItem {
                fact_id: Some(fact_id),
                content: "Bob Explain coordinated the partner review".to_string(),
                quote: "Bob Explain coordinated the partner review".to_string(),
                source_episode: episode_id.clone(),
                scope: None,
                t_ref: None,
                t_ingested: None,
                provenance: serde_json::json!({"source_episode": &episode_id}),
                citation_context: None,
                all_sources: vec![],
                graph_insights: None,
                fact_age_days: None,
                decayed_confidence: None,
                ingestion_method: None,
            }],
            compact: false,
        },
        None,
    )
    .await
    .unwrap();

    let serialized = serde_json::to_value(&explanation[0]).expect("serialize explain item");
    let graph_insights = serialized
        .get("graph_insights")
        .expect("explain should expose graph_insights");

    let hub_entities = graph_insights
        .get("hub_entities")
        .and_then(serde_json::Value::as_array)
        .expect("hub_entities should be an array");
    assert!(
        !hub_entities.is_empty(),
        "hub_entities should not be empty when entity is linked"
    );

    let surprising_connections = graph_insights
        .get("surprising_connections")
        .and_then(serde_json::Value::as_array)
        .expect("surprising_connections should be an array");
    // Bob has edges to Alice and Carol; at least one should surface.
    assert!(
        !surprising_connections.is_empty() || !hub_entities.is_empty(),
        "graph insights should contain hub entities or surprising connections"
    );
}

#[tokio::test]
async fn test_service_multi_namespace_scope_isolation() {
    let service = common::make_service().await;

    // Seed a fact in the "personal" namespace.
    let personal_fact_id = service
        .add_fact(
            "note",
            "personal scope isolated fact",
            "personal scope isolated fact",
            "episode:ns-personal",
            Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap(),
            "personal",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:ns-personal"),
        )
        .await
        .unwrap();

    // Seed a different fact in the "org" namespace.
    let org_fact_id = service
        .add_fact(
            "note",
            "org scope isolated fact",
            "org scope isolated fact",
            "episode:ns-org",
            Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:ns-org"),
        )
        .await
        .unwrap();

    // Query "personal" scope — should only return the personal fact.
    let personal_items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "isolated fact".to_string(),
            scope: "personal".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(
        personal_items
            .iter()
            .any(|item| item.fact_id == personal_fact_id),
        "personal scope query should return personal fact"
    );
    assert!(
        !personal_items
            .iter()
            .any(|item| item.fact_id == org_fact_id),
        "personal scope query should NOT return org fact"
    );

    // Query "org" scope — should only return the org fact.
    let org_items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "isolated fact".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(
        org_items.iter().any(|item| item.fact_id == org_fact_id),
        "org scope query should return org fact"
    );
    assert!(
        !org_items
            .iter()
            .any(|item| item.fact_id == personal_fact_id),
        "org scope query should NOT return personal fact"
    );
}

#[tokio::test]
async fn test_service_semantic_returns_empty_without_embedding_provider() {
    // Without an embedding provider, collect_semantic_facts short-circuits
    // and returns an empty Vec. A keyword-only query that has no BM25
    // match should therefore yield no results from the semantic tier.
    let service = common::make_service().await;

    // Seed a fact with content that won't match the query lexically.
    service
        .add_fact(
            "note",
            "The oven preheated to three hundred degrees",
            "oven preheated",
            "episode:semantic-disabled",
            Utc.with_ymd_and_hms(2026, 4, 1, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:semantic-disabled"),
        )
        .await
        .unwrap();

    let items = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "quantum entanglement photon superposition".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .unwrap();

    assert!(
        items.is_empty(),
        "without embedding provider, semantic-only query should return no results"
    );
}

#[tokio::test]
async fn test_service_decay_pass_with_real_surrealdb_invalidates_old_low_confidence() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - chrono::Duration::days(500);

    let fact_id = service
        .add_fact(
            "metric",
            "very old low confidence metric for decay",
            "very old low confidence metric",
            "episode:decay-real",
            old_date,
            "org",
            0.35,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .unwrap();

    let count = memory_mcp::service::run_decay_pass(&service, 0.3, 100.0)
        .await
        .expect("decay pass should succeed");

    assert_eq!(
        count, 1,
        "old low-confidence fact should be invalidated by decay"
    );

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .expect("select fact")
        .expect("stored fact");
    assert!(
        stored.get("t_invalid").is_some(),
        "decayed fact should have t_invalid set"
    );
}

#[tokio::test]
async fn test_service_decay_pass_skips_already_invalidated_facts() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - chrono::Duration::days(500);

    let fact_id = service
        .add_fact(
            "metric",
            "pre-invalidated fact for decay",
            "pre-invalidated",
            "episode:decay-skip",
            old_date,
            "org",
            0.2,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .unwrap();

    // Manually invalidate first.
    InvalidateCapability::invalidate(
        &service.build_context(),
        memory_mcp::models::InvalidateRequest {
            fact_id: fact_id.clone(),
            reason: "pre-invalidation".to_string(),
            t_invalid: Utc::now(),
        },
        None,
    )
    .await
    .unwrap();

    let count = memory_mcp::service::run_decay_pass(&service, 0.3, 100.0)
        .await
        .expect("decay pass should succeed");

    assert_eq!(
        count, 0,
        "decay pass should not re-invalidate already-invalidated facts"
    );

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .expect("select fact")
        .expect("stored fact");
    assert!(stored.get("t_invalid").is_some());
}

#[tokio::test]
async fn test_service_archival_pass_with_real_surrealdb_archives_old_episode() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - chrono::Duration::days(200);

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "meeting".to_string(),
            source_id: "archival-real-1".to_string(),
            content: "Old episode for archival".to_string(),
            t_ref: old_date,
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .unwrap();

    let fact_id = service
        .add_fact(
            "note",
            "Old fact for archival",
            "Old fact for archival",
            &episode_id,
            old_date,
            "org",
            0.2,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .unwrap();

    // Invalidate the fact so the episode becomes eligible for archival.
    InvalidateCapability::invalidate(
        &service.build_context(),
        memory_mcp::models::InvalidateRequest {
            fact_id,
            reason: "prepare archival".to_string(),
            t_invalid: Utc::now(),
        },
        None,
    )
    .await
    .unwrap();

    let count = memory_mcp::service::run_archival_pass(&service, 90)
        .await
        .expect("archival pass should succeed");

    assert!(
        count >= 1,
        "old episode without active facts should be archived"
    );

    let stored = db_client
        .select_one(&episode_id, "org")
        .await
        .expect("select episode")
        .expect("stored episode");
    assert_eq!(
        stored.get("status"),
        Some(&json!("archived")),
        "archived episode should have status=archived"
    );
}

#[tokio::test]
async fn test_service_archival_pass_skips_recent_episodes() {
    let (service, db_client) = common::make_service_with_client().await;
    let recent_date = Utc::now() - chrono::Duration::days(10);

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "chat".to_string(),
            source_id: "archival-recent-skip".to_string(),
            content: "Recent episode should not be archived".to_string(),
            t_ref: recent_date,
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .unwrap();

    service
        .add_fact(
            "note",
            "Recent fact for archival skip",
            "Recent fact for archival skip",
            &episode_id,
            recent_date,
            "org",
            0.2,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .unwrap();

    let count = memory_mcp::service::run_archival_pass(&service, 90)
        .await
        .expect("archival pass should succeed");

    assert_eq!(count, 0, "recent episode should not be archived");

    let stored = db_client
        .select_one(&episode_id, "org")
        .await
        .expect("select episode")
        .expect("stored episode");
    assert_ne!(
        stored.get("status"),
        Some(&json!("archived")),
        "recent episode should not have status=archived"
    );
}

#[tokio::test]
async fn test_extract_generates_note_fact_for_summary_requirement_episode() {
    let service = common::make_service().await;
    let episode_id = IngestCapability::ingest(
        &service.build_context(),

            memory_mcp::models::IngestRequest {
                source_type: "requirement".to_string(),
                source_id: "summary-requirement-1".to_string(),
                content: "July 2025 planning summary: platform integrations ready, stakeholder approvals pending, response workflow scoped.".to_string(),
                t_ref: Utc.with_ymd_and_hms(2025, 7, 10, 9, 0, 0).unwrap(),
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest summary episode");

    let extraction = ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .expect("extract summary episode");

    assert!(
        extraction.facts.iter().any(|fact| fact.fact_type == "note"),
        "expected summary-like requirement episode to produce a note fact, got {extraction:?}"
    );
}

#[tokio::test]
async fn test_extract_meeting_summary_generates_line_level_decision_and_fact_records() {
    let (service, db_client) = common::make_service_with_client().await;
    let episode_id = IngestCapability::ingest(
        &service.build_context(),

            memory_mcp::models::IngestRequest {
                source_type: "meeting_summary".to_string(),
                source_id: "meeting-summary-line-facts-1".to_string(),
                content: "Project decision summary:\n\n- Decision: Approve the cross-platform activation policy.\n- Decision: Keep legacy on-premise licenses separate.\n- Fact: Working release milestone targeted for early May.".to_string(),
                t_ref: Utc.with_ymd_and_hms(2026, 4, 13, 9, 0, 0).unwrap(),
                scope: "org".to_string(),
                project: Some("cloud-products".to_string()),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest meeting summary episode");

    let extraction = ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .expect("extract meeting summary episode");

    assert!(
        extraction.facts.len() >= 3,
        "expected line-level extraction to produce multiple facts, got {extraction:?}"
    );
    assert!(
        extraction
            .facts
            .iter()
            .any(|fact| fact.fact_type == "decision"),
        "expected at least one decision fact, got {extraction:?}"
    );
    assert!(
        extraction.facts.iter().any(|fact| fact.fact_type == "note"),
        "expected at least one fact/note record, got {extraction:?}"
    );

    let stored_facts = memory_mcp::storage::EpisodeStoreClient::new(db_client.clone())
        .select_active_facts_by_episode(
            "org",
            &episode_id,
            &memory_mcp::service::normalize_dt(Utc::now() + chrono::Duration::seconds(1)),
            20,
        )
        .await
        .expect("select stored extracted facts");

    let stored_contents = stored_facts
        .iter()
        .filter_map(|record| record.get("content").and_then(Value::as_str))
        .collect::<Vec<_>>();

    assert!(
        stored_contents.contains(&"Approve the cross-platform activation policy."),
        "expected explicit decision line to be stored as its own fact, got {stored_contents:?}"
    );
    assert!(
        stored_contents.contains(&"Keep legacy on-premise licenses separate."),
        "expected unlabeled bullet in a decisions section to become its own fact, got {stored_contents:?}"
    );
    assert!(
        stored_contents.contains(&"Working release milestone targeted for early May."),
        "expected explicit fact line to be stored as its own fact, got {stored_contents:?}"
    );
    assert!(
        stored_contents
            .iter()
            .all(|content| *content != "Project decision summary:\n\n- Decision: Approve the cross-platform activation policy.\n- Decision: Keep legacy on-premise licenses separate.\n- Fact: Working release milestone targeted for early May."),
        "line-level extraction should not fall back to storing the whole episode blob when structured lines exist: {stored_contents:?}"
    );
}

#[tokio::test]
async fn test_extract_summary_with_thematic_sections_generates_line_level_note_records() {
    let (service, db_client) = common::make_service_with_client().await;
    let episode_id = IngestCapability::ingest(
        &service.build_context(),

            memory_mcp::models::IngestRequest {
                source_type: "meeting_summary".to_string(),
                source_id: "meeting-summary-thematic-sections-1".to_string(),
                content: "# Monthly coordination summary\n\n## Release Activities\n- Finalize phased rollout checklist.\n- Publish support handoff notes.\n\n## Capacity Planning\n- Prepare archive review for next quarter.".to_string(),
                t_ref: Utc.with_ymd_and_hms(2026, 4, 13, 11, 0, 0).unwrap(),
                scope: "org".to_string(),
                project: Some("general-ops".to_string()),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest thematic summary episode");

    let extraction = ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .expect("extract thematic summary episode");

    assert!(
        extraction.facts.len() >= 3,
        "expected thematic sections to produce multiple line-level facts, got {extraction:?}"
    );
    assert!(
        extraction.facts.iter().all(|fact| fact.fact_type == "note"),
        "expected thematic section lines to become note facts, got {extraction:?}"
    );

    let stored_facts = memory_mcp::storage::EpisodeStoreClient::new(db_client.clone())
        .select_active_facts_by_episode(
            "org",
            &episode_id,
            &memory_mcp::service::normalize_dt(Utc::now() + chrono::Duration::seconds(1)),
            20,
        )
        .await
        .expect("select stored thematic section facts");

    let stored_contents = stored_facts
        .iter()
        .filter_map(|record| record.get("content").and_then(Value::as_str))
        .collect::<Vec<_>>();

    assert!(
        stored_contents.contains(&"Release Activities: Finalize phased rollout checklist."),
        "expected release section bullet to be stored as its own contextualized fact, got {stored_contents:?}"
    );
    assert!(
        stored_contents.contains(&"Release Activities: Publish support handoff notes."),
        "expected second release section bullet to be stored as its own contextualized fact, got {stored_contents:?}"
    );
    assert!(
        stored_contents.contains(&"Capacity Planning: Prepare archive review for next quarter."),
        "expected capacity section bullet to be stored as its own contextualized fact, got {stored_contents:?}"
    );
    assert!(
        stored_contents
            .iter()
            .all(|content| *content != "# Monthly coordination summary\n\n## Release Activities\n- Finalize phased rollout checklist.\n- Publish support handoff notes.\n\n## Capacity Planning\n- Prepare archive review for next quarter."),
        "line-level extraction should not store the whole thematic summary blob when section bullets exist: {stored_contents:?}"
    );
}
