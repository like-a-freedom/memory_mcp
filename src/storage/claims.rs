//! Narrow claim storage capability over SurrealDB.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{self, Value};

use super::DbClient;
use crate::models::claim::{Claim, ClaimJob, ClaimRelation, ExtractorFingerprint};
use crate::models::{ClaimJobId, EpisodeId, FactId};
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

    /// Atomically commit relation versions and update job cursor.
    async fn commit_reconciliation_page(
        &self,
        request: CommitReconciliationPageRequest<'_>,
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
#[allow(dead_code)]
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
    #[allow(dead_code)]
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

/// Count of active relations grouped by schema family and outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActiveRelationCount {
    pub schema_family: Option<String>,
    pub outcome: Option<String>,
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

/// Job counters for reconciliation page commits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JobCounters {
    pub processed: u64,
    pub succeeded: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// Commit one reconciliation page with cursor update.
pub(crate) struct CommitReconciliationPageRequest<'a> {
    pub namespace: &'a str,
    pub job_id: &'a crate::models::ClaimJobId,
    pub expected_lease_owner: &'a str,
    pub relations: &'a [crate::models::claim::ClaimRelation],
    pub next_cursor: Option<&'a crate::models::ClaimId>,
    pub completed: bool,
    pub counters: JobCounters,
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

    fn upsert_one_sql(table: &str, record_id: &str) -> Result<String, MemoryError> {
        if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(MemoryError::Validation(format!(
                "invalid table name: {table}"
            )));
        }
        // `record_id` may arrive either as a bare id (`abc...`) or as the full
        // SurrealDB record id including the table prefix (`claim:abc...`).
        // Strip the prefix so we don't produce `claim:⟨claim:abc⟩`.
        let body = record_id
            .strip_prefix(&format!("{table}:"))
            .unwrap_or(record_id);
        let sql = format!("UPDATE {table}:⟨{body}⟩ CONTENT $content");
        Ok(sql)
    }

    fn serialize<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, MemoryError> {
        serde_json::to_value(value)
            .map_err(|e| MemoryError::Storage(format!("serialization failed: {e}")))
    }

    fn deserialize_vec(value: serde_json::Value) -> Vec<serde_json::Value> {
        match value {
            serde_json::Value::Array(arr) => arr,
            _ => vec![],
        }
    }

    fn extract_first(value: serde_json::Value) -> Option<serde_json::Value> {
        match value {
            serde_json::Value::Array(mut arr) if !arr.is_empty() => Some(arr.remove(0)),
            _ => None,
        }
    }
}

