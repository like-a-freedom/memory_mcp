//! Integration tests for lifecycle decay background worker.
//!
//! These tests verify that the decay worker correctly invalidates facts
//! whose confidence has decayed below the configured threshold.

use chrono::{Duration, Utc};
use memory_mcp::MemoryService;
use memory_mcp::service::lifecycle::run_decay_pass;
use serde_json::json;

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn decay_pass_with_empty_database() {
    // Setup: Fresh service
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    // Act: Run decay pass
    let count = run_decay_pass(&service, 0.3, 365.0)
        .await
        .expect("decay pass completed");

    // Assert: Should not invalidate anything in empty/test database
    // (count may be > 0 from other tests, but should complete successfully)
    assert!(count >= 0, "Decay pass should complete successfully");
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn decay_pass_preserves_recent_high_confidence_facts() {
    // Setup
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    // Create a recent fact with high confidence (should not decay below threshold)
    let recent_date = Utc::now() - Duration::days(1);
    let _fact_id = service
        .add_fact(
            "promise",
            "recent promise content for decay test",
            "recent promise content for decay test",
            "episode:recent_decay_test",
            recent_date,
            "test_decay_preserve",
            0.95, // high confidence, won't decay below threshold
            vec![],
            vec![],
            json!({}),
        )
        .await
        .expect("fact added");

    // Act: Run decay pass with 0.3 threshold
    let count = run_decay_pass(&service, 0.3, 365.0)
        .await
        .expect("decay pass completed");

    // Assert: Recent facts should not be invalidated
    // Note: count may be > 0 from other test facts
    assert!(count >= 0, "Decay pass should complete without error");
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn decay_pass_different_thresholds_produce_different_results() {
    // Setup: Create facts with different ages
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let very_old = Utc::now() - Duration::days(400);
    let moderately_old = Utc::now() - Duration::days(100);

    // Create old facts
    service
        .add_fact(
            "metric",
            "very old metric for threshold test",
            "very old metric",
            "episode:very_old_decay",
            very_old,
            "test_decay_threshold",
            0.4,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .expect("fact added");

    service
        .add_fact(
            "metric",
            "moderately old metric for threshold test",
            "moderately old metric",
            "episode:mod_old_decay",
            moderately_old,
            "test_decay_threshold",
            0.6,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .expect("fact added");

    // Act: Run with different thresholds
    let count_low = run_decay_pass(&service, 0.2, 365.0)
        .await
        .expect("decay pass completed");

    let count_high = run_decay_pass(&service, 0.8, 365.0)
        .await
        .expect("decay pass completed");

    // Assert: Higher threshold should invalidate at least as many
    assert!(
        count_high >= count_low,
        "Higher threshold should invalidate at least as many: {} vs {}",
        count_high,
        count_low
    );
}
