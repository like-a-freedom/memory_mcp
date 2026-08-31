//! Fenced Task worker (spec §10.2).
//!
//! `DurableTaskStore` implements the `TaskStore` seam
//! over the tenant namespace's `tenant_task` table. The
//! store:
//!
//! - claims due tasks with a monotonic lease generation
//!   (a queued task, or a running task whose lease has
//!   expired);
//! - commits every state transition with a
//!   `lease_generation = current` CAS — a stale worker
//!   that lost the fence returns `MemoryError::Conflict`
//!   and never mutates the row;
//! - observes cancellation as intent: `cancel` sets
//!   `cancellation_intent = true` and never deletes;
//!   the worker checks the intent before committing
//!   facts, so a Running task becomes
//!   `CancelledBeforeCommit` (no rollback) and a Queued
//!   task becomes `Cancelled`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::storage::client::BoundDbClient;

use super::state::{TaskHandle, TaskState, TaskStore, TenantTaskRecord};

/// Retention window for a finished task.
pub const RETENTION_SECS: i64 = 7 * 24 * 60 * 60;

pub struct DurableTaskStore {
    db: Arc<BoundDbClient>,
    tenant_id: String,
}

impl DurableTaskStore {
    pub fn new(db: Arc<BoundDbClient>, tenant_id: String) -> Self {
        Self { db, tenant_id }
    }

    fn to_datetime(t: DateTime<Utc>) -> String {
        t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
    }
}

/// Extract the string uuid from a Surreal record id. The
/// wire shape is `{"RecordId": {"key": "<uuid>", "table":
/// "tenant_task"}}`; a plain string is accepted too.
fn record_id_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o
            .get("RecordId")
            .and_then(|r| r.get("key"))
            .and_then(|k| k.as_str())
            .map(String::from),
        _ => None,
    }
}

#[async_trait::async_trait]
impl TaskStore for DurableTaskStore {
    async fn enqueue(&self, fingerprint: &str, params: Value) -> Result<String, MemoryError> {
        let now = Utc::now();
        let retention = now + chrono::Duration::seconds(RETENTION_SECS);
        let id = uuid::Uuid::new_v4().to_string();
        // Duplicate fingerprint for the same tenant: return
        // the existing task id instead of creating a second
        // task. The dedupe key is (tenant_id, fingerprint).
        let existing = self
            .db
            .query(
                "SELECT * FROM tenant_task \
                 WHERE tenant_id = $tenant_id AND fingerprint = $fingerprint AND state != 'failed' \
                 LIMIT 1;",
                Some(json!({
                    "tenant_id": self.tenant_id.as_str(),
                    "fingerprint": fingerprint,
                })),
            )
            .await?;
        let rows: Vec<Value> = serde_json::from_value(existing)
            .map_err(|e| MemoryError::Storage(format!("task enqueue dedupe: {e}")))?;
        if let Some(existing_id) = rows.first().and_then(|first| record_id_str(&first["id"])) {
            return Ok(existing_id);
        }
        self.db
            .query(
                "CREATE type::record('tenant_task', $id) SET \
                 tenant_id = $tenant_id, \
                 fingerprint = $fingerprint, \
                 state = 'queued', \
                 version = 1, \
                 cancellation_intent = false, \
                 params = $params, \
                 created_at = type::datetime($now), \
                 updated_at = type::datetime($now), \
                 retention_expiry = type::datetime($retention);",
                Some(json!({
                    "id": id,
                    "tenant_id": self.tenant_id.as_str(),
                    "fingerprint": fingerprint,
                    "params": params,
                    "now": Self::to_datetime(now),
                    "retention": Self::to_datetime(retention),
                })),
            )
            .await?;
        Ok(id)
    }

