//! Historical fact backfill for claim projection.
//!
//! Discovers facts that lack the current extractor fingerprint
//! and schedules projection jobs for them.

#![allow(dead_code)]

use crate::models::claim::{ClaimJob, ClaimJobKind, ClaimJobState};
use crate::models::{ClaimJobId, FactId};
use crate::service::MemoryError;
use crate::storage::claims::BackfillFactQuery;

use super::project::ClaimService;

/// Ensure one deterministic backfill job exists per namespace.
pub(crate) async fn schedule_namespace_backfill(
    claim_service: &ClaimService,
    namespace: &str,
) -> Result<(), MemoryError> {
    let fingerprint = claim_service.registry.extractor_fingerprint();
    let job_id = ClaimJobId::from_raw(format!(
        "claim_job:backfill:{namespace}:{fingerprint}",
        namespace = namespace,
        fingerprint = fingerprint.as_str(),
    ));
    let job = ClaimJob {
        job_id: job_id.clone(),
        kind: ClaimJobKind::Backfill,
        namespace: namespace.to_string(),
        source_fact_id: None,
        claim_id: None,
        extractor_fingerprint: fingerprint.clone(),
        evaluator_fingerprint: None,
        status: ClaimJobState::Pending,
        cursor: None,
        lease_owner: None,
        lease_expires_at: None,
        processed: 0,
        succeeded: 0,
        skipped: 0,
        failed: 0,
        retry_count: 0,
        last_error: None,
        created_at: chrono::Utc::now(),
        started_at: None,
        updated_at: chrono::Utc::now(),
        completed_at: None,
    };
    claim_service.store.ensure_projection_job(&job).await?;
    Ok(())
}

/// Run one page of backfill: discover facts and schedule projections.
pub(crate) async fn run_backfill_page(
    claim_service: &ClaimService,
    namespace: &str,
    cursor: Option<&FactId>,
    limit: usize,
) -> Result<Option<FactId>, MemoryError> {
    let query = BackfillFactQuery {
        namespace,
        after_fact_id: cursor,
        limit,
    };
    let facts = claim_service.store.select_facts_for_backfill(query).await?;

    if facts.is_empty() {
        return Ok(None);
    }

    let mut last_id: Option<FactId> = None;

    for fact in &facts {
        let fact_id_str = fact.get("fact_id").and_then(|v| v.as_str()).unwrap_or("");
        if fact_id_str.is_empty() {
            continue;
        }
        let fact_id = FactId::from(fact_id_str);

        // Schedule a projection for this fact
        let _ = claim_service
            .store
            .ensure_projection_job(&ClaimJob {
                job_id: ClaimJobId::from_raw(format!(
                    "claim_job:project:{}:{}",
                    fact_id,
                    claim_service.registry.extractor_fingerprint().as_str(),
                )),
                kind: ClaimJobKind::Extract,
                namespace: namespace.to_string(),
                source_fact_id: Some(fact_id.clone()),
                claim_id: None,
                extractor_fingerprint: claim_service.registry.extractor_fingerprint().clone(),
                evaluator_fingerprint: None,
                status: ClaimJobState::Pending,
                cursor: None,
                lease_owner: None,
                lease_expires_at: None,
                processed: 0,
                succeeded: 0,
                skipped: 0,
                failed: 0,
                retry_count: 0,
                last_error: None,
                created_at: chrono::Utc::now(),
                started_at: None,
                updated_at: chrono::Utc::now(),
                completed_at: None,
            })
            .await;

        last_id = Some(fact_id);
    }

    Ok(last_id)
}
