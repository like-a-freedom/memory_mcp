//! Regression coverage for the multi-endpoint graph read.
//!
//! The fixtures use embedded SurrealDB so result identity and the planner's
//! endpoint-index choice are observed against the pinned engine.

mod common;

use chrono::{DateTime, Duration, TimeZone, Utc};
use memory_mcp::knowledge::KnowledgeGraphStore;
use memory_mcp::knowledge::queries::build_select_edge_neighbors_for_entities_query;
use memory_mcp::service::{normalize_dt, normalize_text};
use memory_mcp::storage::DbClient;
use serde_json::json;
use std::collections::BTreeSet;

async fn seed_entity_with_bound_key(db: &dyn DbClient, entity_id: &str, label: &str) {
    let (table, key) = entity_id.split_once(':').expect("entity record id");
    db.query(
        "CREATE type::record($table, $key) SET entity_id = $entity_id, entity_type = 'person', canonical_name = $canonical_name, canonical_name_normalized = $normalized_name, aliases = []",
        Some(json!({
            "table": table,
            "key": key,
            "entity_id": entity_id,
            "canonical_name": label,
            "normalized_name": normalize_text(label),
        })),
        "org",
    )
    .await
    .expect("bound entity fixture should be written");
}

async fn seed_edge(
    store: &KnowledgeGraphStore,
    edge_id: &str,
    from_id: &str,
    to_id: &str,
    relation: &str,
) {
    let now = Utc::now();
    seed_edge_at(
        store, edge_id, from_id, to_id, relation, now, now, None, None,
    )
    .await;
}

async fn seed_edge_at(
    store: &KnowledgeGraphStore,
    edge_id: &str,
    from_id: &str,
    to_id: &str,
    relation: &str,
    t_valid: DateTime<Utc>,
    t_ingested: DateTime<Utc>,
    t_invalid: Option<DateTime<Utc>>,
    t_invalid_ingested: Option<DateTime<Utc>>,
) {
    let mut edge = json!({
        "edge_id": edge_id,
        "relation": relation,
        "origin": "inferred",
        "strength": 1.0,
        "confidence": 1.0,
        "provenance": {"source": "test"},
        "t_valid": normalize_dt(t_valid),
        "t_ingested": normalize_dt(t_ingested),
    });
    if let Some(t_invalid) = t_invalid {
        edge["t_invalid"] = json!(normalize_dt(t_invalid));
    }
    if let Some(t_invalid_ingested) = t_invalid_ingested {
        edge["t_invalid_ingested"] = json!(normalize_dt(t_invalid_ingested));
    }
    store
        .relate_edge(edge_id, from_id, to_id, edge)
        .await
        .expect("edge fixture should be written");
}

