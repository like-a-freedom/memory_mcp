//! Transactional outbox: atomically apply a mutation and
//! emit a `TenantChangeEvent`.
//!
//! Every canonical write path (ingest, resolve, invalidate,
//! durable task state changes, durable app session commands)
//! routes through `commit_mutation_with_event`. The helper
//! runs one SurrealDB transaction that increments the
//! tenant-local sequence, applies the mutation, and inserts
//! the event. Any statement error rolls back the entire
//! transaction without emitting an event.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::MemoryError;
use crate::http::fault_injection::{FaultInjector, FaultPoint};
use crate::storage::client::BoundDbClient;

/// A durable change event committed atomically with the
/// mutation that produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantChangeEvent {
    #[serde(rename = "event_seq")]
    pub sequence: u64,
    pub resource_id: String,
    #[serde(rename = "rev")]
    pub revision: u64,
    pub change_kind: String,
    /// Server-set timestamp. On INSERT, the DB uses
    /// `time::now()`; this field is ignored in the
    /// write path but populated on read.
    pub created_at: DateTime<Utc>,
}

/// Internal parameterized mutation used by canonical write owners.
///
/// The SQL statement is assembled by a crate-owned store. Values are carried
/// separately so user-controlled content never has to be interpolated into the
/// transaction script.
#[derive(Debug, Clone)]
pub struct TenantMutation {
    sql: String,
    vars: serde_json::Value,
}

impl TenantMutation {
    pub fn new(sql: impl Into<String>, vars: serde_json::Value) -> Result<Self, MemoryError> {
        if !vars.is_null() && !vars.is_object() {
            return Err(MemoryError::Validation(
                "outbox mutation variables must be a JSON object".to_string(),
            ));
        }
        Ok(Self {
            sql: sql.into(),
            vars,
        })
    }
}

