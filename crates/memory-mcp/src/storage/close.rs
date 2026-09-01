//! One owner for the bi-temporal close protocol.
//!
//! Bi-temporal close — setting the valid-time end (`t_invalid`) together with
//! the transaction-time end (`t_invalid_ingested`) — is the single operation
//! that removes a record from active truth while preserving audit. This module
//! is the only place that composes close SQL or close field sets; service and
//! capability code expresses intent ("close this fact", "retract this fact and
//! its claims", "supersede this edge") and never spells the close itself.
//!
//! The close operation:
//! - Defaults timestamps to server-side `time::now()` so SurrealDB stores a
//!   native datetime, not a string that must survive `option<datetime>`
//!   coercion.
//! - Accepts optional caller-supplied timestamps (edge supersession closes the
//!   old edge with the new edge's `t_valid`/`t_ingested`).
//! - Always closes both fields of the bi-temporal pair together.
//! - Persists the close reason where the table carries `invalidation_reason`
//!   (fact, migration 029).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Caller-supplied close timestamps.
///
/// A `None` field defaults to server-side `time::now()`. Supply both fields to
/// record a specific close instant (edge supersession uses the superseding
/// edge's `t_valid`/`t_ingested`).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CloseTimestamps {
    pub(crate) t_invalid: Option<DateTime<Utc>>,
    pub(crate) t_invalid_ingested: Option<DateTime<Utc>>,
}

impl CloseTimestamps {
    /// Close both fields with server-side `time::now()`.
    pub(crate) fn now() -> Self {
        Self::default()
    }

    /// Close both fields with distinct caller-supplied instants (edge
    /// supersession: the old edge is closed with the new edge's `t_valid` and
    /// `t_ingested`). Pass the same instant twice to close both at once.
    pub(crate) fn at_pair(t_invalid: DateTime<Utc>, t_invalid_ingested: DateTime<Utc>) -> Self {
        Self {
            t_invalid: Some(t_invalid),
            t_invalid_ingested: Some(t_invalid_ingested),
        }
    }
}

fn push_close_assignment(
    assignments: &mut Vec<String>,
    vars: &mut Map<String, Value>,
    field: &str,
    timestamp: Option<DateTime<Utc>>,
) {
    match timestamp {
        Some(dt) => {
            vars.insert(
                field.to_string(),
                Value::String(crate::service::normalize_dt(dt)),
            );
            assignments.push(format!("{field} = type::datetime(${field})"));
        }
        None => {
            assignments.push(format!("{field} = time::now()"));
        }
    }
}

/// Builds the close `UPDATE` for a bi-temporal record (fact/edge/triple).
///
/// Always closes both `t_invalid` and `t_invalid_ingested`. When `reason` is
/// provided it is persisted to `invalidation_reason` (fact only; the field is
/// absent on edge/triple, so callers closing those pass `None`).
///
/// The record ID is validated and inlined with `⟨⟩` bracket escaping — the
/// same pattern as [`crate::storage::queries::build_update_query`].
pub(crate) fn build_close_query(
    record_id: &str,
    timestamps: &CloseTimestamps,
    reason: Option<&str>,
) -> Result<(String, Value), MemoryError> {
    let record_id = record_id.trim();
    crate::storage::queries::validate_record_id(record_id)?;
    let (table, key) = record_id.split_once(':').ok_or_else(|| {
        MemoryError::Storage(format!(
            "Invalid record_id format: expected 'table:id', got '{record_id}'"
        ))
    })?;

    let mut assignments = Vec::with_capacity(3);
    let mut vars = Map::new();

    push_close_assignment(
        &mut assignments,
        &mut vars,
        "t_invalid",
        timestamps.t_invalid,
    );
    push_close_assignment(
        &mut assignments,
        &mut vars,
        "t_invalid_ingested",
        timestamps.t_invalid_ingested,
    );
    if let Some(reason) = reason {
        vars.insert("reason".to_string(), Value::String(reason.to_string()));
        assignments.push("invalidation_reason = $reason".to_string());
    }

    let sql = format!(
        "UPDATE {table}:⟨{key}⟩ SET {} RETURN NONE",
        assignments.join(", ")
    );
    Ok((sql, Value::Object(vars)))
}

/// Narrow store that owns every bi-temporal close in the Active Namespace.
#[derive(Clone)]
pub(crate) struct CloseStoreClient {
    db: BoundDbClient,
    #[cfg(feature = "streamable-http")]
    outbox_enabled: bool,
}

