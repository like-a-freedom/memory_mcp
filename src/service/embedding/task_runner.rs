use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;

/// Identifies the kind of background embedding task.
#[derive(Debug, Clone)]
pub enum EmbeddingTaskKind {
    Fact {
        namespace: String,
        fact_id: String,
        input: String,
    },
    Query {
        input: String,
    },
}

impl EmbeddingTaskKind {
    pub fn label(&self) -> &'static str {
        match self {
            EmbeddingTaskKind::Fact { .. } => "fact",
            EmbeddingTaskKind::Query { .. } => "query",
        }
    }
}

/// Manages deduplication of background embedding tasks.
pub struct BackgroundTaskRunner {
    inflight: Arc<Mutex<HashSet<String>>>,
}

impl BackgroundTaskRunner {
    pub fn new() -> Self {
        Self {
            inflight: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn inflight_set(&self) -> Arc<Mutex<HashSet<String>>> {
        self.inflight.clone()
    }

    /// Returns true if the task was reserved (not already inflight).
    pub async fn try_reserve(&self, task_key: &str) -> bool {
        self.inflight.lock().await.insert(task_key.to_string())
    }

    /// Releases a completed task from the inflight set.
    pub async fn release(&self, task_key: &str) {
        self.inflight.lock().await.remove(task_key);
    }

    /// Returns true if a task with the given key is currently inflight.
    pub async fn is_inflight(&self, task_key: &str) -> bool {
        self.inflight.lock().await.contains(task_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn try_reserve_prevents_duplicate_tasks() {
        let runner = BackgroundTaskRunner::new();
        assert!(runner.try_reserve("task:1").await);
        assert!(!runner.try_reserve("task:1").await);
        runner.release("task:1").await;
        assert!(runner.try_reserve("task:1").await);
    }

    #[tokio::test]
    async fn is_inflight_reports_correctly() {
        let runner = BackgroundTaskRunner::new();
        assert!(!runner.is_inflight("missing").await);
        runner.try_reserve("task:2").await;
        assert!(runner.is_inflight("task:2").await);
        runner.release("task:2").await;
        assert!(!runner.is_inflight("task:2").await);
    }
}
