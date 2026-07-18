//! Claim projection orchestration after fact persistence.

use std::sync::Arc;

use crate::config::claims::ClaimConfig;
use crate::models::ClaimJobId;
use crate::models::FactId;
use crate::models::claim::{
    ClaimBuildInput, ClaimDraft, ClaimJob, ClaimJobKind, ClaimJobState, ClaimSlot,
    ComparisonKeyHash, ExtractorFingerprint, PolicyFingerprint, QualifierHash, build_claim,
};
use crate::service::MemoryError;
use crate::storage::claims::{ClaimStore, PersistProjectionRequest};

use super::extract::project_fact;
use super::schema::{ClaimProjectionInput, ClaimSchemaRegistry};

/// Projection summary for a single fact.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct FactProjectionSummary {
    pub fact_id: FactId,
    pub claims_projected: usize,
    pub claims_skipped: usize,
}

/// Orchestration facade for claim extraction and reconciliation.
#[derive(Clone)]
pub(crate) struct ClaimService {
    pub(crate) store: Arc<dyn ClaimStore>,
    pub(crate) registry: Arc<ClaimSchemaRegistry>,
    pub(crate) config: ClaimConfig,
}

/// Parameters for `ClaimService::after_fact_persisted`.
pub(crate) struct FactPersistedParams<'a> {
    pub namespace: &'a str,
    pub fact_id: &'a FactId,
    pub content: &'a str,
    pub scope: &'a str,
    pub project: Option<&'a str>,
    pub entity_links: &'a [String],
    pub t_valid: chrono::DateTime<chrono::Utc>,
}

impl ClaimService {
    /// Create a new ClaimService with the built-in registry and default config.
    pub fn new(store: Arc<dyn ClaimStore>) -> Self {
        let fingerprint = ExtractorFingerprint::compute(1, "builtin");
        let registry = Arc::new(ClaimSchemaRegistry::built_in(fingerprint));
        Self {
            store,
            registry,
            config: ClaimConfig::default(),
        }
    }

    /// Create with a custom config.
    #[allow(dead_code)]
    pub fn with_config(self, config: ClaimConfig) -> Self {
        Self {
            config,
            store: self.store,
            ..self
        }
    }

    /// Whether claim extraction is enabled (not Disabled or Shadow).
    pub fn is_enabled(&self) -> bool {
        !matches!(
            self.config.rollout_stage,
            crate::config::claims::ClaimRolloutStage::Disabled
                | crate::config::claims::ClaimRolloutStage::Shadow
        )
    }

