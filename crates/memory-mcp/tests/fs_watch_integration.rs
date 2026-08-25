//! In-process integration tests for the filesystem ingestion pipeline:
//! discovery, durable store, and sequential `ingest → extract` processing.

#![cfg(feature = "fs-watch")]

mod common;

use std::sync::Arc;

use chrono::Utc;
use memory_mcp::models::inbox_revision::InboxFailureClass;
use memory_mcp::service::MemoryService;
use memory_mcp::service::fs_watch::candidate::CandidateOutcome;
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
    let service = MemoryService::new(db.clone(), "org".to_string(), "warn".to_string(), 50, 100)
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
    store.discover_prepared(&record).await.expect("discover");

    let outcome = process_first_claim(&service, &store).await;
    assert_eq!(outcome, ProcessOutcome::Processed);

    let expected_episode_id = memory_mcp::service::deterministic_episode_id_v2(
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

    store.discover_prepared(&record).await.expect("discover 1");
    assert_eq!(
        process_first_claim(&service, &store).await,
        ProcessOutcome::Processed
    );

    // Rediscovery of identical bytes returns the same (now processed) row.
    let (again, created) = store.discover_prepared(&record).await.expect("discover 2");
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
    store.discover_prepared(&record).await.expect("discover");

    // Simulate a crash after discovery but before ingest: lease expires and
    // the revision is requeued, still carrying the durable snapshot.
    let claim = store
        .claim_next("crashed-worker", chrono::Duration::seconds(1))
        .await
        .expect("claim")
        .expect("claimable");
    assert_eq!(claim.prepared_content, content);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    store.requeue_expired_leases().await.expect("requeue");

    let outcome = process_first_claim(&service, &store).await;
    assert_eq!(outcome, ProcessOutcome::Processed);

    // The source path is irrelevant after discovery: processing used the
    // snapshot, so the episode exists regardless of the filesystem.
    let expected_episode_id = memory_mcp::service::deterministic_episode_id_v2(
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
    store.discover_prepared(&record).await.expect("discover");

    let claim = store
        .claim_next("processor-test", chrono::Duration::seconds(120))
        .await
        .expect("claim")
        .expect("claimable");
    // Empty prepared content fails validation deterministically on ingest.
    let outcome = process_claimed_revision(&service, &store, claim, &FsWatchTelemetry::new()).await;
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
    let good = make_record(
        "fs:good",
        "good.md",
        "Alice Smith reports ARR is $5M.",
        t_ref,
    );
    store.discover_prepared(&good).await.expect("discover good");

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

// ─── Acceptance scenarios ─────────────────────────────────────────────────────

/// Waits until `state = processed` for the revision at `relative_path`.
async fn wait_for_processed(db: &Arc<SurrealDbClient>, relative_path: &str, timeout_secs: u64) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while tokio::time::Instant::now() < deadline {
        let rows = db
            .query(
                "SELECT count() AS cnt FROM inbox_revision WHERE relative_path = $p AND state = 'processed'",
                Some(serde_json::json!({"p": relative_path})),
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
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("revision `{relative_path}` was not processed in time");
}

/// Overwrites a file atomically via a temp sibling + rename so the change is
/// observable by filesystem watchers (mtime-only content writes can be
/// coalesced away by FSEvents on macOS).
fn atomic_write(path: &std::path::Path, content: &str) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).expect("write temp");
    std::fs::rename(&tmp, path).expect("rename into place");
}

/// Waits until `state = processed` for the revision at `relative_path`.

#[tokio::test]
async fn two_revisions_of_same_path_keep_one_lineage_and_two_episodes() {
    use memory_mcp::config::fs_watch::FsWatchConfig;

    let inbox = tempfile::tempdir().expect("temp inbox");
    let path = inbox.path().join("spec.md");
    std::fs::write(&path, "Alice Smith reports ARR is $5M.").expect("write v1");

    let (service, db, store) = make_pipeline().await;
    let config = FsWatchConfig {
        inbox: inbox.path().to_path_buf(),
    };
    let cancel = tokio_util::sync::CancellationToken::new();

    // Version one: discover + process through the shared pipeline.
    let v1 =
        memory_mcp::service::fs_watch::candidate::prepare_candidate(&config, &path, None, &cancel)
            .await
            .expect("prepare v1");
    let CandidateOutcome::Ready(v1) = v1 else {
        panic!("expected ready");
    };
    let record_v1 = new_revision_record(
        v1.lineage.clone(),
        v1.relative_path.clone(),
        v1.content_sha256.clone(),
        v1.source_type.clone(),
        v1.t_ref,
        v1.prepared_content.clone(),
        memory_mcp::service::deterministic_episode_id_v2(&v1.source_type, &v1.source_id, v1.t_ref),
        Utc::now(),
    );
    store
        .discover_prepared(&record_v1)
        .await
        .expect("discover v1");
    assert_eq!(
        process_first_claim(&service, &store).await,
        ProcessOutcome::Processed
    );

    // Overwrite with version two while preserving the relative path.
    atomic_write(&path, "Alice Smith reports ARR is $6M.");
    let v2 =
        memory_mcp::service::fs_watch::candidate::prepare_candidate(&config, &path, None, &cancel)
            .await
            .expect("prepare v2");
    let CandidateOutcome::Ready(v2) = v2 else {
        panic!("expected ready");
    };
    let record_v2 = new_revision_record(
        v2.lineage.clone(),
        v2.relative_path.clone(),
        v2.content_sha256.clone(),
        v2.source_type.clone(),
        v2.t_ref,
        v2.prepared_content.clone(),
        memory_mcp::service::deterministic_episode_id_v2(&v2.source_type, &v2.source_id, v2.t_ref),
        Utc::now(),
    );
    store
        .discover_prepared(&record_v2)
        .await
        .expect("discover v2");
    assert_eq!(
        process_first_claim(&service, &store).await,
        ProcessOutcome::Processed
    );

    // Two immutable revision rows, both processed.
    let rows = db
        .query(
            "SELECT revision_id, content_sha256 FROM inbox_revision WHERE relative_path = 'spec.md' ORDER BY content_sha256",
            None,
            "org",
        )
        .await
        .expect("select revisions");
    let revision_count = rows.as_array().map(Vec::len).unwrap_or(0);
    assert_eq!(revision_count, 2, "expected two revision rows");

    // Two episodes, one stable lineage, two distinct source ids.
    let episodes = db
        .query(
            "SELECT episode_id, source_id, source_lineage FROM episode WHERE source_lineage = 'fs:spec.md' ORDER BY episode_id",
            None,
            "org",
        )
        .await
        .expect("select episodes");
    let episodes = episodes.as_array().cloned().unwrap_or_default();
    assert_eq!(episodes.len(), 2, "expected two episodes for two revisions");
    let source_ids: std::collections::HashSet<String> = episodes
        .iter()
        .filter_map(|e| {
            e.get("source_id")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .collect();
    assert_eq!(
        source_ids.len(),
        2,
        "source ids must be distinct per revision"
    );
    for episode in &episodes {
        assert_eq!(
            episode.get("source_lineage").and_then(|v| v.as_str()),
            Some("fs:spec.md")
        );
    }

    // Deleting the file must not invalidate facts: both revision facts stay
    // persisted (the second fact may supersede the first via reconciliation,
    // which is expected — deletion itself must add no invalidation).
    let facts_before = db
        .query("SELECT fact_id FROM fact", None, "org")
        .await
        .expect("count facts");
    let before = facts_before.as_array().map(Vec::len).unwrap_or(0);
    assert!(before >= 2, "both revisions must produce facts");

    std::fs::remove_file(&path).expect("remove file");
    let facts_after = db
        .query("SELECT fact_id FROM fact", None, "org")
        .await
        .expect("count facts after delete");
    let after = facts_after.as_array().map(Vec::len).unwrap_or(0);
    assert_eq!(
        before, after,
        "deleting the file must not invalidate any fact"
    );
}

#[tokio::test]
async fn rename_starts_a_new_lineage() {
    use memory_mcp::config::fs_watch::FsWatchConfig;

    let inbox = tempfile::tempdir().expect("temp inbox");
    let original = inbox.path().join("original.md");
    std::fs::write(&original, "Alice Smith reports ARR is $5M.").expect("write");

    let (service, db, _store) = make_pipeline().await;
    let runtime = service
        .start_fs_watch(FsWatchConfig {
            inbox: inbox.path().to_path_buf(),
        })
        .await
        .expect("start runtime");

    wait_for_processed(&db, "original.md", 15).await;

    // Rename: a new lineage begins.
    std::fs::rename(&original, inbox.path().join("renamed.md")).expect("rename");
    wait_for_processed(&db, "renamed.md", 15).await;

    let lineages = db
        .query(
            "SELECT source_lineage FROM episode ORDER BY source_lineage",
            None,
            "org",
        )
        .await
        .expect("select lineages");
    let lineages: std::collections::HashSet<String> = lineages
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            e.get("source_lineage")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .collect();
    assert!(
        lineages.contains("fs:original.md") && lineages.contains("fs:renamed.md"),
        "rename must produce a new lineage, got {lineages:?}"
    );

    runtime.shutdown().await;
}

#[tokio::test]
async fn crash_before_record_episode_recovers_through_expected_episode_id() {
    let (service, db, store) = make_pipeline().await;
    let t_ref = Utc::now();
    let content = "Alice Smith reports ARR is $5M.";
    let hash = content_sha256(content);
    let expected_episode_id = memory_mcp::service::deterministic_episode_id_v2(
        "document",
        &format!("fs:crash2:{hash}"),
        t_ref,
    );
    let record = new_revision_record(
        "fs:crash2".to_string(),
        "crash2.md".to_string(),
        hash.clone(),
        "document".to_string(),
        t_ref,
        content.to_string(),
        expected_episode_id.clone(),
        Utc::now(),
    );
    store.discover_prepared(&record).await.expect("discover");

    // Simulate a crash after deterministic episode creation but before
    // `record_episode`: the episode exists, the revision is still processing.
    let owner = "crashed-worker";
    let claim = store
        .claim_next(owner, chrono::Duration::seconds(1))
        .await
        .expect("claim")
        .expect("claimable");
    // Create the episode directly (as ingest would have before the crash).
    db.create(
        &expected_episode_id,
        serde_json::json!({
            "episode_id": expected_episode_id,
            "source_type": "document",
            "source_id": format!("fs:crash2:{hash}"),
            "content": content,
            "t_ref": memory_mcp::service::normalize_dt(t_ref),
            "t_ingested": memory_mcp::service::normalize_dt(Utc::now()),
            "policy_tags": [],
            "source_lineage": "fs:crash2",
        }),
        "org",
    )
    .await
    .expect("create episode");
    let _ = claim;
    // Crash: lease expires; the revision is requeued.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    store.requeue_expired_leases().await.expect("requeue");

    // Recovery: deterministic ingest reuses the episode; record_episode and
    // extract complete; mark_processed clears the snapshot.
    let outcome = process_first_claim(&service, &store).await;
    assert_eq!(outcome, ProcessOutcome::Processed);

    let row = db
        .select_one(record.revision_id.as_str(), "org")
        .await
        .expect("select row")
        .expect("row");
    assert_eq!(row.get("state").and_then(|v| v.as_str()), Some("processed"));
    assert_eq!(
        row.get("episode_id").and_then(|v| v.as_str()),
        Some(expected_episode_id.as_str())
    );
}
