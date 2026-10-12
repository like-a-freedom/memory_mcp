//! The lookup indexes the context read path depends on.
//!
//! Stage 3 of `select_episodes_via_entity` reads `episode_id INSIDE [...]`, and
//! every explain follows `fact.source_episode` item by item. Both columns need
//! an index of their own for those lookups to be answered from it rather than
//! by scanning the table — so this asserts not only that the indexes exist
//! after migrations, but that the episode lookup is planned against one.

mod common;

use memory_mcp::memory::queries::build_select_episodes_by_ids_query;
use memory_mcp::storage::DbClient;

#[tokio::test]
async fn context_lookup_indexes_registered_by_migrations() {
    let (_service, db) = common::make_service_with_client().await;
    let episode = db
        .query("INFO FOR TABLE episode", None, "org")
        .await
        .expect("info episode")
        .to_string();
    let fact = db
        .query("INFO FOR TABLE fact", None, "org")
        .await
        .expect("info fact")
        .to_string();
    assert!(episode.contains("episode_episode_id"), "{episode}");
    assert!(fact.contains("fact_source_episode"), "{fact}");
}

#[tokio::test]
async fn episode_lookup_is_planned_against_its_index() {
    let (_service, db) = common::make_service_with_client().await;
    let (sql, vars) =
        build_select_episodes_by_ids_query(&[serde_json::json!("episode:missing000000000000")]);
    let plan = db
        .query(&format!("EXPLAIN {sql}"), Some(vars), "org")
        .await
        .expect("explain")
        .to_string();
    assert!(plan.contains("IndexScan"), "{plan}");
    assert!(plan.contains("episode_episode_id"), "{plan}");
}
