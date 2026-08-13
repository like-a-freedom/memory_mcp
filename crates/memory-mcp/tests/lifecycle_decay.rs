//! Integration tests for lifecycle decay background worker.
//!
//! These tests verify that the decay worker correctly invalidates facts
//! whose confidence has decayed below the configured threshold.
//!
//! Note: These tests require `--test-threads=1` due to embedded SurrealDB lock.
//! Run with: cargo test lifecycle_decay -- --test-threads=1

use chrono::{Duration, Utc};
use memory_mcp::models::Provenance;
use memory_mcp::service::capabilities::invalidate::InvalidateCapability;
use memory_mcp::service::run_decay_pass;
use memory_mcp::storage::DbClient;
use serde_json::json;

mod common;

#[tokio::test]
async fn decay_pass_invalidates_active_fact_with_absent_t_invalid_field() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - Duration::days(400);

    let fact_id = service
        .add_fact(
            "metric",
            "old metric in default namespace",
            "old metric in default namespace",
            "episode:default_decay_none",
            old_date,
            0.4,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    let count = run_decay_pass(&service, 0.3, 100.0)
        .await
        .expect("decay pass completed");

    assert_eq!(count, 1, "active fact with absent t_invalid should decay");

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .expect("select fact")
        .expect("stored fact");
    assert!(stored.get("t_invalid").is_some());
}

#[tokio::test]
async fn decay_pass_processes_only_active_namespace() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - Duration::days(400);

    let fact_id = service
        .add_fact(
            "metric",
            "old metric in active namespace",
            "old metric in active namespace",
            "episode:personal_decay_old",
            old_date,
            0.4,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    let count = run_decay_pass(&service, 0.3, 100.0)
        .await
        .expect("decay pass completed");

    assert_eq!(count, 1, "decay should process the active namespace");

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .expect("select fact")
        .expect("stored fact");
    assert!(stored.get("t_invalid").is_some());
}

