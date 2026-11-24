mod common;

use chrono::{Duration, Utc};
use memory_mcp::models::{AssembleContextRequest, Provenance, TemporalContextQuery};
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::storage::DbClient;
use serde_json::json;

fn request(query: &str) -> AssembleContextRequest {
    AssembleContextRequest {
        query: query.to_string(),
        as_of: None,
        budget: 10,
        fact_types: Vec::new(),
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: true,
    }
}

async fn latest_revision(database: &impl memory_mcp::storage::DbClient) -> i64 {
    let rows = database
        .query(
            "SELECT latest_revision FROM knowledge_revision_clock:active LIMIT 1",
            None,
            "org",
        )
        .await
        .expect("read revision clock");
    rows.as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row["latest_revision"].as_i64())
        .expect("revision clock row")
}

async fn historical_items(
    service: &memory_mcp::service::MemoryService,
    query: &str,
    revision: i64,
) -> Vec<memory_mcp::models::AssembledContextItem> {
    AssembleContextCapability::assemble_context_temporal_from_service(
        service,
        request(query),
        TemporalContextQuery {
            valid_at: Some(Utc::now() + Duration::days(1)),
            known_at: None,
            known_revision: Some(revision),
        },
    )
    .await
    .expect("assemble exact historical context")
    .items
}

#[tokio::test]
async fn historical_read_does_not_broaden_through_the_current_alias_projection() {
    let memory = common::TestMemory::new(false).await;
    common::seed_entity(
        &memory.db_client,
        "org",
        "entity:orchid_portal_alias_history",
        "concept",
        "Orchid Portal",
        &["night orchid".to_string()],
    )
    .await;
    let fact_id = memory
        .service
        .add_fact(
            "note",
            "Night Orchid migrated the service to canary deployment.",
            "Night Orchid migrated the service to canary deployment.",
            "episode:alias-history",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:alias-history"),
        )
        .await
        .expect("persist alias-searchable fact");
    let revision = latest_revision(memory.db_client.as_ref()).await;

    let historical = historical_items(&memory.service, "Orchid Portal", revision).await;
    assert!(
        historical.iter().all(|item| item.fact_id != fact_id),
        "the historical read must not use an alias projection whose history is unknown"
    );
}

#[tokio::test]
async fn historical_read_ignores_a_community_summary_added_after_its_revision() {
    let memory = common::TestMemory::new(false).await;
    let entity_id = common::resolve_entity(&memory.service, "concept", "Quartz Console")
        .await
        .expect("create community member entity");
    let fact_id = memory
        .service
        .add_fact(
            "note",
            "The maintenance window starts at sunrise.",
            "The maintenance window starts at sunrise.",
            "episode:community-history",
            Utc::now(),
            0.9,
            vec![entity_id.clone()],
            vec![],
            Provenance::agent_observation("episode:community-history"),
        )
        .await
        .expect("persist fact linked to the community member");
    let revision = latest_revision(memory.db_client.as_ref()).await;
    common::seed_community(
        &memory.db_client,
        "org",
        "community:historical-summary",
        std::slice::from_ref(&entity_id),
        "violet protocol archive marker",
        Utc::now(),
    )
    .await;

    let current = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        request("violet protocol archive marker"),
    )
    .await
    .expect("current community-assisted read");
    assert!(
        current.iter().any(|item| item.fact_id == fact_id),
        "the current summary projection makes the linked fact discoverable"
    );

    let historical =
        historical_items(&memory.service, "violet protocol archive marker", revision).await;
    assert!(
        historical.iter().all(|item| item.fact_id != fact_id),
        "the historical read must not use a community summary created after its token"
    );
}

#[tokio::test]
async fn historical_read_ignores_a_triple_link_added_after_its_revision() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = memory
        .service
        .add_fact(
            "note",
            "The reference detail is filed in the local inventory.",
            "The reference detail is filed in the local inventory.",
            "episode:triple-history",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:triple-history"),
        )
        .await
        .expect("persist fact addressed only by a later triple");
    let revision = latest_revision(memory.db_client.as_ref()).await;
    memory
        .db_client
        .create(
            "triple:historical-link",
            json!({
                "subject": "entity:inventory",
                "predicate": "lumen",
                "object": "reference",
                "confidence": 0.9,
                "source_fact_id": fact_id,
            }),
            "org",
            memory_mcp::knowledge::queries::TRIPLE_TEMPORAL_FIELDS,
        )
        .await
        .expect("create current triple selector");

    let triple_candidates = memory
        .db_client
        .query(
            "SELECT VALUE source_fact_id FROM triple WHERE predicate CONTAINS $query",
            Some(json!({"query": "lumen"})),
            "org",
        )
        .await
        .expect("read the same triple selector used by current retrieval");
    assert_eq!(
        triple_candidates.as_array().and_then(|rows| rows.first()),
        Some(&json!(fact_id)),
        "the later triple row must identify the fixture fact"
    );

    let historical = historical_items(&memory.service, "lumen", revision).await;
    assert!(
        historical.iter().all(|item| item.fact_id != fact_id),
        "the historical read must not use a triple created after its token"
    );
}

