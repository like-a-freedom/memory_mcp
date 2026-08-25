//! In-process integration tests for the filesystem ingestion pipeline:
//! discovery, durable store, and sequential `ingest → extract` processing.

mod common;

use std::sync::Arc;

use chrono::Utc;
use memory_mcp::models::inbox_revision::InboxFailureClass;
use memory_mcp::service::MemoryService;
use memory_mcp::service::fs_watch::processor::{ProcessOutcome, process_claimed_revision};
use memory_mcp::service::fs_watch::telemetry::FsWatchTelemetry;
use memory_mcp::storage::inbox_revision_store::new_revision_record;
use memory_mcp::storage::{DbClient, InboxRevisionStoreClient, SurrealDbClient};
use sha2::Digest;

fn content_sha256(content: &str) -> String {
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

async fn make_pipeline() -> (
    MemoryService,
    Arc<SurrealDbClient>,
    InboxRevisionStoreClient,
) {
    let db = Arc::new(
        SurrealDbClient::connect_in_memory("fs_watch_integration", "org", "warn")
            .await
            .expect("connect in memory"),
    );
    db.apply_migrations("org").await.expect("migrations");
    let service =
        MemoryService::new(db.clone(), "org".to_string(), "warn".to_string(), 50, 100)
            .expect("service");
    let store = InboxRevisionStoreClient::new(db.clone(), "org".to_string());
    (service, db, store)
}

fn make_record(
    lineage: &str,
    relative_path: &str,
    content: &str,
    t_ref: chrono::DateTime<Utc>,
) -> memory_mcp::models::inbox_revision::InboxRevisionRecord {
    let hash = content_sha256(content);
    let expected_episode_id = memory_mcp::service::deterministic_episode_id_v2(
        "document",
        &format!("{lineage}:{hash}"),
        t_ref,
    );
    new_revision_record(
        lineage.to_string(),
        relative_path.to_string(),
        hash,
        "document".to_string(),
        t_ref,
        content.to_string(),
        expected_episode_id,
        Utc::now(),
    )
}

async fn process_first_claim(
    service: &MemoryService,
    store: &InboxRevisionStoreClient,
) -> ProcessOutcome {
    let claim = store
        .claim_next("processor-test", chrono::Duration::seconds(120))
        .await
        .expect("claim")
        .expect("claimable revision");
    process_claimed_revision(service, store, claim, &FsWatchTelemetry::new()).await
}

#[tokio::test]
async fn successful_processing_persists_episode_with_lineage_and_facts() {
    let (service, db, store) = make_pipeline().await;
    let t_ref = Utc::now();
    let content = "Alice Smith reports ARR is $5M.";
    let record = make_record("fs:docs/spec.md", "docs/spec.md", content, t_ref);
    store
        .discover_prepared(&record)
        .await
        .expect("discover");

    let outcome = process_first_claim(&service, &store).await;
    assert_eq!(outcome, ProcessOutcome::Processed);

    let expected_episode_id =
        memory_mcp::service::deterministic_episode_id_v2(
            "document",
            &format!("fs:docs/spec.md:{}", content_sha256(content)),
            t_ref,
        );
    let expected_source_id = format!("fs:docs/spec.md:{}", content_sha256(content));
    let episode = db
        .select_one(&expected_episode_id, "org")
        .await
        .expect("select episode")
        .expect("episode exists");
    assert_eq!(
        episode.get("source_lineage").and_then(|v| v.as_str()),
        Some("fs:docs/spec.md")
    );
    assert_eq!(
        episode.get("source_id").and_then(|v| v.as_str()),
        Some(expected_source_id.as_str())
    );

    // Facts were extracted.
    let facts = db
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $ep",
            Some(serde_json::json!({"ep": expected_episode_id})),
            "org",
        )
        .await
        .expect("query facts");
    assert!(
        !facts.as_array().is_none_or(|rows| rows.is_empty()),
        "expected extracted facts"
    );

    // Revision is processed with snapshot cleared.
    let row = db
        .select_one(record.revision_id.as_str(), "org")
        .await
        .expect("select row")
        .expect("row exists");
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("processed"));
    assert!(row.get("prepared_content").is_none());
}

