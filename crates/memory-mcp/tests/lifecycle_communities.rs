//! Integration tests for periodic community recomputation.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use memory_mcp::service::{normalize_dt, run_community_rebuild_pass};
use memory_mcp::storage::DbClient;
use serde_json::json;

mod common;

async fn seed_edge(
    db_client: &Arc<memory_mcp::storage::SurrealDbClient>,
    namespace: &str,
    edge_id: &str,
    from_id: &str,
    relation: &str,
    to_id: &str,
    t_valid: chrono::DateTime<Utc>,
) {
    memory_mcp::storage::EpisodeStoreClient::new(db_client.clone())
        .relate_edge(
            edge_id,
            from_id,
            to_id,
            json!({
                "edge_id": edge_id,
                "in": from_id,
                "relation": relation,
                "out": to_id,
                "origin": "inferred",
                "strength": 1.0,
                "confidence": 0.8,
                "provenance": {"source": "test"},
                "t_valid": normalize_dt(t_valid),
                "t_ingested": normalize_dt(t_valid),
            }),
            namespace,
        )
        .await
        .expect("seed edge should succeed");
}

#[tokio::test]
async fn community_rebuild_pass_creates_component_community_with_condensed_summary() {
    let (service, db_client) = common::make_service_with_client().await;
    let t_valid = Utc.with_ymd_and_hms(2026, 4, 7, 12, 0, 0).unwrap();

    for (entity_id, canonical_name) in [
        ("entity:alice_smith", "Alice Smith"),
        ("entity:bob_jones", "Bob Jones"),
        ("entity:carol_white", "Carol White"),
        ("entity:dana_black", "Dana Black"),
    ] {
        common::seed_entity(&db_client, "org", entity_id, "person", canonical_name, &[]).await;
    }

    seed_edge(
        &db_client,
        "org",
        "edge:alice-bob",
        "entity:alice_smith",
        "knows",
        "entity:bob_jones",
        t_valid,
    )
    .await;
    seed_edge(
        &db_client,
        "org",
        "edge:bob-carol",
        "entity:bob_jones",
        "knows",
        "entity:carol_white",
        t_valid,
    )
    .await;
    seed_edge(
        &db_client,
        "org",
        "edge:carol-dana",
        "entity:carol_white",
        "knows",
        "entity:dana_black",
        t_valid,
    )
    .await;

    let rebuilt = run_community_rebuild_pass(&service)
        .await
        .expect("community rebuild pass should succeed");

    assert_eq!(rebuilt, 1, "expected one rebuilt community in org");

    let communities = db_client.select_table("community", "org").await.unwrap();
    let rebuilt = communities
        .iter()
        .find(|community| {
            let Some(members) = community
                .get("member_entities")
                .and_then(|value| value.as_array())
            else {
                return false;
            };

            let members: Vec<_> = members.iter().filter_map(|value| value.as_str()).collect();
            members.contains(&"entity:alice_smith")
                && members.contains(&"entity:bob_jones")
                && members.contains(&"entity:carol_white")
                && members.contains(&"entity:dana_black")
        })
        .expect("rebuilt community should be stored");

    assert_eq!(
        rebuilt.get("summary"),
        Some(&json!("Alice Smith, Bob Jones, Carol White (+1 more)"))
    );
}

#[tokio::test]
async fn community_rebuild_pass_prunes_stale_communities_without_active_edges() {
    let (service, db_client) = common::make_service_with_client().await;

    for (entity_id, canonical_name) in [
        ("entity:legacy_alice", "Legacy Alice"),
        ("entity:legacy_bob", "Legacy Bob"),
    ] {
        common::seed_entity(&db_client, "org", entity_id, "person", canonical_name, &[]).await;
    }

    common::seed_community(
        &db_client,
        "org",
        "community:stale",
        &[
            "entity:legacy_alice".to_string(),
            "entity:legacy_bob".to_string(),
        ],
        "Legacy Alice, Legacy Bob",
        Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap(),
    )
    .await;

    let rebuilt = run_community_rebuild_pass(&service)
        .await
        .expect("community rebuild pass should succeed");

    assert_eq!(rebuilt, 0, "no active edges means no rebuilt communities");
    assert!(
        db_client
            .select_table("community", "org")
            .await
            .unwrap()
            .is_empty(),
        "stale community records should be deleted when they are no longer backed by active edges"
    );
}

#[tokio::test]
async fn community_rebuild_pass_processes_all_configured_namespaces() {
    let (service, db_client) = common::make_service_with_client().await;
    let t_valid = Utc.with_ymd_and_hms(2026, 4, 7, 13, 0, 0).unwrap();

    for (entity_id, canonical_name) in [
        ("entity:ivy_lane", "Ivy Lane"),
        ("entity:jade_park", "Jade Park"),
    ] {
        common::seed_entity(
            &db_client,
            "personal",
            entity_id,
            "person",
            canonical_name,
            &[],
        )
        .await;
    }

    seed_edge(
        &db_client,
        "personal",
        "edge:ivy-jade",
        "entity:ivy_lane",
        "knows",
        "entity:jade_park",
        t_valid,
    )
    .await;

    let rebuilt = run_community_rebuild_pass(&service)
        .await
        .expect("community rebuild pass should succeed");

    assert_eq!(
        rebuilt, 1,
        "expected one rebuilt community across namespaces"
    );

    let communities = db_client
        .select_table("community", "personal")
        .await
        .unwrap();
    assert!(
        communities.iter().any(|community| {
            let Some(members) = community
                .get("member_entities")
                .and_then(|value| value.as_array())
            else {
                return false;
            };

            let members: Vec<_> = members.iter().filter_map(|value| value.as_str()).collect();
            members.contains(&"entity:ivy_lane") && members.contains(&"entity:jade_park")
        }),
        "community rebuild should include non-default namespaces"
    );
}
