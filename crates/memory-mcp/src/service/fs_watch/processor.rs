//! Sequential lease-based revision processing.
//!
//! The processor dequeues one revision at a time from the durable store,
//! runs internal `ingest → extract` on the durable prepared-content snapshot
//! (never rereading the live filesystem path), and advances the revision to
//! `processed` only after both succeed.
//!
//! The processor runtime is started by the filesystem runtime in a later task;
//! until then its entry points are exercised only by tests, so dead-code
//! analysis is relaxed.
#![allow(dead_code)]

use std::time::Duration;

use crate::error::MemoryError;
use crate::models::inbox_revision::{
    ClaimedInboxRevision, InboxFailureClass, InboxProcessingStage, InboxRevisionLease,
};
use crate::service::MemoryService;
use crate::service::ingestion::IngestionMetadata;
use crate::storage::InboxRevisionStoreClient;

use super::telemetry::FsWatchTelemetry;

/// Maximum processor attempts for transient model/extractor failures.
pub(crate) const MAX_PROCESSOR_ATTEMPTS: u32 = 3;
/// Base exponential delay for processor retries.
pub(crate) const PROCESSOR_RETRY_BASE_MS: u64 = 750;
/// Per-attempt timeout for a single ingest+extract cycle.
pub(crate) const PROCESSOR_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Outcome of processing one claimed revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessOutcome {
    Processed,
    FailedNonRetryable,
    FailedRetriesExhausted,
    Interrupted,
}

/// Sequential processor runtime.
pub(crate) struct InboxRevisionProcessor {
    store: InboxRevisionStoreClient,
    service: MemoryService,
    telemetry: FsWatchTelemetry,
    stop_dequeue: tokio_util::sync::CancellationToken,
    /// Current lease, published so the runtime can release it on bounded
    /// shutdown when the processor task is aborted.
    current_lease: std::sync::Arc<tokio::sync::Mutex<Option<InboxRevisionLease>>>,
}

impl InboxRevisionProcessor {
    pub(crate) fn new(
        store: InboxRevisionStoreClient,
        service: MemoryService,
        telemetry: FsWatchTelemetry,
        stop_dequeue: tokio_util::sync::CancellationToken,
        current_lease: std::sync::Arc<tokio::sync::Mutex<Option<InboxRevisionLease>>>,
    ) -> Self {
        Self {
            store,
            service,
            telemetry,
            stop_dequeue,
            current_lease,
        }
    }

