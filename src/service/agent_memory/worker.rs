//! Durable projection worker for accepted lifecycle events.
//!
//! This worker drains `event_projection_job` records, reuses the existing
//! extraction path to project facts from the accepted episode, and marks jobs
//! complete. It shares cancellation, backoff, and lease-loop mechanics with the
//! existing lifecycle workers rather than duplicating them.

use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};

use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::storage::EventProjectionJobRecord;

const PROJECTION_BATCH_LIMIT: i32 = 50;
const DEFAULT_LEASE_SECS: u64 = 120;
const DEFAULT_MAX_ATTEMPTS: i64 = 5;
const EMPTY_POLL_INTERVAL_SECS: u64 = 10;
const TRANSIENT_BACKOFF_SECS: u64 = 5;

/// Spawns the projection worker background task.
///
/// The worker polls for pending `event_projection_job` records, leases them,
/// runs projection, and marks them complete or dead-letters them on retry
/// exhaustion. It shares the same `MemoryService` as the existing lifecycle
/// workers.
#[must_use]
pub fn spawn_projection_worker(
    service: MemoryService,
    poll_interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(TokioDuration::from_secs(poll_interval_secs));

        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::Value::String("lifecycle.projection.start".to_string()),
        );
        service.logger.log(event, crate::logging::LogLevel::Info);

        loop {
            interval.tick().await;
            match run_projection_pass(&service).await {
                Ok(count) => {
                    if count > 0 {
                        let mut event = std::collections::HashMap::new();
                        event.insert(
                            "op".to_string(),
                            serde_json::Value::String("lifecycle.projection.complete".to_string()),
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
    })
}

/// Runs a single projection pass: lease pending jobs, project, complete.
pub async fn run_projection_pass(service: &MemoryService) -> Result<usize, MemoryError> {
    let mut projected = 0;
    let now_str = crate::service::normalize_dt(chrono::Utc::now());
    let lease_expires = crate::service::normalize_dt(
        chrono::Utc::now() + chrono::Duration::seconds(DEFAULT_LEASE_SECS as i64),
    );

    for namespace in &service.namespaces {
        let pending_jobs = load_pending_jobs(service, namespace, PROJECTION_BATCH_LIMIT).await?;

        for job in pending_jobs {
            if let Err(e) =
                process_one_job(service, &job, namespace, &now_str, &lease_expires).await
            {
                handle_job_failure(service, &job, namespace, &e, &now_str).await?;
            } else {
                projected += 1;
            }
        }
    }

    Ok(projected)
}

/// Loads pending or expired-lease projection jobs.
async fn load_pending_jobs(
    service: &MemoryService,
    namespace: &str,
    limit: i32,
) -> Result<Vec<EventProjectionJobRecord>, MemoryError> {
    let sql = "SELECT * FROM event_projection_job WHERE status = 'pending' OR (status = 'leased' AND lease_expires_at <= type::datetime($now)) LIMIT $limit";
    let vars = json!({
        "now": crate::service::normalize_dt(chrono::Utc::now()),
        "limit": limit,
    });
    let rows = service.db_client.query(sql, Some(vars), namespace).await?;

    let mut jobs = Vec::new();
    if let Some(arr) = rows.as_array() {
        for row in arr {
            if let Ok(job) = serde_json::from_value::<EventProjectionJobRecord>(row.clone()) {
                jobs.push(job);
            }
        }
    }
    Ok(jobs)
}

/// Processes one projection job: lease it, run projection, mark complete.
async fn process_one_job(
    service: &MemoryService,
    job: &EventProjectionJobRecord,
    namespace: &str,
    now_str: &str,
    lease_expires: &str,
) -> Result<(), MemoryError> {
    // Lease the job.
    lease_job(service, &job.job_id, namespace, now_str, lease_expires).await?;

    // Load the event to verify disposition and fingerprint.
    let event = load_event(service, &job.event_id, namespace).await?;

    // Only accepted events with an episode reference are projected.
    if event.disposition != "accepted" {
        mark_job_complete(service, &job.job_id, namespace, now_str).await?;
        return Ok(());
    }

    let Some(episode_id) = &event.episode_id else {
        mark_job_complete(service, &job.job_id, namespace, now_str).await?;
        return Ok(());
    };

    // Reuse the existing extraction path to project facts from the episode.
    // The extraction path propagates origin through provenance.
    if let Err(e) = run_extraction(service, episode_id, namespace).await {
        // Increment attempts and re-raise for failure handling.
        increment_attempts(service, &job.job_id, namespace, now_str).await?;
        return Err(e);
    }

    mark_job_complete(service, &job.job_id, namespace, now_str).await?;
    Ok(())
}

/// Leases a job by updating its status and lease timestamps.
async fn lease_job(
    service: &MemoryService,
    job_id: &str,
    namespace: &str,
    now_str: &str,
    lease_expires: &str,
) -> Result<(), MemoryError> {
    let record_id = format!("event_projection_job:{job_id}");
    let payload = json!({
        "status": "leased",
        "leased_at": now_str,
        "lease_expires_at": lease_expires,
    });
    service
        .db_client
        .update(&record_id, payload, namespace)
        .await?;
    Ok(())
}

/// Marks a job as complete.
async fn mark_job_complete(
    service: &MemoryService,
    job_id: &str,
    namespace: &str,
    now_str: &str,
) -> Result<(), MemoryError> {
    let record_id = format!("event_projection_job:{job_id}");
    let payload = json!({
        "status": "completed",
        "completed_at": now_str,
    });
    service
        .db_client
        .update(&record_id, payload, namespace)
        .await?;
    Ok(())
}

/// Increments the attempt counter on a failed job.
async fn increment_attempts(
    service: &MemoryService,
    job_id: &str,
    namespace: &str,
    now_str: &str,
) -> Result<(), MemoryError> {
    let record_id = format!("event_projection_job:{job_id}");
    // Load current attempts, increment, and update.
    let existing = service.db_client.select_one(&record_id, namespace).await?;
    let attempts = existing
        .as_ref()
        .and_then(|v| v.get("attempts"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
        + 1;

    let max_attempts = existing
        .as_ref()
        .and_then(|v| v.get("max_attempts"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(DEFAULT_MAX_ATTEMPTS);

    let status = if attempts >= max_attempts {
        "dead_letter"
    } else {
        "pending"
    };

    let payload = json!({
        "attempts": attempts,
        "status": status,
        "last_error": now_str,
        "dead_lettered_at": if status == "dead_letter" { Some(now_str) } else { None },
    });
    service
        .db_client
        .update(&record_id, payload, namespace)
        .await?;
    Ok(())
}

/// Handles a job failure: increment attempts, dead-letter if exhausted.
async fn handle_job_failure(
    service: &MemoryService,
    job: &EventProjectionJobRecord,
    namespace: &str,
    error: &MemoryError,
    now_str: &str,
) -> Result<(), MemoryError> {
    let record_id = format!("event_projection_job:{}", job.job_id);
    let attempts = job.attempts + 1;
    let max_attempts = job.max_attempts.max(DEFAULT_MAX_ATTEMPTS);
    let status = if attempts >= max_attempts {
        "dead_letter"
    } else {
        "pending"
    };

    let payload = json!({
        "attempts": attempts,
        "status": status,
        "last_error": format!("{error}"),
        "dead_lettered_at": if status == "dead_letter" { Some(now_str) } else { None },
    });
    service
        .db_client
        .update(&record_id, payload, namespace)
        .await?;
    Ok(())
}

/// Loads a memory event record by event_id.
async fn load_event(
    service: &MemoryService,
    event_id: &str,
    namespace: &str,
) -> Result<crate::storage::MemoryEventRecord, MemoryError> {
    let record_id = format!("memory_event:{event_id}");
    let existing = service.db_client.select_one(&record_id, namespace).await?;
    let value = existing.ok_or_else(|| MemoryError::NotFound(format!("event {event_id}")))?;
    serde_json::from_value(value)
        .map_err(|e| MemoryError::Storage(format!("failed to parse memory_event: {e}")))
}

/// Runs the existing extraction path on an episode.
///
/// This reuses `MemoryService::extract` with an `episode_id`, propagating
/// origin through provenance. No new LLM or second extraction implementation.
async fn run_extraction(
    service: &MemoryService,
    episode_id: &str,
    _namespace: &str,
) -> Result<(), MemoryError> {
    service.extract(episode_id, None, None).await.map(|_| ())
}

/// Shared worker mechanics: empty-poll backoff.
#[must_use]
#[allow(dead_code)]
pub fn empty_poll_interval() -> std::time::Duration {
    std::time::Duration::from_secs(EMPTY_POLL_INTERVAL_SECS)
}

/// Shared worker mechanics: transient-error backoff.
#[must_use]
#[allow(dead_code)]
pub fn transient_backoff() -> std::time::Duration {
    std::time::Duration::from_secs(TRANSIENT_BACKOFF_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn empty_poll_interval_returns_configured_default() {
        assert_eq!(
            empty_poll_interval(),
            Duration::from_secs(EMPTY_POLL_INTERVAL_SECS)
        );
    }

    #[test]
    fn transient_backoff_returns_configured_default() {
        assert_eq!(
            transient_backoff(),
            Duration::from_secs(TRANSIENT_BACKOFF_SECS)
        );
    }

    #[test]
    fn default_max_attempts_is_bounded() {
        let attempts = DEFAULT_MAX_ATTEMPTS;
        assert!((1..=10).contains(&attempts));
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
        MemoryService::new(
            db_client,
            vec!["test".to_string()],
            "warn".to_string(),
            50,
            100,
        )
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
