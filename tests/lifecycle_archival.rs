//! Integration tests for lifecycle archival background worker.
//!
//! These tests verify that the archival worker correctly archives episodes
//! that are older than the threshold and have no active facts.
//!
//! Note: These tests require `--test-threads=1` due to embedded SurrealDB lock.
//! Run with: cargo test lifecycle_archival -- --test-threads=1

use chrono::{Duration, Utc};
use memory_mcp::MemoryService;
use memory_mcp::service::lifecycle::run_archival_pass;
use memory_mcp::storage::DbClient;
use serde_json::json;

mod common;

#[tokio::test]
async fn archival_pass_processes_all_configured_namespaces() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - Duration::days(150);

    let episode_id = service
        .ingest(
            memory_mcp::models::IngestRequest {
                source_type: "meeting".to_string(),
                source_id: "personal-archival-1".to_string(),
                content: "Personal archival candidate".to_string(),
                t_ref: old_date,
                scope: "personal".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest episode");

    let fact_id = common::add_fact(
        &service,
        "note",
        "Personal archival fact",
        "Personal archival fact",
        &episode_id,
        old_date,
        "personal",
        0.2,
        vec![],
        vec![],
        json!({}),
    )
    .await
    .expect("add fact");

    service
        .invalidate(
            memory_mcp::models::InvalidateRequest {
                fact_id,
                reason: "prepare archival".to_string(),
                t_invalid: Utc::now(),
            },
            None,
        )
        .await
        .expect("invalidate fact");

    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    assert_eq!(count, 1, "archival should include non-default namespaces");

    let episode = db_client
        .select_one(&episode_id, "personal")
        .await
        .expect("select episode")
        .expect("stored episode");
    assert_eq!(episode.get("status"), Some(&json!("archived")));
}

#[tokio::test]
async fn archival_pass_when_episode_fact_was_recently_accessed_then_skips_archival() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - Duration::days(150);

    let episode_id = service
        .ingest(
            memory_mcp::models::IngestRequest {
                source_type: "meeting".to_string(),
                source_id: "personal-archival-hot-1".to_string(),
                content: "Personal hot archival candidate".to_string(),
                t_ref: old_date,
                scope: "personal".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest episode");

    let fact_id = common::add_fact(
        &service,
        "note",
        "Personal hot archival fact",
        "Personal hot archival fact",
        &episode_id,
        old_date,
        "personal",
        0.2,
        vec![],
        vec![],
        json!({}),
    )
    .await
    .expect("add fact");

    service
        .invalidate(
            memory_mcp::models::InvalidateRequest {
                fact_id: fact_id.clone(),
                reason: "prepare archival".to_string(),
                t_invalid: Utc::now(),
            },
            None,
        )
        .await
        .expect("invalidate fact");

    db_client
        .update(
            &fact_id,
            json!({
                "access_count": 5,
                "last_accessed": memory_mcp::service::normalize_dt(Utc::now()),
            }),
            "personal",
        )
        .await
        .expect("touch fact");

    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    assert_eq!(
        count, 0,
        "episode with recently accessed fact should stay live"
    );

    let episode = db_client
        .select_one(&episode_id, "personal")
        .await
        .expect("select episode")
        .expect("stored episode");
    assert_ne!(episode.get("status"), Some(&json!("archived")));
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn archival_pass_with_empty_database() {
    // Setup
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    // Act: Run archival pass
    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    // Assert: Should complete successfully with 0 archives on empty DB
    assert_eq!(count, 0, "Empty database should archive 0 episodes");
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn archival_pass_preserves_recent_episodes() {
    // Setup: Create a recent episode (should not be archived)
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let recent_date = Utc::now() - Duration::days(10);

    common::add_fact(
        &service,
        "promise",
        "recent promise content",
        "recent promise",
        "episode:recent_archival_test",
        recent_date,
        "test_archival_recent",
        0.9,
        vec![],
        vec![],
        json!({}),
    )
    .await
    .expect("fact added");

    // Act: Run archival pass with 90 day threshold
    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    // Assert: Recent episode should not be archived
    assert_eq!(count, 0, "Recent episode should not be archived");
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn archival_pass_archives_old_episodes_without_active_facts() {
    // Setup: Create an old episode with a fact that gets invalidated
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let old_date = Utc::now() - Duration::days(150);

    // Add an old fact
    let fact_id = common::add_fact(
        &service,
        "promise",
        "old promise for archival test",
        "old promise",
        "episode:old_archival_test",
        old_date,
        "test_archival_old",
        0.3, // low confidence, will decay
        vec![],
        vec![],
        json!({}),
    )
    .await
    .expect("fact added");

    // Invalidate the fact first (so episode has no active facts)
    service
        .invalidate(
            memory_mcp::models::InvalidateRequest {
                fact_id: fact_id.clone(),
                reason: "test invalidation".to_string(),
                t_invalid: Utc::now(),
            },
            None,
        )
        .await
        .expect("fact invalidated");

    // Act: Run archival pass with 90 day threshold
    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    // Assert: Old episode without active facts should be archived
    assert!(
        count >= 1,
        "Old episode without active facts should be archived"
    );
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn archival_pass_respects_age_threshold() {
    // Setup: Create episodes with different ages
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    // Episode just under threshold (should not be archived)
    let just_under = Utc::now() - Duration::days(89);
    common::add_fact(
        &service,
        "metric",
        "metric just under threshold",
        "metric under",
        "episode:under_threshold",
        just_under,
        "test_archival_boundary",
        0.5,
        vec![],
        vec![],
        json!({}),
    )
    .await
    .expect("fact added");

    // Episode well over threshold (should be archived if no active facts)
    let well_over = Utc::now() - Duration::days(200);
    let fact_id = common::add_fact(
        &service,
        "metric",
        "metric well over threshold",
        "metric over",
        "episode:over_threshold",
        well_over,
        "test_archival_boundary",
        0.2,
        vec![],
        vec![],
        json!({}),
    )
    .await
    .expect("fact added");

    // Invalidate the old fact so episode can be archived
    service
        .invalidate(
            memory_mcp::models::InvalidateRequest {
                fact_id,
                reason: "test".to_string(),
                t_invalid: Utc::now(),
            },
            None,
        )
        .await
        .expect("fact invalidated");

    // Act: Run archival pass with 90 day threshold
    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    // Assert: Should archive the old episode
    assert!(count >= 1, "Should archive episode over threshold");
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn archival_pass_batch_limit_respected() {
    // Setup: Create many old episodes
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let old_date = Utc::now() - Duration::days(200);

    // Create multiple old episodes with invalidated facts
    for i in 0..10 {
        let fact_id = common::add_fact(
            &service,
            "metric",
            &format!("old metric {}", i),
            &format!("metric {}", i),
            &format!("episode:batch_{}", i),
            old_date,
            "test_archival_batch",
            0.2,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .expect("fact added");

        // Invalidate to allow archival
        service
            .invalidate(
                memory_mcp::models::InvalidateRequest {
                    fact_id,
                    reason: "test".to_string(),
                    t_invalid: Utc::now(),
                },
                None,
            )
            .await
            .expect("fact invalidated");
    }

    // Act: Run archival pass
    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    // Assert: Should archive episodes (up to batch limit of 500)
    assert!(count > 0, "Should archive some episodes");
    assert!(count <= 500, "Should respect batch limit");
}
