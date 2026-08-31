//! Tenant Task state machine + version (spec §10.2).
//!
//! `TaskState` is the closed set of states a durable Task
//! can be in. `is_terminal` distinguishes terminal states
//! (Completed, CompletedBeforeCancel, Cancelled,
//! CancelledBeforeCommit, Failed) from in-flight ones
//! (Queued, Running, CancelRequested). The
//! `TenantTaskRecord` is the projection of the 041
//! migration; `TaskHandle` is the fenced lease identity
//! shared by the worker and the store.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::MemoryError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Completed,
    CompletedBeforeCancel,
    CancelRequested,
    Cancelled,
    CancelledBeforeCommit,
    Failed,
}

/// True if the state is a terminal outcome. The reconciler
/// derives the terminal outcome from durable artifacts;
/// this is the spec §10.2 atomicity boundary.
pub fn is_terminal(s: TaskState) -> bool {
    matches!(
        s,
        TaskState::Completed
            | TaskState::CompletedBeforeCancel
            | TaskState::Cancelled
            | TaskState::CancelledBeforeCommit
            | TaskState::Failed
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantTaskRecord {
    pub id: String,
    pub tenant_id: String,
    pub fingerprint: String,
    pub state: TaskState,
    pub version: u64,
    pub cancellation_intent: bool,
    pub lease_owner: Option<String>,
    pub lease_generation: Option<u64>,
    pub lease_expiry: Option<DateTime<Utc>>,
    pub progress: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub retention_expiry: DateTime<Utc>,
}

/// Task lease identity shared by the worker and the
/// durable `TaskStore` trait. A worker commits a state
/// transition only when `lease_generation` matches the
/// stored row; a claim that loses the fence returns
/// `MemoryError::Conflict`.
#[derive(Debug, Clone)]
pub struct TaskHandle {
    pub tenant_id: String,
    pub task_id: String,
    pub lease_owner: String,
    pub lease_generation: u64,
    pub lease_expiry: DateTime<Utc>,
}

/// Durable Tenant Task seam (spec §10). Implemented by
/// the fenced worker store over the tenant namespace's
/// `tenant_task` table.
#[async_trait::async_trait]
pub trait TaskStore: Send + Sync + 'static {
    /// Enqueue a new task; returns the task id. A
    /// duplicate fingerprint returns the existing task id
    /// (the same Tenant-local unique dedupe invariant the
    /// episode path enforces).
    async fn enqueue(
        &self,
        fingerprint: &str,
        params: serde_json::Value,
    ) -> Result<String, MemoryError>;

    /// Load the durable record (state, version, progress,
    /// result, error). A missing record is `Ok(None)`.
    async fn load(&self, task_id: &str) -> Result<Option<TenantTaskRecord>, MemoryError>;

    /// Set cancellation intent; never deletes (spec §10.2).
    async fn set_cancellation_intent(&self, task_id: &str) -> Result<(), MemoryError>;

    /// Claim a queued task or a running task whose lease
    /// has expired. Increments the generation, sets
    /// owner/id/expiry, and returns the resulting
    /// `TaskHandle`. `Ok(None)` when no task is due; never
    /// creates a second task for the same fingerprint.
    async fn claim_next_due(&self, replica_id: &str) -> Result<Option<TaskHandle>, MemoryError>;

    /// Update progress with a `lease_generation = current`
    /// CAS.
    async fn update_progress_fenced(
        &self,
        handle: &TaskHandle,
        progress: serde_json::Value,
    ) -> Result<(), MemoryError>;

    /// Mark the task completed with a CAS. When
    /// `completed_before_cancel` is true the task had
    /// already committed facts before the intent arrived;
    /// the state becomes `CompletedBeforeCancel`.
    async fn complete_fenced(
        &self,
        handle: &TaskHandle,
        result: serde_json::Value,
        completed_before_cancel: bool,
    ) -> Result<(), MemoryError>;

    /// Mark the task failed with a CAS.
    async fn fail_fenced(
        &self,
        handle: &TaskHandle,
        error: serde_json::Value,
    ) -> Result<(), MemoryError>;

    /// Requeue `running` tasks whose lease expired back to
    /// `Queued`. Returns the number of tasks requeued.
    async fn requeue_expired_running(&self) -> Result<u64, MemoryError>;

    /// Reconcile the terminal outcome from durable
    /// artifacts + fingerprint (spec §10.2). Returns the
    /// number of tasks reconciled.
    async fn reconcile_artifacts(&self) -> Result<u64, MemoryError>;

    /// Delete rows past `retention_expiry`. Returns the
    /// number deleted.
    async fn delete_expired(&self) -> Result<u64, MemoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_terminal_states_are_terminal() {
        assert!(is_terminal(TaskState::Completed));
        assert!(is_terminal(TaskState::CompletedBeforeCancel));
        assert!(is_terminal(TaskState::Cancelled));
        assert!(is_terminal(TaskState::CancelledBeforeCommit));
        assert!(is_terminal(TaskState::Failed));
        assert!(!is_terminal(TaskState::Queued));
        assert!(!is_terminal(TaskState::Running));
        assert!(!is_terminal(TaskState::CancelRequested));
    }

    #[test]
    fn cancelled_before_commit_is_distinct_from_completed_before_cancel() {
        // Both are terminal, but they are distinct states:
        // CancelledBeforeCommit means the worker observed
        // the intent and aborted BEFORE writing facts;
        // CompletedBeforeCancel means the worker committed
        // facts and only then observed the intent.
        assert_ne!(
            TaskState::CancelledBeforeCommit,
            TaskState::CompletedBeforeCancel
        );
        // And both serialize to distinct snake_case strings.
        assert_eq!(
            serde_json::to_string(&TaskState::CancelledBeforeCommit).unwrap(),
            "\"cancelled_before_commit\""
        );
        assert_eq!(
            serde_json::to_string(&TaskState::CompletedBeforeCancel).unwrap(),
            "\"completed_before_cancel\""
        );
    }
}
