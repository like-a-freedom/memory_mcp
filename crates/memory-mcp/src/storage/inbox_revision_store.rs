//! Narrow store for durable inbox revisions.
//!
//! Sole owner of revision discovery, leasing, transitions, recovery, and queue
//! counts. Every state mutation is a compare-and-set against the current lease
//! owner; `discover_prepared` is create-or-select against the unique
//! lineage/content-hash index.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::models::inbox_revision::{
    ClaimedInboxRevision, InboxFailureClass, InboxProcessingStage, InboxRevisionId,
    InboxRevisionLease, InboxRevisionRecord, InboxRevisionState,
};
use crate::service::value_helpers::string_from_value;
use crate::storage::BoundDbClient;

/// Default lease duration for one revision claim.
pub(crate) const DEFAULT_REVISION_LEASE_SECS: i64 = 120;
/// Bounded max `last_error` characters persisted with a failed revision.
pub(crate) const MAX_LAST_ERROR_CHARS: usize = 2048;

/// Deterministic revision ID from lineage + raw-byte SHA-256. A rename starts
/// a new lineage and therefore a new revision even when the bytes are identical.
#[allow(dead_code)]
pub fn revision_id_from_hash(lineage: &str, content_sha256: &str) -> InboxRevisionId {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{lineage}\0{content_sha256}").as_bytes());
    InboxRevisionId::from_hash(&hex::encode(digest))
}

/// Constructs a new record with all timestamps defaulted to `now`.
#[allow(clippy::too_many_arguments, dead_code)]
pub fn new_revision_record(
    lineage: String,
    relative_path: String,
    content_sha256: String,
    source_type: String,
    t_ref: DateTime<Utc>,
    prepared_content: String,
    expected_episode_id: String,
    now: DateTime<Utc>,
) -> InboxRevisionRecord {
    let revision_id = revision_id_from_hash(&lineage, &content_sha256);
    InboxRevisionRecord {
        revision_id,
        lineage,
        relative_path,
        content_sha256,
        source_type,
        t_ref,
        prepared_content: Some(prepared_content),
        state: InboxRevisionState::Discovered,
        processing_stage: InboxProcessingStage::Prepared,
        expected_episode_id,
        episode_id: None,
        attempt_count: 0,
        failure_count: 0,
        retry_generation: None,
        lease_owner: None,
        lease_expires_at: None,
        failure_class: None,
        last_error: None,
        discovered_at: now,
        updated_at: now,
        processed_at: None,
    }
}

/// Serializes a record into the schema field set (surreal JSON).
pub fn record_to_json(record: &InboxRevisionRecord) -> Value {
    json!({
        "revision_id": record.revision_id.as_str(),
        "lineage": record.lineage,
        "relative_path": record.relative_path,
        "content_sha256": record.content_sha256,
        "source_type": record.source_type,
        "t_ref": crate::service::normalize_dt(record.t_ref),
        "prepared_content": record.prepared_content,
        "state": record.state.as_str(),
        "processing_stage": record.processing_stage.as_str(),
        "expected_episode_id": record.expected_episode_id,
        "episode_id": record.episode_id,
        "attempt_count": record.attempt_count,
        "failure_count": record.failure_count,
        "retry_generation": record.retry_generation,
        "lease_owner": record.lease_owner,
        "lease_expires_at": record.lease_expires_at.map(crate::service::normalize_dt),
        "failure_class": record.failure_class.map(|c| c.as_str()),
        "last_error": record.last_error,
        "discovered_at": crate::service::normalize_dt(record.discovered_at),
        "updated_at": crate::service::normalize_dt(record.updated_at),
        "processed_at": record.processed_at.map(crate::service::normalize_dt),
    })
}

