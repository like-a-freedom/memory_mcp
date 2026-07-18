//! Claim projection orchestration after fact persistence.

use std::sync::Arc;

use crate::config::claims::ClaimConfig;
use crate::models::FactId;
use crate::models::claim::ExtractorFingerprint;
use crate::service::MemoryError;

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
    pub(crate) registry: Arc<ClaimSchemaRegistry>,
    pub(crate) config: ClaimConfig,
}

/// Parameters for `ClaimService::after_fact_persisted`.
pub(crate) struct FactPersistedParams<'a> {
    pub namespace: &'a str,
    pub fact_id: &'a FactId,
    pub _fact_type: &'a str,
    pub content: &'a str,
    pub scope: &'a str,
    pub project: Option<&'a str>,
    pub entity_links: &'a [String],
    pub t_valid: chrono::DateTime<chrono::Utc>,
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
    #[allow(dead_code)]
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

        // Build structured fields from entity links (simplified extraction)
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

    fn test_params<'a>(fact_id: &'a FactId, content: &'a str) -> FactPersistedParams<'a> {
        FactPersistedParams {
            namespace: "ns",
            fact_id,
            _fact_type: "general",
            content,
            scope: "personal",
            project: None,
            entity_links: &[],
            t_valid: chrono::Utc::now(),
        }
    }

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
        let params = test_params(&fact_id, "The height is 180 cm");
        let result = svc.after_fact_persisted(&params).await;
        assert!(result.is_ok());
        let summary = result.unwrap();
        assert_eq!(summary.fact_id, fact_id);
    }

    #[tokio::test]
    async fn after_fact_persisted_extracts_from_content() {
        let svc = ClaimService::new();
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
        let svc = ClaimService::new().with_config(ClaimConfig {
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
        let svc = ClaimService::new();
        let fact_id = FactId::from("fact:test4");
        let params = test_params(&fact_id, "");
        let result = svc.after_fact_persisted(&params).await;
        assert!(result.is_ok());
        let summary = result.unwrap();
        assert_eq!(summary.claims_projected, 0);
    }
}