impl CloseStoreClient {
    pub(crate) fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
            #[cfg(feature = "streamable-http")]
            outbox_enabled: false,
        }
    }

    pub(crate) fn from_bound(db: BoundDbClient) -> Self {
        Self {
            db,
            #[cfg(feature = "streamable-http")]
            outbox_enabled: false,
        }
    }

    #[cfg(feature = "streamable-http")]
    pub(crate) fn with_outbox(mut self) -> Self {
        self.outbox_enabled = true;
        self
    }

    /// Closes a bi-temporal record (fact/edge/triple): sets both `t_invalid`
    /// and `t_invalid_ingested`, plus `invalidation_reason` when `reason` is
    /// provided.
    pub(crate) async fn close_record(
        &self,
        record_id: &str,
        timestamps: &CloseTimestamps,
        reason: Option<&str>,
    ) -> Result<(), MemoryError> {
        let (sql, vars) = build_close_query(record_id, timestamps, reason)?;
        #[cfg(feature = "streamable-http")]
        if self.outbox_enabled {
            let close_sql = sql.replace("RETURN NONE", "RETURN BEFORE");
            let mutation_sql = format!(
                "LET $closed = ({close_sql}); IF array::len($closed) = 0 {{ THROW 'record to invalidate was not found'; }}"
            );
            let mutation =
                crate::http::subscriptions::outbox::TenantMutation::new(mutation_sql, vars)?;
            return crate::http::subscriptions::outbox::commit_tenant_mutation_with_event(
                &self.db,
                mutation,
                crate::http::subscriptions::outbox::TenantChangeEvent {
                    sequence: 0,
                    resource_id: "ui://memory/apps/inspector".into(),
                    revision: 1,
                    change_kind: "record_invalidated".into(),
                    created_at: chrono::Utc::now(),
                },
            )
            .await;
        }
        self.db.query(&sql, Some(vars)).await?;
        Ok(())
    }

    /// Closes the claim and claim_relation rows derived from a fact.
    ///
    /// These are transaction-time-only tables: only `t_invalid_ingested` is
    /// set, guarded so an already-closed row is never re-closed.
    pub(crate) async fn close_claims_for_fact(&self, fact_id: &str) -> Result<(), MemoryError> {
        let claim_sql = "UPDATE claim SET t_invalid_ingested = time::now() \
            WHERE source_fact_id = $fact_id \
            AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)";
        let vars = json!({ "fact_id": fact_id });
        self.db.query(claim_sql, Some(vars.clone())).await?;

        let relation_sql = "UPDATE claim_relation SET t_invalid_ingested = time::now() \
            WHERE (left_fact_id = $fact_id OR right_fact_id = $fact_id) \
            AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)";
        self.db.query(relation_sql, Some(vars)).await?;
        Ok(())
    }

    /// Retracts a fact and closes its derived claims in one intent.
    ///
    /// Closes the fact with both bi-temporal fields and the given reason, then
    /// closes the claim and claim_relation rows derived from it.
    pub(crate) async fn retract_fact_and_claims(
        &self,
        fact_id: &str,
        reason: &str,
    ) -> Result<(), MemoryError> {
        self.close_record(fact_id, &CloseTimestamps::now(), Some(reason))
            .await?;
        self.close_claims_for_fact(fact_id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_query_defaults_both_fields_to_server_now() {
        let (sql, vars) =
            build_close_query("fact:abc", &CloseTimestamps::now(), None).expect("valid record id");
        assert!(sql.contains("t_invalid = time::now()"), "sql: {sql}");
        assert!(
            sql.contains("t_invalid_ingested = time::now()"),
            "sql: {sql}"
        );
        assert!(!sql.contains("invalidation_reason"), "sql: {sql}");
        assert!(sql.starts_with("UPDATE fact:⟨abc⟩ SET "), "sql: {sql}");
        assert_eq!(vars, json!({}), "server-side now() needs no bind variables");
    }

    #[test]
    fn close_query_binds_caller_supplied_timestamps() {
        let instant = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
            .expect("valid datetime")
            .with_timezone(&Utc);
        let timestamps = CloseTimestamps::at_pair(instant, instant);
        let (sql, vars) =
            build_close_query("edge:xyz", &timestamps, None).expect("valid record id");
        assert!(
            sql.contains("t_invalid = type::datetime($t_invalid)"),
            "sql: {sql}"
        );
        assert!(
            sql.contains("t_invalid_ingested = type::datetime($t_invalid_ingested)"),
            "sql: {sql}"
        );
        assert_eq!(vars["t_invalid"], json!("2026-01-02T03:04:05+00:00"));
        assert_eq!(
            vars["t_invalid_ingested"],
            json!("2026-01-02T03:04:05+00:00")
        );
    }

    #[test]
    fn close_query_supports_mixed_default_and_supplied_timestamps() {
        let instant = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
            .expect("valid datetime")
            .with_timezone(&Utc);
        let timestamps = CloseTimestamps {
            t_invalid: Some(instant),
            t_invalid_ingested: None,
        };
        let (sql, vars) =
            build_close_query("fact:abc", &timestamps, None).expect("valid record id");
        assert!(
            sql.contains("t_invalid = type::datetime($t_invalid)"),
            "sql: {sql}"
        );
        assert!(
            sql.contains("t_invalid_ingested = time::now()"),
            "sql: {sql}"
        );
        assert_eq!(vars["t_invalid"], json!("2026-01-02T03:04:05+00:00"));
        assert!(vars.get("t_invalid_ingested").is_none());
    }

    #[test]
    fn close_query_persists_reason_when_provided() {
        let (sql, vars) = build_close_query(
            "fact:abc",
            &CloseTimestamps::now(),
            Some("manual_invalidation"),
        )
        .expect("valid record id");
        assert!(sql.contains("invalidation_reason = $reason"), "sql: {sql}");
        assert_eq!(vars["reason"], json!("manual_invalidation"));
    }

    #[test]
    fn close_query_rejects_malformed_record_id() {
        let result = build_close_query("no-colon", &CloseTimestamps::now(), None);
        assert!(result.is_err());
    }

    async fn embedded_close_store() -> (CloseStoreClient, Arc<crate::storage::SurrealDbClient>) {
        let db_name = format!(
            "close_store_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        );
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                &db_name,
                &["org".to_string()],
                "error",
            )
            .await
            .expect("connect in memory db"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");
        #[cfg(feature = "streamable-http")]
        db_client
            .execute_migration_script(
                include_str!("../../migrations/042_tenant_change_event.surql"),
                "org",
            )
            .await
            .expect("apply HTTP outbox migration");
        let store = CloseStoreClient::new(db_client.clone(), "org");
        (store, db_client)
    }

    async fn seed_fact(db_client: &Arc<crate::storage::SurrealDbClient>, fact_id: &str) {
        let now = crate::service::normalize_dt(chrono::Utc::now());
        db_client
            .create(
                fact_id,
                json!({
                    "fact_id": fact_id,
                    "fact_type": "note",
                    "content": format!("content {fact_id}"),
                    "quote": format!("content {fact_id}"),
                    "source_episode": "episode:seed",
                    "t_valid": now,
                    "t_ingested": now,
                    "confidence": 0.9,
                    "entity_links": [],
                    "scope": "org",
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:seed"},
                }),
                "org",
            )
            .await
            .expect("seed fact should succeed");
    }

    #[cfg(feature = "streamable-http")]
    #[tokio::test]
    async fn outbox_enabled_close_emits_invalidation_atomically() {
        let (store, db_client) = embedded_close_store().await;
        seed_fact(&db_client, "fact:outbox_close").await;
        store
            .with_outbox()
            .close_record(
                "fact:outbox_close",
                &CloseTimestamps::now(),
                Some("test_invalidation"),
            )
            .await
            .expect("close with outbox");
        let events = db_client
            .query(
                "SELECT * FROM tenant_change_event WHERE change_kind = $kind",
                Some(json!({"kind": "record_invalidated"})),
                "org",
            )
            .await
            .expect("select invalidation event");
        let events: Vec<Value> = serde_json::from_value(events).expect("parse events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["resource_id"], "ui://memory/apps/inspector");
    }

    #[tokio::test]
    async fn retract_fact_and_claims_closes_both_bitemporal_fields() {
        let (store, db_client) = embedded_close_store().await;
        seed_fact(&db_client, "fact:retract-1").await;

        store
            .retract_fact_and_claims("fact:retract-1", "source_retraction")
            .await
            .expect("retract should succeed");

        let stored = db_client
            .select_one("fact:retract-1", "org")
            .await
            .expect("select fact")
            .expect("fact must exist");

        assert!(
            stored.get("t_invalid").is_some_and(|v| !v.is_null()),
            "t_invalid must be closed: {stored}"
        );
        assert!(
            stored
                .get("t_invalid_ingested")
                .is_some_and(|v| !v.is_null()),
            "t_invalid_ingested must be closed whenever t_invalid is: {stored}"
        );
        assert_eq!(
            stored.get("invalidation_reason").and_then(|v| v.as_str()),
            Some("source_retraction"),
            "close reason must be persisted: {stored}"
        );
    }

    #[tokio::test]
    async fn close_record_with_caller_timestamps_records_supersession_times() {
        let (store, db_client) = embedded_close_store().await;
        seed_fact(&db_client, "fact:supersede-1").await;

        let instant = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
            .expect("valid datetime")
            .with_timezone(&Utc);
        store
            .close_record(
                "fact:supersede-1",
                &CloseTimestamps::at_pair(instant, instant),
                None,
            )
            .await
            .expect("close should succeed");

        let stored = db_client
            .select_one("fact:supersede-1", "org")
            .await
            .expect("select fact")
            .expect("fact must exist");

        let t_invalid = stored
            .get("t_invalid")
            .and_then(|v| v.as_str())
            .expect("t_invalid must be a datetime string");
        assert!(
            t_invalid.starts_with("2026-01-02T03:04:05"),
            "caller-supplied t_invalid must be recorded, got {t_invalid}"
        );
        let t_invalid_ingested = stored
            .get("t_invalid_ingested")
            .and_then(|v| v.as_str())
            .expect("t_invalid_ingested must be a datetime string");
        assert!(
            t_invalid_ingested.starts_with("2026-01-02T03:04:05"),
            "caller-supplied t_invalid_ingested must be recorded, got {t_invalid_ingested}"
        );
    }
}