#[tokio::test]
async fn decay_pass_when_fact_was_recently_accessed_then_skips_invalidation() {
    let (service, db_client) = common::make_service_with_client().await;
    let old_date = Utc::now() - Duration::days(400);

    let fact_id = service
        .add_fact(
            "metric",
            "old but hot metric",
            "old but hot metric",
            "episode:hot_decay_skip",
            old_date,
            0.4,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

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

    let count = run_decay_pass(&service, 0.3, 100.0)
        .await
        .expect("decay pass completed");

    assert_eq!(count, 0, "recently accessed fact should stay active");

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .expect("select fact")
        .expect("stored fact");
    assert!(stored.get("t_invalid").is_none_or(|value| value.is_null()));
}

#[tokio::test]
async fn decay_pass_with_empty_database() {
    let (service, _db_client) = common::make_service_with_client().await;

    // Act: Run decay pass
    let count = run_decay_pass(&service, 0.3, 365.0)
        .await
        .expect("decay pass completed");

    // Assert: Should not invalidate anything in empty database
    assert_eq!(count, 0, "Empty database should invalidate 0 facts");
}

#[tokio::test]
async fn decay_pass_preserves_recent_high_confidence_facts() {
    let (service, _db_client) = common::make_service_with_client().await;

    // Create a recent fact with high confidence (should not decay below threshold)
    let recent_date = Utc::now() - Duration::days(1);
    let _fact_id = service
        .add_fact(
            "promise",
            "recent promise content for decay test",
            "recent promise content for decay test",
            "episode:recent_decay_test",
            recent_date,
            0.95, // high confidence, won't decay below threshold
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Act: Run decay pass with 0.3 threshold and 365 day half-life
    let count = run_decay_pass(&service, 0.3, 365.0)
        .await
        .expect("decay pass completed");

    // Assert: Recent high-confidence facts should not be invalidated
    assert_eq!(
        count, 0,
        "Recent high-confidence facts should not be invalidated"
    );
}

#[tokio::test]
async fn decay_pass_invalidates_old_low_confidence_facts() {
    let (service, _db_client) = common::make_service_with_client().await;

    let old_date = Utc::now() - Duration::days(400);

    let _fact_id = service
        .add_fact(
            "metric",
            "old metric for decay test",
            "old metric",
            "episode:old_decay_test",
            old_date,
            0.4, // moderate confidence, will decay
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Act: Run decay pass with 0.3 threshold and 100 day half-life (fast decay)
    let count = run_decay_pass(&service, 0.3, 100.0)
        .await
        .expect("decay pass completed");

    // Assert: Old fact should be invalidated
    assert!(count >= 1, "Old low-confidence fact should be invalidated");
}

#[tokio::test]
async fn decay_pass_respects_threshold_parameter() {
    let (service, _db_client) = common::make_service_with_client().await;

    let old_date = Utc::now() - Duration::days(200);

    // Higher confidence fact
    service
        .add_fact(
            "metric",
            "high confidence old fact",
            "high confidence",
            "episode:high_conf_decay",
            old_date,
            0.8,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Lower confidence fact
    let _low_fact_id = service
        .add_fact(
            "metric",
            "low confidence old fact",
            "low confidence",
            "episode:low_conf_decay",
            old_date,
            0.3,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Act: Run decay pass with moderate threshold
    let count = run_decay_pass(&service, 0.2, 100.0)
        .await
        .expect("decay pass completed");

    // Assert: Lower confidence fact should be invalidated
    assert!(count >= 1, "Low confidence fact should be invalidated");
}

#[tokio::test]
async fn decay_pass_skips_already_invalidated_facts() {
    let (service, _db_client) = common::make_service_with_client().await;

    let old_date = Utc::now() - Duration::days(200);

    let fact_id = service
        .add_fact(
            "metric",
            "fact to pre-invalidate",
            "pre-invalidated",
            "episode:pre_invalid_decay",
            old_date,
            0.2,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Pre-invalidate the fact
    InvalidateCapability::invalidate(
        &service.build_context(),
        memory_mcp::models::InvalidateRequest {
            fact_id: fact_id.clone(),
            reason: "test pre-invalidation".to_string(),
            t_invalid: Utc::now(),
        },
        None,
    )
    .await
    .expect("fact invalidated");

    // Act: Run decay pass
    let count = run_decay_pass(&service, 0.3, 100.0)
        .await
        .expect("decay pass completed");

    // Assert: Already invalidated facts should not be counted again
    // (The decay pass skips facts with t_invalid set)
    assert_eq!(count, 0, "Decay pass should skip already invalidated facts");
}

#[tokio::test]
async fn decay_pass_half_life_affects_decay_rate() {
    let (service, _db_client) = common::make_service_with_client().await;

    let old_date = Utc::now() - Duration::days(300);

    // Create two identical old facts
    service
        .add_fact(
            "metric",
            "fact for short half-life test",
            "short half-life",
            "episode:short_halflife",
            old_date,
            0.5,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    service
        .add_fact(
            "metric",
            "fact for long half-life test",
            "long half-life",
            "episode:long_halflife",
            old_date,
            0.5,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Act: Run with short half-life (faster decay)
    let count_short = run_decay_pass(&service, 0.3, 50.0)
        .await
        .expect("decay pass completed");

    // Reset by creating facts again for long half-life test
    service
        .add_fact(
            "metric",
            "fact for long half-life test 2",
            "long half-life 2",
            "episode:long_halflife_2",
            old_date,
            0.5,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Act: Run with long half-life (slower decay)
    let count_long = run_decay_pass(&service, 0.3, 500.0)
        .await
        .expect("decay pass completed");

    // Assert: Shorter half-life should invalidate at least as many
    // (faster decay means more facts fall below threshold)
    assert!(
        count_short >= count_long,
        "Shorter half-life should invalidate at least as many: {} vs {}",
        count_short,
        count_long
    );
}

#[tokio::test]
async fn decay_confidence_calculation_exponential() {
    let (service, _db_client) = common::make_service_with_client().await;

    // Create a fact that's exactly 100 days old with 0.5 confidence
    // With half-life of 100 days, it should decay to 0.25 (half)
    let old_date = Utc::now() - Duration::days(100);

    service
        .add_fact(
            "metric",
            "fact for exponential decay test",
            "exponential decay",
            "episode:exponential_decay",
            old_date,
            0.5, // base confidence
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Act: Run with threshold just above expected decayed value (0.26)
    // Expected: 0.5 * exp(-ln(2)/100 * 100) = 0.5 * exp(-ln(2)) = 0.5 * 0.5 = 0.25
    let count = run_decay_pass(&service, 0.26, 100.0)
        .await
        .expect("decay pass completed");

    // Assert: Fact should be invalidated (0.25 < 0.26 threshold)
    assert!(
        count >= 1,
        "Fact decayed to 0.25 should be invalidated by 0.26 threshold"
    );
}