#[tokio::test]
async fn rediscovery_of_same_bytes_does_not_create_second_episode() {
    let (service, db, store) = make_pipeline().await;
    let t_ref = Utc::now();
    let content = "Alice Smith reports ARR is $5M.";
    let record = make_record("fs:docs/spec.md", "docs/spec.md", content, t_ref);

    store
        .discover_prepared(&record)
        .await
        .expect("discover 1");
    assert_eq!(process_first_claim(&service, &store).await, ProcessOutcome::Processed);

    // Rediscovery of identical bytes returns the same (now processed) row.
    let (again, created) = store
        .discover_prepared(&record)
        .await
        .expect("discover 2");
    assert!(!created);
    assert_eq!(
        again.state.as_str(),
        "processed",
        "same bytes must not re-enqueue"
    );

    // Only one episode.
    let count: usize = db
        .query(
            "SELECT count() AS cnt FROM episode WHERE source_lineage = 'fs:docs/spec.md'",
            None,
            "org",
        )
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.first()
                .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                .unwrap_or(0) as usize
        })
        .unwrap_or(0);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn crash_after_discovery_recovers_from_prepared_snapshot() {
    let (service, db, store) = make_pipeline().await;
    let t_ref = Utc::now();
    let content = "Alice Smith reports ARR is $5M.";
    let record = make_record("fs:crash", "crash.md", content, t_ref);
    store
        .discover_prepared(&record)
        .await
        .expect("discover");

    // Simulate a crash after discovery but before ingest: lease expires and
    // the revision is requeued, still carrying the durable snapshot.
    let claim = store
        .claim_next("crashed-worker", chrono::Duration::seconds(1))
        .await
        .expect("claim")
        .expect("claimable");
    assert_eq!(claim.prepared_content, content);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    store
        .requeue_expired_leases()
        .await
        .expect("requeue");

    let outcome = process_first_claim(&service, &store).await;
    assert_eq!(outcome, ProcessOutcome::Processed);

    // The source path is irrelevant after discovery: processing used the
    // snapshot, so the episode exists regardless of the filesystem.
    let expected_episode_id =
        memory_mcp::service::deterministic_episode_id_v2(
            "document",
            &format!("fs:crash:{}", content_sha256(content)),
            t_ref,
        );
    assert!(
        db.select_one(&expected_episode_id, "org")
            .await
            .expect("select episode")
            .is_some()
    );
}

#[tokio::test]
async fn deterministic_failures_do_not_retry_in_the_cycle() {
    let (service, db, store) = make_pipeline().await;
    let t_ref = Utc::now();
    // JSON is unsupported; discovery is only possible when the content parses,
    // so this test simulates a validation failure by recording a corrupt
    // prepared snapshot directly.
    let record = new_revision_record(
        "fs:corrupt".to_string(),
        "corrupt.md".to_string(),
        content_sha256("corrupt"),
        "document".to_string(),
        t_ref,
        String::new(),
        "episode:corrupt".to_string(),
        Utc::now(),
    );
    store
        .discover_prepared(&record)
        .await
        .expect("discover");

    let claim = store
        .claim_next("processor-test", chrono::Duration::seconds(120))
        .await
        .expect("claim")
        .expect("claimable");
    // Empty prepared content fails validation deterministically on ingest.
    let outcome =
        process_claimed_revision(&service, &store, claim, &FsWatchTelemetry::new()).await;
    assert_eq!(outcome, ProcessOutcome::FailedNonRetryable);

    let row = db
        .select_one(record.revision_id.as_str(), "org")
        .await
        .expect("select row")
        .expect("row");
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("failed"));
    assert_eq!(
        row.get("failure_class").and_then(|v| v.as_str()),
        Some(InboxFailureClass::Validation.as_str())
    );
}

#[tokio::test]
async fn one_failed_revision_does_not_prevent_next_revision() {
    let (service, db, store) = make_pipeline().await;
    let t_ref = Utc::now();

    // First revision fails deterministically (empty content).
    let bad = new_revision_record(
        "fs:bad".to_string(),
        "bad.md".to_string(),
        content_sha256("bad"),
        "document".to_string(),
        t_ref,
        String::new(),
        "episode:bad".to_string(),
        Utc::now(),
    );
    store.discover_prepared(&bad).await.expect("discover bad");

    // Second revision processes successfully.
    let good = make_record("fs:good", "good.md", "Alice Smith reports ARR is $5M.", t_ref);
    store
        .discover_prepared(&good)
        .await
        .expect("discover good");

    // Process whatever is claimable, in any order: both must reach a terminal
    // state and neither failure may block the other.
    for _ in 0..2 {
        let claim = store
            .claim_next("processor-test", chrono::Duration::seconds(120))
            .await
            .expect("claim")
            .expect("claimable");
        let _ = process_claimed_revision(&service, &store, claim, &FsWatchTelemetry::new()).await;
    }

    let bad_row = db
        .select_one(bad.revision_id.as_str(), "org")
        .await
        .expect("select bad row")
        .expect("bad row exists");
    assert_eq!(
        bad_row.get("state").and_then(|v| v.as_str()),
        Some("failed")
    );
    let good_row = db
        .select_one(good.revision_id.as_str(), "org")
        .await
        .expect("select good row")
        .expect("good row exists");
    assert_eq!(
        good_row.get("state").and_then(|v| v.as_str()),
        Some("processed")
    );
}