#[tokio::test]
async fn historical_read_does_not_traverse_an_edge_added_after_its_revision() {
    let memory = common::TestMemory::new(false).await;
    common::seed_entity(
        &memory.db_client,
        "org",
        "entity:edge_history_alice",
        "person",
        "Alice Stone",
        &[],
    )
    .await;
    common::seed_entity(
        &memory.db_client,
        "org",
        "entity:edge_history_bob",
        "person",
        "Bob Chen",
        &[],
    )
    .await;
    let fact_id = common::seed_fact_with_links(
        &memory.service,
        "org",
        "Bob Chen owns the Atlas launch checklist.",
        Utc::now(),
        vec!["entity:edge_history_bob".into()],
    )
    .await;
    let revision = latest_revision(memory.db_client.as_ref()).await;

    memory
        .service
        .relate(
            "entity:edge_history_alice",
            "knows",
            "entity:edge_history_bob",
            memory_mcp::models::EdgeAttributes::inferred(),
        )
        .await
        .expect("create the later graph selector edge");

    let current = AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        request("Alice Stone"),
    )
    .await
    .expect("current graph-assisted read");
    assert!(
        current.iter().any(|item| item.fact_id == fact_id),
        "the current graph edge makes the linked neighbor fact discoverable"
    );

    let historical = historical_items(&memory.service, "Alice Stone", revision).await;
    assert!(
        historical.iter().all(|item| item.fact_id != fact_id),
        "the exact historical read must not traverse an edge created after its token"
    );
}

#[tokio::test]
async fn historical_read_ignores_experience_added_after_its_revision() {
    let memory = common::TestMemory::new(false).await;
    let revision = latest_revision(memory.db_client.as_ref()).await;
    let fact_id = memory
        .service
        .add_fact(
            "experience",
            "Prefers quiet harbor hotels with early breakfast.",
            "Prefers quiet harbor hotels with early breakfast.",
            "episode:experience-history",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:experience-history"),
        )
        .await
        .expect("persist experience after the requested token");

    let mut current_request = request("quiet harbor hotels early breakfast");
    current_request.fact_types = vec!["experience".into()];
    current_request.as_of = Some(Utc::now() + Duration::days(1));
    let current =
        AssembleContextCapability::assemble_context_from_service(&memory.service, current_request)
            .await
            .expect("current experience retrieval");
    assert!(
        current.iter().any(|item| item.fact_id == fact_id),
        "the seeded experience is a current retrievable item"
    );

    let historical = historical_items(
        &memory.service,
        "quiet harbor hotels early breakfast",
        revision,
    )
    .await;
    assert!(
        historical.iter().all(|item| item.fact_id != fact_id),
        "current experience preselection must not leak a later fact into history"
    );
}

#[tokio::test]
async fn historical_read_ignores_a_later_sibling_from_the_same_episode() {
    let memory = common::TestMemory::new(false).await;
    let anchor_id = memory
        .service
        .add_fact(
            "note",
            "Project Cedar has a six week deployment window.",
            "Project Cedar has a six week deployment window.",
            "episode:neighborhood-history",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:neighborhood-history"),
        )
        .await
        .expect("persist the query anchor");
    let revision = latest_revision(memory.db_client.as_ref()).await;
    let sibling_id = memory
        .service
        .add_fact(
            "note",
            "The deployment rollback phrase is saffron lantern.",
            "The deployment rollback phrase is saffron lantern.",
            "episode:neighborhood-history",
            Utc::now(),
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:neighborhood-history"),
        )
        .await
        .expect("persist a later sibling in the same episode");

    let historical =
        historical_items(&memory.service, "Project Cedar deployment window", revision).await;
    assert!(historical.iter().any(|item| item.fact_id == anchor_id));
    assert!(historical.iter().all(|item| item.fact_id != sibling_id));
}
