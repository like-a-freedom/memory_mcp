//! Claim projection orchestration after fact persistence.
#![allow(dead_code)]

use std::sync::Arc;

use crate::config::claims::ClaimConfig;
use crate::models::FactId;
use crate::models::claim::ExtractorFingerprint;
use crate::service::MemoryError;

use super::schema::ClaimSchemaRegistry;

/// Projection summary for a single fact.
#[derive(Debug, Clone)]
pub(crate) struct FactProjectionSummary {
    pub fact_id: FactId,
    pub claims_projected: usize,
    pub claims_skipped: usize,
    pub jobs_created: usize,
}

/// Orchestration facade for claim extraction and reconciliation.
#[derive(Clone)]
pub(crate) struct ClaimService {
    pub(crate) registry: Arc<ClaimSchemaRegistry>,
    pub(crate) config: ClaimConfig,
}

impl ClaimService {
    /// Create a new ClaimService with the built-in registry and default config.
    pub fn new() -> Self {
        let fingerprint = ExtractorFingerprint::compute(1, "builtin");
        let registry = Arc::new(ClaimSchemaRegistry::built_in(fingerprint));
        Self {
            registry,
            config: ClaimConfig::default(),
        }
    }

    /// Create with a custom config.
    pub fn with_config(self, config: ClaimConfig) -> Self {
        Self { config, ..self }
    }

    /// Whether claim extraction is enabled (not Disabled or Shadow).
    pub fn is_enabled(&self) -> bool {
        !matches!(
            self.config.rollout_stage,
            crate::config::claims::ClaimRolloutStage::Disabled
                | crate::config::claims::ClaimRolloutStage::Shadow
        )
    }

    /// Called after a fact is persisted. Schedules extraction.
    /// In the current stub, this is a no-op that always succeeds.
    pub async fn after_fact_persisted(
        &self,
        _namespace: &str,
        _fact_id: &FactId,
    ) -> Result<FactProjectionSummary, MemoryError> {
        // TODO: Full implementation in follow-up when ClaimStore is wired
        Ok(FactProjectionSummary {
            fact_id: FactId::from("stub"),
            claims_projected: 0,
            claims_skipped: 0,
            jobs_created: 0,
        })
    }

    /// Record a non-fatal failure from post-fact projection.
    pub fn record_post_fact_failure(&self, namespace: &str, fact_id: &FactId, error: &MemoryError) {
        eprintln!(
            "[claim] projection failed after fact persistence (non-fatal): namespace={namespace} fact_id={fact_id} error={error}"
        );
    }

    /// The extractor fingerprint used by this service.
    pub fn extractor_fingerprint(&self) -> &ExtractorFingerprint {
        self.registry.extractor_fingerprint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_service_is_enabled_for_evidence_stage() {
        let svc = ClaimService::new();
        assert!(svc.is_enabled());
    }

    #[test]
    fn claim_service_disabled_for_disabled_stage() {
        let svc = ClaimService::new().with_config(ClaimConfig {
            rollout_stage: crate::config::claims::ClaimRolloutStage::Disabled,
            ..Default::default()
        });
        assert!(!svc.is_enabled());
    }

    #[test]
    fn claim_service_disabled_for_shadow_stage() {
        let svc = ClaimService::new().with_config(ClaimConfig {
            rollout_stage: crate::config::claims::ClaimRolloutStage::Shadow,
            ..Default::default()
        });
        assert!(!svc.is_enabled());
    }

    #[tokio::test]
    async fn after_fact_persisted_returns_summary() {
        let svc = ClaimService::new();
        let fact_id = FactId::from("fact:test1");
        let result = svc.after_fact_persisted("ns", &fact_id).await;
        assert!(result.is_ok());
    }
}