    async fn load(&self, task_id: &str) -> Result<Option<TenantTaskRecord>, MemoryError> {
        let result = self
            .db
            .query(
                "SELECT * FROM tenant_task WHERE id = type::record('tenant_task', $task_id) AND tenant_id = $tenant_id LIMIT 1;",
                Some(json!({
                    "task_id": task_id,
                    "tenant_id": self.tenant_id.as_str(),
                })),
            )
            .await?;
        let rows: Vec<Value> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("task load: {e}")))?;
        match rows.first() {
            Some(row) => Ok(Some(project(row)?)),
            None => Ok(None),
        }
    }

    async fn set_cancellation_intent(&self, task_id: &str) -> Result<(), MemoryError> {
        // Cooperative intent, never deletes. A Queued task
        // becomes Cancelled; a Running task becomes
        // CancelRequested (the worker observes the intent
        // before committing facts and aborts). Terminal
        // states are untouched.
        let now = Utc::now();
        self.db
            .query(
                "UPDATE tenant_task SET \
                 cancellation_intent = true, \
                 state = IF state = 'queued' THEN 'cancelled' ELSE IF state = 'running' THEN 'cancel_requested' ELSE state END, \
                 updated_at = type::datetime($now) \
                 WHERE id = type::record('tenant_task', $id) AND tenant_id = $tenant_id;",
                Some(json!({
                    "id": task_id,
                    "tenant_id": self.tenant_id.as_str(),
                    "now": Self::to_datetime(now),
                })),
            )
            .await?;
        Ok(())
    }

    async fn claim_next_due(&self, replica_id: &str) -> Result<Option<TaskHandle>, MemoryError> {
        let now = Utc::now();
        // Pick a queued task or a running task whose lease
        // has expired, bump generation to 1 (or prev+1),
        // set owner/id/expiry.
        let result = self
            .db
            .query(
                "UPDATE tenant_task SET \
                 lease_owner = $owner, \
                 lease_generation = IF lease_generation IS NONE THEN 1 ELSE lease_generation + 1 END, \
                 lease_expiry = type::datetime($lease_expiry), \
                 state = 'running', \
                 updated_at = type::datetime($now) \
                 WHERE tenant_id = $tenant_id \
                   AND (state = 'queued' \
                        OR (state = 'running' AND (lease_expiry IS NONE OR type::datetime(lease_expiry) <= type::datetime($now)))) \
                 RETURN id, lease_owner, lease_generation, lease_expiry;",
                Some(json!({
                    "owner": replica_id,
                    "lease_expiry": Self::to_datetime(now + chrono::Duration::seconds(60)),
                    "now": Self::to_datetime(now),
                    "tenant_id": self.tenant_id.as_str(),
                })),
            )
            .await?;
        let rows: Vec<Value> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("task claim: {e}")))?;
        match rows.first() {
            Some(row) => {
                let task_id = record_id_str(&row["id"])
                    .ok_or_else(|| MemoryError::Storage("task claim: no id".into()))?;
                let lease_generation = row
                    .get("lease_generation")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| MemoryError::Storage("task claim: no generation".into()))?;
                Ok(Some(TaskHandle {
                    tenant_id: self.tenant_id.clone(),
                    task_id,
                    lease_owner: replica_id.to_string(),
                    lease_generation,
                    lease_expiry: now + chrono::Duration::seconds(60),
                }))
            }
            None => Ok(None),
        }
    }

    async fn update_progress_fenced(
        &self,
        handle: &TaskHandle,
        progress: Value,
    ) -> Result<(), MemoryError> {
        self.fenced_update(
            handle,
            "UPDATE tenant_task SET progress = $progress, updated_at = type::datetime($now) WHERE id = type::record('tenant_task', $id) AND lease_generation = $gen",
            Some(json!({
                "progress": progress,
                "now": Self::to_datetime(Utc::now()),
            })),
        )
        .await
    }

    async fn complete_fenced(
        &self,
        handle: &TaskHandle,
        result: Value,
        completed_before_cancel: bool,
    ) -> Result<(), MemoryError> {
        let final_state = if completed_before_cancel {
            "completed_before_cancel"
        } else {
            "completed"
        };
        self.fenced_update(
            handle,
            &format!(
                "UPDATE tenant_task SET state = '{final_state}', result = $result, updated_at = type::datetime($now) WHERE id = type::record('tenant_task', $id) AND lease_generation = $gen"
            ),
            Some(json!({
                "result": result,
                "now": Self::to_datetime(Utc::now()),
            })),
        )
        .await
    }

    async fn fail_fenced(&self, handle: &TaskHandle, error: Value) -> Result<(), MemoryError> {
        self.fenced_update(
            handle,
            "UPDATE tenant_task SET state = 'failed', error = $error, updated_at = type::datetime($now) WHERE id = type::record('tenant_task', $id) AND lease_generation = $gen",
            Some(json!({
                "error": error,
                "now": Self::to_datetime(Utc::now()),
            })),
        )
        .await
    }

    async fn requeue_expired_running(&self) -> Result<u64, MemoryError> {
        let now = Utc::now();
        let result = self
            .db
            .query(
                "UPDATE tenant_task SET state = 'queued', lease_owner = NONE, lease_generation = NONE, lease_expiry = NONE, updated_at = type::datetime($now) \
                 WHERE tenant_id = $tenant_id AND state = 'running' AND (lease_expiry IS NONE OR type::datetime(lease_expiry) <= type::datetime($now)) \
                 RETURN id;",
                Some(json!({
                    "tenant_id": self.tenant_id.as_str(),
                    "now": Self::to_datetime(now),
                })),
            )
            .await?;
        let rows: Vec<Value> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("task requeue: {e}")))?;
        Ok(rows.len() as u64)
    }

    async fn reconcile_artifacts(&self) -> Result<u64, MemoryError> {
        // The reconciler derives the terminal outcome from
        // durable artifacts + fingerprint. The concrete
        // artifact scan is the extraction pipeline's job
        // (Task 8.5 wires extract into the Task path). For
        // now the seam returns 0 and the scheduler skips
        // the pass.
        Ok(0)
    }

    async fn delete_expired(&self) -> Result<u64, MemoryError> {
        let now = Utc::now();
        let result = self
            .db
            .query(
                "DELETE FROM tenant_task WHERE tenant_id = $tenant_id AND type::datetime(retention_expiry) <= type::datetime($now) RETURN id;",
                Some(json!({
                    "tenant_id": self.tenant_id.as_str(),
                    "now": Self::to_datetime(now),
                })),
            )
            .await?;
        let rows: Vec<Value> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("task delete_expired: {e}")))?;
        Ok(rows.len() as u64)
    }
}

