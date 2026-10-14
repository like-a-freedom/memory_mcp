#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

use std::sync::Arc;

use memory_mcp::http::leases::migration::{ApplyMigrations, SurrealTenantMigrations};
use memory_mcp::http::registry::RegistryHandle;
use memory_mcp::http::registry::SurrealRegistryStore;
use memory_mcp::http::subscriptions::SubscriptionTestDriver;
use memory_mcp::platform::fault_injection::{FaultInjector, NoFaults};
use memory_mcp::platform::persistence::outbox::{
    TenantChangeEvent, TenantMutation, commit_tenant_mutation_with_event, ensure_tenant_sequence,
};
use memory_mcp::storage::{BoundDbClient, DbClient};

async fn outbox_fixture() -> (Arc<dyn DbClient>, Arc<BoundDbClient>, RegistryHandle) {
    let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
    let raw = registry
        .tenant_engine_optional()
        .expect("test engine is configured")
        .bind_to_test_namespace("ns_outbox_upgrade")
        .await;
    let db: Arc<dyn DbClient> = raw;
    let bound = Arc::new(BoundDbClient::new(Arc::clone(&db), "ns_outbox_upgrade"));
    let driver = SubscriptionTestDriver::new(Arc::clone(&bound), "tenant_outbox".to_owned());
    driver
        .apply_schema_for_test("ns_outbox_upgrade")
        .await
        .expect("outbox schema applies");
    db.query(
        "DEFINE TABLE outbox_probe SCHEMAFULL; DEFINE FIELD value ON outbox_probe TYPE string;",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("mutation fixture schema applies");
    (db, bound, registry)
}

fn event() -> TenantChangeEvent {
    TenantChangeEvent {
        sequence: 0,
        resource_id: "fact:outbox_probe".to_owned(),
        revision: 1,
        change_kind: "updated".to_owned(),
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn commit_recovers_missing_sequence_from_durable_event_history() {
    let (db, bound, _registry) = outbox_fixture().await;
    let driver = SubscriptionTestDriver::new(Arc::clone(&bound), "tenant_outbox".to_owned());
    driver
        .append_event_for_test(&TenantChangeEvent {
            sequence: 1341,
            ..event()
        })
        .await
        .expect("seed durable event history");
    db.query(
        "DELETE tenant_change_sequence:default",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("remove sequence record to reproduce recovery");

    let mutation = TenantMutation::new(
        "CREATE outbox_probe SET value = 'committed';",
        serde_json::Value::Null,
    )
    .expect("mutation is valid");
    let faults: Arc<dyn FaultInjector> = Arc::new(NoFaults);
    let result = commit_tenant_mutation_with_event(&bound, mutation, event(), &faults).await;

    assert!(
        result.is_ok(),
        "missing sequence row should recover: {result:?}"
    );
    let rows = db
        .query(
            "SELECT event_seq FROM tenant_change_event ORDER BY event_seq ASC",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read durable event history");
    let rows: Vec<serde_json::Value> = serde_json::from_value(rows).expect("parse events");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["event_seq"], 1341);
    assert_eq!(rows[1]["event_seq"], 1342);
}

#[tokio::test]
async fn commit_creates_the_fixed_sequence_record_when_history_is_empty() {
    let (db, bound, _registry) = outbox_fixture().await;
    db.query(
        "DELETE tenant_change_sequence:default",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("remove sequence record to reproduce empty initialization");
    let mutation = TenantMutation::new(
        "CREATE outbox_probe SET value = 'committed';",
        serde_json::Value::Null,
    )
    .expect("mutation is valid");
    let faults: Arc<dyn FaultInjector> = Arc::new(NoFaults);

    let result = commit_tenant_mutation_with_event(&bound, mutation, event(), &faults).await;

    assert!(
        result.is_ok(),
        "empty sequence should initialize: {result:?}"
    );
    let sequence = db
        .query(
            "SELECT * FROM tenant_change_sequence",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read sequence records");
    let rows: Vec<serde_json::Value> = serde_json::from_value(sequence).expect("parse sequence");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"]["RecordId"]["table"], "tenant_change_sequence");
    assert_eq!(rows[0]["id"]["RecordId"]["key"], "default");
    assert_eq!(rows[0]["value"], 1);
}

#[tokio::test]
async fn maintenance_repairs_missing_sequence_from_durable_event_history() {
    let (db, bound, _registry) = outbox_fixture().await;
    let driver = SubscriptionTestDriver::new(Arc::clone(&bound), "tenant_outbox".to_owned());
    driver
        .append_event_for_test(&TenantChangeEvent {
            sequence: 1341,
            ..event()
        })
        .await
        .expect("seed durable event history");
    db.query(
        "DELETE tenant_change_sequence:default",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("remove sequence record to reproduce recovery");

    ensure_tenant_sequence(&bound)
        .await
        .expect("maintenance should restore the durable high-water mark");

    let sequence = db
        .query(
            "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read repaired sequence");
    let values: Vec<u64> = serde_json::from_value(sequence).expect("parse sequence");
    assert_eq!(values, vec![1341]);
}

#[tokio::test]
async fn maintenance_repairs_a_lagging_sequence_from_durable_event_history() {
    let (db, bound, _registry) = outbox_fixture().await;
    let driver = SubscriptionTestDriver::new(Arc::clone(&bound), "tenant_outbox".to_owned());
    driver
        .append_event_for_test(&TenantChangeEvent {
            sequence: 1341,
            ..event()
        })
        .await
        .expect("seed durable event history");
    db.query(
        "UPSERT tenant_change_sequence:default SET value = 10",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("reproduce sequence behind durable history");

    ensure_tenant_sequence(&bound)
        .await
        .expect("maintenance should advance a lagging sequence");

    let sequence = db
        .query(
            "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read repaired sequence");
    let values: Vec<u64> = serde_json::from_value(sequence).expect("parse sequence");
    assert_eq!(values, vec![1341]);
}

#[tokio::test]
async fn maintenance_preserves_a_sequence_ahead_of_durable_event_history() {
    let (db, bound, _registry) = outbox_fixture().await;
    let driver = SubscriptionTestDriver::new(Arc::clone(&bound), "tenant_outbox".to_owned());
    driver
        .append_event_for_test(&TenantChangeEvent {
            sequence: 17,
            ..event()
        })
        .await
        .expect("seed durable event history");
    db.query(
        "UPSERT tenant_change_sequence:default SET value = 41",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("advance sequence beyond current history");

    ensure_tenant_sequence(&bound)
        .await
        .expect("maintenance should preserve the high-water mark");

    let sequence = db
        .query(
            "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read repaired sequence");
    let values: Vec<u64> = serde_json::from_value(sequence).expect("parse sequence");
    assert_eq!(values, vec![41]);
}

#[tokio::test]
async fn concurrent_repairs_create_one_fixed_sequence_record() {
    let (db, bound, _registry) = outbox_fixture().await;
    db.query(
        "DELETE tenant_change_sequence:default",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("remove sequence record to race repair");

    let (first, second) = tokio::join!(
        ensure_tenant_sequence(&bound),
        ensure_tenant_sequence(&bound),
    );

    assert!(first.is_ok(), "first repair should succeed: {first:?}");
    assert!(second.is_ok(), "second repair should succeed: {second:?}");
    let sequence = db
        .query(
            "SELECT * FROM tenant_change_sequence",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read sequence records");
    let rows: Vec<serde_json::Value> = serde_json::from_value(sequence).expect("parse sequence");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"]["RecordId"]["table"], "tenant_change_sequence");
    assert_eq!(rows[0]["id"]["RecordId"]["key"], "default");
    assert_eq!(rows[0]["value"], 0);
}

#[tokio::test]
async fn racing_repair_and_writer_never_reuse_an_event_sequence() {
    let (db, bound, _registry) = outbox_fixture().await;
    let driver = SubscriptionTestDriver::new(Arc::clone(&bound), "tenant_outbox".to_owned());
    driver
        .append_event_for_test(&TenantChangeEvent {
            sequence: 1341,
            ..event()
        })
        .await
        .expect("seed durable event history");
    db.query(
        "DELETE tenant_change_sequence:default",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("remove sequence record to race repair with writer");
    let mutation = TenantMutation::new(
        "UPSERT outbox_probe:writer SET value = 'committed'",
        serde_json::Value::Null,
    )
    .expect("mutation is valid");
    let faults: Arc<dyn FaultInjector> = Arc::new(NoFaults);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let repair = async {
        barrier.wait().await;
        ensure_tenant_sequence(&bound).await
    };
    let writer = async {
        barrier.wait().await;
        commit_tenant_mutation_with_event(&bound, mutation, event(), &faults).await
    };
    let release = async { barrier.wait().await };

    let (repair, writer, _) = tokio::join!(repair, writer, release);

    assert!(repair.is_ok(), "repair should finish: {repair:?}");
    assert!(writer.is_ok(), "writer should finish: {writer:?}");
    let events = db
        .query(
            "SELECT event_seq FROM tenant_change_event ORDER BY event_seq ASC",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read durable event history");
    let rows: Vec<serde_json::Value> = serde_json::from_value(events).expect("parse events");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["event_seq"], 1341);
    assert_eq!(rows[1]["event_seq"], 1342);
}

#[tokio::test]
async fn failed_mutation_keeps_sequence_and_events_unchanged() {
    let (db, bound, _registry) = outbox_fixture().await;
    let driver = SubscriptionTestDriver::new(Arc::clone(&bound), "tenant_outbox".to_owned());
    driver
        .append_event_for_test(&TenantChangeEvent {
            sequence: 9,
            ..event()
        })
        .await
        .expect("seed durable event history");
    db.query(
        "UPSERT tenant_change_sequence:default SET value = 9",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("align sequence with durable history");
    let mutation = TenantMutation::new(
        "CREATE outbox_probe SET value = 'bad', extra = true",
        serde_json::Value::Null,
    )
    .expect("mutation input is syntactically valid");
    let faults: Arc<dyn FaultInjector> = Arc::new(NoFaults);

    let result = commit_tenant_mutation_with_event(&bound, mutation, event(), &faults).await;

    assert!(result.is_err(), "schema-invalid mutation must fail");
    let sequence = db
        .query(
            "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read sequence after rollback");
    let values: Vec<u64> = serde_json::from_value(sequence).expect("parse sequence");
    assert_eq!(values, vec![9]);
    let events = db
        .query(
            "SELECT event_seq FROM tenant_change_event",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read events after rollback");
    let rows: Vec<serde_json::Value> = serde_json::from_value(events).expect("parse events");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["event_seq"], 9);
}

#[tokio::test]
async fn sequence_repair_reports_a_missing_event_table() {
    let (db, bound, _registry) = outbox_fixture().await;
    db.query(
        "REMOVE TABLE tenant_change_event",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("remove event table to reproduce unavailable history");

    let result = ensure_tenant_sequence(&bound).await;

    assert!(result.is_err(), "repair must report unavailable history");
}

#[tokio::test]
async fn replaying_http_migrations_preserves_sequence_and_event_history() {
    let control = SurrealRegistryStore::connect_in_memory("control_outbox", "registry")
        .await
        .expect("registry engine is available");
    let engine = control.privileged_engine();
    let migrations = SurrealTenantMigrations::new(engine.clone());
    migrations
        .ensure_namespace("ns_outbox_replay", "memory")
        .await
        .expect("tenant namespace is created");
    migrations
        .apply_migrations("ns_outbox_replay", "memory")
        .await
        .expect("initial base and HTTP migrations apply");

    let client = engine.bind_to_test_namespace("ns_outbox_replay").await;
    let raw: Arc<dyn DbClient> = client;
    let bound = Arc::new(BoundDbClient::new(Arc::clone(&raw), "ns_outbox_replay"));
    let driver = SubscriptionTestDriver::new(Arc::clone(&bound), "tenant_replay".to_owned());
    driver
        .append_event_for_test(&TenantChangeEvent {
            sequence: 1341,
            ..event()
        })
        .await
        .expect("seed durable event history");
    raw.query(
        "UPSERT tenant_change_sequence:default SET value = 1341",
        None,
        "ns_outbox_replay",
    )
    .await
    .expect("align sequence with durable history");

    migrations
        .apply_migrations("ns_outbox_replay", "memory")
        .await
        .expect("replayed HTTP migrations complete");

    let sequence = raw
        .query(
            "SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1",
            None,
            "ns_outbox_replay",
        )
        .await
        .expect("read sequence after migration replay");
    let values: Vec<u64> = serde_json::from_value(sequence).expect("parse sequence");
    assert_eq!(values, vec![1341]);

    let mutation = TenantMutation::new(
        "UPDATE tenant_change_sequence:default SET value = value",
        serde_json::Value::Null,
    )
    .expect("mutation is valid");
    let faults: Arc<dyn FaultInjector> = Arc::new(NoFaults);
    commit_tenant_mutation_with_event(&bound, mutation, event(), &faults)
        .await
        .expect("next mutation uses the preserved high-water mark");
    let events = raw
        .query(
            "SELECT event_seq FROM tenant_change_event ORDER BY event_seq ASC",
            None,
            "ns_outbox_replay",
        )
        .await
        .expect("read event history after next mutation");
    let rows: Vec<serde_json::Value> = serde_json::from_value(events).expect("parse events");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["event_seq"], 1341);
    assert_eq!(rows[1]["event_seq"], 1342);
}

#[tokio::test]
async fn concurrent_first_commits_allocate_distinct_sequences() {
    let (db, bound, _registry) = outbox_fixture().await;
    db.query(
        "DELETE tenant_change_sequence:default",
        None,
        "ns_outbox_upgrade",
    )
    .await
    .expect("remove sequence record to race initialization");
    let first = TenantMutation::new(
        "CREATE outbox_probe SET value = 'first';",
        serde_json::Value::Null,
    )
    .expect("first mutation is valid");
    let second = TenantMutation::new(
        "CREATE outbox_probe SET value = 'second';",
        serde_json::Value::Null,
    )
    .expect("second mutation is valid");
    let faults: Arc<dyn FaultInjector> = Arc::new(NoFaults);

    let (first, second) = tokio::join!(
        commit_tenant_mutation_with_event(&bound, first, event(), &faults),
        commit_tenant_mutation_with_event(&bound, second, event(), &faults),
    );

    assert!(first.is_ok(), "first commit should recover: {first:?}");
    assert!(second.is_ok(), "second commit should recover: {second:?}");
    let events = db
        .query(
            "SELECT event_seq FROM tenant_change_event ORDER BY event_seq ASC",
            None,
            "ns_outbox_upgrade",
        )
        .await
        .expect("read committed events");
    let rows: Vec<serde_json::Value> = serde_json::from_value(events).expect("parse events");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["event_seq"], 1);
    assert_eq!(rows[1]["event_seq"], 2);
}
