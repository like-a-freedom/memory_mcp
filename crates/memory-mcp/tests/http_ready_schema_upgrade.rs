#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

use std::sync::Arc;

use memory_mcp::http::leases::migration::{ApplyMigrations, SurrealTenantMigrations};
use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
use memory_mcp::http::runtime::storage::{RuntimeOptions, build_runtime_with_options};
use memory_mcp::storage::DbClient;
use serde_json::Value;

async fn ready_legacy_fixture() -> (
    RegistryHandle,
    Arc<dyn DbClient>,
    SurrealTenantMigrations,
    Tenant,
) {
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let engine = registry
        .tenant_engine_optional()
        .expect("test engine is configured");
    let migrations = SurrealTenantMigrations::new(engine.clone());
    let tenant = Tenant {
        id: "tenant_legacy_ready".to_owned(),
        status: TenantStatus::Ready,
        namespace_binding: NamespaceBinding {
            namespace: "ns_legacy_ready".to_owned(),
            database: "memory".to_owned(),
        },
        plan_version: 1,
        schema_version: 44,
        retry_stage: None,
        provisioning_lease: None,
        created_at: chrono::Utc::now(),
        version: 1,
    };
    registry
        .tenants()
        .write_tenant(&tenant)
        .await
        .expect("ready tenant is registered");
    migrations
        .ensure_namespace(
            &tenant.namespace_binding.namespace,
            &tenant.namespace_binding.database,
        )
        .await
        .expect("legacy tenant namespace is created");
    migrations
        .apply_migrations(
            &tenant.namespace_binding.namespace,
            &tenant.namespace_binding.database,
        )
        .await
        .expect("catalog is initially provisioned");
    let db = engine
        .bind_to_test_namespace(&tenant.namespace_binding.namespace)
        .await;
    let db: Arc<dyn DbClient> = db;
    db.query(
        "CREATE fact:ready_schema_probe SET fact_id = 'fact:ready_schema_probe', fact_type = 'decision', content = 'preserve this fact', quote = 'preserve this fact', source_episode = 'episode:ready_schema_probe', t_valid = d'2026-10-14T00:00:00Z', t_ingested = d'2026-10-14T00:00:00Z', confidence = 1.0, entity_links = [], scope = 'main', policy_tags = [], provenance = {}",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("seed durable fact");
    db.query(
        "REMOVE INDEX fact_embedding_hnsw ON TABLE fact",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("remove current embedding index before dimension preservation probe");
    db.query(
        "DEFINE INDEX fact_embedding_hnsw ON TABLE fact FIELDS embedding HNSW DIMENSION 17",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("seed a dimension-specific index");
    let stored_vector: Vec<f64> = (1..=17).map(|value| f64::from(value) / 17.0).collect();
    db.query(
        "UPDATE fact:ready_schema_probe SET embedding = $embedding",
        Some(serde_json::json!({ "embedding": stored_vector })),
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("seed durable vector at its existing dimension");
    db.query(
        "UPSERT tenant_change_sequence:default SET value = 1341",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("seed an outbox high-water mark");
    // Rewind the catalog records and the definitions under test while leaving
    // unrelated schema and durable application data intact.
    for sql in [
        "REMOVE INDEX episode_episode_id ON TABLE episode",
        "REMOVE INDEX fact_source_episode ON TABLE fact",
        "REMOVE INDEX fact_t_valid ON TABLE fact",
        "REMOVE INDEX episode_t_ref ON TABLE episode",
        "REMOVE INDEX edge_t_valid ON TABLE edge",
        "REMOVE INDEX entity_aliases ON TABLE entity",
        "REMOVE INDEX community_members ON TABLE community",
        "REMOVE INDEX episode_source_context ON TABLE episode",
        "REMOVE FIELD source_group ON TABLE episode",
        "REMOVE FIELD source_position ON TABLE episode",
        "REMOVE FIELD kind ON TABLE tenant_task",
    ] {
        db.query(sql, None, &tenant.namespace_binding.namespace)
            .await
            .unwrap_or_else(|error| panic!("legacy schema rewind failed for `{sql}`: {error}"));
    }
    for record_id in [
        "script_migration:053_context_lookup_indexes_surql",
        "script_migration:054_query_index_coverage_surql",
        "script_migration:055_episode_source_context_surql",
    ] {
        db.query(
            &format!("DELETE {record_id}"),
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .unwrap_or_else(|error| panic!("legacy ledger rewind failed for `{record_id}`: {error}"));
    }

    (registry, db, migrations, tenant)
}

#[tokio::test]
async fn ready_legacy_tenant_receives_missing_catalog_before_runtime_admission() {
    let (registry, db, _migrations, tenant) = ready_legacy_fixture().await;

    let before_episode = db
        .query(
            "INFO FOR TABLE episode",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("inspect legacy episode schema")
        .to_string();
    let before_task = db
        .query(
            "INFO FOR TABLE tenant_task",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("inspect legacy task schema")
        .to_string();
    assert!(!before_episode.contains("episode_episode_id"));
    assert!(!before_episode.contains("source_group"));
    assert!(!before_task.contains("kind"));

    let runtime = build_runtime_with_options(&registry, &tenant, RuntimeOptions::default())
        .await
        .expect("legacy ready tenant upgrades before runtime admission");
    drop(runtime);

    let after_episode = db
        .query(
            "INFO FOR TABLE episode",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("inspect upgraded episode schema")
        .to_string();
    let after_fact = db
        .query(
            "INFO FOR TABLE fact",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("inspect upgraded fact schema")
        .to_string();
    let after_task = db
        .query(
            "INFO FOR TABLE tenant_task",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("inspect upgraded task schema")
        .to_string();
    assert!(
        after_episode.contains("episode_episode_id"),
        "{after_episode}"
    );
    assert!(after_episode.contains("episode_t_ref"));
    assert!(after_episode.contains("source_group"));
    assert!(after_episode.contains("source_position"));
    assert!(after_fact.contains("fact_source_episode"));
    assert!(after_fact.contains("fact_t_valid"));
    assert!(after_task.contains("kind"));

    let applied = db
        .query(
            "SELECT VALUE script_name FROM script_migration WHERE script_name IN ['053_context_lookup_indexes.surql', '054_query_index_coverage.surql', '055_episode_source_context.surql'] ORDER BY script_name ASC",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read upgraded migration ledger");
    let applied: Vec<String> = serde_json::from_value(applied).expect("parse migration ledger");
    assert_eq!(
        applied,
        vec![
            "053_context_lookup_indexes.surql",
            "054_query_index_coverage.surql",
            "055_episode_source_context.surql",
        ]
    );

    let fact = db
        .query(
            "SELECT content, embedding FROM fact:ready_schema_probe",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read durable fact after upgrade");
    assert!(fact.to_string().contains("preserve this fact"));
    let fact_rows: Vec<Value> = serde_json::from_value(fact).expect("parse preserved fact");
    let stored_vector: Vec<f64> =
        serde_json::from_value(fact_rows[0]["embedding"].clone()).expect("parse preserved vector");
    assert_eq!(
        stored_vector,
        (1..=17)
            .map(|value| f64::from(value) / 17.0)
            .collect::<Vec<_>>()
    );
    let sequence = db
        .query(
            "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read outbox high-water mark after upgrade");
    let sequence: Vec<u64> = serde_json::from_value(sequence).expect("parse sequence");
    assert_eq!(sequence, vec![1341]);
    assert!(after_fact.to_lowercase().contains("dimension 17"));

    let persisted_tenant = registry
        .tenants()
        .find_tenant_by_id(&tenant.id)
        .await
        .expect("read upgraded tenant")
        .expect("tenant remains registered");
    assert_eq!(persisted_tenant.status, TenantStatus::Ready);
    assert_eq!(persisted_tenant.schema_version, 45);
}

#[tokio::test]
async fn wrong_existing_index_definition_denies_admission_and_can_be_retried() {
    let (registry, db, _migrations, tenant) = ready_legacy_fixture().await;
    db.query(
        "DEFINE INDEX episode_episode_id ON episode FIELDS source_id",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("seed conflicting same-name index");
    let options = RuntimeOptions::default();

    let result = build_runtime_with_options(&registry, &tenant, options.clone()).await;

    assert!(
        matches!(&result, Err(memory_mcp::MemoryError::Storage(message)) if message.contains("episode_episode_id") && message.contains("incompatible definition")),
        "same-name wrong-definition index must deny activation: {:?}",
        result.as_ref().err()
    );
    let stored = registry
        .tenants()
        .find_tenant_by_id(&tenant.id)
        .await
        .expect("read tenant after failed upgrade")
        .expect("tenant remains registered");
    assert_eq!(stored.schema_version, 44);

    db.query(
        "REMOVE INDEX episode_episode_id ON TABLE episode; DEFINE INDEX episode_episode_id ON TABLE episode FIELDS episode_id",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("repair the catalog definition for retry");
    let runtime = build_runtime_with_options(&registry, &tenant, options)
        .await
        .expect("failed upgrades are retried instead of cached");
    drop(runtime);
}

#[tokio::test]
async fn ready_catalog_upgrade_refuses_to_overwrite_a_migration_checksum() {
    let (registry, db, _migrations, tenant) = ready_legacy_fixture().await;
    db.query(
        "CREATE script_migration:054_query_index_coverage_surql SET script_name = '054_query_index_coverage.surql', checksum = 'foreign-checksum', status = 'applied', executed_at = time::now()",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("seed a foreign applied checksum");

    let result = build_runtime_with_options(&registry, &tenant, RuntimeOptions::default()).await;

    assert!(
        matches!(&result, Err(memory_mcp::MemoryError::ConfigInvalid(message)) if message.contains("054_query_index_coverage.surql") && message.contains("modified")),
        "checksum drift must refuse activation without rewriting the record: {:?}",
        result.as_ref().err()
    );
    let checksum = db
        .query(
            "SELECT VALUE checksum FROM script_migration:054_query_index_coverage_surql LIMIT 1",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read original foreign checksum");
    let checksum: Vec<String> = serde_json::from_value(checksum).expect("parse checksum");
    assert_eq!(checksum, vec!["foreign-checksum"]);
}

#[tokio::test]
async fn two_runtime_replicas_share_the_migration_lease_and_advance_once() {
    let (registry, db, _migrations, tenant) = ready_legacy_fixture().await;
    let first = build_runtime_with_options(&registry, &tenant, RuntimeOptions::default());
    let second = build_runtime_with_options(&registry, &tenant, RuntimeOptions::default());

    let (first, second) = tokio::join!(first, second);
    assert!(
        first.is_ok(),
        "first replica upgrades: {:?}",
        first.as_ref().err()
    );
    assert!(
        second.is_ok(),
        "second replica observes the same upgrade: {:?}",
        second.as_ref().err()
    );

    let applied = db
        .query(
            "SELECT VALUE script_name FROM script_migration WHERE script_name IN ['053_context_lookup_indexes.surql', '054_query_index_coverage.surql', '055_episode_source_context.surql'] ORDER BY script_name ASC",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read shared migration ledger");
    let applied: Vec<String> = serde_json::from_value(applied).expect("parse migration ledger");
    assert_eq!(applied.len(), 3);
    assert_eq!(
        applied,
        vec![
            "053_context_lookup_indexes.surql",
            "054_query_index_coverage.surql",
            "055_episode_source_context.surql",
        ]
    );
}

#[tokio::test]
async fn replay_after_055_ddl_before_ledger_commit_preserves_existing_rows() {
    let (_registry, db, migrations, tenant) = ready_legacy_fixture().await;
    memory_mcp::provisioning::api::ensure_ready_schema(
        &migrations,
        &tenant.namespace_binding,
        None,
    )
    .await
    .expect("first ready-schema pass commits DDL and migration ledger");
    db.query(
        "UPDATE script_migration:055_episode_source_context_surql SET status = 'applying', lease_expires_at = d'2020-01-01T00:00:00Z'",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("simulate crash after durable DDL but before applied ledger status");
    let migration_state = db
        .query(
            "SELECT status, lease_expires_at FROM script_migration:055_episode_source_context_surql LIMIT 1",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read crashed migration state");
    assert!(
        migration_state.to_string().contains("2020-01-01"),
        "crash fixture must expire its lease: {migration_state}"
    );
    let replay = memory_mcp::provisioning::api::ensure_ready_schema(
        &migrations,
        &tenant.namespace_binding,
        None,
    )
    .await;
    let migration_state_after_replay = db
        .query(
            "SELECT status, lease_expires_at, last_error FROM script_migration:055_episode_source_context_surql LIMIT 1",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read migration state after replay");
    assert!(
        replay.is_ok(),
        "replay recognizes and verifies the already-applied immutable DDL: {:?}; state={migration_state_after_replay}",
        replay.as_ref().err()
    );

    let fact = db
        .query(
            "SELECT content FROM fact:ready_schema_probe",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read fact after replay");
    assert!(fact.to_string().contains("preserve this fact"));
    let applied = db
        .query(
            "SELECT VALUE status FROM script_migration:055_episode_source_context_surql LIMIT 1",
            None,
            &tenant.namespace_binding.namespace,
        )
        .await
        .expect("read repaired ledger state");
    let applied: Vec<String> = serde_json::from_value(applied).expect("parse migration status");
    assert_eq!(applied, vec!["applied"]);
}

#[tokio::test]
async fn missing_source_context_index_blocks_readiness_after_migration_ledger_is_applied() {
    let (_registry, db, migrations, tenant) = ready_legacy_fixture().await;
    memory_mcp::provisioning::api::ensure_ready_schema(
        &migrations,
        &tenant.namespace_binding,
        None,
    )
    .await
    .expect("apply the current schema before simulating index loss");
    db.query(
        "REMOVE INDEX episode_source_context ON TABLE episode",
        None,
        &tenant.namespace_binding.namespace,
    )
    .await
    .expect("simulate missing source context index after schema ledger commit");

    let result = memory_mcp::provisioning::api::ensure_ready_schema(
        &migrations,
        &tenant.namespace_binding,
        None,
    )
    .await;

    assert!(
        matches!(&result, Err(memory_mcp::MemoryError::Storage(message)) if message.contains("episode_source_context") && message.contains("missing")),
        "readiness must reject a missing indexed source-context field: {:?}",
        result.as_ref().err()
    );
}

#[tokio::test]
async fn ready_schema_upgrade_preserves_existing_vector_dimension_for_all_supported_widths() {
    let (_registry, db, migrations, tenant) = ready_legacy_fixture().await;
    let expected_vector: Vec<f64> = (1..=17).map(|value| f64::from(value) / 17.0).collect();

    for dimension in [17, 384, 768, 1536, 2048, 3072] {
        memory_mcp::provisioning::api::ensure_ready_schema(
            &migrations,
            &tenant.namespace_binding,
            Some(dimension),
        )
        .await
        .unwrap_or_else(|error| panic!("schema upgrade failed at dimension {dimension}: {error}"));

        let schema = db
            .query(
                "INFO FOR TABLE fact",
                None,
                &tenant.namespace_binding.namespace,
            )
            .await
            .expect("inspect fact schema after dimension-specific upgrade")
            .to_string();
        assert!(
            schema.to_lowercase().contains("dimension 17"),
            "migration at deployment dimension {dimension} must not rebuild the existing index: {schema}"
        );
        let vector = db
            .query(
                "SELECT VALUE embedding FROM fact:ready_schema_probe LIMIT 1",
                None,
                &tenant.namespace_binding.namespace,
            )
            .await
            .expect("read vector after dimension-specific upgrade");
        let vector: Vec<Vec<f64>> = serde_json::from_value(vector).expect("parse stored vector");
        assert_eq!(vector, vec![expected_vector.clone()]);
    }
}