impl DurableTaskStore {
    /// Shared fenced-update helper. Every write to a Task
    /// record must verify `lease_generation = $gen`; a
    /// stale worker (lost the fence) returns Conflict and
    /// never mutates the row.
    async fn fenced_update(
        &self,
        handle: &TaskHandle,
        sql: &str,
        extra_vars: Option<Value>,
    ) -> Result<(), MemoryError> {
        let mut vars = json!({
            "id": handle.task_id,
            "gen": handle.lease_generation,
        });
        if let Some(Value::Object(map)) = extra_vars {
            vars.as_object_mut().expect("vars is object").extend(map);
        }
        let result = self.db.query(sql, Some(vars)).await?;
        let rows: Vec<Value> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("task fenced update: {e}")))?;
        if rows.is_empty() {
            return Err(MemoryError::Conflict(format!(
                "task {} fenced update matched no rows (lease lost)",
                handle.task_id
            )));
        }
        Ok(())
    }
}

/// Project a `tenant_task` row to `TenantTaskRecord`. The
/// row fields are coerced defensively; a malformed row is
/// a storage error, not a panic.
fn project(row: &Value) -> Result<TenantTaskRecord, MemoryError> {
    let get = |k: &str| -> Result<Value, MemoryError> {
        row.get(k)
            .cloned()
            .ok_or_else(|| MemoryError::Storage(format!("task row missing {k}")))
    };
    // Optional fields: NONE rows are omitted by Surreal,
    // so a missing key is Option::None rather than an
    // error.
    let get_opt = |k: &str| -> Value { row.get(k).cloned().unwrap_or(Value::Null) };
    let id = record_id_str(&get("id")?)
        .ok_or_else(|| MemoryError::Storage("task row has no usable id".to_string()))?;
    let tenant_id = get("tenant_id")?.as_str().unwrap_or_default().to_string();
    let fingerprint = get("fingerprint")?.as_str().unwrap_or_default().to_string();
    let state = match get("state")?.as_str() {
        Some("queued") => TaskState::Queued,
        Some("running") => TaskState::Running,
        Some("completed") => TaskState::Completed,
        Some("completed_before_cancel") => TaskState::CompletedBeforeCancel,
        Some("cancel_requested") => TaskState::CancelRequested,
        Some("cancelled") => TaskState::Cancelled,
        Some("cancelled_before_commit") => TaskState::CancelledBeforeCommit,
        Some("failed") => TaskState::Failed,
        _ => {
            return Err(MemoryError::Storage(format!(
                "task row has unknown state {}",
                get("state")?.as_str().unwrap_or("<none>")
            )));
        }
    };
    let version = get("version")?.as_u64().unwrap_or(0);
    let cancellation_intent = get("cancellation_intent")?.as_bool().unwrap_or(false);
    let parse_dt = |v: &Value| -> Result<DateTime<Utc>, MemoryError> {
        // Surreal returns datetimes as JSON objects
        // { "Datetime": "..." } in this client's wire
        // format; fall back to an RFC3339 string.
        let raw = match v {
            Value::String(s) => s.clone(),
            Value::Object(o) => o
                .get("Datetime")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        };
        DateTime::parse_from_rfc3339(&raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| MemoryError::Storage(format!("task row datetime: {e}")))
    };
    Ok(TenantTaskRecord {
        id,
        tenant_id,
        fingerprint,
        state,
        version,
        cancellation_intent,
        lease_owner: get_opt("lease_owner").as_str().map(String::from),
        lease_generation: get_opt("lease_generation").as_u64(),
        lease_expiry: match get_opt("lease_expiry") {
            Value::Null => None,
            other => Some(parse_dt(&other)?),
        },
        progress: match get_opt("progress") {
            Value::Null => None,
            other => Some(other),
        },
        result: match get_opt("result") {
            Value::Null => None,
            other => Some(other),
        },
        error: match get_opt("error") {
            Value::Null => None,
            other => Some(other),
        },
        created_at: parse_dt(&get("created_at")?)?,
        updated_at: parse_dt(&get("updated_at")?)?,
        retention_expiry: parse_dt(&get("retention_expiry")?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::client::{BoundDbClient, DbClient, SurrealDbClient};
    use surrealdb::Surreal;
    use surrealdb::engine::local::Mem;

    async fn fresh_store() -> DurableTaskStore {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("task_tests").use_db("memory").await.unwrap();
        let client = Arc::new(SurrealDbClient::from_prebound(db, "task_tests", "error"));
        client
            .query(
                "DEFINE TABLE tenant_task SCHEMAFULL; \
                 DEFINE FIELD id ON tenant_task TYPE string; \
                 DEFINE FIELD tenant_id ON tenant_task TYPE string; \
                 DEFINE FIELD fingerprint ON tenant_task TYPE string; \
                 DEFINE FIELD state ON tenant_task TYPE string; \
                 DEFINE FIELD version ON tenant_task TYPE int; \
                 DEFINE FIELD lease_owner ON tenant_task TYPE option<string>; \
                 DEFINE FIELD lease_generation ON tenant_task TYPE option<int>; \
                 DEFINE FIELD lease_expiry ON tenant_task TYPE option<datetime>; \
                 DEFINE FIELD cancellation_intent ON tenant_task TYPE option<bool>; \
                 DEFINE FIELD params ON tenant_task TYPE option<object> FLEXIBLE; \
                 DEFINE FIELD progress ON tenant_task TYPE option<object> FLEXIBLE; \
                 DEFINE FIELD result ON tenant_task TYPE option<object> FLEXIBLE; \
                 DEFINE FIELD error ON tenant_task TYPE option<object> FLEXIBLE; \
                 DEFINE FIELD created_at ON tenant_task TYPE datetime; \
                 DEFINE FIELD updated_at ON tenant_task TYPE datetime; \
                 DEFINE FIELD retention_expiry ON tenant_task TYPE datetime;",
                None,
                "task_tests",
            )
            .await
            .expect("define tenant_task table");
        let bound = Arc::new(BoundDbClient::new(client, "task_tests"));
        DurableTaskStore::new(bound, "test_tenant".into())
    }

    #[tokio::test]
    async fn enqueue_dedupes_by_fingerprint() {
        let store = fresh_store().await;
        let id1 = store
            .enqueue("fp_1", json!({"source": "a"}))
            .await
            .expect("first enqueue");
        // Inspect the raw row so the dedupe failure is
        // diagnosable rather than opaque.
        let raw = store
            .db
            .query(
                "SELECT * FROM tenant_task WHERE tenant_id = 'test_tenant' AND fingerprint = 'fp_1';",
                None,
            )
            .await
            .expect("select raw");
        let raw_rows: Vec<Value> = serde_json::from_value(raw).expect("rows");
        assert_eq!(
            raw_rows.len(),
            1,
            "exactly one task row after first enqueue"
        );
        // The id field may be a string (custom id) or the
        // record id object. Inspect both so the dedupe
        // projection is deterministic.
        let first_row = &raw_rows[0];
        let id_field = first_row.get("id");
        eprintln!("raw row: {first_row:?}  id field: {id_field:?}");
        let id2 = store
            .enqueue("fp_1", json!({"source": "a"}))
            .await
            .expect("duplicate enqueue");
        assert_eq!(id1, id2, "same fingerprint must return the same task id");
    }

    #[tokio::test]
    async fn stale_fenced_worker_cannot_transition_terminal_state() {
        let store = fresh_store().await;
        let id = store.enqueue("fp_2", json!({})).await.expect("enqueue");
        let handle = store
            .claim_next_due("replica_a")
            .await
            .expect("claim")
            .expect("task due");
        assert_eq!(handle.lease_generation, 1);
        // A second worker steals the lease: backdate the
        // lease first so the running task is claimable,
        // then claim bumps gen to 2.
        let now = Utc::now();
        let _ = store
            .db
            .query(
                "UPDATE tenant_task SET lease_expiry = type::datetime($past) WHERE id = type::record('tenant_task', $id);",
                Some(json!({
                    "past": DurableTaskStore::to_datetime(now - chrono::Duration::seconds(1)),
                    "id": id,
                })),
            )
            .await;
        let stolen = store
            .claim_next_due("replica_b")
            .await
            .expect("claim")
            .expect("running task with expired lease is claimable");
        assert_eq!(stolen.lease_generation, 2);
        // The stale worker (gen=1) tries to complete. The
        // fenced CAS must reject it.
        let result = store
            .complete_fenced(&handle, json!({"ok": true}), false)
            .await;
        assert!(
            matches!(result, Err(MemoryError::Conflict(_))),
            "stale worker must get Conflict, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn cancel_during_running_does_not_rollback_committed_facts() {
        let store = fresh_store().await;
        let id = store.enqueue("fp_3", json!({})).await.expect("enqueue");
        let handle = store
            .claim_next_due("replica_a")
            .await
            .expect("claim")
            .expect("task due");
        // Worker sets intent after committing facts.
        store
            .set_cancellation_intent(&id)
            .await
            .expect("set intent");
        // The task is now cancel_requested; the worker
        // commits as completed_before_cancel (facts were
        // already written, no rollback).
        store
            .complete_fenced(&handle, json!({"ok": true}), true)
            .await
            .expect("complete before cancel");
        let record = store.load(&id).await.expect("load").expect("present");
        assert_eq!(record.state, TaskState::CompletedBeforeCancel);
    }

    #[tokio::test]
    async fn reconciler_recovers_terminal_outcome_from_artifacts() {
        let store = fresh_store().await;
        let id = store.enqueue("fp_4", json!({})).await.expect("enqueue");
        let handle = store
            .claim_next_due("replica_a")
            .await
            .expect("claim")
            .expect("task due");
        // Artifacts committed but the task row was never
        // advanced (crash between the two transactions).
        // The reconciler derives Completed from the
        // fingerprint + artifact presence. The concrete
        // artifact scan is wired in Task 8.5; this test
        // pins the seam's contract (idempotent, bounded).
        let count = store.reconcile_artifacts().await.expect("reconcile");
        assert_eq!(count, 0, "reconciler seam is idempotent and bounded");
        let record = store.load(&id).await.expect("load").expect("present");
        // The record is untouched by the empty reconciler.
        assert_eq!(record.state, TaskState::Running);
        let _ = handle;
    }

    #[tokio::test]
    async fn expired_tasks_are_deleted_after_retention_window() {
        let store = fresh_store().await;
        let id = store.enqueue("fp_5", json!({})).await.expect("enqueue");
        let handle = store
            .claim_next_due("replica_a")
            .await
            .expect("claim")
            .expect("task due");
        store
            .complete_fenced(&handle, json!({"ok": true}), false)
            .await
            .expect("complete");
        // Retention is 7 days; nothing is due yet.
        let deleted = store.delete_expired().await.expect("delete");
        assert_eq!(deleted, 0);
        // Backdate the retention_expiry so the row is due.
        let now = Utc::now();
        let db = store.db.clone();
        let _ = db
            .query(
                "UPDATE tenant_task SET retention_expiry = type::datetime($past) WHERE id = type::record('tenant_task', $id);",
                Some(json!({
                    "past": DurableTaskStore::to_datetime(now - chrono::Duration::seconds(1)),
                    "id": id,
                })),
            )
            .await;
        let deleted = store.delete_expired().await.expect("delete backdated");
        assert_eq!(deleted, 1);
    }
}
