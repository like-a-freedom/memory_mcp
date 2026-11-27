//! Upgrade evidence: legacy heat is transferred before the namespace is ready.
mod common;

use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::{Value, json};

const MIGRATION: &str = "script_migration:057_fact_access_isolation_surql";

async fn legacy(db: &SurrealDbClient, key: &str, count: i64, stamp: Option<&str>) {
    let id = format!("fact:{key}");
    db.create(&id, json!({"fact_id":id,"fact_type":"note","content":"Unchanged source","quote":"Unchanged source","source_episode":"episode:upgrade","t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","confidence":0.8,"index_keys":[],"entity_links":[],"scope":"org","policy_tags":[],"access_count":count,"last_accessed":stamp,"provenance":{"source":"manual"}}), "org", &["t_valid","t_ingested","last_accessed"]).await.unwrap();
}

async fn reopen_migration(db: &SurrealDbClient) {
    // Only migration bookkeeping in this disposable fixture is removed.
    db.query(&format!("DELETE {MIGRATION}"), None, "org")
        .await
        .unwrap();
}

#[tokio::test]
async fn upgrade_rejects_a_preexisting_unique_recent_access_index() {
    let (_, db) = common::make_service_with_client().await;
    db.query(
        "DEFINE INDEX OVERWRITE fact_access_source_recent ON fact_access FIELDS source_episode, last_accessed UNIQUE",
        None,
        "org",
    )
    .await
    .expect("install incompatible index on empty side table");
    reopen_migration(&db).await;

    let error = db
        .apply_migrations("org")
        .await
        .expect_err("same-episode accesses at the same time must be permitted");

    assert!(
        error.to_string().contains(
            "index `fact_access_source_recent` on table `fact_access` has an incompatible definition"
        ),
        "{error}"
    );
}

#[tokio::test]
async fn upgrade_copies_signed_heat_and_zero_count_timestamp_without_changing_fact() {
    let (_, db) = common::make_service_with_client().await;
    legacy(&db, "negative", -7, Some("2026-10-03T12:00:00Z")).await;
    legacy(&db, "zero_timestamp", 0, Some("2026-10-04T12:00:00Z")).await;
    legacy(&db, "untouched", 0, None).await;
    let before = db.select_one("fact:negative", "org").await.unwrap();
    reopen_migration(&db).await;

    db.apply_migrations("org").await.unwrap();

    let heat = db
        .select_one("fact_access:negative", "org")
        .await
        .unwrap()
        .expect("legacy heat migrated");
    assert_eq!(heat["access_count"], -7);
    assert_eq!(heat["source_episode"], "episode:upgrade");
    assert_eq!(heat["last_accessed"], "2026-10-03T12:00:00Z");
    let zero = db
        .select_one("fact_access:zero_timestamp", "org")
        .await
        .unwrap()
        .expect("timestamp alone must survive");
    assert_eq!(zero["access_count"], 0);
    assert_eq!(zero["last_accessed"], "2026-10-04T12:00:00Z");
    assert_eq!(db.select_one("fact:negative", "org").await.unwrap(), before);
    assert!(
        db.select_one("fact_access:untouched", "org")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn replay_does_not_add_the_legacy_counter_twice() {
    let (_, db) = common::make_service_with_client().await;
    legacy(&db, "replay", 9, None).await;
    reopen_migration(&db).await;
    db.apply_migrations("org").await.unwrap();
    reopen_migration(&db).await;

    db.apply_migrations("org").await.unwrap();

    let heat = db
        .select_one("fact_access:replay", "org")
        .await
        .unwrap()
        .expect("migrated heat");
    assert_eq!(heat["access_count"], 9);
}

#[tokio::test]
async fn conflicting_side_row_aborts_upgrade_without_overwriting_either_counter() {
    let (_, db) = common::make_service_with_client().await;
    legacy(&db, "conflict", 9, None).await;
    db.create(
        "fact_access:conflict",
        json!({"source_episode":"episode:upgrade","access_count":12}),
        "org",
        &[],
    )
    .await
    .unwrap();
    reopen_migration(&db).await;

    let result = db.apply_migrations("org").await;

    assert!(result.is_err(), "conflicting heat must prevent readiness");
    let heat = db
        .select_one("fact_access:conflict", "org")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(heat["access_count"], 12);
    let original = db
        .select_one("fact:conflict", "org")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original["access_count"], 9);
    let state: Value = db.select_one(MIGRATION, "org").await.unwrap().unwrap();
    assert_eq!(state["status"], "failed");
}

#[tokio::test]
async fn failed_second_batch_resumes_from_committed_cursor() {
    let (_, db) = common::make_service_with_client().await;
    for n in 0..129 {
        legacy(
            &db,
            &format!("row{n:03}"),
            if n == 128 { 999 } else { 1 },
            None,
        )
        .await;
    }
    db.query(
        "DEFINE FIELD OVERWRITE access_count ON fact_access TYPE int ASSERT $value != 999",
        None,
        "org",
    )
    .await
    .unwrap();
    reopen_migration(&db).await;

    assert!(db.apply_migrations("org").await.is_err());

    let state = db.select_one(MIGRATION, "org").await.unwrap().unwrap();
    assert_eq!(state["fact_access_cursor"], "row127");
    let before = db
        .query("SELECT access_count FROM fact_access", None, "org")
        .await
        .unwrap();
    assert_eq!(before.as_array().unwrap().len(), 128);
    db.query(
        "DEFINE FIELD OVERWRITE access_count ON fact_access TYPE int",
        None,
        "org",
    )
    .await
    .unwrap();
    db.apply_migrations("org").await.unwrap();
    let first = db
        .select_one("fact_access:row000", "org")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first["access_count"], 1);
    let last = db
        .select_one("fact_access:row128", "org")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(last["access_count"], 999);
    assert_eq!(
        db.select_one(MIGRATION, "org").await.unwrap().unwrap()["status"],
        "applied"
    );
}

