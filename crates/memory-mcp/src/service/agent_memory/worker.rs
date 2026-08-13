//! Durable projection worker for accepted lifecycle events.
//!
//! The worker runtime polls for pending `event_projection_job` records and
//! delegates to `projection::run_projection_pass` to lease, project, and
//! complete them. It shares cancellation, backoff, and lease-loop mechanics
//! with the existing lifecycle workers.

use std::sync::Arc;

use tokio::time::{self, Duration as TokioDuration};
use tokio_util::sync::CancellationToken;

use crate::service::MemoryService;
use crate::service::durable_work;

use super::projection::run_projection_pass;

/// Bounded worker runtime for lifecycle event projection.
///
/// Mirrors `ClaimWorkerRuntime`: owns a cancellation token and a list of
/// spawned `JoinHandle`s so shutdown drains cleanly.
#[derive(Clone)]
pub struct LifecycleWorkerRuntime {
    shutdown: CancellationToken,
    handles: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl LifecycleWorkerRuntime {
    pub(crate) fn new() -> Self {
        Self {
            shutdown: CancellationToken::new(),
            handles: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    /// Spawn the projection worker, polling every `poll_interval_secs`.
    pub(crate) async fn spawn(&self, service: MemoryService, poll_interval_secs: u64) {
        let shutdown = self.shutdown.clone();
        let handle = tokio::spawn(async move {
            let mut interval = time::interval(TokioDuration::from_secs(poll_interval_secs));
            let mut startup_event = std::collections::HashMap::new();
            startup_event.insert(
                "op".to_string(),
                serde_json::Value::String("lifecycle.projection.start".to_string()),
            );
            service
                .logger
                .log(startup_event, crate::logging::LogLevel::Info);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {}
                }
                match run_projection_pass(&service).await {
                    Ok(count) => {
                        if count > 0 {
                            let mut event = std::collections::HashMap::new();
                            event.insert(
                                "op".to_string(),
                                serde_json::Value::String(
                                    "lifecycle.projection.complete".to_string(),
                                ),
                            );
                            event.insert(
                                "jobs_projected".to_string(),
                                serde_json::Value::Number(serde_json::Number::from(count)),
                            );
                            service.logger.log(event, crate::logging::LogLevel::Info);
                        }
                    }
                    Err(e) => {
                        let mut event = std::collections::HashMap::new();
                        event.insert(
                            "op".to_string(),
                            serde_json::Value::String("lifecycle.projection.error".to_string()),
                        );
                        event.insert(
                            "error".to_string(),
                            serde_json::Value::String(format!("{e}")),
                        );
                        service.logger.log(event, crate::logging::LogLevel::Warn);
                    }
                }
            }
        });
        let mut handles = self.handles.lock().await;
        handles.push(handle);
    }

    pub(crate) async fn shutdown(&self) {
        self.shutdown.cancel();
        let handles = std::mem::take(&mut *self.handles.lock().await);
        for handle in handles {
            let _ = handle.await;
        }
    }
}

impl Default for LifecycleWorkerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

// Projection logic (run_projection_pass, job leasing, failure handling, and
// extraction delegation) lives in `projection.rs`. The worker runtime calls
// `projection::run_projection_pass` on each poll tick.

/// Shared worker mechanics: empty-poll backoff. Delegates to
/// `service::durable_work::empty_poll_backoff` (ADR-0026 single timing home).
#[must_use]
pub fn empty_poll_interval() -> std::time::Duration {
    durable_work::empty_poll_backoff()
}

/// Shared worker mechanics: transient-error backoff. Delegates to
/// `service::durable_work::transient_error_backoff` (ADR-0026 single timing home).
#[must_use]
pub fn transient_backoff() -> std::time::Duration {
    durable_work::transient_error_backoff()
}

#[cfg(test)]
mod tests {
    use super::super::projection::run_projection_pass;
    use super::*;
    use std::time::Duration;

    #[test]
    fn empty_poll_interval_returns_configured_default() {
        assert_eq!(empty_poll_interval(), durable_work::empty_poll_backoff());
    }

    #[test]
    fn transient_backoff_returns_configured_default() {
        assert_eq!(transient_backoff(), durable_work::transient_error_backoff());
    }

    #[tokio::test]
    async fn lifecycle_worker_runtime_starts_and_shuts_down_cleanly() {
        // Wiring test: the LifecycleWorkerRuntime must spawn a task and shut
        // it down without panicking or hanging. This guards against regressions
        // where the worker is defined but not wired to the runtime.
        let runtime = LifecycleWorkerRuntime::new();
        // Spawn a no-op task using a dummy service. We only need to verify
        // the spawn/shutdown lifecycle, not projection itself (covered by
        // projection_pass_completes_accepted_job).
        //
        // We use a minimal in-memory service so the worker can poll without
        // panicking even if there are no jobs.
        let service = setup_service().await;
        runtime.spawn(service, 1).await; // 1-second poll
        // Give the worker a moment to run at least one poll.
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Shutdown must complete without hanging.
        let shutdown_result =
            tokio::time::timeout(Duration::from_secs(5), runtime.shutdown()).await;
        assert!(shutdown_result.is_ok(), "shutdown must not hang");
    }

    // --- E2E tests with a real in-memory SurrealDB ---

    use crate::storage::SurrealDbClient;
    use std::sync::Arc;

