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

    let store = crate::storage::AgentMemoryStore::new(
        service.db_client.clone(),
        service.active_namespace.clone(),
    );
    let pending_jobs = store
        .load_pending_jobs(&now_str, PROJECTION_BATCH_LIMIT)
        .await?;

    for job in pending_jobs {
        if let Err(e) = process_one_job(service, &store, &job, &now_str, &lease_expires).await {
            handle_job_failure(&store, &job, &e, &now_str).await?;
        } else {
            projected += 1;
        }
    }

    Ok(projected)
}

/// Processes one projection job: lease it, run projection, mark complete.
async fn process_one_job(
    service: &MemoryService,
    store: &crate::storage::AgentMemoryStore,
    job: &EventProjectionJobRecord,
    now_str: &str,
    lease_expires: &str,
) -> Result<(), MemoryError> {
    // Lease the job.
    lease_job(store, &job.job_id, now_str, lease_expires).await?;

    // Load the event to verify disposition and fingerprint.
    let event = load_event(store, &job.event_id).await?;

    // Only accepted events with an episode reference are projected.
    if event.disposition != "accepted" {
        mark_job_complete(store, &job.job_id, now_str).await?;
        return Ok(());
    }

    let Some(episode_id) = &event.episode_id else {
        mark_job_complete(store, &job.job_id, now_str).await?;
        return Ok(());
    };

    // Reuse the existing extraction path to project facts from the episode.
    // The extraction path propagates origin through provenance.
    if let Err(e) = run_extraction(service, episode_id).await {
        // Increment attempts and re-raise for failure handling.
        increment_attempts(store, &job.job_id, now_str).await?;
        return Err(e);
    }

    mark_job_complete(store, &job.job_id, now_str).await?;
    Ok(())
}

/// Leases a job by updating its status and lease timestamps.
async fn lease_job(
    store: &crate::storage::AgentMemoryStore,
    job_id: &str,
    now_str: &str,
    lease_expires: &str,
) -> Result<(), MemoryError> {
    let payload = json!({
        "status": "leased",
        "leased_at": now_str,
        "lease_expires_at": lease_expires,
    });
    store.update_job(job_id, payload).await
}

/// Marks a job as complete.
async fn mark_job_complete(
    store: &crate::storage::AgentMemoryStore,
    job_id: &str,
    now_str: &str,
) -> Result<(), MemoryError> {
    let payload = json!({
        "status": "completed",
        "completed_at": now_str,
    });
    store.update_job(job_id, payload).await
}

/// Increments the attempt counter on a failed job.
async fn increment_attempts(
    store: &crate::storage::AgentMemoryStore,
    job_id: &str,
    now_str: &str,
) -> Result<(), MemoryError> {
    // Load current attempts, increment, and update through the bound store.
    let existing = store.load_job(job_id).await?;
    let attempts = existing.as_ref().map_or(0, |job| job.attempts) + 1;
    let max_attempts = existing
        .as_ref()
        .map_or(durable_work::DEFAULT_MAX_ATTEMPTS, |job| job.max_attempts);
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
    store.update_job(job_id, payload).await
}

/// Handles a job failure: increment attempts, dead-letter if exhausted.
async fn handle_job_failure(
    store: &crate::storage::AgentMemoryStore,
    job: &EventProjectionJobRecord,
    error: &MemoryError,
    now_str: &str,
) -> Result<(), MemoryError> {
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
    store.update_job(&job.job_id, payload).await
}

/// Loads a memory event record by event_id.
async fn load_event(
    store: &crate::storage::AgentMemoryStore,
    event_id: &str,
) -> Result<crate::storage::MemoryEventRecord, MemoryError> {
    let existing = store.load_event(event_id).await?;
    existing.ok_or_else(|| MemoryError::NotFound(format!("event {event_id}")))
}

/// Runs the existing extraction path on an episode.
///
/// This reuses the `ExtractCapability` with an `episode_id`, propagating
/// origin through provenance. No new LLM or second extraction implementation.
async fn run_extraction(service: &MemoryService, episode_id: &str) -> Result<(), MemoryError> {
    crate::service::capabilities::extract::ExtractCapability::extract(
        &service.build_context(),
        episode_id,
        None,
        None,
    )
    .await
    .map(|_| ())
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