    /// Called after a fact is persisted. Runs deterministic claim extraction.
    pub async fn after_fact_persisted(
        &self,
        params: &FactPersistedParams<'_>,
    ) -> Result<FactProjectionSummary, MemoryError> {
        if !self.is_enabled() {
            return Ok(FactProjectionSummary {
                fact_id: params.fact_id.clone(),
                claims_projected: 0,
                claims_skipped: 0,
            });
        }

        let structured_fields = std::collections::BTreeMap::new();

        let subject = params
            .entity_links
            .first()
            .map(|s| s.as_str())
            .unwrap_or("entity:unknown");

        let input = ClaimProjectionInput {
            namespace: params.namespace,
            source_fact_id: params.fact_id.clone(),
            source_episode_id: crate::models::EpisodeId::from("ep:inline"),
            scope: params.scope,
            project: params.project,
            policy_tags: &[],
            subject,
            t_ref: params.t_valid,
            content: params.content,
            structured_fields: &structured_fields,
        };

        let result = project_fact(&self.registry, &input)?;

        // Build persisted claims from drafts
        let t_ingested = crate::service::query::now();
        let fingerprint = self.registry.extractor_fingerprint();
        let mut claims = Vec::with_capacity(result.drafts.len());

        for draft in &result.drafts {
            let comparison_key_hash = ComparisonKeyHash::compute(&draft.comparison_key);
            let qualifier_hash = QualifierHash::compute(&draft.qualifiers);
            let access_policy_fingerprint =
                PolicyFingerprint::compute(params.scope, params.project, &[]);
            let project_identity = params.project.unwrap_or("__none__").to_string();

            let subject_slot = ClaimSlot {
                namespace: params.namespace.to_string(),
                scope: params.scope.to_string(),
                project_identity,
                access_policy_fingerprint: access_policy_fingerprint.clone(),
                schema_ref: draft.schema_ref,
                subject_key: draft.subject.clone(),
                comparison_key_hash: comparison_key_hash.clone(),
                qualifier_hash: qualifier_hash.clone(),
            };

            let claim_draft = ClaimDraft {
                schema_ref: draft.schema_ref,
                subject: subject_slot,
                comparison_key: draft.comparison_key.clone(),
                qualifiers: draft.qualifiers.clone(),
                value: draft.value.clone(),
                cardinality: draft.cardinality,
                observed_at: draft.observed_at,
                valid_from: draft.valid_from,
                valid_to: draft.valid_to,
                validity_source: draft.validity_source,
                source_lineage: draft.source_lineage.clone(),
            };

            let claim = build_claim(ClaimBuildInput {
                namespace: params.namespace,
                source_fact_id: params.fact_id,
                source_episode_id: &crate::models::EpisodeId::from("ep:inline"),
                scope: params.scope,
                project: params.project,
                policy_tags: &[],
                draft: claim_draft,
                extractor_fingerprint: fingerprint,
                t_ingested,
            })?;

            claims.push(claim);
        }

        // Create projection job
        let job_id = ClaimJobId::from_raw(format!(
            "claim_job:project:{}:{}",
            params.fact_id,
            fingerprint.as_str()
        ));
        let projection_job = ClaimJob {
            job_id,
            kind: ClaimJobKind::Extract,
            namespace: params.namespace.to_string(),
            source_fact_id: Some(params.fact_id.clone()),
            claim_id: None,
            extractor_fingerprint: fingerprint.clone(),
            evaluator_fingerprint: None,
            status: ClaimJobState::Completed,
            cursor: None,
            lease_owner: None,
            lease_expires_at: None,
            processed: claims.len() as u64,
            succeeded: claims.len() as u64,
            skipped: result.skips.len() as u64,
            failed: 0,
            retry_count: 0,
            last_error: None,
            created_at: t_ingested,
            started_at: Some(t_ingested),
            updated_at: t_ingested,
            completed_at: Some(t_ingested),
        };

        // Persist via store
        let persist_request = PersistProjectionRequest {
            namespace: params.namespace,
            fact_id: params.fact_id,
            episode_id: &crate::models::EpisodeId::from("ep:inline"),
            scope: params.scope,
            project: params.project,
            policy_tags: &[],
            extractor_fingerprint: fingerprint,
            t_ingested,
            claims,
            jobs: vec![projection_job],
        };

        self.store.persist_projection(persist_request).await?;

        Ok(FactProjectionSummary {
            fact_id: params.fact_id.clone(),
            claims_projected: result.drafts.len(),
            claims_skipped: result.skips.len(),
        })
    }

    /// Record a non-fatal failure from post-fact projection.
    pub fn record_post_fact_failure(&self, namespace: &str, fact_id: &FactId, error: &MemoryError) {
        eprintln!(
            "[claim] projection failed after fact persistence (non-fatal): namespace={namespace} fact_id={fact_id} error={error}"
        );
    }