    async fn setup_service() -> MemoryService {
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory("worker_e2e", "test", "warn")
                .await
                .expect("in-memory db"),
        );
        db_client
            .apply_migrations_impl("test")
            .await
            .expect("migrations");
        MemoryService::new(db_client, "test".to_string(), "warn".to_string(), 50, 100)
            .expect("create service")
    }

    #[tokio::test]
    async fn projection_pass_completes_accepted_job() {
        let service = setup_service().await;
        let namespace = "test";
        let episode_id = "episode:e2e-1";
        let now = crate::service::now();

        let episode_payload = serde_json::json!({
            "episode_id": episode_id,
            "source_type": "inline",
            "source_id": "e2e-source-1",
            "content": "Alice works at Acme Corp on the copper-palm project.",
            "t_ref": now,
            "t_ingested": now,
            "scope": "org",
            "visibility_scope": "org",
            "policy_tags": [],
        });
        service
            .db_client
            .create(episode_id, episode_payload, namespace)
            .await
            .expect("create episode");

        let event_payload = serde_json::json!({
            "event_id": "evt-e2e-1",
            "event_kind": "post_tool_result",
            "task_fingerprint": "task:e2e",
            "normalized_task": "do work",
            "scope": "org",
            "project": "copper-palm",
            "policy_tags": [],
            "capture_signal": "verified_success",
            "disposition": "accepted",
            "trust_class": "lifecycle_evidence",
            "source_kind": "tool_result",
            "content_hash": "abc",
            "content_byte_len": 42,
            "artifact_uri_count": 0,
            "reason_codes": ["accepted_outcome"],
            "episode_id": episode_id,
            "origin_kind": "lifecycle_adapter",
            "created_at": now,
        });
        service
            .db_client
            .create("memory_event:evt-e2e-1", event_payload, namespace)
            .await
            .expect("create event");

        let job_payload = serde_json::json!({
            "job_id": "job-e2e-1",
            "event_id": "evt-e2e-1",
            "episode_id": episode_id,
            "scope": "org",
            "project": "copper-palm",
            "status": "pending",
            "attempts": 0,
            "max_attempts": 5,
            "origin_kind": "lifecycle_adapter",
            "created_at": now,
        });
        service
            .db_client
            .create("event_projection_job:job-e2e-1", job_payload, namespace)
            .await
            .expect("create job");

        let count = run_projection_pass(&service)
            .await
            .expect("projection pass");
        assert!(count >= 1, "at least one job should be projected");

        let job = service
            .db_client
            .select_one("event_projection_job:job-e2e-1", namespace)
            .await
            .expect("load job")
            .expect("job exists");
        assert_eq!(job["status"], "completed");
    }

    #[tokio::test]
    async fn expired_lease_is_reacquired() {
        let service = setup_service().await;
        let namespace = "test";
        let now = crate::service::now();

        let job_payload = serde_json::json!({
            "job_id": "job-expired",
            "event_id": "evt-expired",
            "episode_id": "episode:nonexistent",
            "scope": "org",
            "project": "test",
            "status": "leased",
            "attempts": 0,
            "max_attempts": 5,
            "leased_at": "2020-01-01T00:00:00Z",
            "lease_expires_at": "2020-01-01T00:00:01Z",
            "origin_kind": "lifecycle_adapter",
            "created_at": now,
        });
        service
            .db_client
            .create("event_projection_job:job-expired", job_payload, namespace)
            .await
            .expect("create job");

        let _ = run_projection_pass(&service).await;

        let job = service
            .db_client
            .select_one("event_projection_job:job-expired", namespace)
            .await
            .expect("load job")
            .expect("job exists");
        assert_ne!(job["status"], "leased");
    }

    #[tokio::test]
    async fn retry_exhaustion_enters_visible_dead_letter() {
        let service = setup_service().await;
        let namespace = "test";
        let now = crate::service::now();

        let job_payload = serde_json::json!({
            "job_id": "job-deadletter",
            "event_id": "evt-deadletter",
            "episode_id": "episode:nonexistent",
            "scope": "org",
            "project": "test",
            "status": "pending",
            "attempts": 4,
            "max_attempts": 5,
            "origin_kind": "lifecycle_adapter",
            "created_at": now,
        });
        service
            .db_client
            .create(
                "event_projection_job:job-deadletter",
                job_payload,
                namespace,
            )
            .await
            .expect("create job");

        let event_payload = serde_json::json!({
            "event_id": "evt-deadletter",
            "event_kind": "post_tool_result",
            "task_fingerprint": "task:deadletter",
            "normalized_task": "do work",
            "scope": "org",
            "project": "test",
            "policy_tags": [],
            "capture_signal": "verified_success",
            "disposition": "accepted",
            "trust_class": "lifecycle_evidence",
            "episode_id": "episode:nonexistent",
            "origin_kind": "lifecycle_adapter",
            "created_at": now,
        });
        service
            .db_client
            .create("memory_event:evt-deadletter", event_payload, namespace)
            .await
            .expect("create event");

        let _ = run_projection_pass(&service).await;

        let job = service
            .db_client
            .select_one("event_projection_job:job-deadletter", namespace)
            .await
            .expect("load job")
            .expect("job exists");
        assert_eq!(job["status"], "dead_letter");
        assert!(job["dead_lettered_at"].as_str().is_some());
    }
}