    /// Sequential dequeue loop; exits when `stop_dequeue` is cancelled.
    pub(crate) async fn run(&self) {
        loop {
            if self.stop_dequeue.is_cancelled() {
                break;
            }
            let owner = format!("processor-{}", std::process::id());
            let lease_secs = crate::storage::inbox_revision_store::DEFAULT_REVISION_LEASE_SECS;
            let claim = match self
                .store
                .claim_next(&owner, chrono::Duration::seconds(lease_secs))
                .await
            {
                Ok(Some(claim)) => claim,
                Ok(None) => {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            {
                let mut guard = self.current_lease.lock().await;
                *guard = Some(claim.lease.clone());
            }
            let relative_path = claim.record.relative_path.clone();
            let revision_prefix = claim
                .record
                .content_sha256
                .chars()
                .take(12)
                .collect::<String>();
            self.telemetry.set_inflight(1);
            let started = std::time::Instant::now();
            let outcome =
                process_claimed_revision(&self.service, &self.store, claim, &self.telemetry).await;
            self.telemetry
                .record_revision_duration(outcome, started.elapsed());
            self.telemetry.set_inflight(0);
            {
                let mut guard = self.current_lease.lock().await;
                *guard = None;
            }
            self.service.logger.log(
                crate::service::log_event(
                    "fs_watch.revision",
                    serde_json::json!({
                        "path": relative_path,
                        "revision": revision_prefix,
                    }),
                    serde_json::json!({
                        "outcome": outcome_label(outcome),
                    }),
                    None,
                    None,
                    None,
                ),
                crate::logging::LogLevel::Info,
            );
            self.telemetry.record_revision(outcome);
        }
    }
}

/// Processes one claimed revision using only its durable snapshot.
pub async fn process_claimed_revision(
    service: &MemoryService,
    store: &InboxRevisionStoreClient,
    claim: ClaimedInboxRevision,
    telemetry: &FsWatchTelemetry,
) -> ProcessOutcome {
    let revision_id = claim.record.revision_id.clone();
    let owner = claim.lease.owner.clone();
    let expected_episode_id = claim.record.expected_episode_id.clone();
    let lineage = claim.record.lineage.clone();
    let source_type = claim.record.source_type.clone();
    let t_ref = claim.record.t_ref;
    let content_hash = claim.record.content_sha256.clone();
    let prepared_content = claim.prepared_content.clone();
    let log_source_id = format!("fs:{}", &content_hash[..12.min(content_hash.len())]);

    // Advance to `ingesting` before the first ingest attempt.
    if let Err(_err) = store
        .advance_stage(&revision_id, &owner, InboxProcessingStage::Ingesting)
        .await
    {
        return ProcessOutcome::Interrupted;
    }

    // Phase 1: internal ingest, retried for transient model/extractor
    // failures. Deterministic validation/corrupt failures and exhausted
    // storage-layer retries are non-retryable here.
    let ingest_outcome = retry_until_settled("ingest", MAX_PROCESSOR_ATTEMPTS, telemetry, || {
        let source_type = source_type.clone();
        let lineage = lineage.clone();
        let content_hash = content_hash.clone();
        let prepared_content = prepared_content.clone();
        let log_source_id = log_source_id.clone();
        async move {
            let episode_id = service
                .ingestion_service
                .ingest_with_metadata(
                    crate::models::IngestRequest {
                        source_type,
                        source_id: format!("{lineage}:{content_hash}"),
                        content: prepared_content,
                        t_ref,
                        t_ingested: None,
                        policy_tags: vec![],
                    },
                    None,
                    IngestionMetadata {
                        source_lineage: Some(lineage),
                        log_source_id: Some(log_source_id),
                    },
                )
                .await?;
            Ok::<_, MemoryError>(episode_id)
        }
    })
    .await;

    let episode_id = match ingest_outcome {
        SettleOutcome::Succeeded(episode_id) => episode_id,
        SettleOutcome::Failed {
            attempts,
            non_retryable,
        } => {
            return fail_cycle(
                store,
                &revision_id,
                &owner,
                attempts,
                non_retryable,
                "ingest failed",
            )
            .await;
        }
    };

    // The deterministic episode id must match what we precomputed.
    if episode_id != expected_episode_id {
        return fail_cycle_with_class(
            store,
            &revision_id,
            &owner,
            1,
            InboxFailureClass::Storage,
            "inbox revision episode id mismatch (storage invariant)",
        )
        .await;
    }

    if let Err(_err) = store
        .record_episode(&revision_id, &owner, &episode_id)
        .await
    {
        return ProcessOutcome::Interrupted;
    }

    // Phase 2: extract, retried for transient model/extractor failures.
    let extract_outcome = retry_until_settled("extract", MAX_PROCESSOR_ATTEMPTS, telemetry, || {
        let episode_id = episode_id.clone();
        let context = service.build_context();
        async move {
            crate::service::capabilities::extract::ExtractCapability::extract(
                &context,
                &episode_id,
                None,
                None,
            )
            .await
            .map(|_| ())
        }
    })
    .await;

    match extract_outcome {
        SettleOutcome::Succeeded(()) => {}
        SettleOutcome::Failed {
            attempts,
            non_retryable,
        } => {
            return fail_cycle(
                store,
                &revision_id,
                &owner,
                attempts,
                non_retryable,
                "extract failed",
            )
            .await;
        }
    }

    if store.mark_processed(&revision_id, &owner).await.is_err() {
        // The row stays `processing`; lease expiry + requeue recovers it.
        return ProcessOutcome::Interrupted;
    }
    ProcessOutcome::Processed
}

/// Outcome of a bounded retry cycle.
enum SettleOutcome<T> {
    Succeeded(T),
    Failed {
        attempts: u32,
        /// `true` when the failure was deterministic (validation/corrupt/
        /// non-transient storage) and was not retried; `false` when transient
        /// retries were exhausted.
        non_retryable: bool,
    },
}

/// Retries a fallible operation with bounded exponential backoff for transient
/// failures.
async fn retry_until_settled<T, F, Fut>(
    stage: &'static str,
    max_attempts: u32,
    telemetry: &FsWatchTelemetry,
    mut operation: F,
) -> SettleOutcome<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, MemoryError>>,
{
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let outcome = tokio::time::timeout(PROCESSOR_ATTEMPT_TIMEOUT, operation()).await;
        match outcome {
            Ok(Ok(value)) => return SettleOutcome::Succeeded(value),
            Ok(Err(err)) => {
                let class = classify_failure(&err);
                telemetry.record_retry(stage, class);
                if !is_retryable(&err, class) {
                    return SettleOutcome::Failed {
                        attempts: attempt,
                        non_retryable: true,
                    };
                }
                if attempt >= max_attempts {
                    return SettleOutcome::Failed {
                        attempts: attempt,
                        non_retryable: false,
                    };
                }
                let delay_ms = PROCESSOR_RETRY_BASE_MS << (attempt - 1).min(4);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(_elapsed) => {
                telemetry.record_retry(stage, InboxFailureClass::Timeout);
                if attempt >= max_attempts {
                    return SettleOutcome::Failed {
                        attempts: attempt,
                        non_retryable: false,
                    };
                }
                let delay_ms = PROCESSOR_RETRY_BASE_MS << (attempt - 1).min(4);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

/// Marks a revision failed after a bounded retry cycle.
async fn fail_cycle(
    store: &InboxRevisionStoreClient,
    revision_id: &crate::models::inbox_revision::InboxRevisionId,
    owner: &str,
    attempts: u32,
    non_retryable: bool,
    message: &str,
) -> ProcessOutcome {
    let class = if non_retryable {
        InboxFailureClass::Validation
    } else {
        InboxFailureClass::OtherTransient
    };
    let outcome = if non_retryable {
        ProcessOutcome::FailedNonRetryable
    } else {
        ProcessOutcome::FailedRetriesExhausted
    };
    let _ = store
        .mark_failed_cycle(revision_id, owner, class, message, attempts, None)
        .await;
    outcome
}

async fn fail_cycle_with_class(
    store: &InboxRevisionStoreClient,
    revision_id: &crate::models::inbox_revision::InboxRevisionId,
    owner: &str,
    attempts: u32,
    class: InboxFailureClass,
    message: &str,
) -> ProcessOutcome {
    let _ = store
        .mark_failed_cycle(revision_id, owner, class, message, attempts, None)
        .await;
    ProcessOutcome::FailedNonRetryable
}

fn classify_failure(err: &MemoryError) -> InboxFailureClass {
    match err {
        MemoryError::Validation(message) if is_corrupt_content(message) => {
            InboxFailureClass::Corrupt
        }
        MemoryError::Validation(_) => InboxFailureClass::Validation,
        MemoryError::Storage(message) if crate::service::is_transient_db_error(err) => {
            InboxFailureClass::Storage
        }
        MemoryError::Storage(message) if message.contains("table") => InboxFailureClass::Storage,
        MemoryError::Storage(_) => InboxFailureClass::Storage,
        MemoryError::Transient(message) if message.contains("model") => InboxFailureClass::Model,
        MemoryError::Transient(message) if message.contains("timeout") => {
            InboxFailureClass::Timeout
        }
        MemoryError::Transient(_) => InboxFailureClass::OtherTransient,
        MemoryError::NotFound(_) => InboxFailureClass::Validation,
        MemoryError::Conflict(_) => InboxFailureClass::Validation,
        MemoryError::ConfigMissing(_) | MemoryError::ConfigInvalid(_) => {
            InboxFailureClass::Validation
        }
        MemoryError::BudgetExhausted(_) => InboxFailureClass::Validation,
    }
}

fn is_corrupt_content(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("corrupt")
        || lowered.contains("invalid zip")
        || lowered.contains("failed to parse")
}

/// A failure is retryable by the processor only when it is a transient
/// model/extractor/io/timeout class, and never when it is a storage error that
/// already exhausted the storage-layer retry policy (`storage/client.rs` owns
/// DB query retries; the processor must not multiply them).
fn is_retryable(err: &MemoryError, class: InboxFailureClass) -> bool {
    if let MemoryError::Storage(_) = err {
        return crate::service::is_transient_db_error(err);
    }
    is_transient_class(class)
}

fn is_transient_class(class: InboxFailureClass) -> bool {
    matches!(
        class,
        InboxFailureClass::Io
            | InboxFailureClass::Storage
            | InboxFailureClass::Model
            | InboxFailureClass::Timeout
            | InboxFailureClass::Channel
            | InboxFailureClass::OtherTransient
    )
}

/// Stable structured-log label for a processing outcome.
fn outcome_label(outcome: ProcessOutcome) -> &'static str {
    match outcome {
        ProcessOutcome::Processed => "processed",
        ProcessOutcome::FailedNonRetryable => "failed",
        ProcessOutcome::FailedRetriesExhausted => "failed",
        ProcessOutcome::Interrupted => "interrupted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::fs_watch::telemetry::FsWatchTelemetry;
    use crate::service::util::deterministic_episode_id_v2;
    use crate::storage::{DbClient, SurrealDbClient};
    use chrono::Utc;
    use sha2::Digest;
    use std::sync::Arc;

    async fn make_processor_service() -> (MemoryService, Arc<SurrealDbClient>) {
        let db = Arc::new(
            SurrealDbClient::connect_in_memory("fs_processor_test", "org", "warn")
                .await
                .expect("connect in memory"),
        );
        db.apply_migrations("org").await.expect("migrations");
        let service =
            MemoryService::new(db.clone(), "org".to_string(), "warn".to_string(), 50, 100)
                .expect("service");
        (service, db)
    }

    fn make_store(db: Arc<SurrealDbClient>) -> InboxRevisionStoreClient {
        InboxRevisionStoreClient::new(db, "org".to_string())
    }

    fn make_telemetry() -> FsWatchTelemetry {
        FsWatchTelemetry::new()
    }

    #[tokio::test]
    async fn successful_processing_stores_episode_and_marks_processed() {
        let (service, db) = make_processor_service().await;
        let store = make_store(db.clone());
        let content = "Alice Smith reports ARR is $5M.";
        let content_sha256 = hex::encode(sha2::Sha256::digest(content.as_bytes()));
        let t_ref = Utc::now();
        let expected_episode_id = deterministic_episode_id_v2(
            "document",
            &format!("fs:docs/spec.md:{content_sha256}"),
            t_ref,
        );
        let record = crate::storage::inbox_revision_store::new_revision_record(
            "fs:docs/spec.md".to_string(),
            "docs/spec.md".to_string(),
            content_sha256,
            "document".to_string(),
            t_ref,
            content.to_string(),
            expected_episode_id.clone(),
            Utc::now(),
        );
        store.discover_prepared(&record).await.expect("discover");

        let owner = "processor-test";
        let claim = store
            .claim_next(owner, chrono::Duration::seconds(120))
            .await
            .expect("claim")
            .expect("claimable");

        let outcome = process_claimed_revision(&service, &store, claim, &make_telemetry()).await;
        assert_eq!(outcome, ProcessOutcome::Processed);

        // Episode exists with lineage.
        let episode = db
            .select_one(&expected_episode_id, "org")
            .await
            .expect("select")
            .expect("episode");
        assert_eq!(
            episode.get("source_lineage").and_then(|v| v.as_str()),
            Some("fs:docs/spec.md")
        );

        // Revision row is processed and snapshot cleared.
        let row = db
            .select_one(record.revision_id.as_str(), "org")
            .await
            .expect("select row")
            .expect("row");
        assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("processed"));
        assert!(row.get("prepared_content").is_none());
    }

    #[tokio::test]
    async fn transient_extractor_failures_are_retried_within_bounds() {
        use crate::service::EntityExtractor;
        use crate::service::embedding::DisabledEmbeddingProvider;
        use crate::service::entity_extraction::NerScheduling;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct FlakyExtractor {
            failed_once: AtomicBool,
        }

        #[async_trait::async_trait]
        impl EntityExtractor for FlakyExtractor {
            fn provider_name(&self) -> &'static str {
                "flaky"
            }

            fn scheduling(&self) -> NerScheduling {
                NerScheduling::Inline
            }

            async fn extract_candidates(
                &self,
                _content: &str,
            ) -> Result<Vec<crate::models::EntityCandidate>, MemoryError> {
                if !self.failed_once.swap(true, Ordering::SeqCst) {
                    return Err(MemoryError::Transient(
                        "model extraction timed out".to_string(),
                    ));
                }
                Ok(Vec::new())
            }
        }

        let db = Arc::new(
            SurrealDbClient::connect_in_memory("fs_retry_test", "org", "warn")
                .await
                .expect("connect in memory"),
        );
        db.apply_migrations("org").await.expect("migrations");
        let service = MemoryService::new_with_embedding_provider(
            db.clone(),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(DisabledEmbeddingProvider::new(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            )),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(FlakyExtractor {
                failed_once: AtomicBool::new(false),
            }),
        )
        .expect("service");

        let store = make_store(db.clone());
        let t_ref = Utc::now();
        let content = "Alice Smith reports ARR is $5M.";
        let content_hash = hex::encode(sha2::Sha256::digest(content.as_bytes()));
        let expected_episode_id =
            deterministic_episode_id_v2("document", &format!("fs:retry:{content_hash}"), t_ref);
        let record = crate::storage::inbox_revision_store::new_revision_record(
            "fs:retry".to_string(),
            "retry.md".to_string(),
            content_hash,
            "document".to_string(),
            t_ref,
            content.to_string(),
            expected_episode_id,
            Utc::now(),
        );
        store.discover_prepared(&record).await.expect("discover");

        let claim = store
            .claim_next("processor-test", chrono::Duration::seconds(120))
            .await
            .expect("claim")
            .expect("claimable");
        // The flaky extractor fails once, then succeeds; the processor retries
        // within its bounded cycle and reaches Processed.
        let outcome = process_claimed_revision(&service, &store, claim, &make_telemetry()).await;
        assert_eq!(outcome, ProcessOutcome::Processed);
    }

    #[tokio::test]
    async fn retries_exhausted_marks_failed_after_bounded_attempts() {
        use crate::service::EntityExtractor;
        use crate::service::embedding::DisabledEmbeddingProvider;
        use crate::service::entity_extraction::NerScheduling;

        struct AlwaysFailExtractor;

        #[async_trait::async_trait]
        impl EntityExtractor for AlwaysFailExtractor {
            fn provider_name(&self) -> &'static str {
                "always-fail"
            }

            fn scheduling(&self) -> NerScheduling {
                NerScheduling::Inline
            }

            async fn extract_candidates(
                &self,
                _content: &str,
            ) -> Result<Vec<crate::models::EntityCandidate>, MemoryError> {
                Err(MemoryError::Transient(
                    "model extraction timed out".to_string(),
                ))
            }
        }

        let db = Arc::new(
            SurrealDbClient::connect_in_memory("fs_retry_exhaust_test", "org", "warn")
                .await
                .expect("connect in memory"),
        );
        db.apply_migrations("org").await.expect("migrations");
        let service = MemoryService::new_with_embedding_provider(
            db.clone(),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(DisabledEmbeddingProvider::new(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            )),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(AlwaysFailExtractor),
        )
        .expect("service");

        let store = make_store(db.clone());
        let t_ref = Utc::now();
        let content = "Alice Smith reports ARR is $5M.";
        let content_hash = hex::encode(sha2::Sha256::digest(content.as_bytes()));
        let expected_episode_id = deterministic_episode_id_v2(
            "document",
            &format!("fs:retry-exhaust:{content_hash}"),
            t_ref,
        );
        let record = crate::storage::inbox_revision_store::new_revision_record(
            "fs:retry-exhaust".to_string(),
            "retry-exhaust.md".to_string(),
            content_hash,
            "document".to_string(),
            t_ref,
            content.to_string(),
            expected_episode_id,
            Utc::now(),
        );
        store.discover_prepared(&record).await.expect("discover");

        let claim = store
            .claim_next("processor-test", chrono::Duration::seconds(120))
            .await
            .expect("claim")
            .expect("claimable");
        let outcome = process_claimed_revision(&service, &store, claim, &make_telemetry()).await;
        assert_eq!(outcome, ProcessOutcome::FailedRetriesExhausted);

        let row = db
            .select_one(record.revision_id.as_str(), "org")
            .await
            .expect("select row")
            .expect("row");
        assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("failed"));
        assert_eq!(
            row.get("attempt_count").and_then(|v| v.as_u64()),
            Some(MAX_PROCESSOR_ATTEMPTS as u64)
        );
    }
}
