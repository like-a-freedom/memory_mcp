//! Every read the context-assembly, explain and entity-resolution paths issue
//! must be planned against an index, not a table scan.
//!
//! The plan shapes are asserted as substrings of the serialized EXPLAIN value
//! because the exact tree is version-specific; the observable signal is that an
//! `IndexScan` names the expected index. See the 2026-10-09 query-index-coverage
//! plan for the measurements this pins.

mod common;

use memory_mcp::knowledge::queries::{
    build_select_active_facts_query, build_select_communities_by_member_entities_query,
    build_select_edges_filtered_page_query, build_select_entity_by_alias_query,
    build_select_facts_filtered_query,
};
use memory_mcp::memory::queries::build_select_episodes_by_content_query;
use memory_mcp::storage::DbClient;

async fn plan_for(db: &dyn DbClient, sql: &str, vars: serde_json::Value) -> String {
    db.query(&format!("EXPLAIN {sql}"), Some(vars), "org")
        .await
        .expect("explain")
        .to_string()
}

fn assert_indexed(plan: &str, index: &str, label: &str) {
    assert!(
        plan.contains("IndexScan"),
        "{label}: expected an index scan, got:\n{plan}"
    );
    assert!(
        plan.contains(index),
        "{label}: expected index {index}, got:\n{plan}"
    );
}

#[tokio::test]
async fn fact_retrieval_orders_through_the_time_index() {
    let (_service, db) = common::make_service_with_client().await;
    let (sql, vars) = build_select_facts_filtered_query("2026-01-01T00:00:00Z", None, 10, &[]);
    let plan = plan_for(&*db, &sql, vars).await;
    assert_indexed(&plan, "fact_t_valid", "facts filtered");
}

#[tokio::test]
async fn active_facts_read_through_the_time_index() {
    let (_service, db) = common::make_service_with_client().await;
    let (sql, vars) = build_select_active_facts_query("2026-01-01T00:00:00Z", 5);
    let plan = plan_for(&*db, &sql, vars).await;
    assert_indexed(&plan, "fact_t_valid", "active facts");
}

#[tokio::test]
async fn episode_content_fallback_orders_through_the_time_index() {
    let (_service, db) = common::make_service_with_client().await;
    let (sql, vars) = build_select_episodes_by_content_query("2026-01-01T00:00:00Z", None, 10);
    let plan = plan_for(&*db, &sql, vars).await;
    assert_indexed(&plan, "episode_t_ref", "episodes by content");
}

#[tokio::test]
async fn edge_page_orders_through_the_time_index() {
    let (_service, db) = common::make_service_with_client().await;
    let (sql, vars) = build_select_edges_filtered_page_query("2026-01-01T00:00:00Z", 250, 0);
    let plan = plan_for(&*db, &sql, vars).await;
    assert_indexed(&plan, "edge_t_valid", "edges page");
}

#[tokio::test]
async fn communities_by_member_use_the_element_index() {
    let (_service, db) = common::make_service_with_client().await;
    let (sql, vars) = build_select_communities_by_member_entities_query(&["entity:x".to_string()]);
    let plan = plan_for(&*db, &sql, vars).await;
    assert_indexed(&plan, "community_members", "communities by member");
}

#[tokio::test]
async fn entity_by_alias_uses_the_element_index() {
    let (_service, db) = common::make_service_with_client().await;
    let (sql, vars) = build_select_entity_by_alias_query("al");
    let plan = plan_for(&*db, &sql, vars).await;
    assert_indexed(&plan, "entity_aliases", "entity by alias");
}
