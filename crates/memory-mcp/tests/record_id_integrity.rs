//! Record-id round-trip integrity for graph writes.
//!
//! A record id written into the `entity` table and the same id used as an edge
//! endpoint must denote one and the same record. On SurrealDB 3.3 the
//! `<record> $param` cast parses its input as a record-id *literal* and
//! truncates at the first character outside `[A-Za-z0-9_]`, so
//! `entity:trip-a` resolves to the key `trip`. Reads built on
//! `type::record($table, $key)` keep the key verbatim, so a cast write and a
//! `type::record` read disagree — and the edge ends up pointing at a record
//! that does not exist. Constructing the record in-query keeps the two halves
//! of the graph in agreement for every key shape.

mod common;

use memory_mcp::models::EdgeAttributes;
use memory_mcp::storage::DbClient;

/// The id of the record a row is stored under, as `(table, key)`.
fn record_id_of(row: &serde_json::Value) -> (String, String) {
    (
        row["id"]["RecordId"]["table"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        row["id"]["RecordId"]["key"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

#[tokio::test]
async fn edge_endpoints_are_the_records_the_entities_were_stored_under() {
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

    let entities = db_client
        .query("SELECT * FROM entity", None, "org")
        .await
        .expect("read entities");
    let entities = entities.as_array().expect("entity rows").clone();

    service
        .relate(
            "entity:trip-a",
            "knows",
            "entity:trip-c",
            EdgeAttributes::inferred(),
        )
        .await
        .expect("relate trip-a to trip-c");

    let edges = db_client
        .query("SELECT * FROM edge", None, "org")
        .await
        .expect("read edges");
    let edge = &edges.as_array().expect("edge rows")[0];

    let stored = |field: &str| {
        (
            edge[field]["RecordId"]["table"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            edge[field]["RecordId"]["key"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    };

    assert!(
        entities.iter().any(|row| record_id_of(row) == stored("in")),
        "edge `in` {:?} must be a record an entity is actually stored under: {entities:?}",
        stored("in")
    );
    assert!(
        entities
            .iter()
            .any(|row| record_id_of(row) == stored("out")),
        "edge `out` {:?} must be a record an entity is actually stored under: {entities:?}",
        stored("out")
    );
}
