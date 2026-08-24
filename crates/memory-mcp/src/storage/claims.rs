//! Narrow claim storage capability over SurrealDB.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{self, Value};

use super::{BoundDbClient, DbClient};
use crate::models::FactId;
use crate::models::claim::{Claim, ClaimIdentityVersion, ClaimJob, ClaimRelation};
use crate::service::MemoryError;

/// True when the error reports a duplicate deterministic record ID. For claim
/// projection this is an idempotent no-op (repeat projection of an identical
/// claim), not a failure.
fn is_already_exists_error(err: &MemoryError) -> bool {
    matches!(
        err,
        MemoryError::Storage(message)
            if super::client::is_record_already_exists_error(message)
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
}

#[async_trait]
impl ClaimStore for SurrealClaimStore {
    async fn ensure_projection_job(&self, job: &ClaimJob) -> Result<(), MemoryError> {
        let content = Self::serialize(job)?;
        // `UPSERT` with a record ID inserts the job or replaces its fields;
        // SurrealDB's `UPDATE` silently no-ops on a missing record and
        // `CONTENT`-style writes reject JSON nulls on SCHEMAFULL `option<>`
        // fields, so the SET-assignment builder is used instead.
        let (sql, vars) =
            crate::storage::queries::build_upsert_query(job.job_id.as_ref(), content)?;
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
            "expires": crate::service::normalize_dt(expires),
            "now": crate::service::normalize_dt(chrono::Utc::now()),
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
            if let Err(err) = self.db.create(claim.claim_id.as_ref(), content).await {
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
            if let Err(err) = self.db.create(job.job_id.as_ref(), content).await
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
        crate::storage::CloseStoreClient::from_bound(self.db.clone())
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
                .create(record_id, content)
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
    use std::sync::{Arc, Mutex};

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
            _table: &str,
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
        assert_eq!(last.file_name, "039_filesystem_ingestion.surql");
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
}