#[async_trait]
impl ClaimStore for SurrealClaimStore {
    async fn load_projection_source(
        &self,
        namespace: &str,
        fact_id: &FactId,
    ) -> Result<Option<ClaimProjectionSource>, MemoryError> {
        let sql = format!("SELECT * FROM fact:⟨{}⟩", fact_id.as_ref());
        let result = self.db.query(&sql, None, namespace).await?;
        match Self::extract_first(result) {
            Some(Value::Object(map)) => Ok(Some(ClaimProjectionSource {
                fact_id: map
                    .get("fact_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                content: map
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                t_ref: map
                    .get("t_valid")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                scope: map
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                project: map.get("project").and_then(Value::as_str).map(String::from),
                policy_tags: map
                    .get("policy_tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            })),
            _ => Ok(None),
        }
    }

    async fn ensure_projection_job(&self, job: &ClaimJob) -> Result<(), MemoryError> {
        let content = Self::serialize(job)?;
        let sql = Self::upsert_one_sql("claim_job", job.job_id.as_ref())?;
        let vars = serde_json::json!({"content": content});
        self.db.query(&sql, Some(vars), &job.namespace).await?;
        Ok(())
    }

    async fn load_job(
        &self,
        namespace: &str,
        job_id: &ClaimJobId,
    ) -> Result<Option<ClaimJob>, MemoryError> {
        let sql = format!("SELECT * FROM claim_job:⟨{}⟩", job_id.body());
        let result = self.db.query(&sql, None, namespace).await?;
        match Self::extract_first(result) {
            Some(v) => serde_json::from_value(v)
                .map(Some)
                .map_err(|e| MemoryError::Storage(format!("job deser: {e}"))),
            None => Ok(None),
        }
    }

    async fn lease_next_job(
        &self,
        request: LeaseJobRequest<'_>,
    ) -> Result<Option<ClaimJob>, MemoryError> {
        let expires = chrono::Utc::now() + request.lease_duration;
        // Atomically find the next pending/expired job and lease it
        let sql = "UPDATE claim_job SET status = 'leased', lease_owner = $owner, \
                   lease_expires_at = $expires, started_at = $now \
                   WHERE status = 'pending' \
                   AND (lease_expires_at IS NONE OR lease_expires_at < time::now()) \
                   ORDER BY job_id LIMIT 1 RETURN BEFORE";
        let vars = serde_json::json!({
            "owner": request.lease_owner,
            "expires": crate::service::normalize_dt(expires),
            "now": crate::service::normalize_dt(chrono::Utc::now()),
        });
        let result = self.db.query(sql, Some(vars), request.namespace).await?;
        match Self::extract_first(result) {
            Some(v) => serde_json::from_value(v)
                .map(Some)
                .map_err(|e| MemoryError::Storage(format!("job deser: {e}"))),
            None => Ok(None),
        }
    }

    async fn persist_projection(
        &self,
        request: PersistProjectionRequest<'_>,
    ) -> Result<(), MemoryError> {
        let namespace = request.namespace;
        for claim in &request.claims {
            let content = serde_json::to_value(claim)
                .map_err(|e| MemoryError::Storage(format!("serialize claim: {e}")))?;
            self.db
                .create(claim.claim_id.as_ref(), content, namespace)
                .await
                .map_err(|e| MemoryError::Storage(format!("persist claim: {e}")))?;
        }
        for job in &request.jobs {
            let content = serde_json::to_value(job)
                .map_err(|e| MemoryError::Storage(format!("serialize job: {e}")))?;
            self.db
                .create(job.job_id.as_ref(), content, namespace)
                .await
                .map_err(|e| MemoryError::Storage(format!("persist job: {e}")))?;
        }
        Ok(())
    }

    async fn select_candidates_page(
        &self,
        query: ClaimCandidateQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError> {
        if query.limit == 0 {
            return Ok(vec![]);
        }
        let sql = match query.after_claim_id {
            Some(_) => {
                "SELECT * FROM claim WHERE slot_fingerprint = $slot_fp AND claim_id > $after ORDER BY claim_id LIMIT $limit"
            }
            None => {
                "SELECT * FROM claim WHERE slot_fingerprint = $slot_fp ORDER BY claim_id LIMIT $limit"
            }
        };
        let mut vars = serde_json::json!({
            "slot_fp": query.slot_fingerprint,
            "limit": query.limit,
        });
        if let Some(after) = query.after_claim_id {
            vars.as_object_mut()
                .unwrap()
                .insert("after".to_string(), Value::String(after.to_string()));
        }
        let result = self.db.query(sql, Some(vars), query.namespace).await?;
        let records = Self::deserialize_vec(result);
        records
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| MemoryError::Storage(format!("claim deser: {e}")))
            })
            .collect()
    }

    async fn commit_relation(&self, request: CommitRelationRequest<'_>) -> Result<(), MemoryError> {
        let content = Self::serialize(&request.relation)?;
        let sql = Self::upsert_one_sql(
            "claim_relation",
            request.relation.claim_relation_id.as_ref(),
        )?;
        let vars = serde_json::json!({"content": content});
        self.db.query(&sql, Some(vars), request.namespace).await?;
        Ok(())
    }

    async fn select_claims_for_facts(
        &self,
        q: ClaimsForFactsQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError> {
        let fact_ids: Vec<&str> = q.fact_ids.iter().map(|f| f.as_ref()).collect();
        let sql = "SELECT * FROM claim WHERE source_fact_id IN $fact_ids";
        let vars = serde_json::json!({"fact_ids": fact_ids});
        let result = self.db.query(sql, Some(vars), q.namespace).await?;
        let records = Self::deserialize_vec(result);
        records
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| MemoryError::Storage(format!("claim deser: {e}")))
            })
            .collect()
    }

    async fn select_relations_for_facts(
        &self,
        q: RelationsForFactsQuery<'_>,
    ) -> Result<Vec<ClaimRelation>, MemoryError> {
        let fact_ids: Vec<&str> = q.fact_ids.iter().map(|f| f.as_ref()).collect();
        let sql = "SELECT * FROM claim_relation WHERE (left_fact_id IN $fact_ids OR right_fact_id IN $fact_ids) AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)";
        let vars = serde_json::json!({"fact_ids": fact_ids});
        let result = self.db.query(sql, Some(vars), q.namespace).await?;
        let records = Self::deserialize_vec(result);
        records
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| MemoryError::Storage(format!("relation deser: {e}")))
            })
            .collect()
    }

    async fn select_source_evidence(
        &self,
        q: SourceEvidenceQuery<'_>,
    ) -> Result<Vec<SourceEvidenceRecord>, MemoryError> {
        let sql = "SELECT claim_id, source_episode_id, source_lineage FROM claim WHERE source_fact_id = $fact_id";
        let vars = serde_json::json!({"fact_id": q.fact_id.as_ref()});
        let result = self.db.query(sql, Some(vars), q.namespace).await?;
        let records = Self::deserialize_vec(result);
        records
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| MemoryError::Storage(format!("evidence deser: {e}")))
            })
            .collect()
    }

    async fn count_active_relations(
        &self,
        namespace: &str,
    ) -> Result<Vec<ActiveRelationCount>, MemoryError> {
        let sql = "SELECT schema_family, outcome, count() AS count FROM claim_relation WHERE t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL GROUP BY schema_family, outcome";
        let result = self.db.query(sql, None, namespace).await?;
        let records = Self::deserialize_vec(result);
        records
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| MemoryError::Storage(format!("count deser: {e}")))
            })
            .collect()
    }

    async fn select_facts_for_backfill(
        &self,
        q: BackfillFactQuery<'_>,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        if q.limit == 0 {
            return Ok(vec![]);
        }
        let sql = match q.after_fact_id {
            Some(_) => {
                "SELECT * FROM fact WHERE fact_id IS NOT NONE AND fact_id > $after ORDER BY fact_id LIMIT $limit"
            }
            None => "SELECT * FROM fact WHERE fact_id IS NOT NONE ORDER BY fact_id LIMIT $limit",
        };
        let mut vars = serde_json::json!({"limit": q.limit});
        if let Some(after) = q.after_fact_id {
            vars.as_object_mut()
                .unwrap()
                .insert("after".to_string(), Value::String(after.to_string()));
        }
        let result = self.db.query(sql, Some(vars), q.namespace).await?;
        Ok(Self::deserialize_vec(result))
    }

    async fn retract_fact_and_claims(
        &self,
        request: RetractFactAndClaimsRequest<'_>,
    ) -> Result<(), MemoryError> {
        // Use `time::now()` in SQL so SurrealDB stores a native datetime,
        // not a string that fails `option<datetime>` coercion.
        let sql1 = "UPDATE fact:⟨$id⟩ SET t_invalid = time::now(), invalidation_reason = $reason";
        let vars1 = serde_json::json!({
            "id": request.fact_id.as_ref(),
            "reason": request.retract_reason,
        });
        self.db.query(sql1, Some(vars1), request.namespace).await?;
        let sql2 = "UPDATE claim SET t_invalid_ingested = time::now() WHERE source_fact_id = $fact_id AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)";
        let vars2 = serde_json::json!({
            "fact_id": request.fact_id.as_ref(),
        });
        self.db.query(sql2, Some(vars2), request.namespace).await?;
        let sql3 = "UPDATE claim_relation SET t_invalid_ingested = time::now() WHERE (left_fact_id = $fact_id OR right_fact_id = $fact_id) AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)";
        let vars3 = serde_json::json!({
            "fact_id": request.fact_id.as_ref(),
        });
        self.db.query(sql3, Some(vars3), request.namespace).await?;
        Ok(())
    }

    async fn upsert_compiled_policies(
        &self,
        namespace: &str,
        policies: &[ClaimPolicyRecord],
    ) -> Result<(), MemoryError> {
        for policy in policies {
            let content = Self::serialize(policy)?;
            let sql = Self::upsert_one_sql("claim_policy", &policy.policy_id)?;
            let vars = serde_json::json!({"content": content});
            self.db.query(&sql, Some(vars), namespace).await?;
        }
        Ok(())
    }

    async fn commit_reconciliation_page(
        &self,
        request: CommitReconciliationPageRequest<'_>,
    ) -> Result<(), MemoryError> {
        // Execute each statement independently — embedded SurrealDB's
        // transaction support is unreliable across mixed UPDATE statements,
        // and relation persistence must not be coupled to job-counter updates
        // (a failed job update leaves the relation uncommitted, which breaks
        // `extract`'s contradiction detection).
        let mut vars: serde_json::Map<String, Value> = serde_json::Map::new();
        vars.insert(
            "job_id".to_string(),
            Value::String(request.job_id.to_string()),
        );
        vars.insert(
            "owner".to_string(),
            Value::String(request.expected_lease_owner.to_string()),
        );

        for relation in request.relations.iter() {
            // Use `create` instead of `UPDATE ... CONTENT` — embedded SurrealDB's
            // UPDATE-with-CONTENT path has been observed to silently no-op when
            // the record doesn't exist yet, which drops relations and breaks
            // contradiction detection in `extract`.
            let record_id = relation.claim_relation_id.as_ref();
            let content = Self::serialize(relation)?;
            self.db
                .create(record_id, content, request.namespace)
                .await
                .map_err(|e| MemoryError::Storage(format!("persist relation: {e}")))?;
        }

        // Update job counters and cursor
        let cursor_update = match request.next_cursor {
            Some(c) => {
                vars.insert("cursor".to_string(), Value::String(c.to_string()));
                "cursor = $cursor,"
            }
            None => "",
        };
        let processed = request.counters.processed;
        let succeeded = request.counters.succeeded;
        let skipped = request.counters.skipped;
        let failed = request.counters.failed;

        if request.completed {
            vars.insert(
                "now".to_string(),
                Value::String(crate::service::normalize_dt(chrono::Utc::now())),
            );
            let sql = format!(
                "UPDATE claim_job:⟨{}⟩ SET status = 'completed', {cursor_update} processed += {processed}, \
                 succeeded += {succeeded}, skipped += {skipped}, failed += {failed}, \
                 completed_at = time::now(), updated_at = time::now() \
                 WHERE lease_owner = $owner",
                request.job_id.body()
            );
            self.db
                .query(&sql, Some(Value::Object(vars.clone())), request.namespace)
                .await?;
        } else {
            let sql = format!(
                "UPDATE claim_job:⟨{}⟩ SET status = 'running', {cursor_update} processed += {processed}, \
                 succeeded += {succeeded}, skipped += {skipped}, failed += {failed}, \
                 updated_at = time::now() \
                 WHERE lease_owner = $owner",
                request.job_id.body()
            );
            self.db
                .query(&sql, Some(Value::Object(vars)), request.namespace)
                .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_028_is_last_registered() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let last = migrations.last().unwrap();
        assert_eq!(last.file_name, "028_claim_reconciliation_hardening.surql");
    }

    #[test]
    fn migration_028_is_registered_once() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let count = migrations
            .iter()
            .filter(|m| m.file_name == "028_claim_reconciliation_hardening.surql")
            .count();
        assert_eq!(count, 1, "028 should be registered exactly once");
    }

    #[test]
    fn migration_028_defines_new_fields_and_indexes() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let m028 = migrations
            .iter()
            .find(|m| m.file_name == "028_claim_reconciliation_hardening.surql")
            .expect("028 not found");
        let sql = m028.sql;
        assert!(sql.contains("DEFINE FIELD schema_family ON claim_relation"));
        assert!(sql.contains("DEFINE FIELD left_fact_id ON claim_relation"));
        assert!(sql.contains("DEFINE FIELD right_fact_id ON claim_relation"));
        assert!(sql.contains("claim_relation_left_fact_active_idx"));
        assert!(sql.contains("claim_relation_right_fact_active_idx"));
        assert!(sql.contains("claim_relation_schema_outcome_active_idx"));
    }

    #[test]
    fn migration_027_is_still_registered() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let has_027 = migrations
            .iter()
            .any(|m| m.file_name == "027_claim_reconciliation.surql");
        assert!(has_027, "027 should still be registered");
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
