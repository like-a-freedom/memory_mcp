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
use serde_json::{Value, json};

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
async fn community_member_element_index_preserves_popular_and_missing_results() {
    let (_service, db) = common::make_service_with_client().await;
    let now = chrono::Utc::now();
    for community_id in [
        "community:member_match_a",
        "community:member_match_b",
        "community:member_match_c",
    ] {
        common::seed_community(
            &db,
            "legacy-scope",
            community_id,
            &["entity:popular_member".to_owned()],
            "popular member community",
            now,
        )
        .await;
    }
    common::seed_community(
        &db,
        "legacy-scope",
        "community:member_nonmatch",
        &["entity:other_member".to_owned()],
        "nonmatching community",
        now,
    )
    .await;
    db.query(
        "CREATE community:member_legacy_empty SET community_id = 'community:member_legacy_empty', member_entities = [], summary = 'legacy row without memberships', updated_at = d'2026-10-14T10:00:00Z'",
        None,
        "org",
    )
    .await
    .expect("seed legacy community without membership values");

    let (sql, vars) =
        build_select_communities_by_member_entities_query(&["entity:popular_member".to_owned()]);
    let plan = plan_for(&*db, &sql, vars.clone()).await;
    assert_indexed(&plan, "community_members", "popular community membership");
    let rows = db
        .query(&sql, Some(vars), "org")
        .await
        .expect("read communities containing the popular member");
    let rows: Vec<Value> = serde_json::from_value(rows).expect("community rows");
    let community_ids: Vec<&str> = rows
        .iter()
        .map(|row| row["community_id"].as_str().expect("community id"))
        .collect();
    assert_eq!(
        community_ids,
        vec![
            "community:member_match_a",
            "community:member_match_b",
            "community:member_match_c",
        ]
    );

    let (missing_sql, missing_vars) =
        build_select_communities_by_member_entities_query(&["entity:missing_member".to_owned()]);
    let missing = db
        .query(&missing_sql, Some(missing_vars), "org")
        .await
        .expect("read missing community membership");
    let missing: Vec<Value> = serde_json::from_value(missing).expect("missing community rows");
    assert!(missing.is_empty());
}

#[tokio::test]
async fn entity_by_alias_uses_the_element_index() {
    let (_service, db) = common::make_service_with_client().await;
    let (sql, vars) = build_select_entity_by_alias_query("al");
    let plan = plan_for(&*db, &sql, vars).await;
    assert_indexed(&plan, "entity_aliases", "entity by alias");
}

#[tokio::test]
async fn alias_element_index_returns_the_alias_owner_and_ignores_legacy_empty_lists() {
    let (_service, db) = common::make_service_with_client().await;
    common::seed_entity(
        &db,
        "legacy-scope",
        "entity:alias_owner",
        "person",
        "Alias owner",
        &["distinctive alias".to_owned()],
    )
    .await;
    db.query(
        "CREATE entity:alias_legacy_empty SET entity_id = 'entity:alias_legacy_empty', entity_type = 'person', canonical_name = 'Legacy entity', canonical_name_normalized = 'legacy entity', aliases = []",
        None,
        "org",
    )
    .await
    .expect("seed legacy entity without aliases");

    let (sql, vars) = build_select_entity_by_alias_query("distinctive alias");
    let plan = plan_for(&*db, &sql, vars.clone()).await;
    assert_indexed(&plan, "entity_aliases", "alias membership");
    let rows = db
        .query(&sql, Some(vars), "org")
        .await
        .expect("read alias owner");
    let rows: Vec<Value> = serde_json::from_value(rows).expect("entity rows");
    assert_eq!(rows, vec![json!({ "entity_id": "entity:alias_owner" })]);

    let (missing_sql, missing_vars) = build_select_entity_by_alias_query("alias with no owner");
    let missing = db
        .query(&missing_sql, Some(missing_vars), "org")
        .await
        .expect("read missing alias");
    let missing: Vec<Value> = serde_json::from_value(missing).expect("missing entity rows");
    assert!(missing.is_empty());
}