/// Apply a trusted, parameterized mutation and emit a change event atomically.
///
/// The sequence increment, mutation, and event insert execute in one
/// SurrealDB transaction. A failed mutation therefore rolls back both the
/// event and the sequence increment. The event sequence is always assigned by
/// the transaction; `event.sequence` is intentionally ignored.
///
/// The fault injector is consulted AFTER the transaction commits
/// successfully. A transient here leaves the outbox row committed
/// but raises the error so the caller surfaces the failure to the
/// client; the next mutation will use a fresh `event_seq`.
#[cfg_attr(not(any(test, feature = "test-fixtures")), allow(dead_code))]
pub async fn commit_tenant_mutation_with_event(
    db: &BoundDbClient,
    mutation: TenantMutation,
    event: TenantChangeEvent,
    fault_injector: &Arc<dyn FaultInjector>,
) -> Result<(), MemoryError> {
    let mut vars = match mutation.vars {
        serde_json::Value::Null => serde_json::Map::new(),
        serde_json::Value::Object(vars) => vars,
        _ => {
            return Err(MemoryError::Validation(
                "outbox mutation variables must be a JSON object".to_string(),
            ));
        }
    };
    for reserved in ["__outbox_resource_id", "__outbox_revision", "__outbox_kind"] {
        if vars.contains_key(reserved) {
            return Err(MemoryError::Validation(format!(
                "outbox mutation variables reserve {reserved}"
            )));
        }
    }
    vars.insert(
        "__outbox_resource_id".to_string(),
        serde_json::Value::String(event.resource_id),
    );
    vars.insert(
        "__outbox_revision".to_string(),
        serde_json::json!(event.revision),
    );
    vars.insert(
        "__outbox_kind".to_string(),
        serde_json::Value::String(event.change_kind),
    );

    let script = format!(
        "BEGIN TRANSACTION; UPDATE tenant_change_sequence:default SET value = value + 1; LET $outbox_event_seq = (SELECT VALUE value FROM tenant_change_sequence:default LIMIT 1)[0]; {}; CREATE tenant_change_event SET event_seq = $outbox_event_seq, resource_id = $__outbox_resource_id, rev = $__outbox_revision, change_kind = $__outbox_kind, created_at = time::now(); COMMIT TRANSACTION;",
        mutation.sql
    );
    let vars = serde_json::Value::Object(vars);
    for attempt in 0..5u64 {
        match db.query(&script, Some(vars.clone())).await {
            Ok(_) => {
                // Hit AFTER the transaction is durable. The event
                // row and sequence increment are committed; the
                // caller observes the transient and the row stays.
                fault_injector.hit(FaultPoint::OutboxMutationCommitted)?;
                return Ok(());
            }
            Err(error) if is_retryable_transaction_conflict(&error) && attempt < 4 => {
                tokio::time::sleep(std::time::Duration::from_millis(10 * (attempt + 1))).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(MemoryError::Unavailable(
        "outbox transaction retry budget exhausted".into(),
    ))
}

fn is_retryable_transaction_conflict(error: &MemoryError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("transaction conflict") || message.contains("write conflict")
}

/// Compatibility wrapper for internal tests and already-validated statements
/// that do not need additional bindings. New production write owners should
/// use [`TenantMutation`] and `commit_tenant_mutation_with_event`.
pub async fn commit_mutation_with_event(
    db: &BoundDbClient,
    mutation_sql: &str,
    event: TenantChangeEvent,
    fault_injector: &Arc<dyn FaultInjector>,
) -> Result<(), MemoryError> {
    let mutation = TenantMutation::new(mutation_sql, serde_json::Value::Null)?;
    commit_tenant_mutation_with_event(db, mutation, event, fault_injector).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_faults() -> std::sync::Arc<dyn crate::http::fault_injection::FaultInjector> {
        std::sync::Arc::new(crate::http::fault_injection::NoFaults)
    }

    async fn fresh_db() -> BoundDbClient {
        use crate::storage::client::{DbClient, SurrealDbClient};

        let raw = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        raw.use_ns("outbox_test")
            .use_db("outbox_test")
            .await
            .expect("use ns/db");
        let client =
            std::sync::Arc::new(SurrealDbClient::from_prebound(raw, "outbox_test", "warn"));
        // Inline schema (same as 042_tenant_change_event.surql).
        client
            .query(
                "DEFINE TABLE tenant_change_sequence SCHEMAFULL; \
                 DEFINE FIELD value ON tenant_change_sequence TYPE int DEFAULT 0; \
                 UPSERT tenant_change_sequence:default SET value = 0; \
                 DEFINE TABLE tenant_change_event SCHEMAFULL; \
                 DEFINE FIELD event_seq ON tenant_change_event TYPE int; \
                 DEFINE FIELD resource_id ON tenant_change_event TYPE string; \
                 DEFINE FIELD rev ON tenant_change_event TYPE int; \
                 DEFINE FIELD change_kind ON tenant_change_event TYPE string; \
                 DEFINE FIELD created_at ON tenant_change_event TYPE datetime; \
                 DEFINE INDEX idx_event_seq ON tenant_change_event FIELDS event_seq; \
                 DEFINE INDEX idx_event_seq_unique ON tenant_change_event FIELDS event_seq UNIQUE; \
                 DEFINE TABLE tenant_outbox_test SCHEMAFULL; \
                 DEFINE FIELD name ON tenant_outbox_test TYPE string;",
                None,
                "outbox_test",
            )
            .await
            .expect("define schema");
        BoundDbClient::new(client, "outbox_test")
    }

    #[tokio::test]
    async fn mutation_and_event_commit_atomically() {
        let db = fresh_db().await;
        let event = TenantChangeEvent {
            sequence: 0, // will be overwritten
            resource_id: "fact_1".into(),
            revision: 1,
            change_kind: "ingest".into(),
            created_at: Utc::now(),
        };
        // Simple mutation: create a placeholder record.
        let mutation = "CREATE tenant_outbox_test SET name = 'test';";
        commit_mutation_with_event(&db, mutation, event, &no_faults())
            .await
            .expect("commit");

        // Verify the event was emitted.
        let result = db
            .query(
                "SELECT * FROM tenant_change_event ORDER BY event_seq LIMIT 1;",
                None,
            )
            .await
            .expect("query events");
        let rows: Vec<serde_json::Value> = serde_json::from_value(result).expect("parse events");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["resource_id"], "fact_1");
        assert_eq!(rows[0]["event_seq"], 1);

        // Verify the sequence was incremented.
        let seq = db
            .query(
                "SELECT VALUE value FROM tenant_change_sequence:default;",
                None,
            )
            .await
            .expect("query seq");
        let seq_val: Vec<serde_json::Value> = serde_json::from_value(seq).expect("parse seq");
        assert_eq!(seq_val[0], 1);
    }

    #[tokio::test]
    async fn rolled_back_mutation_does_not_emit_event() {
        let db = fresh_db().await;
        let event = TenantChangeEvent {
            sequence: 0,
            resource_id: "fact_2".into(),
            revision: 1,
            change_kind: "ingest".into(),
            created_at: Utc::now(),
        };
        // Intentionally invalid SQL to trigger a rollback.
        let mutation = "INVALID SQL STATEMENT;";
        let result = commit_mutation_with_event(&db, mutation, event, &no_faults()).await;
        assert!(result.is_err());

        // No event should have been emitted.
        let rows = db
            .query("SELECT * FROM tenant_change_event;", None)
            .await
            .expect("query events");
        let rows: Vec<serde_json::Value> = serde_json::from_value(rows).expect("parse events");
        assert!(rows.is_empty());
    }
}