// ─── Tracked runtime ──────────────────────────────────────────────────────────

#[tokio::test]
async fn runtime_startup_scan_enqueues_and_processes_existing_files() {
    use memory_mcp::config::fs_watch::FsWatchConfig;

    let inbox = tempfile::tempdir().expect("temp inbox");
    std::fs::write(
        inbox.path().join("spec.md"),
        "Alice Smith reports ARR is $5M.",
    )
    .expect("write markdown");
    std::fs::write(inbox.path().join("ignored.json"), "{}").expect("write json");

    let (service, db, _store) = make_pipeline().await;
    let runtime = service
        .start_fs_watch(FsWatchConfig {
            inbox: inbox.path().to_path_buf(),
        })
        .await
        .expect("start runtime");

    // Wait for the scan + processor to complete the supported file.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut processed = false;
    while tokio::time::Instant::now() < deadline {
        let rows = db
            .query(
                "SELECT count() AS cnt FROM inbox_revision WHERE state = 'processed'",
                None,
                "org",
            )
            .await
            .expect("count processed");
        let count = rows
            .as_array()
            .and_then(|rows| rows.first())
            .and_then(|r| r.get("cnt"))
            .and_then(|c| c.as_i64())
            .unwrap_or(0);
        if count >= 1 {
            processed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(processed, "startup scan should process the supported file");

    // The episode exists with lineage.
    let episodes = db
        .query(
            "SELECT count() AS cnt FROM episode WHERE source_lineage = 'fs:spec.md'",
            None,
            "org",
        )
        .await
        .expect("count episodes");
    let count = episodes
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|r| r.get("cnt"))
        .and_then(|c| c.as_i64())
        .unwrap_or(0);
    assert_eq!(count, 1, "exactly one episode for the supported file");

    // The unsupported file is skipped.
    let unsupported = db
        .query(
            "SELECT count() AS cnt FROM inbox_revision WHERE relative_path = 'ignored.json'",
            None,
            "org",
        )
        .await
        .expect("count unsupported");
    let count = unsupported
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|r| r.get("cnt"))
        .and_then(|c| c.as_i64())
        .unwrap_or(0);
    assert_eq!(count, 0, "unsupported file must be skipped");

    runtime.shutdown().await;
}

#[tokio::test]
async fn runtime_drops_supported_file_event_and_shutdown_is_bounded() {
    use memory_mcp::config::fs_watch::FsWatchConfig;

    let inbox = tempfile::tempdir().expect("temp inbox");
    let (service, db, _store) = make_pipeline().await;
    let runtime = service
        .start_fs_watch(FsWatchConfig {
            inbox: inbox.path().to_path_buf(),
        })
        .await
        .expect("start runtime");

    // Dropping a supported file into the inbox is picked up by the watcher.
    std::fs::write(
        inbox.path().join("event.md"),
        "Alice Smith reports ARR is $5M.",
    )
    .expect("write file");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut processed = false;
    while tokio::time::Instant::now() < deadline {
        let rows = db
            .query(
                "SELECT count() AS cnt FROM inbox_revision WHERE relative_path = 'event.md' AND state = 'processed'",
                None,
                "org",
            )
            .await
            .expect("count processed");
        let count = rows
            .as_array()
            .and_then(|rows| rows.first())
            .and_then(|r| r.get("cnt"))
            .and_then(|c| c.as_i64())
            .unwrap_or(0);
        if count >= 1 {
            processed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(processed, "watcher event should be processed");

    // Shutdown must complete promptly and cleanly.
    let shutdown = tokio::time::timeout(std::time::Duration::from_secs(35), runtime.shutdown())
        .await
        .expect("bounded shutdown");
    assert!(shutdown.waited_secs <= 30);
}
