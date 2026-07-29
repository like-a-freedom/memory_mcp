//! MCP task lifecycle state owned by the protocol adapter.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rmcp::ErrorData;
use rmcp::model::{ErrorCode, Meta, Task, TaskStatus};
use serde_json::Value;
use tokio::task::AbortHandle;

pub(crate) const DEFAULT_TASK_TTL_MS: u64 = 300_000;
pub(crate) const DEFAULT_TASK_POLL_INTERVAL_MS: u64 = 100;
pub(crate) const MAX_TASK_TTL_MS: u64 = 3_600_000;
const MIN_TASK_TTL_MS: u64 = 1_000;
const MAX_ACTIVE_TASKS: usize = 64;
const MAX_RETAINED_TASKS: usize = 1_024;
const TASK_CANCELLED_ERROR_CODE: ErrorCode = ErrorCode(-32800);

struct TaskEntry {
    task: Task,
    payload: Option<Value>,
    protocol_error: Option<ErrorData>,
    abort_handle: Option<AbortHandle>,
    expires_at: Instant,
}

#[derive(Debug)]
pub(crate) enum TaskPayloadState {
    Ready(Value),
    ProtocolError(ErrorData),
    Pending,
    Unavailable(TaskStatus),
    Missing,
}

#[derive(Debug)]
pub(crate) enum TaskOutcome {
    ToolResult { payload: Value, is_error: bool },
    ProtocolError(ErrorData),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TaskCreateError {
    ActiveCapacity,
    RetainedCapacity,
}

#[derive(Debug)]
pub(crate) enum TaskCancelState {
    Cancelled(Task),
    NotCancellable(TaskStatus),
    Missing,
}

#[derive(Default)]
pub(crate) struct TaskStore {
    entries: HashMap<String, TaskEntry>,
}

impl TaskStore {
    pub(crate) fn create(
        &mut self,
        task_id: String,
        requested_ttl: Option<u64>,
    ) -> Result<Task, TaskCreateError> {
        self.remove_expired();
        if self.entries.len() >= MAX_RETAINED_TASKS {
            return Err(TaskCreateError::RetainedCapacity);
        }
        if self
            .entries
            .values()
            .filter(|entry| {
                matches!(
                    entry.task.status,
                    TaskStatus::Working | TaskStatus::InputRequired
                )
            })
            .count()
            >= MAX_ACTIVE_TASKS
        {
            return Err(TaskCreateError::ActiveCapacity);
        }

        let ttl = normalize_task_ttl(requested_ttl);
        let timestamp = current_timestamp();
        let task = Task::new(
            task_id.clone(),
            TaskStatus::Working,
            timestamp.clone(),
            timestamp,
        )
        .with_status_message("Task accepted")
        .with_ttl(ttl)
        .with_poll_interval(DEFAULT_TASK_POLL_INTERVAL_MS);
        self.entries.insert(
            task_id,
            TaskEntry {
                task: task.clone(),
                payload: None,
                protocol_error: None,
                abort_handle: None,
                expires_at: Instant::now() + Duration::from_millis(ttl),
            },
        );
        Ok(task)
    }

    pub(crate) fn attach_abort_handle(&mut self, task_id: &str, abort_handle: AbortHandle) {
        if let Some(entry) = self.entries.get_mut(task_id)
            && entry.task.status == TaskStatus::Working
        {
            entry.abort_handle = Some(abort_handle);
        }
    }

    pub(crate) fn complete(&mut self, task_id: &str, outcome: TaskOutcome) {
        let Some(entry) = self.entries.get_mut(task_id) else {
            return;
        };
        if entry.task.status != TaskStatus::Working {
            return;
        }

        entry.task.last_updated_at = current_timestamp();
        entry.abort_handle = None;
        match outcome {
            TaskOutcome::ToolResult { payload, is_error } => {
                entry.task.status = if is_error {
                    TaskStatus::Failed
                } else {
                    TaskStatus::Completed
                };
                entry.task.status_message = Some(if is_error {
                    "Tool returned an error".to_string()
                } else {
                    "Task completed".to_string()
                });
                entry.payload = Some(payload);
                entry.protocol_error = None;
            }
            TaskOutcome::ProtocolError(error) => {
                entry.task.status = TaskStatus::Failed;
                entry.task.status_message = Some(error.message.to_string());
                entry.payload = None;
                entry.protocol_error = Some(error);
            }
        }
    }

