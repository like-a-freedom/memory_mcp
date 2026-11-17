use std::sync::Arc;

use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::{Value, json};
use tokio::sync::Barrier;

async fn client_with_memory_migrations(database: &str, namespace: &str) -> SurrealDbClient {
    let client = SurrealDbClient::connect_in_memory(database, namespace, "ERROR")
        .await
        .expect("in-memory SurrealDB client");
    client
        .apply_migrations(namespace)
        .await
        .expect("memory migrations should apply");
    client
}

async fn stored_index_definition(
    client: &SurrealDbClient,
    table: &str,
    index: &str,
    namespace: &str,
) -> String {
    let info = client
        .query(&format!("INFO FOR TABLE {table};"), None, namespace)
        .await
        .expect("read table index definitions");
    info.as_array()
        .and_then(|rows| rows.first())
        .and_then(Value::as_object)
        .and_then(|table| table.get("indexes"))
        .and_then(Value::as_object)
        .and_then(|indexes| indexes.get(index))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("index `{index}` is missing from table `{table}`: {info:#?}"))
        .to_owned()
}

macro_rules! managed_index_migration_contract {
    ($test_name:ident, $table:literal, $index:literal, $definition_fragment:literal) => {
        #[tokio::test]
        async fn $test_name() {
            let test_name = stringify!($test_name);
            let namespace = format!("{test_name}_namespace");
            let database = format!("{test_name}_database");
            let client = client_with_memory_migrations(&database, &namespace).await;
            let definition = stored_index_definition(&client, $table, $index, &namespace).await;
            let prefix = format!("DEFINE INDEX {} ON {} ", $index, $table);

            assert!(definition.starts_with(&prefix), "{definition}");
            assert!(definition.contains($definition_fragment), "{definition}");
        }
    };
}

managed_index_migration_contract!(
    edge_temporal_index_is_defined_on_edge,
    "edge",
    "edge_temporal_idx",
    "FIELDS in, out, t_valid, t_invalid"
);
managed_index_migration_contract!(
    fact_validity_index_is_defined_on_fact,
    "fact",
    "fact_t_valid",
    "FIELDS t_valid"
);
managed_index_migration_contract!(
    episode_reference_time_index_is_defined_on_episode,
    "episode",
    "episode_t_ref",
    "FIELDS t_ref"
);
managed_index_migration_contract!(
    edge_validity_index_is_defined_on_edge,
    "edge",
    "edge_t_valid",
    "FIELDS t_valid"
);
managed_index_migration_contract!(
    left_active_relation_index_is_defined_on_claim_relation,
    "claim_relation",
    "claim_relation_left_active_idx",
    "FIELDS left_claim_id, t_invalid_ingested"
);
managed_index_migration_contract!(
    right_active_relation_index_is_defined_on_claim_relation,
    "claim_relation",
    "claim_relation_right_active_idx",
    "FIELDS right_claim_id, t_invalid_ingested"
);
managed_index_migration_contract!(
    claim_job_lease_index_is_defined_on_claim_job,
    "claim_job",
    "claim_job_lease_idx",
    "FIELDS status, lease_expires_at, job_id"
);
managed_index_migration_contract!(
    inbox_revision_lease_index_is_defined_on_inbox_revision,
    "inbox_revision",
    "inbox_revision_state_lease",
    "FIELDS state, lease_expires_at"
);
managed_index_migration_contract!(
    community_summary_index_keeps_its_fulltext_parameters,
    "community",
    "community_summary_search",
    "FULLTEXT ANALYZER memory_fts BM25(1.2,0.75)"
);
managed_index_migration_contract!(
    fact_embedding_index_keeps_its_hnsw_parameters,
    "fact",
    "fact_embedding_hnsw",
    "HNSW DIMENSION 1536 DIST EUCLIDEAN TYPE F32 EFC 150 M 12 M0 24 LM"
);

#[cfg(feature = "eval-support")]
#[tokio::test]
async fn fact_embedding_index_uses_the_configured_provider_dimension() {
    let client = SurrealDbClient::connect_in_memory_with_dimension(
        "embedding_dimension_database",
        "embedding_dimension_namespace",
        "ERROR",
        3072,
    )
    .await
    .expect("in-memory SurrealDB client with configured dimension");
    client
        .apply_migrations("embedding_dimension_namespace")
        .await
        .expect("memory migrations should apply");

    let definition = stored_index_definition(
        &client,
        "fact",
        "fact_embedding_hnsw",
        "embedding_dimension_namespace",
    )
    .await;

    assert!(definition.contains("DIMENSION 3072"), "{definition}");
}

#[tokio::test]
async fn surrealdb_reports_index_definition_and_build_state_separately() {
    let client = SurrealDbClient::connect_in_memory(
        "index_info_probe_database",
        "index_info_probe_namespace",
        "ERROR",
    )
    .await
    .expect("in-memory SurrealDB client");
    client
        .query(
            "DEFINE TABLE index_info_probe SCHEMALESS;",
            None,
            "index_info_probe_namespace",
        )
        .await
        .expect("create probe table");
    client
        .query(
            "DEFINE INDEX index_info_probe_idx ON TABLE index_info_probe FIELDS value;",
            None,
            "index_info_probe_namespace",
        )
        .await
        .expect("create probe index");

    let definition = stored_index_definition(
        &client,
        "index_info_probe",
        "index_info_probe_idx",
        "index_info_probe_namespace",
    )
    .await;
    let state = client
        .query(
            "INFO FOR INDEX index_info_probe_idx ON TABLE index_info_probe;",
            None,
            "index_info_probe_namespace",
        )
        .await
        .expect("read index build state");
    let status = state
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(Value::as_object)
        .and_then(|info| info.get("building"))
        .and_then(Value::as_object)
        .and_then(|building| building.get("status"))
        .and_then(Value::as_str);

    assert!(
        definition.contains("ON index_info_probe FIELDS `value`"),
        "{definition}"
    );
    assert_eq!(status, Some("ready"), "{state:#?}");
}

