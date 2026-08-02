//! Projection logic for accepted lifecycle events.
//!
//! This module contains the pure projection pass that leases pending
//! `event_projection_job` records, runs the existing extraction path on the
//! linked episode, and marks jobs complete or dead-letters them on retry
//! exhaustion. The worker runtime (`worker.rs`) calls `run_projection_pass`
//! on each poll tick.
//!
//! No new LLM or second extraction implementation lives here — the projection
//! reuses `MemoryService::extract` to propagate origin through provenance.

use serde_json::json;

use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::service::durable_work;
use crate::storage::EventProjectionJobRecord;

/// Maximum number of jobs leased per projection pass.
pub(crate) const PROJECTION_BATCH_LIMIT: i32 = 50;

/// Runs a single projection pass: lease pending jobs, project, complete.
///
/// Returns the number of jobs successfully projected in this pass.
pub async fn run_projection_pass(service: &MemoryService) -> Result<usize, MemoryError> {
    let mut projected = 0;
    let now_str = crate::service::normalize_dt(chrono::Utc::now());
    let lease_expires = crate::service::normalize_dt(
        chrono::Utc::now() + chrono::Duration::seconds(durable_work::DEFAULT_LEASE_SECS as i64),
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
        .unwrap_or(durable_work::DEFAULT_MAX_ATTEMPTS);

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
    let max_attempts = job.max_attempts.max(durable_work::DEFAULT_MAX_ATTEMPTS);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_attempts_is_bounded() {
        let attempts = durable_work::DEFAULT_MAX_ATTEMPTS;
        assert!((1..=10).contains(&attempts));
    }

    #[test]
    fn projection_batch_limit_is_bounded() {
        // PROJECTION_BATCH_LIMIT is a compile-time constant; verify its range
        // at a value level. This guards against accidental misconfiguration.
        let limit = PROJECTION_BATCH_LIMIT;
        assert!((1..=100).contains(&limit));
    }
}
