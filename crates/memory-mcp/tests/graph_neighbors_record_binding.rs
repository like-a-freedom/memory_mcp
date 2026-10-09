//! Regression coverage for the index-safe record bindings.
//!
//! The unit tests in `knowledge::queries` pin the emitted SQL; these pin the
//! observable consequence on a real embedded SurrealDB: the predicate shape
//! must produce an IndexScan, and it must resolve the same rows a record
//! equality resolves to.

mod common;

use chrono::{Duration, Utc};
use memory_mcp::knowledge::queries::build_select_edge_neighbors_query;
use memory_mcp::models::EdgeAttributes;
use memory_mcp::storage::{DbClient, GraphDirection};

#[tokio::test]
async fn neighbors_resolve_through_type_record_binding() {
    let (service, db_client) = common::make_service_with_client().await;
    let alice = memory_mcp::service::deterministic_entity_id("person", "Alice Ann");
    let bob = memory_mcp::service::deterministic_entity_id("person", "Bob Ben");
    common::seed_entity(&db_client, "org", &alice, "person", "Alice Ann", &[]).await;
    common::seed_entity(&db_client, "org", &bob, "person", "Bob Ben", &[]).await;
    service
        .relate(&alice, "knows", &bob, EdgeAttributes::inferred())
        .await
        .expect("relate alice to bob");
    let cutoff = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let store = memory_mcp::knowledge::KnowledgeGraphStore::new(db_client, "org");
    let rows = store
        .select_edge_neighbors(&alice, &cutoff, GraphDirection::Outgoing)
        .await
        .expect("outgoing neighbors");
    let bob_key = bob.split_once(':').expect("seeded id is a record id").1;
    assert!(
        rows.iter().any(|row| {
            row["out"]["RecordId"]["key"].as_str() == Some(bob_key)
                && row["out"]["RecordId"]["table"].as_str() == Some("entity")
        }),
        "{rows:?}"
    );
}

#[tokio::test]
async fn neighbor_query_plan_is_index_scan_on_edge_in() {
    let (_service, db_client) = common::make_service_with_client().await;
    let cutoff = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let (sql, vars) = build_select_edge_neighbors_query(
        "entity:planner-probe",
        &cutoff,
        GraphDirection::Outgoing,
    );
    let plan = db_client
        .query(&format!("EXPLAIN {sql}"), Some(vars), "org")
        .await
        .expect("explain");
    let plan = plan.to_string();
    // Ruling: the plan tree is asserted as substrings of the serialized value
    // because its exact shape is version-specific; this pair is the
    // observable index-usage signal (verified on the v3.3.0 image).
    assert!(plan.contains("IndexScan"), "{plan}");
    assert!(plan.contains("edge_in"), "{plan}");
}

#[tokio::test]
async fn triple_lookup_finds_only_the_matching_edge() {
    let (service, db_client) = common::make_service_with_client().await;
    common::seed_entity(
        &db_client,
        "org",
        "entity:trip-a",
        "person",
        "Trip Ada",
        &[],
    )
    .await;
    common::seed_entity(
        &db_client,
        "org",
        "entity:trip-c",
        "person",
        "Trip Cal",
        &[],
    )
    .await;
    service
        .relate(
            "entity:trip-a",
            "knows",
            "entity:trip-c",
            EdgeAttributes::inferred(),
        )
        .await
        .expect("knows edge");
    service
        .relate(
            "entity:trip-a",
            "owns",
            "entity:trip-c",
            EdgeAttributes::inferred(),
        )
        .await
        .expect("owns edge");
    let store = memory_mcp::knowledge::KnowledgeGraphStore::new(db_client, "org");
    let hits = store
        .select_edges_for_triple("entity:trip-a", "knows", "entity:trip-c")
        .await
        .expect("dedup lookup");
    assert_eq!(
        hits.len(),
        1,
        "dedup lookup must return only the matching edge: {hits:?}"
    );
    assert_eq!(hits[0]["relation"], "knows");
}