    pub(crate) fn list(&mut self) -> Vec<Task> {
        self.remove_expired();
        let mut tasks = self
            .entries
            .values()
            .map(|entry| entry.task.clone())
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.task_id.cmp(&right.task_id))
        });
        tasks
    }

    pub(crate) fn get(&mut self, task_id: &str) -> Option<Task> {
        self.remove_if_expired(task_id);
        self.entries.get(task_id).map(|entry| entry.task.clone())
    }

    pub(crate) fn payload(&mut self, task_id: &str) -> TaskPayloadState {
        self.remove_if_expired(task_id);
        let Some(entry) = self.entries.get(task_id) else {
            return TaskPayloadState::Missing;
        };
        match entry.task.status {
            TaskStatus::Working | TaskStatus::InputRequired => TaskPayloadState::Pending,
            TaskStatus::Completed | TaskStatus::Failed => entry
                .payload
                .clone()
                .map(TaskPayloadState::Ready)
                .or_else(|| {
                    entry
                        .protocol_error
                        .clone()
                        .map(TaskPayloadState::ProtocolError)
                })
                .unwrap_or_else(|| TaskPayloadState::Unavailable(entry.task.status.clone())),
            TaskStatus::Cancelled => entry.protocol_error.clone().map_or(
                TaskPayloadState::Unavailable(TaskStatus::Cancelled),
                TaskPayloadState::ProtocolError,
            ),
            _ => TaskPayloadState::Unavailable(entry.task.status.clone()),
        }
    }

    pub(crate) fn cancel(&mut self, task_id: &str) -> TaskCancelState {
        self.remove_if_expired(task_id);
        let Some(entry) = self.entries.get_mut(task_id) else {
            return TaskCancelState::Missing;
        };
        if entry.task.status != TaskStatus::Working
            && entry.task.status != TaskStatus::InputRequired
        {
            return TaskCancelState::NotCancellable(entry.task.status.clone());
        }

        if let Some(abort_handle) = entry.abort_handle.take() {
            abort_handle.abort();
        }
        entry.task.status = TaskStatus::Cancelled;
        entry.task.status_message = Some("Task cancelled".to_string());
        entry.task.last_updated_at = current_timestamp();
        entry.payload = None;
        entry.protocol_error = Some(ErrorData::new(
            TASK_CANCELLED_ERROR_CODE,
            "task was cancelled",
            None,
        ));
        TaskCancelState::Cancelled(entry.task.clone())
    }

    fn remove_expired(&mut self) {
        let now = Instant::now();
        self.entries.retain(|_, entry| {
            let keep = now < entry.expires_at;
            if !keep && let Some(abort_handle) = entry.abort_handle.take() {
                abort_handle.abort();
            }
            keep
        });
    }

    fn remove_if_expired(&mut self, task_id: &str) {
        let expired = self
            .entries
            .get(task_id)
            .is_some_and(|entry| Instant::now() >= entry.expires_at);
        if expired
            && let Some(mut entry) = self.entries.remove(task_id)
            && let Some(abort_handle) = entry.abort_handle.take()
        {
            abort_handle.abort();
        }
    }
}

fn normalize_task_ttl(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(DEFAULT_TASK_TTL_MS)
        .clamp(MIN_TASK_TTL_MS, MAX_TASK_TTL_MS)
}

pub(crate) fn add_related_task_metadata(
    payload: &mut Value,
    task_id: &str,
) -> Result<(), &'static str> {
    let object = payload
        .as_object_mut()
        .ok_or("task result payload must be a JSON object")?;
    let meta = object
        .entry("_meta")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or("task result _meta must be a JSON object")?;
    meta.insert(
        rmcp::model::RelatedTaskMetadata::META_KEY.to_string(),
        serde_json::json!({"taskId": task_id}),
    );
    Ok(())
}

pub(crate) fn related_task_metadata(task_id: &str) -> Meta {
    let mut fields = serde_json::Map::new();
    fields.insert(
        rmcp::model::RelatedTaskMetadata::META_KEY.to_string(),
        serde_json::json!({"taskId": task_id}),
    );
    Meta(fields)
}

fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_task_is_stable_listable_and_repeatable() {
        let mut store = TaskStore::default();
        let created = store
            .create("task-1".to_string(), Some(DEFAULT_TASK_TTL_MS))
            .unwrap();
        let created_at = created.created_at.clone();
        let mut payload = serde_json::json!({"structuredContent": {"status": "ok"}});
        add_related_task_metadata(&mut payload, "task-1").unwrap();

        store.complete(
            "task-1",
            TaskOutcome::ToolResult {
                payload: payload.clone(),
                is_error: false,
            },
        );

        let completed = store.get("task-1").expect("completed task");
        assert_eq!(completed.status, TaskStatus::Completed);
        assert_eq!(completed.created_at, created_at);
        assert!(store.list().iter().any(|task| task.task_id == "task-1"));
        assert!(
            matches!(store.payload("task-1"), TaskPayloadState::Ready(value) if value == payload)
        );
        assert_eq!(
            payload["_meta"][rmcp::model::RelatedTaskMetadata::META_KEY]["taskId"],
            "task-1"
        );
    }

    #[test]
    fn cancelled_task_remains_cancelled() {
        let mut store = TaskStore::default();
        let created = store
            .create("task-2".to_string(), Some(DEFAULT_TASK_TTL_MS))
            .unwrap();

        assert!(matches!(
            store.cancel("task-2"),
            TaskCancelState::Cancelled(_)
        ));
        let cancelled = store.get("task-2").expect("cancelled task");
        assert_eq!(cancelled.status, TaskStatus::Cancelled);
        assert_eq!(cancelled.created_at, created.created_at);
        assert!(matches!(
            store.payload("task-2"),
            TaskPayloadState::ProtocolError(error)
                if error.code == TASK_CANCELLED_ERROR_CODE
        ));
    }

    #[test]
    fn failed_tool_and_protocol_results_are_preserved() {
        let mut store = TaskStore::default();
        store
            .create("tool-error".to_string(), Some(DEFAULT_TASK_TTL_MS))
            .unwrap();
        let payload = serde_json::json!({"isError": true, "content": []});
        store.complete(
            "tool-error",
            TaskOutcome::ToolResult {
                payload: payload.clone(),
                is_error: true,
            },
        );
        assert_eq!(store.get("tool-error").unwrap().status, TaskStatus::Failed);
        assert!(
            matches!(store.payload("tool-error"), TaskPayloadState::Ready(value) if value == payload)
        );

        store
            .create("protocol-error".to_string(), Some(DEFAULT_TASK_TTL_MS))
            .unwrap();
        let error = ErrorData::invalid_params("bad task request", None);
        store.complete("protocol-error", TaskOutcome::ProtocolError(error.clone()));
        assert!(matches!(
            store.payload("protocol-error"),
            TaskPayloadState::ProtocolError(actual)
                if actual.code == error.code && actual.message == error.message
        ));
    }

    #[test]
    fn active_task_count_is_bounded() {
        let mut store = TaskStore::default();
        for index in 0..MAX_ACTIVE_TASKS {
            store
                .create(format!("task-{index}"), Some(DEFAULT_TASK_TTL_MS))
                .unwrap();
        }
        assert_eq!(
            store.create("one-too-many".to_string(), Some(DEFAULT_TASK_TTL_MS)),
            Err(TaskCreateError::ActiveCapacity)
        );
    }

    #[test]
    fn requested_ttl_is_clamped_to_safe_bounds() {
        assert_eq!(normalize_task_ttl(None), DEFAULT_TASK_TTL_MS);
        assert_eq!(normalize_task_ttl(Some(0)), MIN_TASK_TTL_MS);
        assert_eq!(
            normalize_task_ttl(Some(MAX_TASK_TTL_MS + 1)),
            MAX_TASK_TTL_MS
        );
    }

    #[tokio::test]
    async fn cancellation_aborts_the_attached_worker() {
        let mut store = TaskStore::default();
        store
            .create("abort-worker".to_string(), Some(DEFAULT_TASK_TTL_MS))
            .unwrap();
        let worker = tokio::spawn(std::future::pending::<()>());
        store.attach_abort_handle("abort-worker", worker.abort_handle());

        assert!(matches!(
            store.cancel("abort-worker"),
            TaskCancelState::Cancelled(_)
        ));
        assert!(
            worker
                .await
                .expect_err("worker must be aborted")
                .is_cancelled()
        );
        assert_eq!(
            store.get("abort-worker").unwrap().status,
            TaskStatus::Cancelled
        );
    }
}
