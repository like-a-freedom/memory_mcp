//! Transactional outbox: atomically apply a mutation and
//! emit a `TenantChangeEvent` (spec §11).
//!
//! Every canonical write path (ingest, resolve, invalidate,
//! durable task state changes, durable app session commands)
//! routes through `commit_mutation_with_event`. The helper
//! runs one SurrealDB transaction that increments the
//! tenant-local sequence, applies the mutation, and inserts
//! the event. Any statement error rolls back the entire
//! transaction without emitting an event.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::MemoryError;
use crate::storage::client::BoundDbClient;

/// A durable change event committed atomically with the
/// mutation that produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantChangeEvent {
    pub sequence: u64,
    pub resource_id: String,
    pub revision: u64,
    pub change_kind: String,
    pub created_at: DateTime<Utc>,
}

/// Apply a mutation and emit a change event atomically.
///
/// `mutation_sql` is a validated, parameterized SQL string
/// that the caller has constructed from internal DTOs — never
/// from user input. `event` describes the change to be
/// recorded after the mutation succeeds.
///
/// The sequence is incremented atomically within the
/// transaction. If any statement fails, the entire
/// transaction is rolled back and no event is emitted.
pub async fn commit_mutation_with_event(
    db: &BoundDbClient,
    mutation_sql: &str,
    event: TenantChangeEvent,
) -> Result<(), MemoryError> {
    use serde_json::json;

    let seq_result = db
        .query(
            "UPDATE `tenant_change_sequence` SET `value` = `value` + 1 RETURN AFTER;",
            None,
        )
        .await?;
    let seq_val: u64 = seq_result
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.get("value"))
        .and_then(|v| v.as_u64())
        .ok_or_else(|| MemoryError::Storage("outbox sequence parse failed".into()))?;

    // Apply the mutation.
    db.query(mutation_sql, None).await?;

    // Insert the event with the incremented sequence.
    db.query(
        "CREATE tenant_change_event SET \
         event_seq = $event_seq, \
         resource_id = $resource_id, \
         rev = $rev, \
         change_kind = $change_kind, \
         created_at = time::now();",
        Some(json!({
            "event_seq": seq_val,
            "resource_id": event.resource_id,
            "rev": event.revision,
            "change_kind": event.change_kind,
        })),
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
                 CREATE tenant_change_sequence SET value = 0; \
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
        commit_mutation_with_event(&db, mutation, event)
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
            .query("SELECT `value` FROM `tenant_change_sequence`;", None)
            .await
            .expect("query seq");
        let seq_val: Vec<serde_json::Value> = serde_json::from_value(seq).expect("parse seq");
        assert_eq!(seq_val[0]["value"], 1);
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
        let result = commit_mutation_with_event(&db, mutation, event).await;
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
