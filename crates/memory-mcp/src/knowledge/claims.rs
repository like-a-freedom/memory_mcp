//! Narrow claim storage capability over SurrealDB.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{self, Value};

use crate::error::MemoryError;
use crate::models::claim::{Claim, ClaimIdentityVersion, ClaimJob, ClaimRelation};
use crate::models::{EpisodeId, FactId};
use crate::shared::temporal::normalize_dt;
use crate::storage::{BoundDbClient, DbClient};

/// True when the error reports a duplicate deterministic record ID. For claim
/// projection this is an idempotent no-op (repeat projection of an identical
/// claim), not a failure.
fn is_already_exists_error(err: &MemoryError) -> bool {
    matches!(
        err,
        MemoryError::Storage(message)
            if crate::storage::is_record_already_exists_error(message)
    )
}

// ─── ClaimStore Trait ─────────────────────────────────────────────────────────

/// Narrow storage capability for the claim reconciliation pipeline.
#[async_trait]
pub(crate) trait ClaimStore: Send + Sync {
    async fn ensure_projection_job(&self, job: &ClaimJob) -> Result<(), MemoryError>;

    async fn lease_next_job(
        &self,
        request: LeaseJobRequest<'_>,
    ) -> Result<Option<ClaimJob>, MemoryError>;

    async fn persist_projection(
        &self,
        request: PersistProjectionRequest,
    ) -> Result<(), MemoryError>;

    async fn select_candidates_page(
        &self,
        query: ClaimCandidateQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError>;

    async fn select_claims_for_facts(
        &self,
        query: ClaimsForFactsQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError>;

    async fn select_relations_for_facts(
        &self,
        query: RelationsForFactsQuery<'_>,
    ) -> Result<Vec<ClaimRelation>, MemoryError>;

    async fn count_active_relations(&self) -> Result<Vec<ActiveRelationCount>, MemoryError>;

    async fn select_facts_for_backfill(
        &self,
        query: BackfillFactQuery<'_>,
    ) -> Result<Vec<serde_json::Value>, MemoryError>;

    async fn retract_fact_and_claims(
        &self,
        request: RetractFactAndClaimsRequest<'_>,
    ) -> Result<(), MemoryError>;

    /// Atomically commit relation versions and update job cursor.
    async fn commit_reconciliation_page(
        &self,
        request: CommitReconciliationPageRequest<'_>,
    ) -> Result<(), MemoryError>;
}

/// Bounded storage reads used by the knowledge-owned temporal relation use case.
/// Each method is set-based; callers cannot issue caller-named table reads.
#[async_trait]
pub(crate) trait TemporalRelationReadStore: Send + Sync {
    async fn claims_by_facts_at(
        &self,
        fact_ids: &[FactId],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, MemoryError>;

    async fn claims_by_ids_at(
        &self,
        claim_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, MemoryError>;

    async fn relations_by_claim_ids_at(
        &self,
        claim_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<ClaimRelation>, MemoryError>;

    async fn facts_by_ids_at(
        &self,
        fact_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<TemporalCounterpartFact>, MemoryError>;
}

/// Minimal persisted metadata needed to decide whether a counterpart may be
/// traversed. Relation reads must not hydrate or deserialize fact content.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct TemporalCounterpartFact {
    pub fact_id: FactId,
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

// ─── Request/Response Types ───────────────────────────────────────────────────

/// Lease request for the next pending job.
pub(crate) struct LeaseJobRequest<'a> {
    pub lease_owner: &'a str,
    pub lease_duration: std::time::Duration,
}

/// Persist projection output (claims + jobs).
pub(crate) struct PersistProjectionRequest {
    pub claims: Vec<Claim>,
    pub jobs: Vec<ClaimJob>,
}

/// Query for candidate claims in a slot.
pub(crate) struct ClaimCandidateQuery<'a> {
    pub slot_fingerprint: &'a str,
    pub identity_version: ClaimIdentityVersion,
    pub after_claim_id: Option<&'a crate::models::ClaimId>,
    pub limit: usize,
}

/// Query for claims belonging to specific facts.
pub(crate) struct ClaimsForFactsQuery<'a> {
    pub fact_ids: &'a [FactId],
}

/// Query for relations involving specific facts.
pub(crate) struct RelationsForFactsQuery<'a> {
    pub fact_ids: &'a [FactId],
}

/// One reconciliation relation, resolved to the facts each side's claim came
/// from. This is what the read path consumes: the direction the evaluator
/// recorded (`predecessor` / `successor`), already projected onto fact ids so
/// no caller has to join claim data itself.
///
/// `predecessor_fact_id` is the fact whose claim lost; `successor_fact_id` is
/// the fact whose claim replaced it. Both are `None` for outcomes with no
/// direction (`Contradiction`, `TemporalAmbiguity`), and both are `None` when
/// the corresponding claim's source fact is outside the queried set.
#[derive(Debug, Clone)]
pub(crate) struct RelationForFact {
    pub relation_id: String,
    pub outcome: crate::models::claim::ClaimRelationOutcome,
    pub reason_code: String,
    pub evaluator_version: String,
    pub predecessor_fact_id: Option<FactId>,
    pub successor_fact_id: Option<FactId>,
    /// Relation participants among the facts explicitly included in the read.
    /// A counterpart outside that set is deliberately not hydrated here.
    pub participants: Vec<RelationParticipant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationParticipant {
    pub fact_id: FactId,
    pub source_episode_id: EpisodeId,
}

/// Result of a [`ClaimStore::select_relations_by_fact`] call.
///
/// Both halves come out of the same read because they answer one question for
/// the caller: *what does this pack of facts look like to the claim layer?*
/// Splitting them would cost a second identical claim lookup.
#[derive(Debug, Clone, Default)]
pub(crate) struct RelationsByFactResult {
    /// Relations whose claims are sourced from the queried facts.
    pub relations: Vec<RelationForFact>,
    /// The claims of each queried fact, grouped by fact id. A fact in the query
    /// with no claims is absent rather than present-and-empty, so a caller can
    /// tell "no claims" from "not queried".
    pub claims_by_fact: std::collections::HashMap<FactId, Vec<String>>,
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
    pub after_fact_id: Option<&'a FactId>,
    pub limit: usize,
}

/// Request to retract a fact and all its claims/relations.
pub(crate) struct RetractFactAndClaimsRequest<'a> {
    pub fact_id: &'a FactId,
    pub retract_reason: &'a str,
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
    db: BoundDbClient,
}

impl SurrealClaimStore {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
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

    fn candidate_query_sql(after_claim_id: bool) -> &'static str {
        if after_claim_id {
            "SELECT * FROM claim WHERE slot_fingerprint = $slot_fp AND (identity_version = $identity_version OR ($identity_version = 'legacy' AND identity_version IS NONE)) AND claim_id > $after ORDER BY claim_id LIMIT $limit"
        } else {
            "SELECT * FROM claim WHERE slot_fingerprint = $slot_fp AND (identity_version = $identity_version OR ($identity_version = 'legacy' AND identity_version IS NONE)) ORDER BY claim_id LIMIT $limit"
        }
    }

    async fn select_temporal_claims_by_facts(
        &self,
        fact_ids: &[FactId],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, MemoryError> {
        if fact_ids.is_empty() || limit == 0 {
            return Ok(vec![]);
        }
        let ids = fact_ids.iter().map(ToString::to_string).collect::<Vec<_>>();
        let sql = "SELECT * FROM claim WHERE source_fact_id IN $fact_ids \
                   AND (t_ingested IS NONE OR t_ingested IS NULL OR t_ingested <= type::datetime($cutoff)) \
                   AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL OR t_invalid_ingested > type::datetime($cutoff)) \
                   AND (valid_from IS NONE OR valid_from IS NULL OR valid_from <= type::datetime($cutoff)) \
                   AND (valid_to IS NONE OR valid_to IS NULL OR valid_to > type::datetime($cutoff)) \
                   ORDER BY source_fact_id ASC, claim_id ASC LIMIT $limit";
        let result = self
            .db
            .query(
                sql,
                Some(serde_json::json!({
                    "fact_ids": ids,
                    "cutoff": cutoff,
                    "limit": limit,
                })),
            )
            .await?;
        Self::deserialize_vec(result)
            .into_iter()
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|error| MemoryError::Storage(format!("claim deser: {error}")))
            })
            .collect()
    }

    async fn select_temporal_claims_by_ids(
        &self,
        claim_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, MemoryError> {
        if claim_ids.is_empty() || limit == 0 {
            return Ok(vec![]);
        }
        let sql = "SELECT * FROM claim WHERE claim_id IN $claim_ids \
                   AND (t_ingested IS NONE OR t_ingested IS NULL OR t_ingested <= type::datetime($cutoff)) \
                   AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL OR t_invalid_ingested > type::datetime($cutoff)) \
                   AND (valid_from IS NONE OR valid_from IS NULL OR valid_from <= type::datetime($cutoff)) \
                   AND (valid_to IS NONE OR valid_to IS NULL OR valid_to > type::datetime($cutoff)) \
                   ORDER BY claim_id ASC LIMIT $limit";
        let result = self
            .db
            .query(
                sql,
                Some(serde_json::json!({
                    "claim_ids": claim_ids,
                    "cutoff": cutoff,
                    "limit": limit,
                })),
            )
            .await?;
        Self::deserialize_vec(result)
            .into_iter()
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|error| MemoryError::Storage(format!("claim deser: {error}")))
            })
            .collect()
    }

    async fn select_temporal_relations_by_claim_ids(
        &self,
        claim_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<ClaimRelation>, MemoryError> {
        if claim_ids.is_empty() || limit == 0 {
            return Ok(vec![]);
        }
        let sql = "SELECT * FROM claim_relation \
                   WHERE (left_claim_id IN $claim_ids OR right_claim_id IN $claim_ids) \
                   AND (t_ingested IS NONE OR t_ingested IS NULL OR t_ingested <= type::datetime($cutoff)) \
                   AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL OR t_invalid_ingested > type::datetime($cutoff)) \
                   ORDER BY claim_relation_id ASC LIMIT $limit";
        let result = self
            .db
            .query(
                sql,
                Some(serde_json::json!({
                    "claim_ids": claim_ids,
                    "cutoff": cutoff,
                    "limit": limit,
                })),
            )
            .await?;
        Self::deserialize_vec(result)
            .into_iter()
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|error| MemoryError::Storage(format!("relation deser: {error}")))
            })
            .collect()
    }

    async fn select_temporal_facts_by_ids(
        &self,
        fact_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<TemporalCounterpartFact>, MemoryError> {
        if fact_ids.is_empty() || limit == 0 {
            return Ok(vec![]);
        }
        let visibility = crate::shared::temporal::visibility_clause("$cutoff");
        let sql = format!(
            "SELECT fact_id, policy_tags FROM fact WHERE fact_id IN $fact_ids AND {visibility} ORDER BY fact_id ASC LIMIT $limit"
        );
        let result = self
            .db
            .query(
                &sql,
                Some(serde_json::json!({
                    "fact_ids": fact_ids,
                    "cutoff": cutoff,
                    "limit": limit,
                })),
            )
            .await?;
        Self::deserialize_vec(result)
            .into_iter()
            .map(|value| {
                serde_json::from_value(value).map_err(|error| {
                    MemoryError::Storage(format!("counterpart metadata deser: {error}"))
                })
            })
            .collect()
    }
}

