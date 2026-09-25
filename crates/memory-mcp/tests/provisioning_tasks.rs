#![cfg(feature = "streamable-http")]

use std::sync::{Arc, Mutex};

use memory_mcp::MemoryError;
use memory_mcp::provisioning::api::{
    CancelTaskCommand, DurableTaskPort, EnqueueTaskCommand, TaskState, TaskView, cancel_task,
    enqueue_task, task_view,
};

#[derive(Default)]
struct RecordingTaskPort {
    enqueued: Mutex<Vec<(String, serde_json::Value)>>,
    cancelled: Mutex<Vec<String>>,
    state: Mutex<Option<TaskState>>,
}

#[async_trait::async_trait]
impl DurableTaskPort for RecordingTaskPort {
    async fn enqueue(
        &self,
        fingerprint: String,
        params: serde_json::Value,
    ) -> Result<String, MemoryError> {
        self.enqueued
            .lock()
            .expect("enqueued lock")
            .push((fingerprint, params));
        Ok("tsk_1".into())
    }

    async fn load(&self, task_id: &str) -> Result<Option<TaskView>, MemoryError> {
        Ok(
            (*self.state.lock().expect("state lock")).map(|state| TaskView {
                task_id: task_id.to_string(),
                state,
                created_at: chrono::Utc::now(),
                progress: None,
                result: None,
                error: None,
            }),
        )
    }

    async fn set_cancellation_intent(&self, task_id: &str) -> Result<(), MemoryError> {
        self.cancelled
            .lock()
            .expect("cancelled lock")
            .push(task_id.to_string());
        Ok(())
    }
}

#[tokio::test]
async fn durable_task_seam_maps_states_and_keeps_cancel_idempotent() {
    let port = Arc::new(RecordingTaskPort::default());
    let task_id = enqueue_task(
        port.as_ref(),
        EnqueueTaskCommand {
            fingerprint: "extract:1".into(),
            params: serde_json::json!({"content": "hello"}),
        },
    )
    .await
    .expect("enqueue");
    assert_eq!(task_id, "tsk_1");

    for (state, expected) in [
        (TaskState::Queued, "working"),
        (TaskState::Running, "working"),
        (TaskState::CancelRequested, "cancelled"),
        (TaskState::Completed, "completed"),
        (TaskState::CompletedBeforeCancel, "cancelled"),
        (TaskState::Cancelled, "cancelled"),
        (TaskState::CancelledBeforeCommit, "cancelled"),
        (TaskState::Failed, "failed"),
    ] {
        *port.state.lock().expect("state lock") = Some(state);
        let view = task_view(port.as_ref(), &task_id)
            .await
            .expect("view")
            .expect("task exists");
        assert_eq!(view.status(), expected);
    }

    cancel_task(
        port.as_ref(),
        &CancelTaskCommand {
            task_id: task_id.clone(),
        },
    )
    .await
    .expect("cancel");
    cancel_task(port.as_ref(), &CancelTaskCommand { task_id })
        .await
        .expect("cancel replay");
    assert_eq!(port.cancelled.lock().expect("cancelled lock").len(), 2);
}