#[tokio::test]
async fn facts_by_source_episode_use_the_provenance_index() {
    let (_service, db) = common::make_service_with_client().await;
    // The fact→episode provenance lookup (`fact_access_store`, and the
    // graph-store fact read) filters on `source_episode`.
    let plan = plan_for(
        &*db,
        "SELECT * FROM fact WHERE source_episode = $episode_id LIMIT 5",
        serde_json::json!({ "episode_id": "episode:x" }),
    )
    .await;
    assert_indexed(&plan, "fact_source_episode", "facts by source episode");
}

#[tokio::test]
async fn source_episode_index_returns_all_matching_facts_and_skips_nonmatching_rows() {
    let (_service, db) = common::make_service_with_client().await;
    for (fact_id, source_episode) in [
        ("fact:source_match_a", "episode:popular_source"),
        ("fact:source_match_b", "episode:popular_source"),
        ("fact:source_match_c", "episode:popular_source"),
        ("fact:source_nonmatch", "episode:other_source"),
    ] {
        let row = json!({
            "fact_id": fact_id,
            "fact_type": "note",
            "content": format!("content for {fact_id}"),
            "quote": format!("quote for {fact_id}"),
            "source_episode": source_episode,
            "t_valid": "2026-10-13T10:00:00Z",
            "t_ingested": "2026-10-13T10:00:00Z",
            "confidence": 0.9,
            "entity_links": [],
            "scope": "main",
            "policy_tags": [],
            "provenance": {},
        });
        db.create(
            fact_id,
            row,
            "org",
            memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS,
        )
        .await
        .expect("seed source-linked fact");
    }

    let sql = "SELECT fact_id FROM fact WHERE source_episode = $episode_id ORDER BY fact_id ASC";
    let vars = json!({ "episode_id": "episode:popular_source" });
    let plan = plan_for(&*db, sql, vars.clone()).await;
    assert_indexed(&plan, "fact_source_episode", "popular source episode");
    let rows = db
        .query(sql, Some(vars), "org")
        .await
        .expect("read facts linked to source episode");
    let rows: Vec<Value> = serde_json::from_value(rows).expect("source fact rows");
    let fact_ids: Vec<&str> = rows
        .iter()
        .map(|row| row["fact_id"].as_str().expect("fact id"))
        .collect();
    assert_eq!(
        fact_ids,
        vec![
            "fact:source_match_a",
            "fact:source_match_b",
            "fact:source_match_c",
        ]
    );

    let missing = db
        .query(
            sql,
            Some(json!({ "episode_id": "episode:missing_source" })),
            "org",
        )
        .await
        .expect("read missing source episode");
    let missing: Vec<Value> = serde_json::from_value(missing).expect("missing source fact rows");
    assert!(missing.is_empty());
}

#[tokio::test]
async fn temporal_index_preserves_the_cutoff_result_set() {
    let (_service, db) = common::make_service_with_client().await;
    for (fact_id, t_valid) in [
        ("fact:before_cutoff", "2025-12-31T23:59:59Z"),
        ("fact:at_cutoff", "2026-01-01T00:00:00Z"),
        ("fact:after_cutoff", "2026-01-01T00:00:01Z"),
    ] {
        db.create(
            fact_id,
            json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": fact_id,
                "quote": fact_id,
                "source_episode": "episode:time_index_probe",
                "t_valid": t_valid,
                "t_ingested": "2025-12-31T00:00:00Z",
                "confidence": 0.9,
                "entity_links": [],
                "scope": "main",
                "policy_tags": [],
                "provenance": {},
            }),
            "org",
            memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS,
        )
        .await
        .expect("seed temporally ordered fact");
    }

    let (sql, vars) = build_select_facts_filtered_query("2026-01-01T00:00:00Z", None, 10, &[]);
    let plan = plan_for(&*db, &sql, vars.clone()).await;
    assert_indexed(&plan, "fact_t_valid", "temporal cutoff");
    let rows = db
        .query(&sql, Some(vars), "org")
        .await
        .expect("read facts visible at cutoff");
    let rows: Vec<Value> = serde_json::from_value(rows).expect("visible fact rows");
    let fact_ids: Vec<&str> = rows
        .iter()
        .map(|row| row["fact_id"].as_str().expect("fact id"))
        .collect();
    assert_eq!(fact_ids, vec!["fact:at_cutoff", "fact:before_cutoff"]);
}
