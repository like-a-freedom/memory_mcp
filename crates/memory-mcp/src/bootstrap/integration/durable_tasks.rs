use std::sync::Arc;

use crate::MemoryError;
use crate::http::tasks::state::{TaskState, TaskStore};
use crate::provisioning::api::{DurableTaskPort, TaskView};

pub(crate) struct DurableTaskAdapter {
    store: Arc<dyn TaskStore>,
}

impl DurableTaskAdapter {
    pub(crate) fn new(store: Arc<dyn TaskStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl DurableTaskPort for DurableTaskAdapter {
    async fn enqueue(
        &self,
        fingerprint: String,
        params: serde_json::Value,
    ) -> Result<String, MemoryError> {
        self.store.enqueue(&fingerprint, params).await
    }

    async fn load(&self, task_id: &str) -> Result<Option<TaskView>, MemoryError> {
        Ok(self.store.load(task_id).await?.map(|record| TaskView {
            task_id: record.id,
            state: task_state(record.state),
            created_at: record.created_at,
            progress: record.progress,
            result: record.result,
            error: record.error,
        }))
    }

    async fn set_cancellation_intent(&self, task_id: &str) -> Result<(), MemoryError> {
        self.store.set_cancellation_intent(task_id).await
    }
}

fn task_state(state: TaskState) -> crate::provisioning::api::TaskState {
    match state {
        TaskState::Queued => crate::provisioning::api::TaskState::Queued,
        TaskState::Running => crate::provisioning::api::TaskState::Running,
        TaskState::Completed => crate::provisioning::api::TaskState::Completed,
        TaskState::CompletedBeforeCancel => {
            crate::provisioning::api::TaskState::CompletedBeforeCancel
        }
        TaskState::CancelRequested => crate::provisioning::api::TaskState::CancelRequested,
        TaskState::Cancelled => crate::provisioning::api::TaskState::Cancelled,
        TaskState::CancelledBeforeCommit => {
            crate::provisioning::api::TaskState::CancelledBeforeCommit
        }
        TaskState::Failed => crate::provisioning::api::TaskState::Failed,
    }
}
