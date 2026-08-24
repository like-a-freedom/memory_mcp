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

use chrono::Utc;

use crate::error::MemoryError;
use crate::models::inbox_revision::{
    ClaimedInboxRevision, InboxFailureClass, InboxProcessingStage, InboxRevisionLease,
};
use crate::service::ingestion::IngestionMetadata;
use crate::service::MemoryService;
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
}

impl InboxRevisionProcessor {
    pub(crate) fn new(
        store: InboxRevisionStoreClient,
        service: MemoryService,
        telemetry: FsWatchTelemetry,
        stop_dequeue: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            store,
            service,
            telemetry,
            stop_dequeue,
        }
    }

    /// Sequential dequeue loop; exits when `stop_dequeue` is cancelled.
    pub(crate) async fn run(&self) {
        loop {
            if self.stop_dequeue.is_cancelled() {
                break;
            }
            let owner = format!("processor-{}", std::process::id());
            let claim = match self
                .store
                .claim_next(&owner, chrono::Duration::seconds(120))
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
            let outcome = process_claimed_revision(&self.service, &self.store, claim, &self.telemetry).await;
            match outcome {
                ProcessOutcome::Processed => {}
                ProcessOutcome::FailedNonRetryable
                | ProcessOutcome::FailedRetriesExhausted
                | ProcessOutcome::Interrupted => {}
            }
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

    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let attempt_result = tokio::time::timeout(
            PROCESSOR_ATTEMPT_TIMEOUT,
            run_ingest_then_extract(
                service,
                store,
                &revision_id,
                &owner,
                &prepared_content,
                &source_type,
                &lineage,
                &content_hash,
                &log_source_id,
                &expected_episode_id,
                t_ref,
            ),
        )
        .await;

        match attempt_result {
            Ok(Ok(())) => {
                telemetry.record_success();
                let _ = store
                    .mark_processed(&revision_id, &owner)
                    .await;
                return ProcessOutcome::Processed;
            }
            Ok(Err(err)) => {
                let class = classify_failure(&err);
                telemetry.record_retry("ingest", class);
                if !is_transient_class(class) {
                    let _ = store
                        .mark_failed_cycle(
                            &revision_id,
                            &owner,
                            class,
                            &err.to_string(),
                            attempt,
                            None,
                        )
                        .await;
                    return ProcessOutcome::FailedNonRetryable;
                }
                if attempt >= MAX_PROCESSOR_ATTEMPTS {
                    let _ = store
                        .mark_failed_cycle(
                            &revision_id,
                            &owner,
                            class,
                            &err.to_string(),
                            attempt,
                            None,
                        )
                        .await;
                    return ProcessOutcome::FailedRetriesExhausted;
                }
                let delay_ms = PROCESSOR_RETRY_BASE_MS << (attempt - 1).min(4);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(_elapsed) => {
                let class = InboxFailureClass::Timeout;
                telemetry.record_retry("ingest", class);
                if attempt >= MAX_PROCESSOR_ATTEMPTS {
                    let _ = store
                        .mark_failed_cycle(
                            &revision_id,
                            &owner,
                            class,
                            "processor attempt timed out",
                            attempt,
                            None,
                        )
                        .await;
                    return ProcessOutcome::FailedRetriesExhausted;
                }
                let delay_ms = PROCESSOR_RETRY_BASE_MS << (attempt - 1).min(4);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

/// Runs internal `ingest → extract` for a claimed revision.
///
/// Returns `Ok(())` only when the episode exists (or was created) and extract
/// completed successfully.
async fn run_ingest_then_extract(
    service: &MemoryService,
    store: &InboxRevisionStoreClient,
    revision_id: &crate::models::inbox_revision::InboxRevisionId,
    owner: &str,
    prepared_content: &str,
    source_type: &str,
    lineage: &str,
    content_sha256: &str,
    log_source_id: &str,
    expected_episode_id: &str,
    t_ref: chrono::DateTime<Utc>,
) -> Result<(), MemoryError> {
    let request = crate::models::IngestRequest {
        source_type: source_type.to_string(),
        source_id: format!("{lineage}:{content_sha256}"),
        content: prepared_content.to_string(),
        t_ref,
        t_ingested: None,
        policy_tags: vec![],
    };
    let episode_id = service
        .ingestion_service
        .ingest_with_metadata(
            request,
            None,
            IngestionMetadata {
                source_lineage: Some(lineage.to_string()),
                log_source_id: Some(log_source_id.to_string()),
            },
        )
        .await?;

    // The deterministic episode id must match what we precomputed.
    if episode_id != expected_episode_id {
        return Err(MemoryError::Storage(
            "inbox revision episode id mismatch (storage invariant)".to_string(),
        ));
    }

    store
        .record_episode(revision_id, owner, &episode_id)
        .await?;

    crate::service::capabilities::extract::ExtractCapability::extract(
        &service.build_context(),
        &episode_id,
        None,
        None,
    )
    .await?;
    Ok(())
}

fn classify_failure(err: &MemoryError) -> InboxFailureClass {
    match err {
        MemoryError::Validation(message) if is_corrupt_content(message) => InboxFailureClass::Corrupt,
        MemoryError::Validation(_) => InboxFailureClass::Validation,
        MemoryError::Storage(message) if crate::service::is_transient_db_error(err) => {
            InboxFailureClass::Storage
        }
        MemoryError::Storage(message) if message.contains("table") => InboxFailureClass::Storage,
        MemoryError::Storage(_) => InboxFailureClass::Storage,
        MemoryError::Transient(message) if message.contains("model") => InboxFailureClass::Model,
        MemoryError::Transient(message) if message.contains("timeout") => InboxFailureClass::Timeout,
        MemoryError::Transient(_) => InboxFailureClass::OtherTransient,
        MemoryError::NotFound(_) => InboxFailureClass::Validation,
        MemoryError::Conflict(_) => InboxFailureClass::Validation,
        MemoryError::ConfigMissing(_) | MemoryError::ConfigInvalid(_) => InboxFailureClass::Validation,
        MemoryError::BudgetExhausted(_) => InboxFailureClass::Validation,
    }
}

fn is_corrupt_content(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("corrupt")
        || lowered.contains("invalid zip")
        || lowered.contains("failed to parse")
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

/// Whether a claimed revision still belongs to this processor (ownership check
/// used by shutdown).
#[allow(dead_code)]
pub(crate) fn lease_matches(claim: &ClaimedInboxRevision, owner: &str) -> bool {
    claim.lease.owner == owner
}

/// Releases an interrupted claim (used by bounded shutdown).
#[allow(dead_code)]
pub(crate) async fn release_interrupted_claim(
    store: &InboxRevisionStoreClient,
    lease: &InboxRevisionLease,
) -> Result<(), MemoryError> {
    store
        .release_interrupted(&lease.revision_id, &lease.owner)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::fs_watch::telemetry::FsWatchTelemetry;
    use crate::service::util::deterministic_episode_id_v2;
    use crate::storage::{DbClient, SurrealDbClient};
    use sha2::Digest;
    use std::sync::Arc;

    async fn make_processor_service() -> (MemoryService, Arc<SurrealDbClient>) {
        let db = Arc::new(
            SurrealDbClient::connect_in_memory("fs_processor_test", "org", "warn")
                .await
                .expect("connect in memory"),
        );
        db.apply_migrations("org").await.expect("migrations");
        let service = MemoryService::new(db.clone(), "org".to_string(), "warn".to_string(), 50, 100)
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

        let outcome =
            process_claimed_revision(&service, &store, claim, &make_telemetry()).await;
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
        use crate::service::embedding::DisabledEmbeddingProvider;
        use crate::service::entity_extraction::NerScheduling;
        use crate::service::EntityExtractor;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

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
        let expected_episode_id = deterministic_episode_id_v2(
            "document",
            &format!("fs:retry:{content_hash}"),
            t_ref,
        );
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
        let outcome =
            process_claimed_revision(&service, &store, claim, &make_telemetry()).await;
        assert_eq!(outcome, ProcessOutcome::Processed);
    }

    #[tokio::test]
    async fn retries_exhausted_marks_failed_after_bounded_attempts() {
        use crate::service::embedding::DisabledEmbeddingProvider;
        use crate::service::entity_extraction::NerScheduling;
        use crate::service::EntityExtractor;

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
        let outcome =
            process_claimed_revision(&service, &store, claim, &make_telemetry()).await;
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