#[tokio::test]
async fn batch_graph_lookup_preserves_bitemporal_invalidation_boundaries() {
    let (_service, db) = common::make_service_with_client().await;
    common::seed_entity(&db, "org", "entity:history_anchor", "person", "Anchor", &[]).await;
    common::seed_entity(
        &db,
        "org",
        "entity:history_neighbor",
        "person",
        "Neighbor",
        &[],
    )
    .await;
    let store = KnowledgeGraphStore::new(db.clone(), "org");
    let at = |day| Utc.with_ymd_and_hms(2026, 10, day, 0, 0, 0).unwrap();
    seed_edge_at(
        &store,
        "edge:invalidated_later",
        "entity:history_anchor",
        "entity:history_neighbor",
        "invalidated_later",
        at(1),
        at(1),
        Some(at(10)),
        Some(at(10)),
    )
    .await;
    seed_edge_at(
        &store,
        "edge:invalidation_learned_later",
        "entity:history_anchor",
        "entity:history_neighbor",
        "invalidation_learned_later",
        at(1),
        at(1),
        Some(at(2)),
        Some(at(8)),
    )
    .await;
    seed_edge_at(
        &store,
        "edge:invalidation_known_at_cutoff",
        "entity:history_anchor",
        "entity:history_neighbor",
        "invalidation_known_at_cutoff",
        at(1),
        at(1),
        Some(at(5)),
        Some(at(5)),
    )
    .await;
    seed_edge_at(
        &store,
        "edge:invalidation_known_before_cutoff",
        "entity:history_anchor",
        "entity:history_neighbor",
        "invalidation_known_before_cutoff",
        at(1),
        at(1),
        Some(at(2)),
        Some(at(2)),
    )
    .await;

    let seeds = vec!["entity:history_anchor".to_owned()];
    let (sql, vars) = build_select_edge_neighbors_for_entities_query(&seeds, &at(5).to_rfc3339());
    let at_five = db
        .query(&sql, Some(vars), "org")
        .await
        .expect("historical graph query at day five");
    let visible_at_five: BTreeSet<_> = at_five
        .as_array()
        .expect("edge rows")
        .iter()
        .map(|row| row["relation"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        visible_at_five,
        BTreeSet::from([
            "invalidation_learned_later".to_owned(),
            "invalidated_later".to_owned(),
        ])
    );

    let (sql, vars) = build_select_edge_neighbors_for_entities_query(&seeds, &at(11).to_rfc3339());
    let at_eleven = db
        .query(&sql, Some(vars), "org")
        .await
        .expect("historical graph query at day eleven");
    assert!(
        at_eleven.as_array().expect("edge rows").is_empty(),
        "known invalidations must be excluded after their transaction time: {at_eleven}"
    );
}

#[tokio::test]
async fn batch_graph_lookup_preserves_literal_rows_for_hostile_ids_and_self_loops() {
    let (_service, db) = common::make_service_with_client().await;
    let ids = [
        "entity:hostile:colon",
        "entity:space key",
        "entity:雪⟩node",
        "entity:duplicate_seed",
        "entity:neighbor_one",
        "entity:neighbor_two",
    ];
    for (index, entity_id) in ids.iter().enumerate() {
        seed_entity_with_bound_key(&*db, entity_id, &format!("Graph fixture {index}")).await;
    }
    let store = KnowledgeGraphStore::new(db.clone(), "org");
    seed_edge(
        &store,
        "edge:hostile_outgoing",
        ids[0],
        ids[4],
        "hostile_outgoing",
    )
    .await;
    seed_edge(
        &store,
        "edge:hostile_incoming",
        ids[5],
        ids[1],
        "hostile_incoming",
    )
    .await;
    seed_edge(
        &store,
        "edge:unicode_outgoing",
        ids[2],
        ids[5],
        "unicode_outgoing",
    )
    .await;
    seed_edge(&store, "edge:self_loop", ids[3], ids[3], "self_loop").await;
    seed_edge(
        &store,
        "edge:duplicate_seed_row",
        ids[3],
        ids[4],
        "duplicate_seed_row",
    )
    .await;

    let seeds = vec![
        ids[0].to_owned(),
        ids[1].to_owned(),
        ids[2].to_owned(),
        ids[3].to_owned(),
        ids[0].to_owned(),
    ];
    let cutoff = (Utc::now() + Duration::days(1)).to_rfc3339();
    let (sql, vars) = build_select_edge_neighbors_for_entities_query(&seeds, &cutoff);
    let rows = db
        .query(&sql, Some(vars), "org")
        .await
        .expect("batched graph query succeeds");
    let rows = rows.as_array().expect("query returns edge rows");

    assert_eq!(
        rows.len(),
        5,
        "duplicate seeds and self-loops are rows once: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| {
            row["relation"] == "hostile_outgoing" && row["in"]["RecordId"]["key"] == "hostile:colon"
        }),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| {
            row["relation"] == "hostile_incoming" && row["out"]["RecordId"]["key"] == "space key"
        }),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| {
            row["relation"] == "unicode_outgoing" && row["in"]["RecordId"]["key"] == "雪⟩node"
        }),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|row| row["relation"] == "self_loop"),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row["relation"] == "duplicate_seed_row"),
        "{rows:?}"
    );
}

