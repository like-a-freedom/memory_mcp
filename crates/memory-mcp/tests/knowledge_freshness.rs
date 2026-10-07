//! The freshness clock tracks writes, not attempts.
//!
//! `memory_knowledge_last_write_timestamp_seconds` is the only series that can
//! answer "how old is what this service knows", and it is the one an
//! unattended deployment depends on: knowledge that arrives from the inbox
//! never passes through an MCP tool, so a stamp kept at the transport would
//! read *absent* for exactly the deployment nobody is watching.
//!
//! That is a claim about wiring, and the unit tests can only scan the source
//! for it. This is the seam that settles it — the ingestion service, against a
//! real store, through the capability an MCP call and the filesystem watcher
//! both go through.
//!
//! The recorder is process-global, and these tests read the same gauge, so they
//! take one lock for the duration — the same discipline the in-crate metric
//! tests use for exactly this reason.

#![cfg(feature = "prometheus")]

use chrono::{DateTime, Utc};
use memory_mcp::models::IngestRequest;
use memory_mcp::observability::{
    METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS, shared_test_handle,
};
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;

mod common;

/// One test at a time against the process-global recorder.
///
/// Cargo runs the tests in a binary on parallel threads, and both of these read
/// the same gauge, so without this the extraction test's capture can land
/// between the duplicate test's two readings and fail it for a reason that has
/// nothing to do with either test.
async fn alone() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn stamp() -> Option<f64> {
    shared_test_handle()
        .expect("prometheus enabled")
        .render()
        .lines()
        .find(|line| {
            line.starts_with(METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS)
                && line[METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS.len()..].starts_with(' ')
        })
        .and_then(|line| line.split_whitespace().last())
        .and_then(|value| value.parse().ok())
}

fn request(source_id: &str) -> IngestRequest {
    IngestRequest {
        source_type: "meeting".to_string(),
        source_id: source_id.to_string(),
        content: "Standup with Alice about the Q3 launch. Bob owns the rollout plan.".to_string(),
        t_ref: "2026-03-01T10:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("timestamp parses"),
        t_ingested: None,
        policy_tags: vec![],
    }
}

/// Extraction is the second write, and it is the one that makes the *graph*
/// — so it stamps too.
#[tokio::test]
async fn extracting_advances_the_knowledge_clock() {
    let _alone = alone().await;
    shared_test_handle().expect("prometheus enabled");

    let service = common::make_service().await;
    let episode_id =
        IngestCapability::ingest_from_service(&service, request("freshness-extract"), None)
            .await
            .expect("the episode is stored first");

    let before = stamp().expect("the capture above stamped the clock");
    ExtractCapability::extract_from_service(&service, &episode_id, None, None)
        .await
        .expect("extraction writes facts, entities and links");
    let after = stamp().expect("extraction stamped the clock");

    // Strictly greater, and the only thing between the two readings is the
    // extraction: the two calls are separated by real store work, so an equal
    // pair means extraction did not stamp. `>=` would pass here even with the
    // extraction stamp deleted, because the capture above already set the
    // gauge — an assertion that cannot fail is not one.
    assert!(
        after > before,
        "extraction is a knowledge write, so the clock must move: {before} -> \
         {after}"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_secs_f64();
    assert!(
        (after - 60.0..=now + 1.0).contains(&after),
        "and it must be the moment of the extraction, not the capture that \
         preceded it: {after} against a clock reading {now}"
    );
}

#[tokio::test]
async fn capturing_makes_the_knowledge_fresh_and_re_ingesting_it_does_not() {
    let _alone = alone().await;
    // Install the process-global recorder *before* anything records into it.
    // The metrics facade is a no-op until one exists, so a capture that ran
    // first would leave a gauge nothing ever set, and this test would be
    // asserting the absence of a call it had skipped past.
    shared_test_handle().expect("prometheus enabled");

    let service = common::make_service().await;

    let episode_id = IngestCapability::ingest_from_service(&service, request("freshness-1"), None)
        .await
        .expect("the first capture writes an episode");
    assert!(episode_id.starts_with("episode:"));

    let after_capture = stamp().unwrap_or_else(|| {
        panic!(
            "a capture that stored an episode must stamp the freshness clock; \
             without it the age is unreadable on every deployment that ingests \
             anything"
        )
    });
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_secs_f64();
    assert!(
        (after_capture - 60.0..=now + 1.0).contains(&after_capture),
        "the stamp must be the moment of the write, not an arbitrary earlier \
         time: {after_capture} against a clock reading {now}"
    );

    // The same source again. The service takes the duplicate branch and writes
    // nothing, so the caller already had this knowledge — and a clock refreshed
    // by re-offering it would report a service that learned something while it
    // was fed the same document on a timer.
    let duplicate = IngestCapability::ingest_from_service(&service, request("freshness-1"), None)
        .await
        .expect("a duplicate source resolves to the existing episode");
    assert_eq!(
        duplicate, episode_id,
        "the second call must resolve to the episode already stored, or this \
         is not the duplicate path this test is about"
    );

    assert_eq!(
        stamp(),
        Some(after_capture),
        "re-ingesting an unchanged source writes nothing, so the freshness \
         clock must not move: a stamp that follows ingest attempts reports a \
         service as learning when it is being fed the same document"
    );
}