#[async_trait]
impl TemporalRelationReadStore for SurrealClaimStore {
    async fn claims_by_facts_at(
        &self,
        fact_ids: &[FactId],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, MemoryError> {
        self.select_temporal_claims_by_facts(fact_ids, cutoff, limit)
            .await
    }

    async fn claims_by_ids_at(
        &self,
        claim_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, MemoryError> {
        self.select_temporal_claims_by_ids(claim_ids, cutoff, limit)
            .await
    }

    async fn relations_by_claim_ids_at(
        &self,
        claim_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<ClaimRelation>, MemoryError> {
        self.select_temporal_relations_by_claim_ids(claim_ids, cutoff, limit)
            .await
    }

    async fn facts_by_ids_at(
        &self,
        fact_ids: &[String],
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<TemporalCounterpartFact>, MemoryError> {
        self.select_temporal_facts_by_ids(fact_ids, cutoff, limit)
            .await
    }
}

#[async_trait]
impl ClaimStore for SurrealClaimStore {
    async fn ensure_projection_job(&self, job: &ClaimJob) -> Result<(), MemoryError> {
        let content = Self::serialize(job)?;
        // `UPSERT` with a record ID inserts the job or replaces its fields;
        // SurrealDB's `UPDATE` silently no-ops on a missing record and
        // `CONTENT`-style writes reject JSON nulls on SCHEMAFULL `option<>`
        // fields, so the SET-assignment builder is used instead.
        let (sql, vars) = crate::storage::build_upsert_query(
            job.job_id.as_ref(),
            content,
            crate::knowledge::queries::CLAIM_JOB_TEMPORAL_FIELDS,
        )?;
        self.db.query(&sql, Some(vars)).await?;
        Ok(())
    }

    async fn lease_next_job(
        &self,
        request: LeaseJobRequest<'_>,
    ) -> Result<Option<ClaimJob>, MemoryError> {
        let expires = chrono::Utc::now() + request.lease_duration;
        // Atomically find one pending/expired job and lease it. SurrealDB 3.2
        // rejects ORDER BY inside a subquery used as an UPDATE target, and
        // UPDATE itself has no ORDER BY clause, so no ordering is applied;
        // claim-job processing order is not semantically significant (each
        // job reconciles an independent slot). The SCHEMAFULL `option<datetime>`
        // fields require explicit `type::datetime` coercion of bound vars.
        let sql = "UPDATE (SELECT id FROM claim_job \
                   WHERE status = 'pending' \
                   AND (lease_expires_at IS NONE OR lease_expires_at < time::now()) \
                   LIMIT 1) \
                   SET status = 'leased', lease_owner = $owner, \
                   lease_expires_at = type::datetime($expires), started_at = type::datetime($now) RETURN BEFORE";
        let vars = serde_json::json!({
            "owner": request.lease_owner,
            "expires": normalize_dt(expires),
            "now": normalize_dt(chrono::Utc::now()),
        });
        let result = self.db.query(sql, Some(vars)).await?;
        match Self::extract_first(result) {
            Some(v) => serde_json::from_value(v)
                .map(Some)
                .map_err(|e| MemoryError::Storage(format!("job deser: {e}"))),
            None => Ok(None),
        }
    }

    async fn persist_projection(
        &self,
        request: PersistProjectionRequest,
    ) -> Result<(), MemoryError> {
        for claim in &request.claims {
            let content = serde_json::to_value(claim)
                .map_err(|e| MemoryError::Storage(format!("serialize claim: {e}")))?;
            if let Err(err) = self
                .db
                .create(
                    claim.claim_id.as_ref(),
                    content,
                    crate::knowledge::queries::CLAIM_TEMPORAL_FIELDS,
                )
                .await
            {
                // CREATE never overwrites, so a collision is an idempotent
                // no-op (repeat projection of a deterministic claim), not a
                // failure — and an invalidated claim stays invalidated.
                if !is_already_exists_error(&err) {
                    return Err(MemoryError::Storage(format!("persist claim: {err}")));
                }
            }
        }
        for job in &request.jobs {
            let content = serde_json::to_value(job)
                .map_err(|e| MemoryError::Storage(format!("serialize job: {e}")))?;
            if let Err(err) = self
                .db
                .create(
                    job.job_id.as_ref(),
                    content,
                    crate::knowledge::queries::CLAIM_JOB_TEMPORAL_FIELDS,
                )
                .await
                && !is_already_exists_error(&err)
            {
                return Err(MemoryError::Storage(format!("persist job: {err}")));
            }
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
        let sql = Self::candidate_query_sql(query.after_claim_id.is_some());
        let identity_version = match query.identity_version {
            ClaimIdentityVersion::Legacy => "legacy",
            ClaimIdentityVersion::V2 => "v2",
        };
        let mut vars = serde_json::json!({
            "slot_fp": query.slot_fingerprint,
            "identity_version": identity_version,
            "limit": query.limit,
        });
        if let Some(after) = query.after_claim_id {
            let Some(vars) = vars.as_object_mut() else {
                return Err(MemoryError::Storage(
                    "claim pagination variables must be a JSON object".to_string(),
                ));
            };
            vars.insert("after".to_string(), Value::String(after.to_string()));
        }
        let result = self.db.query(sql, Some(vars)).await?;
        let records = Self::deserialize_vec(result);
        records
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| MemoryError::Storage(format!("claim deser: {e}")))
            })
            .collect()
    }

    async fn select_claims_for_facts(
        &self,
        q: ClaimsForFactsQuery<'_>,
    ) -> Result<Vec<Claim>, MemoryError> {
        let fact_ids: Vec<&str> = q.fact_ids.iter().map(|f| f.as_ref()).collect();
        let sql = "SELECT * FROM claim WHERE source_fact_id IN $fact_ids";
        let vars = serde_json::json!({"fact_ids": fact_ids});
        let result = self.db.query(sql, Some(vars)).await?;
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
        let result = self.db.query(sql, Some(vars)).await?;
        let records = Self::deserialize_vec(result);
        records
            .into_iter()
            .map(|v| {
                serde_json::from_value(v)
                    .map_err(|e| MemoryError::Storage(format!("relation deser: {e}")))
            })
            .collect()
    }

    async fn count_active_relations(&self) -> Result<Vec<ActiveRelationCount>, MemoryError> {
        let sql = "SELECT schema_family, outcome, count() AS count FROM claim_relation WHERE t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL GROUP BY schema_family, outcome";
        let result = self.db.query(sql, None).await?;
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
            let Some(vars) = vars.as_object_mut() else {
                return Err(MemoryError::Storage(
                    "fact backfill variables must be a JSON object".to_string(),
                ));
            };
            vars.insert("after".to_string(), Value::String(after.to_string()));
        }
        let result = self.db.query(sql, Some(vars)).await?;
        Ok(Self::deserialize_vec(result))
    }

    async fn retract_fact_and_claims(
        &self,
        request: RetractFactAndClaimsRequest<'_>,
    ) -> Result<(), MemoryError> {
        // ADR-0039: the bi-temporal close owner is the single place that
        // composes close SQL. Retraction delegates the whole close (fact +
        // derived claims) to it, so both bi-temporal fields are always closed
        // together and the reason is persisted.
        crate::knowledge::CloseStoreClient::from_bound(self.db.clone())
            .retract_fact_and_claims(request.fact_id.as_ref(), request.retract_reason)
            .await
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
                .create(
                    record_id,
                    content,
                    crate::knowledge::queries::CLAIM_RELATION_TEMPORAL_FIELDS,
                )
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
                Value::String(normalize_dt(chrono::Utc::now())),
            );
            let sql = format!(
                "UPDATE claim_job:⟨{}⟩ SET status = 'completed', {cursor_update} processed += {processed}, \
                 succeeded += {succeeded}, skipped += {skipped}, failed += {failed}, \
                 completed_at = time::now(), updated_at = time::now() \
                 WHERE lease_owner = $owner",
                request.job_id.body()
            );
            self.db
                .query(&sql, Some(Value::Object(vars.clone())))
                .await?;
        } else {
            let sql = format!(
                "UPDATE claim_job:⟨{}⟩ SET status = 'running', {cursor_update} processed += {processed}, \
                 succeeded += {succeeded}, skipped += {skipped}, failed += {failed}, \
                 updated_at = time::now() \
                 WHERE lease_owner = $owner",
                request.job_id.body()
            );
            self.db.query(&sql, Some(Value::Object(vars))).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::models::claim::ClaimIdentityVersion;
    use async_trait::async_trait;

    #[test]
    fn already_exists_error_is_treated_as_idempotent() {
        let err = MemoryError::Storage(
            "SurrealDB query statement errors:\nstatement 0: Database record `claim:abc` already exists"
                .to_string(),
        );
        assert!(is_already_exists_error(&err));
    }

    #[test]
    fn unrelated_storage_error_is_not_tolerated() {
        let err = MemoryError::Storage("some other storage failure".to_string());
        assert!(!is_already_exists_error(&err));
    }

    fn pending_job(job_id: &str) -> ClaimJob {
        let now = chrono::Utc::now();
        ClaimJob {
            job_id: crate::models::ClaimJobId::from_raw(job_id.to_string()),
            kind: crate::models::claim::ClaimJobKind::Extract,
            namespace: "org".to_string(),
            source_fact_id: None,
            claim_id: None,
            extractor_fingerprint: crate::models::claim::ExtractorFingerprint::compute(1, "test"),
            evaluator_fingerprint: None,
            status: crate::models::claim::ClaimJobState::Pending,
            cursor: None,
            lease_owner: None,
            lease_expires_at: None,
            processed: 0,
            succeeded: 0,
            skipped: 0,
            failed: 0,
            retry_count: 0,
            last_error: None,
            created_at: now,
            started_at: None,
            updated_at: now,
            completed_at: None,
        }
    }

    async fn embedded_claim_store() -> SurrealClaimStore {
        let db = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                "claim_lease_test",
                &["org".to_string()],
                "error",
            )
            .await
            .expect("embedded db"),
        );
        db.apply_migrations("org").await.expect("migrations");
        SurrealClaimStore::new(db, "org")
    }

    struct QueryCountingDbClient {
        inner: Arc<dyn DbClient>,
        query_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DbClient for QueryCountingDbClient {
        async fn select_one(
            &self,
            record_id: &str,
            namespace: &str,
        ) -> Result<Option<Value>, MemoryError> {
            self.inner.select_one(record_id, namespace).await
        }

        async fn select_table(
            &self,
            table: crate::storage::table_scope::OwnedTable,
            namespace: &str,
        ) -> Result<Vec<Value>, MemoryError> {
            self.inner.select_table(table, namespace).await
        }

        async fn create(
            &self,
            record_id: &str,
            content: Value,
            namespace: &str,
            temporal_fields: &[&str],
        ) -> Result<Value, MemoryError> {
            self.inner
                .create(record_id, content, namespace, temporal_fields)
                .await
        }

        async fn update(
            &self,
            record_id: &str,
            content: Value,
            namespace: &str,
            temporal_fields: &[&str],
        ) -> Result<Value, MemoryError> {
            self.inner
                .update(record_id, content, namespace, temporal_fields)
                .await
        }

        async fn query(
            &self,
            sql: &str,
            vars: Option<Value>,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            self.query_count.fetch_add(1, Ordering::SeqCst);
            self.inner.query(sql, vars, namespace).await
        }

        async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
            self.inner.apply_migrations(namespace).await
        }
    }

    async fn embedded_counting_claim_store() -> (SurrealClaimStore, Arc<AtomicUsize>) {
        let db = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                "claim_temporal_relation_test",
                &["org".to_string()],
                "error",
            )
            .await
            .expect("embedded db"),
        );
        db.apply_migrations("org").await.expect("migrations");
        let query_count = Arc::new(AtomicUsize::new(0));
        let db_client: Arc<dyn DbClient> = Arc::new(QueryCountingDbClient {
            inner: db,
            query_count: Arc::clone(&query_count),
        });
        (SurrealClaimStore::new(db_client, "org"), query_count)
    }

    #[tokio::test]
    async fn ensure_projection_job_upserts_idempotently() {
        let store = embedded_claim_store().await;
        store
            .ensure_projection_job(&pending_job("claim_job:job-a"))
            .await
            .expect("first upsert");
        store
            .ensure_projection_job(&pending_job("claim_job:job-a"))
            .await
            .expect("second upsert is idempotent");

        let rows = store
            .db
            .query("SELECT count() AS cnt FROM claim_job", None)
            .await
            .expect("count jobs");
        let count = serde_json::from_value::<Vec<serde_json::Value>>(rows)
            .unwrap_or_default()
            .first()
            .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
            .unwrap_or(0);
        assert_eq!(count, 1, "upsert must not duplicate the job");
    }

    #[tokio::test]
    async fn lease_next_job_leases_exactly_one_pending_job() {
        let store = embedded_claim_store().await;

        store
            .ensure_projection_job(&pending_job("claim_job:lease-1"))
            .await
            .expect("seed job 1");
        store
            .ensure_projection_job(&pending_job("claim_job:lease-2"))
            .await
            .expect("seed job 2");

        let first = store
            .lease_next_job(LeaseJobRequest {
                lease_owner: "worker-1",
                lease_duration: std::time::Duration::from_secs(30),
            })
            .await
            .expect("lease first job")
            .expect("expected one leased job");
        // RETURN BEFORE yields the pre-lease state.
        assert_eq!(first.status, crate::models::claim::ClaimJobState::Pending);
        assert_eq!(first.lease_owner, None);

        // The persisted record is now leased by worker-1.
        let leased_count = store
            .db
            .query(
                "SELECT count() AS cnt FROM claim_job WHERE status = 'leased' AND lease_owner = 'worker-1'",
                None,
            )
            .await
            .expect("count leased");
        let leased =
            serde_json::from_value::<Vec<serde_json::Value>>(leased_count).unwrap_or_default();
        assert_eq!(
            leased
                .first()
                .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                .unwrap_or(0),
            1,
            "exactly one job must be leased"
        );

        // The other job remains pending and can be leased by a second worker.
        let second = store
            .lease_next_job(LeaseJobRequest {
                lease_owner: "worker-2",
                lease_duration: std::time::Duration::from_secs(30),
            })
            .await
            .expect("lease second job")
            .expect("expected the second job");
        assert_eq!(second.status, crate::models::claim::ClaimJobState::Pending);
        assert_ne!(second.job_id, first.job_id, "must lease the other job");

        // No jobs left pending: a third lease finds nothing.
        let none = store
            .lease_next_job(LeaseJobRequest {
                lease_owner: "worker-3",
                lease_duration: std::time::Duration::from_secs(30),
            })
            .await
            .expect("third lease");
        assert!(none.is_none());
    }

    #[derive(Clone, Default)]
    struct NamespaceRecorder {
        namespaces: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl DbClient for NamespaceRecorder {
        async fn select_one(
            &self,
            _record_id: &str,
            namespace: &str,
        ) -> Result<Option<Value>, MemoryError> {
            self.namespaces
                .lock()
                .expect("recorder lock")
                .push(namespace.to_string());
            Ok(None)
        }

        async fn select_table(
            &self,
            _table: crate::storage::table_scope::OwnedTable,
            namespace: &str,
        ) -> Result<Vec<Value>, MemoryError> {
            self.namespaces
                .lock()
                .expect("recorder lock")
                .push(namespace.to_string());
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _record_id: &str,
            _content: Value,
            namespace: &str,
            _temporal_fields: &[&str],
        ) -> Result<Value, MemoryError> {
            self.namespaces
                .lock()
                .expect("recorder lock")
                .push(namespace.to_string());
            Ok(Value::Null)
        }

        async fn update(
            &self,
            _record_id: &str,
            _content: Value,
            namespace: &str,
            _temporal_fields: &[&str],
        ) -> Result<Value, MemoryError> {
            self.namespaces
                .lock()
                .expect("recorder lock")
                .push(namespace.to_string());
            Ok(Value::Null)
        }

        async fn query(
            &self,
            _sql: &str,
            _vars: Option<Value>,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            self.namespaces
                .lock()
                .expect("recorder lock")
                .push(namespace.to_string());
            Ok(Value::Array(Vec::new()))
        }

        async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
            self.namespaces
                .lock()
                .expect("recorder lock")
                .push(namespace.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn claim_store_routes_historical_job_metadata_to_startup_namespace() {
        let recorder = Arc::new(NamespaceRecorder::default());
        let store = SurrealClaimStore::new(recorder.clone(), "main");

        store
            .select_candidates_page(ClaimCandidateQuery {
                slot_fingerprint: "v2:slot",
                identity_version: ClaimIdentityVersion::V2,
                after_claim_id: None,
                limit: 1,
            })
            .await
            .expect("candidate lookup should succeed");

        assert_eq!(
            recorder
                .namespaces
                .lock()
                .expect("recorder lock")
                .as_slice(),
            ["main"]
        );
    }

    #[test]
    fn candidate_lookup_is_versioned_and_namespace_bound() {
        let query = ClaimCandidateQuery {
            slot_fingerprint: "v2:slot",
            identity_version: ClaimIdentityVersion::V2,
            after_claim_id: None,
            limit: 10,
        };
        let sql = SurrealClaimStore::candidate_query_sql(query.after_claim_id.is_some());

        assert!(sql.contains("slot_fingerprint = $slot_fp"));
        assert!(sql.contains("identity_version = $identity_version"));
        assert!(sql.contains("identity_version IS NONE"));
        assert!(!sql.contains("scope ="));
        assert!(!sql.contains("project ="));
        assert_eq!(query.identity_version, ClaimIdentityVersion::V2);
    }

    #[test]
    fn legacy_relation_partition_fields_are_optional_on_deserialize() {
        let raw = serde_json::json!({
            "claim_relation_id": "claim_relation:r",
            "left_claim_id": "claim:l",
            "right_claim_id": "claim:r",
            "pair_fingerprint": "pair",
            "outcome": "duplicate",
            "predecessor_claim_id": null,
            "successor_claim_id": null,
            "reason_code": "duplicate",
            "evidence": {"reason_code": "duplicate", "description": null},
            "evaluator_version": "test",
            "context_fingerprint": "ctx",
            "evaluated_at": "2026-08-13T00:00:00Z",
            "supersedes_relation_id": null,
            "policy_tags": [],
            "t_ingested": "2026-08-13T00:00:00Z",
            "t_invalid_ingested": null
        });

        let relation: ClaimRelation = serde_json::from_value(raw).unwrap();
        assert_eq!(relation.scope, None);
        assert_eq!(relation.project, None);
    }

    #[test]
    fn latest_registered_migration_is_expected() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let last = migrations.last().unwrap();
        assert_eq!(last.file_name, "053_episode_source_context.surql");
    }

    #[test]
    fn migration_038_defines_claim_source_span() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let migration = migrations
            .iter()
            .find(|entry| entry.file_name == "038_claim_source_span.surql");
        assert!(migration.is_some());
        assert!(migration.is_some_and(|entry| {
            entry
                .sql
                .contains("DEFINE FIELD source_span ON claim TYPE option<array>")
        }));
    }

    #[test]
    fn migration_033_defines_identity_version() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let migration = migrations
            .iter()
            .find(|entry| entry.file_name == "033_claim_identity_version.surql");
        assert!(migration.is_some());
        assert!(migration.is_some_and(|entry| {
            entry
                .sql
                .contains("DEFINE FIELD OVERWRITE identity_version ON claim")
        }));
    }

    #[test]
    fn migration_030_is_registered_once() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let count = migrations
            .iter()
            .filter(|m| m.file_name == "030_claim_reconciliation_hardening.surql")
            .count();
        assert_eq!(count, 1, "030 should be registered exactly once");
    }

    #[test]
    fn migration_030_defines_new_fields_and_indexes() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let m030 = migrations
            .iter()
            .find(|m| m.file_name == "030_claim_reconciliation_hardening.surql")
            .expect("030 not found");
        let sql = m030.sql;
        assert!(sql.contains("DEFINE FIELD schema_family ON claim_relation"));
        assert!(sql.contains("DEFINE FIELD left_fact_id ON claim_relation"));
        assert!(sql.contains("DEFINE FIELD right_fact_id ON claim_relation"));
        assert!(sql.contains("claim_relation_left_fact_active_idx"));
        assert!(sql.contains("claim_relation_right_fact_active_idx"));
        assert!(sql.contains("claim_relation_schema_outcome_active_idx"));
    }

    #[test]
    fn migration_029_is_still_registered() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let has_029 = migrations
            .iter()
            .any(|m| m.file_name == "029_claim_reconciliation.surql");
        assert!(has_029, "029 should still be registered");
    }

    #[test]
    fn migration_029_is_registered_once() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let count = migrations
            .iter()
            .filter(|m| m.file_name == "029_claim_reconciliation.surql")
            .count();
        assert_eq!(count, 1, "029 should be registered exactly once");
    }

    #[test]
    fn migration_029_defines_all_five_tables() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let m029 = migrations
            .iter()
            .find(|m| m.file_name == "029_claim_reconciliation.surql")
            .expect("029 not found");
        let sql = m029.sql;
        assert!(sql.contains("DEFINE TABLE claim SCHEMAFULL"));
        assert!(sql.contains("DEFINE TABLE claim_relation SCHEMAFULL"));
        assert!(sql.contains("DEFINE TABLE claim_job SCHEMAFULL"));
        assert!(sql.contains("DEFINE TABLE claim_key_alias SCHEMAFULL"));
        assert!(sql.contains("DEFINE TABLE claim_policy SCHEMAFULL"));
    }

    #[test]
    fn migration_029_defines_expected_indexes() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let m029 = migrations
            .iter()
            .find(|m| m.file_name == "029_claim_reconciliation.surql")
            .expect("029 not found");
        let sql = m029.sql;
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
    fn migration_029_adds_invalidation_reason_to_fact() {
        let migrations = crate::storage::migrations::versioned_migrations();
        let m029 = migrations
            .iter()
            .find(|m| m.file_name == "029_claim_reconciliation.surql")
            .expect("029 not found");
        assert!(
            m029.sql
                .contains("DEFINE FIELD invalidation_reason ON fact")
        );
    }

    #[tokio::test]
    async fn surreal_claim_store_implements_trait() {
        // Verify the trait is object-safe and can be used as dyn
        let _check: Option<&dyn ClaimStore> = None;
    }

    // ─── select_relations_by_fact ─────────────────────────────────────────────

    /// A claim whose source fact is `fact_id`. Mirrors the shape
    /// `claims_policy/reconcile.rs` builds in its own tests.
    fn fact_claim(claim: &str, fact_id: &str, episode_id: &str) -> Claim {
        use crate::models::claim::{
            ClaimCardinality, ClaimSchemaFamily, ClaimSchemaRef, ClaimSlot, ClaimValiditySource,
            ClaimValue, ComparisonKey, ComparisonKeyHash, ExtractorFingerprint, PolicyFingerprint,
            QualifierHash,
        };

        let schema = ClaimSchemaRef {
            family: ClaimSchemaFamily::Attribute,
            version: std::num::NonZeroU16::new(1).unwrap(),
        };
        let mut components = std::collections::BTreeMap::new();
        components.insert("dim".to_string(), "test".to_string());
        let key = ComparisonKey::new(schema, components).unwrap();
        let comparison_key_hash = ComparisonKeyHash::compute(&key);
        let qualifiers = std::collections::BTreeMap::new();
        let qualifier_hash = QualifierHash::compute(&qualifiers);
        let scope = "test";
        let access_policy_fingerprint = PolicyFingerprint::compute(scope, None, &[]);

        Claim {
            identity_version: ClaimIdentityVersion::Legacy,
            claim_id: crate::models::ClaimId::from_raw(format!("claim:{claim}")),
            namespace: "org".to_string(),
            source_fact_id: FactId::from(fact_id),
            source_episode_id: EpisodeId::from(episode_id),
            scope: Some(scope.to_string()),
            project: None,
            project_identity: Some("__none__".to_string()),
            policy_tags: vec![],
            access_policy_fingerprint: access_policy_fingerprint.clone(),
            schema_family: ClaimSchemaFamily::Attribute,
            schema_version: 1,
            subject: ClaimSlot {
                identity_version: ClaimIdentityVersion::Legacy,
                namespace: "org".to_string(),
                scope: Some(scope.to_string()),
                project_identity: Some("__none__".to_string()),
                access_policy_fingerprint: access_policy_fingerprint.clone(),
                schema_ref: schema,
                subject_key: format!("entity:{claim}"),
                comparison_key_hash: comparison_key_hash.clone(),
                qualifier_hash: qualifier_hash.clone(),
            },
            subject_key: format!("entity:{claim}"),
            comparison_key: key,
            comparison_key_hash,
            qualifiers,
            qualifier_hash,
            slot_fingerprint: format!("test:{scope}:__none__:1:entity:{claim}"),
            value: ClaimValue::Integer(1),
            cardinality: ClaimCardinality::SingleValued,
            observed_at: chrono::Utc::now(),
            valid_from: None,
            valid_to: None,
            validity_source: ClaimValiditySource::Explicit,
            source_lineage: None,
            source_span: None,
            derivation: crate::models::claim::ClaimDerivation {
                source_fact_id: FactId::from(fact_id),
                source_episode_id: EpisodeId::from(episode_id),
                extractor_fingerprint: ExtractorFingerprint::compute(1, "test"),
            },
            extractor_fingerprint: ExtractorFingerprint::compute(1, "test"),
            t_ingested: chrono::Utc::now(),
            t_invalid_ingested: None,
        }
    }

    /// A relation row. `left_fact_id` / `right_fact_id` are left `None` so the
    /// tests prove direction comes from `claim.source_fact_id`, not from the
    /// denormalized columns.
    fn fact_relation(
        name: &str,
        left: &str,
        right: &str,
        outcome: crate::models::claim::ClaimRelationOutcome,
        predecessor: Option<&str>,
        successor: Option<&str>,
        t_invalid_ingested: Option<chrono::DateTime<chrono::Utc>>,
    ) -> ClaimRelation {
        use crate::models::claim::{ClaimRelationEvidence, ReconciliationContextFingerprint};

        ClaimRelation {
            claim_relation_id: crate::models::ClaimRelationId::from_raw(format!(
                "claim_relation:{name}"
            )),
            left_claim_id: crate::models::ClaimId::from_raw(format!("claim:{left}")),
            right_claim_id: crate::models::ClaimId::from_raw(format!("claim:{right}")),
            pair_fingerprint: format!("{left}:{right}"),
            outcome,
            predecessor_claim_id: predecessor
                .map(|c| crate::models::ClaimId::from_raw(format!("claim:{c}"))),
            successor_claim_id: successor
                .map(|c| crate::models::ClaimId::from_raw(format!("claim:{c}"))),
            reason_code: outcome.to_string(),
            evidence: ClaimRelationEvidence {
                reason_code: outcome.to_string(),
                description: None,
            },
            evaluator_version: "test".to_string(),
            context_fingerprint: ReconciliationContextFingerprint::compute(
                "test",
                "attribute",
                "alias",
                "policy",
            ),
            evaluated_at: chrono::Utc::now(),
            supersedes_relation_id: None,
            scope: None,
            project: None,
            policy_tags: vec![],
            t_ingested: chrono::Utc::now(),
            t_invalid_ingested,
            schema_family: None,
            schema_version: None,
            left_fact_id: None,
            right_fact_id: None,
        }
    }

    async fn seed_claims(store: &SurrealClaimStore, claims: Vec<Claim>) {
        store
            .persist_projection(PersistProjectionRequest {
                claims,
                jobs: vec![],
            })
            .await
            .expect("seed claims");
    }

    async fn seed_relation(store: &SurrealClaimStore, relation: &ClaimRelation) {
        let content = SurrealClaimStore::serialize(relation).expect("serialize relation");
        store
            .db
            .create(
                relation.claim_relation_id.as_ref(),
                content,
                crate::knowledge::queries::CLAIM_RELATION_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed relation");
    }

    fn test_fact(
        fact_id: &str,
        episode_id: &str,
        content: &str,
        at: chrono::DateTime<chrono::Utc>,
        policy_tags: &[&str],
    ) -> crate::models::Fact {
        crate::models::Fact {
            fact_id: fact_id.to_string(),
            fact_type: "explicit".to_string(),
            content: content.to_string(),
            quote: content.to_string(),
            source_episode: episode_id.to_string(),
            t_valid: at,
            t_ingested: at,
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: policy_tags.iter().map(|tag| tag.to_string()).collect(),
            provenance: crate::models::Provenance::extraction(
                episode_id, "test", episode_id, "test",
            ),
            ft_score: 0.0,
        }
    }

    async fn seed_fact(store: &SurrealClaimStore, fact: &crate::models::Fact) {
        let mut content = SurrealClaimStore::serialize(fact).expect("serialize fact");
        if let Some(object) = content.as_object_mut() {
            object.remove("ft_score");
        }
        store
            .db
            .create(
                &fact.fact_id,
                content,
                crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed fact");
    }

    async fn read_temporal_relations_for_facts(
        store: &SurrealClaimStore,
        fact_ids: Vec<FactId>,
    ) -> RelationsByFactResult {
        let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
        let batch = crate::knowledge::api::temporal_relations_from_store(
            store,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: fact_ids.clone(),
                cutoff,
                policy_tags: vec![],
                counterpart_limit: 32,
                max_relation_depth: 1,
            },
        )
        .await
        .expect("temporal relation query succeeds");
        crate::knowledge::api::visible_temporal_relations_for_facts(&batch, &fact_ids)
    }

    #[tokio::test]
    async fn maps_supersession_direction_to_facts() {
        let store = embedded_claim_store().await;
        // The relation's left claim is the *newer* one, so a reader that derived
        // direction from the unordered storage columns would swap the pair.
        seed_claims(
            &store,
            vec![
                fact_claim("old", "fact:old", "ep:old"),
                fact_claim("new", "fact:new", "ep:new"),
            ],
        )
        .await;
        seed_relation(
            &store,
            &fact_relation(
                "sup",
                "new",
                "old",
                crate::models::claim::ClaimRelationOutcome::Supersession,
                Some("old"),
                Some("new"),
                None,
            ),
        )
        .await;

        let result = read_temporal_relations_for_facts(
            &store,
            vec![FactId::from("fact:old"), FactId::from("fact:new")],
        )
        .await;
        let relations = &result.relations;

        assert_eq!(relations.len(), 1, "exactly one relation expected");
        assert_eq!(
            relations[0].outcome,
            crate::models::claim::ClaimRelationOutcome::Supersession
        );
        assert_eq!(
            relations[0].predecessor_fact_id,
            Some(FactId::from("fact:old")),
            "predecessor is the fact whose claim lost"
        );
        assert_eq!(
            relations[0].successor_fact_id,
            Some(FactId::from("fact:new")),
            "successor is the fact whose claim replaced it"
        );
        assert!(relations[0].participants.iter().any(|participant| {
            participant.fact_id == FactId::from("fact:old")
                && participant.source_episode_id == EpisodeId::from("ep:old")
        }));
        assert!(relations[0].participants.iter().any(|participant| {
            participant.fact_id == FactId::from("fact:new")
                && participant.source_episode_id == EpisodeId::from("ep:new")
        }));
    }

    #[tokio::test]
    async fn excludes_invalidated_relations() {
        let store = embedded_claim_store().await;
        seed_claims(
            &store,
            vec![
                fact_claim("old2", "fact:old2", "ep:old2"),
                fact_claim("new2", "fact:new2", "ep:new2"),
            ],
        )
        .await;
        seed_relation(
            &store,
            &fact_relation(
                "retracted",
                "old2",
                "new2",
                crate::models::claim::ClaimRelationOutcome::Supersession,
                Some("old2"),
                Some("new2"),
                Some(chrono::Utc::now()),
            ),
        )
        .await;

        let result = read_temporal_relations_for_facts(
            &store,
            vec![FactId::from("fact:old2"), FactId::from("fact:new2")],
        )
        .await;
        let relations = &result.relations;

        assert!(
            relations.is_empty(),
            "a relation whose t_invalid_ingested is set must not demote anything"
        );
    }

    #[tokio::test]
    async fn omits_direction_for_undirected_outcomes() {
        let store = embedded_claim_store().await;
        seed_claims(
            &store,
            vec![
                fact_claim("a", "fact:a", "ep:a"),
                fact_claim("b", "fact:b", "ep:b"),
            ],
        )
        .await;
        seed_relation(
            &store,
            &fact_relation(
                "contra",
                "a",
                "b",
                crate::models::claim::ClaimRelationOutcome::Contradiction,
                Some("a"),
                Some("b"),
                None,
            ),
        )
        .await;

        let result = read_temporal_relations_for_facts(
            &store,
            vec![FactId::from("fact:a"), FactId::from("fact:b")],
        )
        .await;
        let relations = &result.relations;

        assert_eq!(relations.len(), 1, "the contradiction is still reported");
        assert_eq!(
            relations[0].outcome,
            crate::models::claim::ClaimRelationOutcome::Contradiction
        );
        assert_eq!(
            relations[0].predecessor_fact_id, None,
            "Contradiction has no winner, so no direction"
        );
        assert_eq!(relations[0].successor_fact_id, None);
    }

    #[tokio::test]
    async fn ignores_relations_for_absent_facts() {
        let store = embedded_claim_store().await;
        seed_claims(
            &store,
            vec![
                fact_claim("c", "fact:c", "ep:c"),
                fact_claim("d", "fact:d", "ep:d"),
            ],
        )
        .await;
        seed_relation(
            &store,
            &fact_relation(
                "unrelated",
                "c",
                "d",
                crate::models::claim::ClaimRelationOutcome::Supersession,
                Some("c"),
                Some("d"),
                None,
            ),
        )
        .await;

        let result =
            read_temporal_relations_for_facts(&store, vec![FactId::from("fact:absent")]).await;
        let relations = &result.relations;

        assert!(
            relations.is_empty(),
            "a fact id the query does not drive must not produce a phantom entry"
        );
    }

    #[tokio::test]
    async fn temporal_relation_reads_apply_relation_ingestion_cutoff() {
        use chrono::TimeZone;

        let store = embedded_claim_store().await;
        let ingested = chrono::Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
        let invalidated = chrono::Utc.with_ymd_and_hms(2026, 1, 20, 0, 0, 0).unwrap();
        let before_invalidation = chrono::Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
        let after_invalidation = chrono::Utc.with_ymd_and_hms(2026, 1, 25, 0, 0, 0).unwrap();

        let mut predecessor = fact_claim("historical-old", "fact:historical-old", "ep:old");
        predecessor.t_ingested = ingested;
        let mut successor = fact_claim("historical-new", "fact:historical-new", "ep:new");
        successor.t_ingested = ingested;
        seed_claims(&store, vec![predecessor, successor]).await;

        let mut relation = fact_relation(
            "historical",
            "historical-old",
            "historical-new",
            crate::models::claim::ClaimRelationOutcome::Supersession,
            Some("historical-old"),
            Some("historical-new"),
            Some(invalidated),
        );
        relation.t_ingested = ingested;
        relation.evaluated_at = ingested;
        seed_relation(&store, &relation).await;
        seed_fact(
            &store,
            &test_fact("fact:historical-old", "ep:old", "old fact", ingested, &[]),
        )
        .await;
        seed_fact(
            &store,
            &test_fact("fact:historical-new", "ep:new", "new fact", ingested, &[]),
        )
        .await;

        let before_ingestion = crate::knowledge::api::temporal_relations_from_store(
            &store,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: vec![FactId::from("fact:historical-old")],
                cutoff: chrono::Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap(),
                policy_tags: vec![],
                counterpart_limit: 32,
                max_relation_depth: 1,
            },
        )
        .await
        .expect("pre-ingestion relation query succeeds");
        assert!(before_ingestion.relations.is_empty());
        assert!(before_ingestion.counterpart_facts.is_empty());

        let before = crate::knowledge::api::temporal_relations_from_store(
            &store,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: vec![FactId::from("fact:historical-old")],
                cutoff: before_invalidation,
                policy_tags: vec![],
                counterpart_limit: 32,
                max_relation_depth: 1,
            },
        )
        .await
        .expect("historical relation query succeeds");
        assert_eq!(before.relations.len(), 1);
        assert_eq!(before.counterpart_facts.len(), 1);
        assert_eq!(
            before.relations[0].successor_fact_id,
            Some(FactId::from("fact:historical-new"))
        );

        let after = crate::knowledge::api::temporal_relations_from_store(
            &store,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: vec![FactId::from("fact:historical-old")],
                cutoff: after_invalidation,
                policy_tags: vec![],
                counterpart_limit: 32,
                max_relation_depth: 1,
            },
        )
        .await
        .expect("current relation query succeeds");
        assert!(after.relations.is_empty());
        assert!(after.counterpart_facts.is_empty());
    }

    #[tokio::test]
    async fn temporal_relation_reads_do_not_hydrate_policy_denied_counterparts() {
        use chrono::TimeZone;

        let store = embedded_claim_store().await;
        let cutoff = chrono::Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
        let mut predecessor = fact_claim("visible-old", "fact:visible-old", "ep:visible-old");
        predecessor.t_ingested = cutoff;
        let mut successor = fact_claim("private-new", "fact:private-new", "ep:private-new");
        successor.t_ingested = cutoff;
        successor.policy_tags = vec!["private".to_string()];
        let mut public_claim_private_fact = fact_claim(
            "public-claim-private-fact",
            "fact:private-fact",
            "ep:private-fact",
        );
        public_claim_private_fact.t_ingested = cutoff;
        seed_claims(
            &store,
            vec![predecessor, successor, public_claim_private_fact],
        )
        .await;

        seed_fact(
            &store,
            &test_fact(
                "fact:visible-old",
                "ep:visible-old",
                "visible source fact",
                cutoff,
                &[],
            ),
        )
        .await;
        seed_fact(
            &store,
            &test_fact(
                "fact:private-fact",
                "ep:private-fact",
                "private fact-level counterpart content",
                cutoff,
                &["private"],
            ),
        )
        .await;
        seed_fact(
            &store,
            &test_fact(
                "fact:private-new",
                "ep:private-new",
                "private counterpart content",
                cutoff,
                &["private"],
            ),
        )
        .await;
        let mut relation = fact_relation(
            "policy-filtered",
            "visible-old",
            "private-new",
            crate::models::claim::ClaimRelationOutcome::Supersession,
            Some("visible-old"),
            Some("private-new"),
            None,
        );
        relation.t_ingested = cutoff;
        relation.evaluated_at = cutoff;
        seed_relation(&store, &relation).await;
        let mut fact_denied_relation = fact_relation(
            "fact-policy-filtered",
            "visible-old",
            "public-claim-private-fact",
            crate::models::claim::ClaimRelationOutcome::Supersession,
            Some("visible-old"),
            Some("public-claim-private-fact"),
            None,
        );
        fact_denied_relation.t_ingested = cutoff;
        fact_denied_relation.evaluated_at = cutoff;
        seed_relation(&store, &fact_denied_relation).await;

        let result = crate::knowledge::api::temporal_relations_from_store(
            &store,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: vec![FactId::from("fact:visible-old")],
                cutoff,
                policy_tags: vec!["public".to_string()],
                counterpart_limit: 32,
                max_relation_depth: 1,
            },
        )
        .await
        .expect("policy-filtered relation query succeeds");

        assert!(
            result
                .counterpart_facts
                .iter()
                .all(|fact| fact.fact_id != FactId::from("fact:private-new"))
        );
        assert!(
            result
                .counterpart_facts
                .iter()
                .all(|fact| !fact.policy_tags.iter().any(|tag| tag == "private")),
            "policy-denied tags must not enter the traversable counterpart set"
        );
        assert!(
            result
                .unresolved_fact_ids
                .contains(&FactId::from("fact:private-new"))
        );
        assert!(
            result
                .unresolved_fact_ids
                .contains(&FactId::from("fact:private-fact"))
        );
        assert_eq!(result.relations.len(), 2);
        assert!(
            result
                .relations
                .iter()
                .all(|relation| relation.successor_fact_id.is_none())
        );
        assert!(
            result
                .relations
                .iter()
                .all(|relation| relation.participants.len() == 1)
        );
    }

    #[tokio::test]
    async fn temporal_relation_hydration_caps_counterparts_and_batches_database_reads() {
        use chrono::TimeZone;

        let (store, query_count) = embedded_counting_claim_store().await;
        let cutoff = chrono::Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let mut root = fact_claim("batch-root", "fact:batch-root", "ep:batch-root");
        root.t_ingested = cutoff;
        let mut claims = vec![root];
        let mut relations = Vec::new();
        let mut facts = Vec::new();

        for index in 0..33 {
            let suffix = format!("{index:02}");
            let claim_id = format!("batch-counterpart-{suffix}");
            let fact_id = format!("fact:{claim_id}");
            let episode_id = format!("ep:{claim_id}");
            let mut claim = fact_claim(&claim_id, &fact_id, &episode_id);
            claim.t_ingested = cutoff;
            claims.push(claim);

            let mut relation = fact_relation(
                &format!("batch-relation-{suffix}"),
                "batch-root",
                &claim_id,
                crate::models::claim::ClaimRelationOutcome::Contradiction,
                None,
                None,
                None,
            );
            relation.t_ingested = cutoff;
            relation.evaluated_at = cutoff;
            relations.push(relation);
            facts.push(test_fact(
                &fact_id,
                &episode_id,
                &format!("counterpart content {suffix}"),
                cutoff,
                &[],
            ));
        }

        seed_claims(&store, claims).await;
        for relation in &relations {
            seed_relation(&store, relation).await;
        }
        for fact in &facts {
            seed_fact(&store, fact).await;
        }
        query_count.store(0, Ordering::SeqCst);

        let result = crate::knowledge::api::temporal_relations_from_store(
            &store,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: vec![FactId::from("fact:batch-root")],
                cutoff,
                policy_tags: vec![],
                counterpart_limit: 100,
                max_relation_depth: 1,
            },
        )
        .await
        .expect("batch relation read succeeds");

        assert_eq!(
            result.counterpart_facts.len(),
            crate::knowledge::api::TEMPORAL_RELATION_MAX_COUNTERPARTS
        );
        assert!(result.truncated, "overflow must be explicit");
        assert_eq!(query_count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn temporal_relation_reader_withholds_rows_below_evidence_stage() {
        use chrono::TimeZone;

        let store = embedded_claim_store().await;
        let cutoff = chrono::Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();
        let mut predecessor = fact_claim("stage-old", "fact:stage-old", "ep:stage-old");
        predecessor.t_ingested = cutoff;
        let mut successor = fact_claim("stage-new", "fact:stage-new", "ep:stage-new");
        successor.t_ingested = cutoff;
        seed_claims(&store, vec![predecessor, successor]).await;
        seed_fact(
            &store,
            &test_fact("fact:stage-old", "ep:stage-old", "old", cutoff, &[]),
        )
        .await;
        seed_fact(
            &store,
            &test_fact("fact:stage-new", "ep:stage-new", "new", cutoff, &[]),
        )
        .await;
        let mut relation = fact_relation(
            "stage-gated",
            "stage-old",
            "stage-new",
            crate::models::claim::ClaimRelationOutcome::Supersession,
            Some("stage-old"),
            Some("stage-new"),
            None,
        );
        relation.t_ingested = cutoff;
        relation.evaluated_at = cutoff;
        seed_relation(&store, &relation).await;

        let db = Arc::clone(&store.db.db);
        let shadow_reader =
            crate::knowledge::api::SurrealRelationReader::new(Arc::clone(&db), "org", false);
        let shadow = crate::knowledge::api::RelationReadPort::temporal_relations(
            &shadow_reader,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: vec![FactId::from("fact:stage-old")],
                cutoff,
                policy_tags: vec![],
                counterpart_limit: 32,
                max_relation_depth: 1,
            },
        )
        .await
        .expect("shadow stage read succeeds");
        assert!(shadow.relations.is_empty());
        assert!(shadow.counterpart_facts.is_empty());

        let enabled_reader = crate::knowledge::api::SurrealRelationReader::new(db, "org", true);
        let enabled = crate::knowledge::api::RelationReadPort::temporal_relations(
            &enabled_reader,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: vec![FactId::from("fact:stage-old")],
                cutoff,
                policy_tags: vec![],
                counterpart_limit: 32,
                max_relation_depth: 1,
            },
        )
        .await
        .expect("evidence stage read succeeds");
        assert_eq!(enabled.relations.len(), 1);
        assert_eq!(enabled.counterpart_facts.len(), 1);
    }

    #[tokio::test]
    async fn temporal_relation_traversal_stops_at_four_hops() {
        use chrono::TimeZone;

        let (store, query_count) = embedded_counting_claim_store().await;
        let cutoff = chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let mut claims = Vec::new();
        let mut facts = Vec::new();
        let mut relations = Vec::new();
        for index in 0..6 {
            let name = format!("depth-{index}");
            let fact_id = format!("fact:{name}");
            let episode_id = format!("ep:{name}");
            let mut claim = fact_claim(&name, &fact_id, &episode_id);
            claim.t_ingested = cutoff;
            claims.push(claim);
            facts.push(test_fact(&fact_id, &episode_id, &name, cutoff, &[]));
        }
        for index in 0..5 {
            let mut relation = fact_relation(
                &format!("depth-edge-{index}"),
                &format!("depth-{index}"),
                &format!("depth-{}", index + 1),
                crate::models::claim::ClaimRelationOutcome::Contradiction,
                None,
                None,
                None,
            );
            relation.t_ingested = cutoff;
            relation.evaluated_at = cutoff;
            relations.push(relation);
        }

        seed_claims(&store, claims).await;
        for fact in &facts {
            seed_fact(&store, fact).await;
        }
        for relation in &relations {
            seed_relation(&store, relation).await;
        }
        query_count.store(0, Ordering::SeqCst);

        let result = crate::knowledge::api::temporal_relations_from_store(
            &store,
            crate::knowledge::api::TemporalRelationsQuery {
                fact_ids: vec![FactId::from("fact:depth-0")],
                cutoff,
                policy_tags: vec![],
                counterpart_limit: 32,
                max_relation_depth: 12,
            },
        )
        .await
        .expect("bounded-depth relation read succeeds");

        assert_eq!(
            result.counterpart_facts.len(),
            crate::knowledge::api::TEMPORAL_RELATION_MAX_DEPTH
        );
        assert_eq!(
            result.relations.len(),
            crate::knowledge::api::TEMPORAL_RELATION_MAX_DEPTH
        );
        assert!(result.truncated);
        assert_eq!(
            query_count.load(Ordering::SeqCst),
            crate::knowledge::api::TEMPORAL_RELATION_MAX_DEPTH * 4
        );
    }
}