/// Parses a record row back into the typed record.
pub fn record_from_json(value: &Value) -> Option<InboxRevisionRecord> {
    let object = value.as_object()?;
    let content_sha256 = string_from_value(object.get("content_sha256")?)?;
    Some(InboxRevisionRecord {
        revision_id: InboxRevisionId::from(string_from_value(object.get("revision_id")?)?),
        lineage: string_from_value(object.get("lineage")?)?,
        relative_path: string_from_value(object.get("relative_path")?)?,
        content_sha256: content_sha256.clone(),
        source_type: string_from_value(object.get("source_type")?)?,
        t_ref: crate::service::parse_iso(&string_from_value(object.get("t_ref")?)?)?,
        prepared_content: object.get("prepared_content").and_then(string_from_value),
        state: parse_state(&string_from_value(object.get("state")?)?)?,
        processing_stage: parse_stage(&string_from_value(object.get("processing_stage")?)?)?,
        expected_episode_id: string_from_value(object.get("expected_episode_id")?)?,
        episode_id: object.get("episode_id").and_then(string_from_value),
        attempt_count: object
            .get("attempt_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        failure_count: object
            .get("failure_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        retry_generation: object.get("retry_generation").and_then(string_from_value),
        lease_owner: object.get("lease_owner").and_then(string_from_value),
        lease_expires_at: object
            .get("lease_expires_at")
            .and_then(string_from_value)
            .and_then(|value| crate::service::parse_iso(&value)),
        failure_class: object
            .get("failure_class")
            .and_then(string_from_value)
            .as_deref()
            .and_then(parse_failure_class),
        last_error: object.get("last_error").and_then(string_from_value),
        discovered_at: crate::service::parse_iso(&string_from_value(
            object.get("discovered_at")?,
        )?)?,
        updated_at: crate::service::parse_iso(&string_from_value(object.get("updated_at")?)?)?,
        processed_at: object
            .get("processed_at")
            .and_then(string_from_value)
            .and_then(|value| crate::service::parse_iso(&value)),
    })
}

fn parse_state(value: &str) -> Option<InboxRevisionState> {
    match value {
        "discovered" => Some(InboxRevisionState::Discovered),
        "processing" => Some(InboxRevisionState::Processing),
        "processed" => Some(InboxRevisionState::Processed),
        "failed" => Some(InboxRevisionState::Failed),
        _ => None,
    }
}

fn parse_stage(value: &str) -> Option<InboxProcessingStage> {
    match value {
        "prepared" => Some(InboxProcessingStage::Prepared),
        "ingesting" => Some(InboxProcessingStage::Ingesting),
        "extracting" => Some(InboxProcessingStage::Extracting),
        "complete" => Some(InboxProcessingStage::Complete),
        _ => None,
    }
}

fn parse_failure_class(value: &str) -> Option<InboxFailureClass> {
    match value {
        "validation" => Some(InboxFailureClass::Validation),
        "corrupt" => Some(InboxFailureClass::Corrupt),
        "io" => Some(InboxFailureClass::Io),
        "storage" => Some(InboxFailureClass::Storage),
        "model" => Some(InboxFailureClass::Model),
        "timeout" => Some(InboxFailureClass::Timeout),
        "channel" => Some(InboxFailureClass::Channel),
        "other_transient" => Some(InboxFailureClass::OtherTransient),
        _ => None,
    }
}

/// The narrow store client bound to the Active Namespace.
#[derive(Clone)]
#[allow(dead_code)]
pub struct InboxRevisionStoreClient {
    db: BoundDbClient,
}

impl InboxRevisionStoreClient {
    pub fn new(db_client: std::sync::Arc<dyn crate::storage::DbClient>, namespace: String) -> Self {
        Self {
            db: BoundDbClient::new(db_client, namespace),
        }
    }

    /// Create-or-select one revision for a lineage + content hash. When the
    /// row already exists, returns `(record, false)`; otherwise inserts the
    /// durable prepared snapshot and returns `(record, true)`.
    pub async fn discover_prepared(
        &self,
        record: &InboxRevisionRecord,
    ) -> Result<(InboxRevisionRecord, bool), MemoryError> {
        let revision_id = record.revision_id.as_str();
        let content = record_to_json(record);
        match self.db.create(revision_id, content).await {
            Ok(created) => {
                let parsed = record_from_json(&created).ok_or_else(|| {
                    MemoryError::Storage("failed to parse created inbox revision".to_string())
                })?;
                Ok((parsed, true))
            }
            Err(MemoryError::Storage(message)) if is_revision_exists_error(&message) => {
                let existing = self.db.select_one(revision_id).await?.ok_or_else(|| {
                    MemoryError::Storage(
                        "duplicate inbox revision vanished during discover".to_string(),
                    )
                })?;
                let parsed = record_from_json(&existing).ok_or_else(|| {
                    MemoryError::Storage("failed to parse existing inbox revision".to_string())
                })?;
                Ok((parsed, false))
            }
            Err(err) => Err(err),
        }
    }

    /// Atomically leases one eligible discovered revision. `failed` revisions
    /// are only claimable after `requeue_failed_for_startup` resets them to
    /// `discovered` for a new startup generation. Returns `None` when nothing
    /// is eligible.
    pub async fn claim_next(
        &self,
        owner: &str,
        lease_duration: Duration,
    ) -> Result<Option<ClaimedInboxRevision>, MemoryError> {
        let expires = Utc::now() + lease_duration;
        let sql = "UPDATE (SELECT id FROM inbox_revision \
                   WHERE state = 'discovered' \
                   AND (lease_expires_at IS NONE OR lease_expires_at < time::now()) \
                   LIMIT 1) \
                   SET state = 'processing', lease_owner = $owner, \
                   lease_expires_at = type::datetime($expires), \
                   updated_at = type::datetime($now) RETURN BEFORE";
        let vars = json!({
            "owner": owner,
            "expires": crate::service::normalize_dt(expires),
            "now": crate::service::normalize_dt(Utc::now()),
        });
        let result = self.db.query(sql, Some(vars)).await?;
        let rows = result.as_array().cloned().unwrap_or_default();
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let record = record_from_json(&row).ok_or_else(|| {
            MemoryError::Storage("failed to parse leased inbox revision".to_string())
        })?;
        let prepared_content = record.prepared_content.clone().ok_or_else(|| {
            MemoryError::Storage(
                "claimed inbox revision has no prepared_content snapshot".to_string(),
            )
        })?;
        let revision_id = record.revision_id.clone();
        Ok(Some(ClaimedInboxRevision {
            record,
            prepared_content,
            lease: InboxRevisionLease {
                revision_id,
                owner: owner.to_string(),
            },
        }))
    }

    /// Compare-and-set advance of the processing stage, requiring the current
    /// lease owner.
    pub async fn advance_stage(
        &self,
        revision_id: &InboxRevisionId,
        lease_owner: &str,
        stage: InboxProcessingStage,
    ) -> Result<(), MemoryError> {
        self.cas_update(
            revision_id,
            lease_owner,
            json!({
                "processing_stage": stage.as_str(),
                "updated_at": crate::service::normalize_dt(Utc::now()),
            }),
        )
        .await
    }

    /// Persists the durable episode id after ingest, requiring the current
    /// lease owner.
    pub async fn record_episode(
        &self,
        revision_id: &InboxRevisionId,
        lease_owner: &str,
        episode_id: &str,
    ) -> Result<(), MemoryError> {
        self.cas_update(
            revision_id,
            lease_owner,
            json!({
                "episode_id": episode_id,
                "processing_stage": InboxProcessingStage::Extracting.as_str(),
                "updated_at": crate::service::normalize_dt(Utc::now()),
            }),
        )
        .await
    }

    /// Marks a revision processed and clears the prepared-content snapshot.
    /// Only valid after the episode exists and extract succeeded.
    pub async fn mark_processed(
        &self,
        revision_id: &InboxRevisionId,
        lease_owner: &str,
    ) -> Result<(), MemoryError> {
        self.cas_update(
            revision_id,
            lease_owner,
            json!({
                "state": InboxRevisionState::Processed.as_str(),
                "processing_stage": InboxProcessingStage::Complete.as_str(),
                "prepared_content": Value::Null,
                "lease_owner": Value::Null,
                "lease_expires_at": Value::Null,
                "processed_at": crate::service::normalize_dt(Utc::now()),
                "updated_at": crate::service::normalize_dt(Utc::now()),
            }),
        )
        .await
    }

    /// Marks one cycle failed, retaining the prepared snapshot and classifying
    /// the failure. Compare-and-set on the current lease owner.
    pub async fn mark_failed_cycle(
        &self,
        revision_id: &InboxRevisionId,
        lease_owner: &str,
        failure_class: InboxFailureClass,
        last_error: &str,
        attempt_count: u32,
        retry_generation: Option<&str>,
    ) -> Result<(), MemoryError> {
        let error = truncate_error(last_error);
        self.cas_update(
            revision_id,
            lease_owner,
            json!({
                "state": InboxRevisionState::Failed.as_str(),
                "attempt_count": attempt_count,
                "failure_count": json!(attempt_count),
                "failure_class": failure_class.as_str(),
                "last_error": error,
                "retry_generation": retry_generation,
                "lease_owner": Value::Null,
                "lease_expires_at": Value::Null,
                "updated_at": crate::service::normalize_dt(Utc::now()),
            }),
        )
        .await
    }

    /// Releases an interrupted lease without a domain failure, retaining the
    /// prepared snapshot for later recovery.
    pub async fn release_interrupted(
        &self,
        revision_id: &InboxRevisionId,
        lease_owner: &str,
    ) -> Result<(), MemoryError> {
        self.cas_update(
            revision_id,
            lease_owner,
            json!({
                "state": InboxRevisionState::Discovered.as_str(),
                "lease_owner": Value::Null,
                "lease_expires_at": Value::Null,
                "updated_at": crate::service::normalize_dt(Utc::now()),
            }),
        )
        .await
    }

    /// Requeues expired leases (crashed processors) back to `discovered`.
    pub async fn requeue_expired_leases(&self) -> Result<usize, MemoryError> {
        let sql = "UPDATE inbox_revision \
                   SET state = 'discovered', lease_owner = NONE, lease_expires_at = NONE, \
                   updated_at = type::datetime($now) \
                   WHERE state = 'processing' \
                   AND (lease_expires_at IS NONE OR lease_expires_at < time::now()) \
                   RETURN count()";
        let vars = json!({
            "now": crate::service::normalize_dt(Utc::now()),
        });
        let result = self.db.query(sql, Some(vars)).await?;
        Ok(rows_count(&result))
    }

    /// Requeues failed revisions for a new startup generation. A revision
    /// failed in the same generation is not requeued twice.
    pub async fn requeue_failed_for_startup(&self, generation: &str) -> Result<usize, MemoryError> {
        let sql = "UPDATE inbox_revision \
                   SET state = 'discovered', failure_count = 0, \
                   retry_generation = $generation, \
                   updated_at = type::datetime($now) \
                   WHERE state = 'failed' \
                   AND (retry_generation IS NONE OR retry_generation != $generation) \
                   RETURN count()";
        let vars = json!({
            "generation": generation,
            "now": crate::service::normalize_dt(Utc::now()),
        });
        let result = self.db.query(sql, Some(vars)).await?;
        Ok(rows_count(&result))
    }

    /// Count of discovered + failed (claimable) revisions.
    pub async fn queue_depth(&self) -> Result<usize, MemoryError> {
        let sql =
            "SELECT count() AS cnt FROM inbox_revision WHERE state IN ['discovered', 'failed']";
        let result = self.db.query(sql, None).await?;
        Ok(count_result(&result))
    }

    async fn cas_update(
        &self,
        revision_id: &InboxRevisionId,
        lease_owner: &str,
        fields: Value,
    ) -> Result<(), MemoryError> {
        let Value::Object(map) = fields else {
            return Err(MemoryError::Storage(
                "cas update fields must be an object".to_string(),
            ));
        };
        const TEMPORAL: &[&str] = &[
            "t_ref",
            "lease_expires_at",
            "discovered_at",
            "updated_at",
            "processed_at",
        ];
        let mut vars = serde_json::Map::new();
        let assignments = map
            .iter()
            .map(|(key, value)| {
                if value.is_null() {
                    format!("{key} = NONE")
                } else if TEMPORAL.contains(&key.as_str()) {
                    vars.insert(key.clone(), value.clone());
                    format!("{key} = type::datetime(${key})")
                } else {
                    vars.insert(key.clone(), value.clone());
                    format!("{key} = ${key}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE {id} SET {assignments} WHERE lease_owner = $lease_owner RETURN AFTER",
            id = revision_id.as_str()
        );
        vars.insert(
            "lease_owner".to_string(),
            Value::String(lease_owner.to_string()),
        );
        let result = self.db.query(&sql, Some(Value::Object(vars))).await?;
        let rows = result.as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            return Err(MemoryError::Storage(format!(
                "inbox revision `{revision_id}` is not leased by `{lease_owner}`"
            )));
        }
        Ok(())
    }
}

fn rows_count(result: &Value) -> usize {
    result
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("count"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize
}

#[allow(dead_code)]
fn count_result(result: &Value) -> usize {
    rows_count(result)
}

fn truncate_error(error: &str) -> String {
    let mut chars = error.chars();
    let mut truncated: String = chars.by_ref().take(MAX_LAST_ERROR_CHARS).collect();
    if chars.next().is_some() {
        truncated.push('…');
    }
    truncated
}

fn is_revision_exists_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("already exists") && lowered.contains("inbox_revision")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::now;
    use crate::storage::{DbClient, SurrealDbClient};
    use sha2::Digest;
    use std::sync::Arc;

    async fn make_store() -> (InboxRevisionStoreClient, Arc<SurrealDbClient>) {
        let db = Arc::new(
            SurrealDbClient::connect_in_memory("inbox_rev_test", "org", "warn")
                .await
                .expect("connect in memory"),
        );
        db.apply_migrations("org").await.expect("apply migrations");
        let store = InboxRevisionStoreClient::new(db.clone(), "org".to_string());
        (store, db)
    }

    fn sample_record(lineage: &str, content: &str, now: DateTime<Utc>) -> InboxRevisionRecord {
        let content_sha256 = hex::encode(sha2::Sha256::digest(content.as_bytes()));
        let episode_id = format!("episode:{}", &content_sha256[..24]);
        new_revision_record(
            lineage.to_string(),
            "docs/spec.md".to_string(),
            content_sha256,
            "document".to_string(),
            now,
            content.to_string(),
            episode_id,
            now,
        )
    }

    #[tokio::test]
    async fn discover_is_atomic_and_idempotent_for_one_revision() {
        let (store, _db) = make_store().await;
        let now = now();
        let record = sample_record("fs:docs/spec.md", "version one", now);

        let (first, created) = store
            .discover_prepared(&record)
            .await
            .expect("first discover");
        assert!(created);
        assert_eq!(first.state, InboxRevisionState::Discovered);
        assert_eq!(first.prepared_content.as_deref(), Some("version one"));

        // Concurrent/idempotent rediscovery returns the same row without
        // creating a duplicate.
        let (second, created) = store
            .discover_prepared(&record)
            .await
            .expect("second discover");
        assert!(!created);
        assert_eq!(first.revision_id, second.revision_id);
    }

    #[tokio::test]
    async fn two_claimers_cannot_own_the_same_revision() {
        let (store, _db) = make_store().await;
        let now = now();
        let record = sample_record("fs:a", "payload", now);
        store.discover_prepared(&record).await.expect("discover");

        let owner_a = format!("worker-{}", std::process::id());
        let owner_b = "worker-other";
        let claim_a = store
            .claim_next(&owner_a, Duration::seconds(60))
            .await
            .expect("claim a");
        let claim_b = store
            .claim_next(owner_b, Duration::seconds(60))
            .await
            .expect("claim b");
        assert!(claim_a.is_some());
        assert!(
            claim_b.is_none(),
            "a second claimer must not own the same revision"
        );
    }

    #[tokio::test]
    async fn expired_processing_lease_is_requeued_with_snapshot_and_stage() {
        let (store, _db) = make_store().await;
        let now = now();
        let record = sample_record("fs:b", "snapshot", now);
        store.discover_prepared(&record).await.expect("discover");

        let owner = "worker-crashed";
        let claim = store
            .claim_next(owner, Duration::seconds(1))
            .await
            .expect("claim")
            .expect("claimable");
        assert_eq!(claim.prepared_content, "snapshot");

        // Simulate a crashed processor: the lease expires.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

        let requeued = store.requeue_expired_leases().await.expect("requeue");
        assert_eq!(requeued, 1);

        let again = store
            .claim_next("worker-new", Duration::seconds(60))
            .await
            .expect("reclaim")
            .expect("reclaimable after expiry");
        assert_eq!(again.prepared_content, "snapshot");
        assert_eq!(
            again.record.processing_stage,
            InboxProcessingStage::Prepared
        );
    }

    #[tokio::test]
    async fn failed_revision_gets_only_one_cycle_per_startup_generation() {
        let (store, _db) = make_store().await;
        let now = now();
        let record = sample_record("fs:c", "bad", now);
        store.discover_prepared(&record).await.expect("discover");

        let owner = "worker-fail";
        let claim = store
            .claim_next(owner, Duration::seconds(60))
            .await
            .expect("claim")
            .expect("claimable");
        store
            .mark_failed_cycle(
                &claim.record.revision_id,
                owner,
                InboxFailureClass::Corrupt,
                "boom",
                1,
                None,
            )
            .await
            .expect("mark failed");

        let first = store
            .requeue_failed_for_startup("generation-1")
            .await
            .expect("requeue gen 1");
        assert_eq!(first, 1);

        let claim_again = store
            .claim_next("worker-new", Duration::seconds(60))
            .await
            .expect("reclaim")
            .expect("requeued");
        assert_eq!(claim_again.record.failure_count, 0);

        // Second start in the same generation must not enqueue it twice.
        let second = store
            .requeue_failed_for_startup("generation-1")
            .await
            .expect("requeue gen 1 again");
        assert_eq!(second, 0);
    }

    #[tokio::test]
    async fn cas_transitions_require_current_lease_owner() {
        let (store, _db) = make_store().await;
        let now = now();
        let record = sample_record("fs:d", "lease", now);
        store.discover_prepared(&record).await.expect("discover");

        let owner = "worker-a";
        let claim = store
            .claim_next(owner, Duration::seconds(60))
            .await
            .expect("claim")
            .expect("claimable");

        // A different owner cannot advance the stage.
        let err = store
            .advance_stage(
                &claim.record.revision_id,
                "worker-b",
                InboxProcessingStage::Ingesting,
            )
            .await
            .expect_err("wrong owner must fail");
        assert!(err.to_string().contains("not leased"));

        // The real owner can.
        store
            .advance_stage(
                &claim.record.revision_id,
                owner,
                InboxProcessingStage::Ingesting,
            )
            .await
            .expect("advance");
    }

    #[tokio::test]
    async fn mark_processed_clears_prepared_content_after_episode() {
        let (store, _db) = make_store().await;
        let now = now();
        let record = sample_record("fs:e", "done", now);
        store.discover_prepared(&record).await.expect("discover");

        let owner = "worker-a";
        let claim = store
            .claim_next(owner, Duration::seconds(60))
            .await
            .expect("claim")
            .expect("claimable");

        store
            .record_episode(&claim.record.revision_id, owner, "episode:done")
            .await
            .expect("record episode");
        store
            .mark_processed(&claim.record.revision_id, owner)
            .await
            .expect("mark processed");

        let after = store
            .db
            .select_one(claim.record.revision_id.as_str())
            .await
            .expect("select")
            .expect("row exists");
        assert_eq!(
            after.get("state").and_then(Value::as_str),
            Some("processed")
        );
        assert_eq!(
            after.get("prepared_content").and_then(Value::as_str),
            None,
            "prepared_content must be cleared after processing"
        );
    }
}