#[tokio::test]
async fn batch_graph_lookup_keeps_all_rows_across_the_32_seed_boundary() {
    let (_service, db) = common::make_service_with_client().await;
    common::seed_entity(
        &db,
        "org",
        "entity:batch_target",
        "person",
        "Batch target",
        &[],
    )
    .await;
    let store = KnowledgeGraphStore::new(db.clone(), "org");
    let seed_ids: Vec<_> = (0..33)
        .map(|index| format!("entity:batch_seed_{index:02}"))
        .collect();
    for (index, seed_id) in seed_ids.iter().enumerate() {
        common::seed_entity(
            &db,
            "org",
            seed_id,
            "person",
            &format!("Batch seed {index}"),
            &[],
        )
        .await;
        seed_edge(
            &store,
            &format!("edge:batch_{index:02}"),
            seed_id,
            "entity:batch_target",
            &format!("batch_{index:02}"),
        )
        .await;
    }

    let cutoff = (Utc::now() + Duration::days(1)).to_rfc3339();
    let mut rows = Vec::new();
    for seed_batch in seed_ids.chunks(32) {
        let (sql, vars) = build_select_edge_neighbors_for_entities_query(seed_batch, &cutoff);
        let batch_rows = db
            .query(&sql, Some(vars), "org")
            .await
            .expect("32-seed graph batch");
        rows.extend(batch_rows.as_array().expect("edge rows").iter().cloned());
    }
    let actual_relations: BTreeSet<_> = rows
        .iter()
        .map(|row| row["relation"].as_str().unwrap_or_default().to_owned())
        .collect();
    let expected_relations: BTreeSet<_> =
        (0..33).map(|index| format!("batch_{index:02}")).collect();

    assert_eq!(rows.len(), 33, "the final one-seed batch must not be lost");
    assert_eq!(actual_relations, expected_relations);
}

#[tokio::test]
async fn batch_graph_lookup_uses_endpoint_indexes_with_many_unrelated_edges() {
    let (_service, db) = common::make_service_with_client().await;
    let seed_ids = vec![
        "entity:planner_seed_a".to_owned(),
        "entity:planner_seed_b".to_owned(),
        "entity:planner_seed_c".to_owned(),
        "entity:planner_seed_d".to_owned(),
    ];
    for (index, seed_id) in seed_ids.iter().enumerate() {
        common::seed_entity(&db, "org", seed_id, "person", &format!("Seed {index}"), &[]).await;
    }
    common::seed_entity(
        &db,
        "org",
        "entity:unrelated_root",
        "person",
        "Unrelated root",
        &[],
    )
    .await;
    common::seed_entity(
        &db,
        "org",
        "entity:unrelated_target",
        "person",
        "Unrelated target",
        &[],
    )
    .await;
    let store = KnowledgeGraphStore::new(db.clone(), "org");
    for index in 0..2_970 {
        seed_edge(
            &store,
            &format!("edge:unrelated_{index:04}"),
            "entity:unrelated_root",
            "entity:unrelated_target",
            "unrelated",
        )
        .await;
    }

    let cutoff = (Utc::now() + Duration::days(1)).to_rfc3339();
    let (sql, vars) = build_select_edge_neighbors_for_entities_query(&seed_ids, &cutoff);
    let legacy_predicates = seed_ids
        .iter()
        .enumerate()
        .map(|(index, _)| {
            format!("(in = <record> $entity_{index} OR out = <record> $entity_{index})")
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    let where_tail = &sql[sql.find(" AND ").expect("temporal predicate position")..];
    let legacy_sql =
        format!("SELECT in, out, relation FROM edge WHERE ({legacy_predicates}){where_tail}");
    let mut legacy_vars = serde_json::Map::from_iter([("cutoff".to_owned(), json!(cutoff))]);
    for (index, entity_id) in seed_ids.iter().enumerate() {
        legacy_vars.insert(format!("entity_{index}"), json!(entity_id));
    }
    let legacy_plan = db
        .query(
            &format!("EXPLAIN {legacy_sql}"),
            Some(serde_json::Value::Object(legacy_vars)),
            "org",
        )
        .await
        .expect("EXPLAIN original cast-based graph query")
        .to_string();
    assert!(
        legacy_plan.contains("edge_t_valid"),
        "the original cast-based query should fall back to the temporal index: {legacy_plan}"
    );
    assert!(
        !legacy_plan.contains("edge_in") && !legacy_plan.contains("edge_out"),
        "the original query unexpectedly uses an endpoint index: {legacy_plan}"
    );

    let plan = db
        .query(&format!("EXPLAIN {sql}"), Some(vars), "org")
        .await
        .expect("EXPLAIN batched graph query")
        .to_string();

    assert!(
        plan.contains("IndexScan"),
        "expected indexed endpoint plan: {plan}"
    );
    assert!(
        plan.contains("edge_in"),
        "expected incoming endpoint index: {plan}"
    );
    assert!(
        plan.contains("edge_out"),
        "expected outgoing endpoint index: {plan}"
    );
    assert!(
        !plan.contains("TableScan"),
        "unexpected endpoint table scan: {plan}"
    );
}