    /// The extractor fingerprint used by this service.
    #[allow(dead_code)]
    pub fn extractor_fingerprint(&self) -> &ExtractorFingerprint {
        self.registry.extractor_fingerprint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::claims::PersistProjectionRequest;
    use async_trait::async_trait;

    struct NoopClaimStore;

    #[async_trait]
    impl ClaimStore for NoopClaimStore {
        async fn load_projection_source(
            &self,
            _ns: &str,
            _fid: &FactId,
        ) -> Result<Option<crate::storage::claims::ClaimProjectionSource>, MemoryError> {
            Ok(None)
        }
        async fn ensure_projection_job(&self, _job: &ClaimJob) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn load_job(
            &self,
            _ns: &str,
            _jid: &ClaimJobId,
        ) -> Result<Option<ClaimJob>, MemoryError> {
            Ok(None)
        }
        async fn lease_next_job(
            &self,
            _req: crate::storage::claims::LeaseJobRequest<'_>,
        ) -> Result<Option<ClaimJob>, MemoryError> {
            Ok(None)
        }
        async fn persist_projection(
            &self,
            _req: PersistProjectionRequest<'_>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn select_candidates_page(
            &self,
            _q: crate::storage::claims::ClaimCandidateQuery<'_>,
        ) -> Result<Vec<crate::models::claim::Claim>, MemoryError> {
            Ok(vec![])
        }
        async fn commit_relation(
            &self,
            _req: crate::storage::claims::CommitRelationRequest<'_>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn select_claims_for_facts(
            &self,
            _q: crate::storage::claims::ClaimsForFactsQuery<'_>,
        ) -> Result<Vec<crate::models::claim::Claim>, MemoryError> {
            Ok(vec![])
        }
        async fn select_relations_for_facts(
            &self,
            _q: crate::storage::claims::RelationsForFactsQuery<'_>,
        ) -> Result<Vec<crate::models::claim::ClaimRelation>, MemoryError> {
            Ok(vec![])
        }
        async fn select_source_evidence(
            &self,
            _q: crate::storage::claims::SourceEvidenceQuery<'_>,
        ) -> Result<Vec<crate::storage::claims::SourceEvidenceRecord>, MemoryError> {
            Ok(vec![])
        }
        async fn count_active_relations(
            &self,
            _ns: &str,
        ) -> Result<Vec<crate::storage::claims::ActiveRelationCount>, MemoryError> {
            Ok(vec![])
        }
        async fn select_facts_for_backfill(
            &self,
            _q: crate::storage::claims::BackfillFactQuery<'_>,
        ) -> Result<Vec<serde_json::Value>, MemoryError> {
            Ok(vec![])
        }
        async fn retract_fact_and_claims(
            &self,
            _req: crate::storage::claims::RetractFactAndClaimsRequest<'_>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn upsert_compiled_policies(
            &self,
            _ns: &str,
            _pols: &[crate::storage::claims::ClaimPolicyRecord],
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    fn noop_store() -> Arc<dyn ClaimStore> {
        Arc::new(NoopClaimStore)
    }

    fn claim_svc() -> ClaimService {
        ClaimService::new(noop_store())
    }

    fn test_params<'a>(fact_id: &'a FactId, content: &'a str) -> FactPersistedParams<'a> {
        FactPersistedParams {
            namespace: "ns",
            fact_id,
            content,
            scope: "personal",
            project: None,
            entity_links: &[],
            t_valid: chrono::Utc::now(),
        }
    }

    #[test]
    fn claim_service_is_enabled_for_evidence_stage() {
        let svc = claim_svc();
        assert!(svc.is_enabled());
    }

    #[test]
    fn claim_service_disabled_for_disabled_stage() {
        let svc = claim_svc().with_config(ClaimConfig {
            rollout_stage: crate::config::claims::ClaimRolloutStage::Disabled,
            ..Default::default()
        });
        assert!(!svc.is_enabled());
    }

    #[test]
    fn claim_service_disabled_for_shadow_stage() {
        let svc = claim_svc().with_config(ClaimConfig {
            rollout_stage: crate::config::claims::ClaimRolloutStage::Shadow,
            ..Default::default()
        });
        assert!(!svc.is_enabled());
    }

    #[tokio::test]
    async fn after_fact_persisted_returns_summary() {
        let svc = claim_svc();
        let fact_id = FactId::from("fact:test1");
        let params = test_params(&fact_id, "The height is 180 cm");
        let result = svc.after_fact_persisted(&params).await;
        assert!(result.is_ok());
        let summary = result.unwrap();
        assert_eq!(summary.fact_id, fact_id);
    }

    #[tokio::test]
    async fn after_fact_persisted_extracts_from_content() {
        let svc = claim_svc();
        let fact_id = FactId::from("fact:test2");
        let params = test_params(&fact_id, "Temperature is 36.5 celsius");
        let result = svc.after_fact_persisted(&params).await;
        assert!(result.is_ok());
        let summary = result.unwrap();
        assert!(
            summary.claims_projected > 0,
            "expected at least 1 projected claim"
        );
    }

    #[tokio::test]
    async fn after_fact_persisted_skips_when_disabled() {
        let svc = claim_svc().with_config(ClaimConfig {
            rollout_stage: crate::config::claims::ClaimRolloutStage::Disabled,
            ..Default::default()
        });
        let fact_id = FactId::from("fact:test3");
        let params = test_params(&fact_id, "The height is 180 cm");
        let result = svc.after_fact_persisted(&params).await;
        assert!(result.is_ok());
        let summary = result.unwrap();
        assert_eq!(summary.claims_projected, 0);
    }

    #[tokio::test]
    async fn after_fact_persisted_handles_empty_content() {
        let svc = claim_svc();
        let fact_id = FactId::from("fact:test4");
        let params = test_params(&fact_id, "");
        let result = svc.after_fact_persisted(&params).await;
        assert!(result.is_ok());
        let summary = result.unwrap();
        assert_eq!(summary.claims_projected, 0);
    }
}