#[tokio::test]
async fn surreal_sdk_reports_the_embedded_server_version() {
    let client = SurrealDbClient::connect_in_memory(
        "server_version_database",
        "server_version_namespace",
        "ERROR",
    )
    .await
    .expect("in-memory SurrealDB client");

    let version = client
        .server_version("server_version_namespace")
        .await
        .expect("read the SDK version operation");

    assert!(version.is_some_and(|version| version.starts_with("SurrealDB 4.")));
}

fn maintenance_record(namespace: &str, database: &str, attempt_id: &str) -> Value {
    json!({
        "key": {
            "namespace": namespace,
            "database": database,
            "revision": "surrealdb_4_index_rebuild_v1"
        },
        "role": "primary_memory",
        "provenance": "pre_existing",
        "server_version": "4.0.1",
        "owner_id": format!("test-{attempt_id}"),
        "attempt_id": attempt_id,
        "state": "active",
        "current_index": null,
        "indexes": {},
        "updated_at_ms": 1
    })
}

#[tokio::test]
async fn surrealdb_create_only_admits_one_concurrent_durable_claim() {
    let client = Arc::new(
        SurrealDbClient::connect_in_memory(
            "maintenance_claim_database",
            "maintenance_claim_namespace",
            "ERROR",
        )
        .await
        .expect("in-memory SurrealDB client"),
    );
    let barrier = Arc::new(Barrier::new(3));
    let mut submissions = Vec::with_capacity(2);
    for attempt_id in ["first-attempt", "second-attempt"] {
        let client = Arc::clone(&client);
        let barrier = Arc::clone(&barrier);
        submissions.push(tokio::spawn(async move {
            barrier.wait().await;
            client
                .query(
                    "CREATE ONLY memory_v4_maintenance_claim:surrealdb_4_index_rebuild_v1 CONTENT $record;",
                    Some(json!({
                                            "record": maintenance_record(
                                                "maintenance_claim_namespace",
                                                "maintenance_claim_database",
                                                attempt_id,
                                            )
                                        })),
                    "maintenance_claim_namespace",
                )
                .await
        }));
    }
    barrier.wait().await;
    let first = submissions.remove(0).await.expect("first task");
    let second = submissions.remove(0).await.expect("second task");
    let outcomes = [first, second];

    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_err()).count(),
        1
    );
}

#[tokio::test]
async fn surrealdb_claim_update_is_fenced_by_the_attempt_id() {
    let client =
        SurrealDbClient::connect_in_memory("claim_cas_database", "claim_cas_namespace", "ERROR")
            .await
            .expect("in-memory SurrealDB client");
    client
        .query(
            "CREATE ONLY memory_v4_maintenance_claim:surrealdb_4_index_rebuild_v1 CONTENT $record;",
            Some(json!({
                "record": maintenance_record(
                    "claim_cas_namespace",
                    "claim_cas_database",
                    "old-attempt",
                )
            })),
            "claim_cas_namespace",
        )
        .await
        .expect("create initial maintenance claim");

    let recovered = client
        .query(
            "UPDATE memory_v4_maintenance_claim:surrealdb_4_index_rebuild_v1 CONTENT $record WHERE attempt_id = $expected_attempt_id RETURN AFTER;",
            Some(json!({
                "record": maintenance_record(
                                    "claim_cas_namespace",
                                    "claim_cas_database",
                                    "recovery-attempt",
                                ),
                "expected_attempt_id": "old-attempt"
            })),
            "claim_cas_namespace",
        )
        .await
        .expect("replace the claim using its prior attempt id");
    let stale_owner = client
        .query(
            "UPDATE memory_v4_maintenance_claim:surrealdb_4_index_rebuild_v1 CONTENT $record WHERE attempt_id = $expected_attempt_id RETURN AFTER;",
            Some(json!({
                "record": maintenance_record(
                                    "claim_cas_namespace",
                                    "claim_cas_database",
                                    "stale-attempt",
                                ),
                "expected_attempt_id": "old-attempt"
            })),
            "claim_cas_namespace",
        )
        .await
        .expect("a stale owner must receive no updated record");

    assert!(
        !recovered.as_array().is_some_and(Vec::is_empty),
        "{recovered:#?}"
    );
    assert!(
        stale_owner.as_array().is_some_and(Vec::is_empty),
        "{stale_owner:#?}"
    );

    let persisted = client
        .query(
            "SELECT * FROM memory_v4_maintenance_claim:surrealdb_4_index_rebuild_v1;",
            None,
            "claim_cas_namespace",
        )
        .await
        .expect("read the durable maintenance claim");

    assert_eq!(persisted[0]["key"]["namespace"], "claim_cas_namespace");
    assert_eq!(persisted[0]["key"]["database"], "claim_cas_database");
}
