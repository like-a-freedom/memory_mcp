//! Narrow claim storage capability over SurrealDB.
#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::DbClient;
use crate::models::claim::{Claim, ClaimJob, ClaimRelation, ExtractorFingerprint};
use crate::models::{EpisodeId, FactId};
use crate::service::MemoryError;

// ─── ClaimStore Trait ─────────────────────────────────────────────────────────

/// Narrow storage capability for the claim reconciliation pipeline.
#[async_trait]
#[allow(dead_code)]
pub(crate) trait ClaimStore: Send + Sync {
    async fn load_projection_source(
        &self,
        namespace: &str,
        fact_id: &FactId,
    ) -> Result<Option<ClaimProjectionSource>, MemoryError>;

    async fn ensure_projection_job(&self, job: &ClaimJob) -> Result<(), MemoryError>;

    async fn load_job(
        &self,
        namespace: &str,
        job_id: &crate::models::ClaimJobId,
    ) -> Result<Option<ClaimJob>, MemoryError>;

    async fn lease_next_job(
        &self,
        request: LeaseJobRequest<'_>,
    ) -> Result<Option<ClaimJob>, MemoryError>;

    async fn persist_projection(
        &self,
        request: PersistProjectionRequest<'_>,
    ) -> Result<(), MemoryError>;

    async fn select_candidates_page(
        &self,
        query: ClaimCandidateQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError>;

    async fn commit_relation(&self, request: CommitRelationRequest<'_>) -> Result<(), MemoryError>;

    async fn select_claims_for_facts(
        &self,
        query: ClaimsForFactsQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError>;

    async fn select_relations_for_facts(
        &self,
        query: RelationsForFactsQuery<'_>,
    ) -> Result<Vec<ClaimRelation>, MemoryError>;

    async fn select_source_evidence(
        &self,
        query: SourceEvidenceQuery<'_>,
    ) -> Result<Vec<SourceEvidenceRecord>, MemoryError>;

    async fn count_active_relations(
        &self,
        namespace: &str,
    ) -> Result<Vec<ActiveRelationCount>, MemoryError>;

    async fn select_facts_for_backfill(
        &self,
        query: BackfillFactQuery<'_>,
    ) -> Result<Vec<serde_json::Value>, MemoryError>;

    async fn retract_fact_and_claims(
        &self,
        request: RetractFactAndClaimsRequest<'_>,
    ) -> Result<(), MemoryError>;

    async fn upsert_compiled_policies(
        &self,
        namespace: &str,
        policies: &[ClaimPolicyRecord],
    ) -> Result<(), MemoryError>;
}

// ─── Request/Response Types ───────────────────────────────────────────────────

/// A fact source record for projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ClaimProjectionSource {
    pub fact_id: String,
    pub content: String,
    pub t_ref: String,
    pub scope: String,
    pub project: Option<String>,
    pub policy_tags: Vec<String>,
}

/// Lease request for the next pending job.
pub(crate) struct LeaseJobRequest<'a> {
    pub namespace: &'a str,
    pub lease_owner: &'a str,
    pub lease_duration: std::time::Duration,
}

/// Persist projection output (claims + jobs).
pub(crate) struct PersistProjectionRequest<'a> {
    pub namespace: &'a str,
    pub fact_id: &'a FactId,
    pub episode_id: &'a EpisodeId,
    pub scope: &'a str,
    pub project: Option<&'a str>,
    pub policy_tags: &'a [String],
    pub extractor_fingerprint: &'a ExtractorFingerprint,
    pub t_ingested: chrono::DateTime<chrono::Utc>,
    pub claims: Vec<Claim>,
    pub jobs: Vec<ClaimJob>,
}

/// Query for candidate claims in a slot.
pub(crate) struct ClaimCandidateQuery<'a> {
    pub namespace: &'a str,
    pub slot_fingerprint: &'a str,
    pub after_claim_id: Option<&'a crate::models::ClaimId>,
    pub limit: usize,
}

/// Commit a relation between two claims.
pub(crate) struct CommitRelationRequest<'a> {
    pub namespace: &'a str,
    pub relation: ClaimRelation,
    pub lifecycle_mutation: Option<&'a str>,
}

/// Query for claims belonging to specific facts.
pub(crate) struct ClaimsForFactsQuery<'a> {
    pub namespace: &'a str,
    pub fact_ids: &'a [FactId],
}

/// Query for relations involving specific facts.
pub(crate) struct RelationsForFactsQuery<'a> {
    pub namespace: &'a str,
    pub fact_ids: &'a [FactId],
}

/// Query for source evidence of a fact's claims.
pub(crate) struct SourceEvidenceQuery<'a> {
    pub namespace: &'a str,
    pub fact_id: &'a FactId,
}

/// A source evidence record for citation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SourceEvidenceRecord {
    pub claim_id: String,
    pub source_episode_id: String,
    pub source_lineage: Option<String>,
    pub content: Option<String>,
}

/// Count of active relations per namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActiveRelationCount {
    pub namespace: String,
    pub count: i64,
}

/// Query for facts eligible for backfill.
pub(crate) struct BackfillFactQuery<'a> {
    pub namespace: &'a str,
    pub after_fact_id: Option<&'a FactId>,
    pub limit: usize,
}

