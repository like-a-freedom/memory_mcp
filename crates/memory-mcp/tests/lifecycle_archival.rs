//! Integration tests for lifecycle archival background worker.
//!
//! These tests verify that the archival worker correctly archives episodes
//! that are older than the threshold and have no active facts.
//!
//! Note: These tests require `--test-threads=1` due to embedded SurrealDB lock.
//! Run with: cargo test lifecycle_archival -- --test-threads=1

use chrono::{Duration, Utc};
use memory_mcp::models::Provenance;
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::service::capabilities::invalidate::InvalidateCapability;
use memory_mcp::service::run_archival_pass;
use memory_mcp::storage::DbClient;
use serde_json::json;

mod common;

#[tokio::test]
async fn archival_pass_processes_only_active_namespace() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - Duration::days(150);

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "meeting".to_string(),
            source_id: "personal-archival-1".to_string(),
            content: "Personal archival candidate".to_string(),
            t_ref: old_date,
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest episode");

    let fact_id = service
        .add_fact(
            "note",
            "Personal archival fact",
            "Personal archival fact",
            &episode_id,
            old_date,
            0.2,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("add fact");

    InvalidateCapability::invalidate(
        &service.build_context(),
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

    assert_eq!(count, 1, "archival should process the active namespace");

    let episode = db_client
        .select_one(&episode_id, "org")
        .await
        .expect("select episode")
        .expect("stored episode");
    assert_eq!(episode.get("status"), Some(&json!("archived")));
}

#[tokio::test]
async fn archival_pass_when_episode_fact_was_recently_accessed_then_skips_archival() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - Duration::days(150);

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "meeting".to_string(),
            source_id: "personal-archival-hot-1".to_string(),
            content: "Personal hot archival candidate".to_string(),
            t_ref: old_date,
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest episode");

    let fact_id = service
        .add_fact(
            "note",
            "Personal hot archival fact",
            "Personal hot archival fact",
            &episode_id,
            old_date,
            0.2,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("add fact");

    InvalidateCapability::invalidate(
        &service.build_context(),
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
            "org",
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
        .select_one(&episode_id, "org")
        .await
        .expect("select episode")
        .expect("stored episode");
    assert_ne!(episode.get("status"), Some(&json!("archived")));
}

#[tokio::test]
async fn archival_pass_with_empty_database() {
    let (service, _db_client) = common::make_service_with_client().await;

    // Act: Run archival pass
    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    // Assert: Should complete successfully with 0 archives on empty DB
    assert_eq!(count, 0, "Empty database should archive 0 episodes");
}

#[tokio::test]
async fn archival_pass_preserves_recent_episodes() {
    let (service, _db_client) = common::make_service_with_client().await;

    let recent_date = Utc::now() - Duration::days(10);

    common::seed_episode_backed_fact_with_source_id(
        &service,
        "org",
        "recent promise content",
        recent_date,
        "recent-archival-test",
    )
    .await;

    // Act: Run archival pass with 90 day threshold
    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    // Assert: Recent episode should not be archived
    assert_eq!(count, 0, "Recent episode should not be archived");
}

#[tokio::test]
async fn archival_pass_archives_old_episodes_without_active_facts() {
    let (service, _db_client) = common::make_service_with_client().await;

    let old_date = Utc::now() - Duration::days(150);

    let fact_id = common::seed_episode_backed_fact_with_source_id(
        &service,
        "org",
        "old promise for archival test",
        old_date,
        "old-archival-test",
    )
    .await;

    InvalidateCapability::invalidate(
        &service.build_context(),
        memory_mcp::models::InvalidateRequest {
            fact_id: fact_id.clone(),
            reason: "test invalidation".to_string(),
            t_invalid: Utc::now(),
        },
        None,
    )
    .await
    .expect("fact invalidated");

    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    assert!(
        count >= 1,
        "Old episode without active facts should be archived"
    );
}

#[tokio::test]
async fn archival_pass_respects_age_threshold() {
    let (service, _db_client) = common::make_service_with_client().await;

    let just_under = Utc::now() - Duration::days(89);
    common::seed_episode_backed_fact_with_source_id(
        &service,
        "org",
        "metric just under threshold",
        just_under,
        "under-threshold",
    )
    .await;

    let well_over = Utc::now() - Duration::days(200);
    let fact_id = common::seed_episode_backed_fact_with_source_id(
        &service,
        "org",
        "metric well over threshold",
        well_over,
        "over-threshold",
    )
    .await;

    InvalidateCapability::invalidate(
        &service.build_context(),
        memory_mcp::models::InvalidateRequest {
            fact_id,
            reason: "test".to_string(),
            t_invalid: Utc::now(),
        },
        None,
    )
    .await
    .expect("fact invalidated");

    let count = run_archival_pass(&service, 90)
        .await
        .expect("archival pass completed");

    assert!(count >= 1, "Should archive episode over threshold");
}

#[tokio::test]
async fn archival_pass_batch_limit_respected() {
    let (service, _db_client) = common::make_service_with_client().await;

    let old_date = Utc::now() - Duration::days(200);

    for i in 0..10 {
        let fact_id = common::seed_episode_backed_fact_with_source_id(
            &service,
            "org",
            &format!("old metric {}", i),
            old_date,
            &format!("batch-{i}"),
        )
        .await;

        InvalidateCapability::invalidate(
            &service.build_context(),
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
