//! Claim-specific leased reconciliation worker.
//!
//! Cancellation-aware worker that leases pending jobs, processes
//! exact-slot candidate pages, and commits relations atomically.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::models::claim::{ClaimJob, ClaimJobKind, ClaimJobState};
use crate::service::MemoryError;
use crate::storage::claims::{
    ClaimCandidateQuery, CommitReconciliationPageRequest, JobCounters, LeaseJobRequest,
};

use super::projection::ClaimService;

/// Bounded worker runtime for claim reconciliation.
#[derive(Clone)]
pub(crate) struct ClaimWorkerRuntime {
    shutdown: CancellationToken,
    handles: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl ClaimWorkerRuntime {
    pub(crate) fn new() -> Self {
        Self {
            shutdown: CancellationToken::new(),
            handles: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub(crate) async fn spawn_worker(&self, claim_service: ClaimService, worker_id: String) {
        let shutdown = self.shutdown.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    outcome = run_next_leased_job(&claim_service, &worker_id) => {
                        match outcome {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(e) => {
                                eprintln!("[claim] worker error: {e}");
                            }
                        }
                    }
                }
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
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

/// Lease and run one job. Returns true if work was done.
pub(crate) async fn run_next_leased_job(
    claim_service: &ClaimService,
    worker_id: &str,
) -> Result<bool, MemoryError> {
    let lease_request = LeaseJobRequest {
        lease_owner: worker_id,
        lease_duration: std::time::Duration::from_secs(30),
    };
    let job = claim_service.store.lease_next_job(lease_request).await?;
    match job {
        Some(job) => {
            process_job(claim_service, &job).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

async fn process_job(claim_service: &ClaimService, job: &ClaimJob) -> Result<(), MemoryError> {
    match job.kind {
        ClaimJobKind::Reconcile => reconcile_page(claim_service, job).await,
        ClaimJobKind::Backfill => {
            let last = super::backfill::run_backfill_page(
                claim_service,
                &job.namespace,
                job.cursor
                    .as_ref()
                    .map(|c| crate::models::FactId::from(c.as_str()))
                    .as_ref(),
                100,
            )
            .await?;
            let _ = last;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Run inline reconciliation for a single claim. Used by `ClaimService::after_fact_persisted`
/// to make relations visible synchronously after `add_fact` returns.
pub(crate) async fn reconcile_claim_inline(
    claim_service: &ClaimService,
    namespace: &str,
    source_fact_id: &crate::models::FactId,
    claim_id: &crate::models::ClaimId,
) -> Result<(), MemoryError> {
    // Look up the owning claim by its source fact so we can read its
    // `slot_fingerprint` and find matching candidates.
    let claims = claim_service
        .store
        .select_claims_for_facts(crate::storage::claims::ClaimsForFactsQuery {
            fact_ids: std::slice::from_ref(source_fact_id),
        })
        .await?;
    let owning = claims
        .into_iter()
        .find(|c| &c.claim_id == claim_id)
        .ok_or_else(|| {
            MemoryError::Storage(format!("claim not found for inline reconcile: {claim_id}"))
        })?;
    let job = ClaimJob {
        job_id: crate::models::ClaimJobId::from_raw(format!(
            "claim_job:reconcile:inline:{}",
            claim_id
        )),
        kind: ClaimJobKind::Reconcile,
        namespace: namespace.to_string(),
        source_fact_id: Some(source_fact_id.clone()),
        claim_id: Some(claim_id.clone()),
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
    };
    reconcile_page_with_owning(claim_service, &job, owning).await
}

async fn reconcile_page(claim_service: &ClaimService, job: &ClaimJob) -> Result<(), MemoryError> {
    let claim_id = match &job.claim_id {
        Some(id) => id.clone(),
        None => return Ok(()),
    };

    // Load the owning claim by fetching the full claim record. The candidate
    // query takes a `slot_fingerprint`, not a `claim_id`; we look up by the
    // owning claim's source fact so we can read its slot fingerprint.
    let source_fact_id = match &job.source_fact_id {
        Some(id) => id.clone(),
        None => return Ok(()),
    };
    let owning_claims = claim_service
        .store
        .select_claims_for_facts(crate::storage::claims::ClaimsForFactsQuery {
            fact_ids: std::slice::from_ref(&source_fact_id),
        })
        .await?;
    let owning = match owning_claims.into_iter().find(|c| c.claim_id == claim_id) {
        Some(c) => c,
        None => return Ok(()),
    };

    reconcile_page_with_owning(claim_service, job, owning).await
}

async fn reconcile_page_with_owning(
    claim_service: &ClaimService,
    job: &ClaimJob,
    owning: crate::models::claim::Claim,
) -> Result<(), MemoryError> {
    let page_start = std::time::Instant::now();
    let slot_fp = owning.slot_fingerprint.clone();

    let candidates = claim_service
        .store
        .select_candidates_page(ClaimCandidateQuery {
            slot_fingerprint: &slot_fp,
            identity_version: owning.identity_version,
            after_claim_id: None,
            limit: claim_service.config.candidate_page_size,
        })
        .await?;

    if candidates.is_empty() {
        let request = CommitReconciliationPageRequest {
            job_id: &job.job_id,
            expected_lease_owner: "",
            relations: &[],
            next_cursor: None,
            completed: true,
            counters: JobCounters {
                processed: 0,
                succeeded: 0,
                skipped: 0,
                failed: 0,
            },
        };
        claim_service
            .store
            .commit_reconciliation_page(request)
            .await?;
        super::telemetry::record_pipeline_duration(
            super::telemetry::ClaimMetricStage::Reconcile,
            owning.schema_family,
            "skipped",
            page_start.elapsed(),
        );
        super::telemetry::record_candidate_count(
            owning.schema_family,
            super::telemetry::ClaimMatchMode::Exact,
            0,
        );
        return Ok(());
    }

    let mut relations = Vec::new();
    let mut processed: u64 = 0;
    let mut succeeded: u64 = 0;
    let mut skipped: u64 = 0;

    for candidate in &candidates {
        if candidate.claim_id == owning.claim_id {
            skipped += 1;
            continue;
        }
        processed += 1;

        let ev = super::reconcile::EvaluatorVersion::new("builtin/v1");
        let cf = crate::models::claim::ReconciliationContextFingerprint::compute(
            ev.as_str(),
            "attribute",
            "",
            "",
        );
        // Look up the schema policy for the owning claim's comparison key.
        // This invokes ClaimSchema::policy which may override cardinality
        // for specific comparison keys (e.g. force set-valued for certain
        // attribute predicates).
        let schema_ref =
            crate::models::claim::ClaimSchemaRef::new(owning.schema_family, owning.schema_version);
        let schema_policy = claim_service
            .registry
            .policy_for(&schema_ref, &owning.comparison_key);
        let input = super::reconcile::ReconciliationInput {
            left: &owning,
            right: candidate,
            policy: &schema_policy,
            confirmed_aliases: &super::reconcile::ConfirmedAliasSet::new(
                std::collections::BTreeMap::new(),
            ),
            evaluator_version: &ev,
            context_fingerprint: &cf,
            evaluated_at: chrono::Utc::now(),
        };
        let decision = super::reconcile::reconcile(&input);
        match decision {
            super::reconcile::ReconciliationDecision::Persist(draft) => {
                let rid =
                    crate::models::claim::relation_id(&draft.left_claim_id, &draft.right_claim_id);
                relations.push(crate::models::claim::ClaimRelation {
                    claim_relation_id: rid,
                    left_claim_id: draft.left_claim_id.clone(),
                    right_claim_id: draft.right_claim_id.clone(),
                    pair_fingerprint: format!("{}:{}", draft.left_claim_id, draft.right_claim_id),
                    outcome: draft.outcome,
                    predecessor_claim_id: draft.predecessor_claim_id,
                    successor_claim_id: draft.successor_claim_id,
                    reason_code: draft.reason_code.to_string(),
                    evidence: crate::models::claim::ClaimRelationEvidence {
                        reason_code: draft.reason_code.to_string(),
                        description: draft.evidence.description,
                    },
                    evaluator_version: draft.evaluator_version,
                    context_fingerprint: draft.context_fingerprint,
                    evaluated_at: draft.evaluated_at,
                    supersedes_relation_id: None,
                    scope: owning.scope.clone(),
                    project: owning.project.clone(),
                    // Relations are evaluated within one slot, whose v2 identity
                    // already includes the active policy tags. Preserve those
                    // tags as persisted evidence instead of reconstructing them
                    // from legacy partition fields during evaluation.
                    policy_tags: owning.policy_tags.clone(),
                    t_ingested: chrono::Utc::now(),
                    t_invalid_ingested: None,
                    schema_family: Some(owning.schema_family),
                    schema_version: Some(owning.schema_version),
                    left_fact_id: Some(owning.source_fact_id.clone()),
                    right_fact_id: Some(candidate.source_fact_id.clone()),
                });
                succeeded += 1;
            }
            super::reconcile::ReconciliationDecision::Skip
            | super::reconcile::ReconciliationDecision::Coexist => {
                skipped += 1;
            }
        }
    }

    let last_id = candidates.last().map(|c| c.claim_id.clone());
    let request = CommitReconciliationPageRequest {
        job_id: &job.job_id,
        expected_lease_owner: "",
        relations: &relations,
        next_cursor: last_id.as_ref(),
        completed: false,
        counters: JobCounters {
            processed,
            succeeded,
            skipped,
            failed: 0,
        },
    };
    claim_service
        .store
        .commit_reconciliation_page(request)
        .await?;

    // Reconciliation metrics (no-op without a recorder).
    let page_outcome = if relations.is_empty() {
        "skipped"
    } else {
        "persisted"
    };
    super::telemetry::record_pipeline_duration(
        super::telemetry::ClaimMetricStage::Reconcile,
        owning.schema_family,
        page_outcome,
        page_start.elapsed(),
    );
    super::telemetry::record_candidate_count(
        owning.schema_family,
        super::telemetry::ClaimMatchMode::Exact,
        candidates.len(),
    );
    for rel in &relations {
        let family = rel.schema_family.unwrap_or(owning.schema_family);
        super::telemetry::record_pipeline_event(
            super::telemetry::ClaimMetricStage::Reconcile,
            family,
            &rel.outcome.to_string(),
            rel.reason_code.as_str(),
        );
    }

    // Refresh the active-relations gauge now that the page's relations are
    // durable. Failures here surface as errors so they are visible rather
    // than silently skipped.
    let counts = claim_service.store.count_active_relations().await?;
    for c in counts {
        let family = match c.schema_family.as_deref() {
            Some("attribute") => Some(crate::models::claim::ClaimSchemaFamily::Attribute),
            Some("quantity") => Some(crate::models::claim::ClaimSchemaFamily::Quantity),
            Some("relation") => Some(crate::models::claim::ClaimSchemaFamily::Relation),
            Some("commitment") => Some(crate::models::claim::ClaimSchemaFamily::Commitment),
            _ => None,
        };
        if let (Some(family), Some(outcome)) = (family, c.outcome.as_deref()) {
            super::telemetry::set_active_relations(family, outcome, c.count as f64);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::super::projection::{ClaimService, FactPersistedParams};
    use crate::config::claims::{ClaimConfig, ClaimRolloutStage};
    use crate::models::claim::{Claim, ClaimJob, ClaimRelation, ClaimRelationOutcome};
    use crate::models::{EpisodeId, FactId};
    use crate::service::MemoryError;
    use crate::storage::claims::{
        ActiveRelationCount, BackfillFactQuery, ClaimCandidateQuery, ClaimStore,
        ClaimsForFactsQuery, CommitReconciliationPageRequest, LeaseJobRequest,
        PersistProjectionRequest, RelationsForFactsQuery, RetractFactAndClaimsRequest,
    };

    #[derive(Default)]
    struct RecordingClaimStore {
        claims: Mutex<Vec<Claim>>,
        relations: Mutex<Vec<ClaimRelation>>,
    }

    #[async_trait]
    impl ClaimStore for RecordingClaimStore {
        async fn ensure_projection_job(&self, _job: &ClaimJob) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn lease_next_job(
            &self,
            _request: LeaseJobRequest<'_>,
        ) -> Result<Option<ClaimJob>, MemoryError> {
            Ok(None)
        }

        async fn persist_projection(
            &self,
            request: PersistProjectionRequest,
        ) -> Result<(), MemoryError> {
            self.claims.lock().unwrap().extend(request.claims);
            Ok(())
        }

        async fn select_candidates_page(
            &self,
            query: ClaimCandidateQuery<'_>,
        ) -> Result<Vec<Claim>, MemoryError> {
            Ok(self
                .claims
                .lock()
                .unwrap()
                .iter()
                .filter(|claim| {
                    claim.slot_fingerprint == query.slot_fingerprint
                        && claim.identity_version == query.identity_version
                })
                .take(query.limit)
                .cloned()
                .collect())
        }

        async fn select_claims_for_facts(
            &self,
            query: ClaimsForFactsQuery<'_>,
        ) -> Result<Vec<Claim>, MemoryError> {
            Ok(self
                .claims
                .lock()
                .unwrap()
                .iter()
                .filter(|claim| query.fact_ids.contains(&claim.source_fact_id))
                .cloned()
                .collect())
        }

        async fn select_relations_for_facts(
            &self,
            _query: RelationsForFactsQuery<'_>,
        ) -> Result<Vec<ClaimRelation>, MemoryError> {
            Ok(vec![])
        }

        async fn count_active_relations(&self) -> Result<Vec<ActiveRelationCount>, MemoryError> {
            Ok(vec![])
        }

        async fn select_facts_for_backfill(
            &self,
            _query: BackfillFactQuery<'_>,
        ) -> Result<Vec<serde_json::Value>, MemoryError> {
            Ok(vec![])
        }

        async fn retract_fact_and_claims(
            &self,
            _request: RetractFactAndClaimsRequest<'_>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn commit_reconciliation_page(
            &self,
            request: CommitReconciliationPageRequest<'_>,
        ) -> Result<(), MemoryError> {
            self.relations
                .lock()
                .unwrap()
                .extend(request.relations.iter().cloned());
            Ok(())
        }
    }

    #[tokio::test]
    async fn inline_reconciliation_persists_owning_policy_tags() {
        let store = Arc::new(RecordingClaimStore::default());
        let service = ClaimService::new(store.clone()).with_config(ClaimConfig {
            rollout_stage: ClaimRolloutStage::Relations,
            ..Default::default()
        });
        let policy_tags = vec!["private".to_string(), "source:chat".to_string()];
        let first_fact_id = FactId::from("fact:worker-old");
        let second_fact_id = FactId::from("fact:worker-new");
        let first_episode_id = EpisodeId::from("episode:worker-old");
        let second_episode_id = EpisodeId::from("episode:worker-new");

        for (fact_id, episode_id) in [
            (&first_fact_id, &first_episode_id),
            (&second_fact_id, &second_episode_id),
        ] {
            service
                .after_fact_persisted(&FactPersistedParams {
                    namespace: "main",
                    fact_id,
                    source_episode_id: episode_id,
                    fact_type: "note",
                    content: "status is active",
                    policy_tags: &policy_tags,
                    entity_links: &[],
                    t_valid: chrono::Utc::now(),
                    source_lineage: Some(episode_id.as_ref()),
                })
                .await
                .expect("fact projection and inline reconciliation should succeed");
        }

        let relations = store.relations.lock().unwrap();
        assert_eq!(relations.len(), 1, "two same-slot claims should reconcile");
        assert_eq!(relations[0].outcome, ClaimRelationOutcome::Duplicate);
        assert_eq!(relations[0].policy_tags, policy_tags);
        let relation_fact_ids = [
            relations[0].left_fact_id.as_ref(),
            relations[0].right_fact_id.as_ref(),
        ];
        assert!(relation_fact_ids.contains(&Some(&first_fact_id)));
        assert!(relation_fact_ids.contains(&Some(&second_fact_id)));
    }
}