#[tokio::test]
async fn resumed_upgrade_rejects_a_changed_already_copied_prefix() {
    let (_, db) = common::make_service_with_client().await;
    for n in 0..129 {
        legacy(
            &db,
            &format!("row{n:03}"),
            if n == 128 { 999 } else { 1 },
            None,
        )
        .await;
    }
    db.query(
        "DEFINE FIELD OVERWRITE access_count ON fact_access TYPE int ASSERT $value != 999",
        None,
        "org",
    )
    .await
    .unwrap();
    reopen_migration(&db).await;
    assert!(db.apply_migrations("org").await.is_err());
    // A fence violation or operator edit must not escape final reconciliation.
    db.query("UPDATE fact:row000 SET access_count = 2; DEFINE FIELD OVERWRITE access_count ON fact_access TYPE int", None, "org").await.unwrap();

    let result = db.apply_migrations("org").await;

    assert!(
        result.is_err(),
        "a copied-prefix mismatch must prevent readiness"
    );
    assert_eq!(
        db.select_one("fact_access:row000", "org")
            .await
            .unwrap()
            .unwrap()["access_count"],
        1
    );
    assert_eq!(
        db.select_one(MIGRATION, "org").await.unwrap().unwrap()["status"],
        "failed"
    );
}

#[tokio::test]
async fn cold_retrieval_rationale_uses_side_heat_instead_of_legacy_counter() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let (service, db) = common::make_service_with_client().await;
    legacy(&db, "rank", 0, None).await;
    db.create(
        "fact_access:rank",
        json!({"source_episode":"episode:upgrade","access_count":9}),
        "org",
        &[],
    )
    .await
    .unwrap();
    let request = memory_mcp::models::AssembleContextRequest {
        query: "Unchanged source".into(),
        as_of: None,
        budget: 5,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };

    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();

    let ranked = items
        .iter()
        .find(|item| item.fact_id == "fact:rank")
        .expect("ranked fact");
    assert!(
        ranked.rationale.contains("access_count=9"),
        "{}",
        ranked.rationale
    );
}

struct HeatReadFault(std::sync::Arc<SurrealDbClient>);

#[async_trait::async_trait]
impl DbClient for HeatReadFault {
    async fn transaction_query(
        &self,
        sql: &str,
        vars: Option<Value>,
        ns: &str,
    ) -> Result<(), memory_mcp::MemoryError> {
        self.0.transaction_query(sql, vars, ns).await
    }
    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        ns: &str,
    ) -> Result<Value, memory_mcp::MemoryError> {
        if sql.contains("type::record('fact_access'") {
            return Err(memory_mcp::MemoryError::Storage(
                "controlled heat read failure".into(),
            ));
        }
        self.0.query(sql, vars, ns).await
    }
    async fn select_one(
        &self,
        id: &str,
        ns: &str,
    ) -> Result<Option<Value>, memory_mcp::MemoryError> {
        self.0.select_one(id, ns).await
    }
    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        ns: &str,
    ) -> Result<Vec<Value>, memory_mcp::MemoryError> {
        self.0.select_table(table, ns).await
    }
    async fn create(
        &self,
        id: &str,
        data: Value,
        ns: &str,
        times: &[&str],
    ) -> Result<Value, memory_mcp::MemoryError> {
        self.0.create(id, data, ns, times).await
    }
    async fn update(
        &self,
        id: &str,
        data: Value,
        ns: &str,
        times: &[&str],
    ) -> Result<Value, memory_mcp::MemoryError> {
        self.0.update(id, data, ns, times).await
    }
    async fn apply_migrations(&self, ns: &str) -> Result<(), memory_mcp::MemoryError> {
        self.0.apply_migrations(ns).await
    }
}

#[tokio::test]
async fn decay_heat_read_failure_leaves_cold_fact_valid() {
    let (_, db) = common::make_service_with_client().await;
    legacy(&db, "heat_read_fault", 0, None).await;
    let service = memory_mcp::service::MemoryService::new(
        std::sync::Arc::new(HeatReadFault(db.clone())),
        "org".into(),
        "error".into(),
        50,
        100,
    )
    .unwrap();

    let outcome = memory_mcp::platform::lifecycle_runtime::decay_pass(&service, 0.9, 1.0).await;

    let error = outcome.expect_err("unread heat must not be treated as zero heat");
    assert!(
        error.to_string().contains("controlled heat read failure"),
        "{error}"
    );
    let fact = db
        .select_one("fact:heat_read_fault", "org")
        .await
        .unwrap()
        .unwrap();
    assert!(fact.get("t_invalid").is_none_or(Value::is_null));
}

#[tokio::test]
async fn retrieval_heat_read_failure_is_not_a_successful_cold_ranking() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let (_, db) = common::make_service_with_client().await;
    legacy(&db, "heat_read_fault", 0, None).await;
    let service = memory_mcp::service::MemoryService::new(
        std::sync::Arc::new(HeatReadFault(db)),
        "org".into(),
        "error".into(),
        50,
        100,
    )
    .unwrap();
    let request = memory_mcp::models::AssembleContextRequest {
        query: "Unchanged source".into(),
        as_of: None,
        budget: 5,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };

    let outcome = AssembleContextCapability::assemble_context_from_service(&service, request).await;

    assert!(
        outcome.is_err(),
        "heat storage fault must not invent zero counters"
    );
}