/// Request to retract a fact and all its claims/relations.
pub(crate) struct RetractFactAndClaimsRequest<'a> {
    pub namespace: &'a str,
    pub fact_id: &'a FactId,
    pub retract_reason: &'a str,
}

/// A compiled policy record for storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClaimPolicyRecord {
    pub policy_id: String,
    pub schema_family: String,
    pub schema_version: u16,
    pub policy_fingerprint: String,
    pub definition: serde_json::Value,
}

// ─── SurrealClaimStore ────────────────────────────────────────────────────────

/// SurrealDB-backed claim store.
pub(crate) struct SurrealClaimStore {
    db: Arc<dyn DbClient>,
}

impl SurrealClaimStore {
    pub fn new(db: Arc<dyn DbClient>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ClaimStore for SurrealClaimStore {
    async fn load_projection_source(
        &self,
        _namespace: &str,
        _fact_id: &FactId,
    ) -> Result<Option<ClaimProjectionSource>, MemoryError> {
        // TODO: Implement in a follow-up when wire-up is needed
        Ok(None)
    }

    async fn ensure_projection_job(&self, _job: &ClaimJob) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn load_job(
        &self,
        _namespace: &str,
        _job_id: &crate::models::ClaimJobId,
    ) -> Result<Option<ClaimJob>, MemoryError> {
        Ok(None)
    }

    async fn lease_next_job(
        &self,
        _request: LeaseJobRequest<'_>,
    ) -> Result<Option<ClaimJob>, MemoryError> {
        Ok(None)
    }

    async fn persist_projection(
        &self,
        _request: PersistProjectionRequest<'_>,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn select_candidates_page(
        &self,
        _query: ClaimCandidateQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError> {
        Ok(vec![])
    }

    async fn commit_relation(
        &self,
        _request: CommitRelationRequest<'_>,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn select_claims_for_facts(
        &self,
        _query: ClaimsForFactsQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError> {
        Ok(vec![])
    }

    async fn select_relations_for_facts(
        &self,
        _query: RelationsForFactsQuery<'_>,
    ) -> Result<Vec<ClaimRelation>, MemoryError> {
        Ok(vec![])
    }

    async fn select_source_evidence(
        &self,
        _query: SourceEvidenceQuery<'_>,
    ) -> Result<Vec<SourceEvidenceRecord>, MemoryError> {
        Ok(vec![])
    }

    async fn count_active_relations(
        &self,
        _namespace: &str,
    ) -> Result<Vec<ActiveRelationCount>, MemoryError> {
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

    async fn upsert_compiled_policies(
        &self,
        _namespace: &str,
        _policies: &[ClaimPolicyRecord],
    ) -> Result<(), MemoryError> {
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_027_is_last_registered() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let last = migrations.last().unwrap();
        assert_eq!(last.file_name, "027_claim_reconciliation.surql");
    }

    #[test]
    fn migration_027_is_registered_once() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let count = migrations
            .iter()
            .filter(|m| m.file_name == "027_claim_reconciliation.surql")
            .count();
        assert_eq!(count, 1, "027 should be registered exactly once");
    }

    #[test]
    fn migration_027_defines_all_five_tables() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let m027 = migrations
            .iter()
            .find(|m| m.file_name == "027_claim_reconciliation.surql")
            .expect("027 not found");
        let sql = m027.sql;
        assert!(sql.contains("DEFINE TABLE claim SCHEMAFULL"));
        assert!(sql.contains("DEFINE TABLE claim_relation SCHEMAFULL"));
        assert!(sql.contains("DEFINE TABLE claim_job SCHEMAFULL"));
        assert!(sql.contains("DEFINE TABLE claim_key_alias SCHEMAFULL"));
        assert!(sql.contains("DEFINE TABLE claim_policy SCHEMAFULL"));
    }

    #[test]
    fn migration_027_defines_expected_indexes() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let m027 = migrations
            .iter()
            .find(|m| m.file_name == "027_claim_reconciliation.surql")
            .expect("027 not found");
        let sql = m027.sql;
        assert!(sql.contains("claim_slot_cursor_idx"));
        assert!(sql.contains("claim_source_projection_idx"));
        assert!(sql.contains("claim_relation_left_active_idx"));
        assert!(sql.contains("claim_relation_right_active_idx"));
        assert!(sql.contains("claim_relation_context_idx"));
        assert!(sql.contains("claim_job_lease_idx"));
        assert!(sql.contains("claim_job_fact_idx"));
        assert!(sql.contains("claim_alias_lookup_idx"));
        assert!(sql.contains("claim_policy_lookup_idx"));
        assert!(sql.contains("fact_claim_backfill_cursor_idx"));
    }

    #[test]
    fn migration_027_adds_invalidation_reason_to_fact() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let m027 = migrations
            .iter()
            .find(|m| m.file_name == "027_claim_reconciliation.surql")
            .expect("027 not found");
        assert!(
            m027.sql
                .contains("DEFINE FIELD invalidation_reason ON fact")
        );
    }

    #[tokio::test]
    async fn surreal_claim_store_implements_trait() {
        // Verify the trait is object-safe and can be used as dyn
        let _check: Option<&dyn ClaimStore> = None;
    }
}
