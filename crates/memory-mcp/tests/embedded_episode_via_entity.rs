//! Contract of `select_episodes_via_entity`, which is being reshaped from one
//! nested-`IN` join into three bounded indexed lookups.
//!
//! These are characterization tests: they pass against the current single-query
//! implementation and must keep passing after the rewrite, so the change is
//! provably behavior-preserving.

mod common;

use chrono::{Duration, Utc};
use memory_mcp::memory::EpisodeContextStore;
use memory_mcp::models::{IngestRequest, Provenance};
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::storage::DbClient;

async fn seed_linked_episode(
    service: &memory_mcp::service::MemoryService,
    entity: &str,
    source: &str,
    text: &str,
    t: chrono::DateTime<Utc>,
) -> String {
    let episode_id = IngestCapability::ingest_from_service(
        service,
        IngestRequest {
            source_type: "email".into(),
            source_id: source.into(),
            content: text.into(),
            t_ref: t,
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("seed episode");
    let fact_id = service
        .add_fact(
            "metric",
            text,
            text,
            &episode_id,
            t,
            0.9,
            vec![entity.to_string()],
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .expect("seed linked fact");
    // `add_fact` records the link on the fact's `entity_links` but does not
    // write the `involved_in` edge; only the ingest-and-extract path does.
    // The read under test resolves the entity through that edge, so the
    // fixture writes it directly.
    service
        .relate(
            entity,
            "involved_in",
            &fact_id,
            memory_mcp::models::EdgeAttributes::inferred(),
        )
        .await
        .expect("seed involved_in edge");
    episode_id
}

fn episode_ids(rows: &[serde_json::Value]) -> Vec<&str> {
    rows.iter()
        .filter_map(|r| r["episode_id"].as_str())
        .collect()
}

#[tokio::test]
async fn linked_episode_returns_via_shared_entity() {
    let (service, db_client) = common::make_service_with_client().await;
    let entity = memory_mcp::service::deterministic_entity_id("person", "Alice Smith");
    let ep =
        seed_linked_episode(&service, &entity, "ep-a", "Alice closed a deal", Utc::now()).await;
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store
        .select_episodes_via_entity(&entity)
        .await
        .expect("entity lookup");
    let ids = episode_ids(&rows);
    assert!(
        ids.contains(&ep.as_str()),
        "linked episode must appear: {ids:?}"
    );
}

#[tokio::test]
async fn entity_without_edges_returns_empty() {
    let (service, db_client) = common::make_service_with_client().await;
    let _ = &service;
    common::seed_entity(
        &db_client,
        "org",
        "entity:ghost-99",
        "person",
        "Ghost Person",
        &[],
    )
    .await;
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store
        .select_episodes_via_entity("entity:ghost-99")
        .await
        .expect("empty is not an error");
    assert!(rows.is_empty(), "no edges must yield no rows: {rows:?}");
}

#[tokio::test]
async fn recency_order_and_limit_preserved() {
    let (service, db_client) = common::make_service_with_client().await;
    let entity = memory_mcp::service::deterministic_entity_id("person", "Busy Bob");
    let base = Utc::now() - Duration::hours(24);
    let mut wanted = Vec::new();
    for i in 0..12 {
        let t = base + Duration::minutes(i);
        wanted.push(
            seed_linked_episode(
                &service,
                &entity,
                &format!("ep-busy-{i}"),
                &format!("note {i}"),
                t,
            )
            .await,
        );
    }
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store
        .select_episodes_via_entity(&entity)
        .await
        .expect("recency query");
    let ids = episode_ids(&rows);
    assert_eq!(ids.len(), 10, "LIMIT 10 must hold");
    assert_eq!(
        ids,
        wanted[2..]
            .iter()
            .rev()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        "newest-first order"
    );
}

#[tokio::test]
async fn fact_with_dangling_source_episode_returns_empty() {
    let (service, db_client) = common::make_service_with_client().await;
    let entity = memory_mcp::service::deterministic_entity_id("person", "Orphan Fact");
    seed_linked_episode(&service, &entity, "ep-orphan", "orphan note", Utc::now()).await;
    // `source_episode` is a non-optional string column, so the link is broken
    // by pointing it at an episode that does not exist rather than by clearing
    // it. Either way the entity resolves through the edge but the episode
    // lookup finds nothing.
    db_client
        .query(
            "UPDATE fact SET source_episode = 'episode:missing00000000000000'",
            None,
            "org",
        )
        .await
        .expect("repoint source_episode");
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store
        .select_episodes_via_entity(&entity)
        .await
        .expect("empty is not an error");
    assert!(
        rows.is_empty(),
        "a fact whose source_episode names no episode must yield no episodes: {rows:?}"
    );
}

#[tokio::test]
async fn unsplit_entity_id_returns_no_rows() {
    let (_service, db_client) = common::make_service_with_client().await;
    let store = EpisodeContextStore::new(db_client, "org");
    let rows = store
        .select_episodes_via_entity("alice-without-colon")
        .await
        .expect("a value that is not a record id matches no edges");
    assert!(
        rows.is_empty(),
        "an unsplit id is not a record id and matches nothing: {rows:?}"
    );
}
