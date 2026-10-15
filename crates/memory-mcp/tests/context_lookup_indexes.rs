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
use serde_json::{Value, json};

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

#[tokio::test]
async fn episode_id_and_source_context_indexes_return_only_matching_rows() {
    let (_service, db) = common::make_service_with_client().await;
    for (episode_id, source_group, source_position) in [
        ("episode:lookup_hit", Some("batch-a"), Some(1)),
        ("episode:lookup_other_position", Some("batch-a"), Some(2)),
        ("episode:lookup_other_group", Some("batch-b"), Some(1)),
        ("episode:lookup_legacy_null", None, None),
    ] {
        let mut row = json!({
            "episode_id": episode_id,
            "source_type": "document",
            "source_id": episode_id,
            "content": format!("content for {episode_id}"),
            "t_ref": "2026-10-14T10:00:00Z",
            "t_ingested": "2026-10-14T10:00:00Z",
            "policy_tags": [],
        });
        if let Some(source_group) = source_group {
            row["source_group"] = json!(source_group);
        }
        if let Some(source_position) = source_position {
            row["source_position"] = json!(source_position);
        }
        db.create(
            episode_id,
            row,
            "org",
            memory_mcp::memory::queries::EPISODE_TEMPORAL_FIELDS,
        )
        .await
        .expect("seed episode row");
    }

    let (episode_sql, episode_vars) =
        build_select_episodes_by_ids_query(&[json!("episode:lookup_hit")]);
    let episode_plan = db
        .query(
            &format!("EXPLAIN {episode_sql}"),
            Some(episode_vars.clone()),
            "org",
        )
        .await
        .expect("explain episode id lookup")
        .to_string();
    assert!(episode_plan.contains("IndexScan"), "{episode_plan}");
    assert!(
        episode_plan.contains("episode_episode_id"),
        "{episode_plan}"
    );
    let episode_rows = db
        .query(&episode_sql, Some(episode_vars), "org")
        .await
        .expect("read episode by id");
    let episode_rows: Vec<Value> = serde_json::from_value(episode_rows).expect("episode rows");
    assert_eq!(episode_rows.len(), 1);
    assert_eq!(episode_rows[0]["episode_id"], "episode:lookup_hit");

    let source_context_sql = "SELECT episode_id FROM episode WHERE source_group = $group AND source_position = $position ORDER BY episode_id ASC";
    let source_context_vars = json!({ "group": "batch-a", "position": 1 });
    let source_context_plan = db
        .query(
            &format!("EXPLAIN {source_context_sql}"),
            Some(source_context_vars.clone()),
            "org",
        )
        .await
        .expect("explain source context lookup")
        .to_string();
    assert!(
        source_context_plan.contains("IndexScan"),
        "{source_context_plan}"
    );
    assert!(
        source_context_plan.contains("episode_source_context"),
        "{source_context_plan}"
    );
    let source_context_rows = db
        .query(source_context_sql, Some(source_context_vars), "org")
        .await
        .expect("read matching source context");
    let source_context_rows: Vec<Value> =
        serde_json::from_value(source_context_rows).expect("source context rows");
    assert_eq!(
        source_context_rows,
        vec![json!({ "episode_id": "episode:lookup_hit" })]
    );
}
