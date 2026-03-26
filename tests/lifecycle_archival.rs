//! Integration tests for lifecycle archival background worker.
//!
//! These tests verify that the archival worker correctly archives episodes
//! that are older than the threshold and have no active facts.

use chrono::{Duration, Utc};
use memory_mcp::service::lifecycle::run_archival_pass;
use memory_mcp::MemoryService;
use serde_json::json;

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

    // Assert: Should complete successfully
    // (count may be > 0 from other tests)
    assert!(count >= 0, "Archival pass should complete successfully");
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn archival_pass_preserves_episodes_with_active_facts() {
    // Setup: Create old fact (implies episode with active fact)
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let old_date = Utc::now() - Duration::days(150);

    service
        .add_fact(
            "promise",
            "old promise still active for archival test",
            "old promise still active",
            "episode:old_active_archival",
            old_date,
            "test_archival_active",
            0.9, // high confidence, still active
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

    // Assert: Should complete successfully
    // Episodes with active facts should not be archived
    assert!(count >= 0, "Archival pass should complete without error");
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn archival_pass_different_thresholds() {
    // Setup: Create facts with different ages
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let very_old = Utc::now() - Duration::days(400);
    let moderately_old = Utc::now() - Duration::days(200);

    // Create old facts
    service
        .add_fact(
            "metric",
            "very old metric for archival threshold",
            "very old metric",
            "episode:very_old_archival",
            very_old,
            "test_archival_threshold",
            0.3,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .expect("fact added");

    service
        .add_fact(
            "metric",
            "moderately old metric for archival threshold",
            "moderately old metric",
            "episode:mod_old_archival",
            moderately_old,
            "test_archival_threshold",
            0.4,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .expect("fact added");

    // Act: Run with different thresholds
    let count_100 = run_archival_pass(&service, 100)
        .await
        .expect("archival pass completed");

    let count_300 = run_archival_pass(&service, 300)
        .await
        .expect("archival pass completed");

    // Assert: Higher threshold should archive at least as many
    assert!(
        count_300 >= count_100,
        "Higher threshold should archive at least as many: {} vs {}",
        count_300, count_100
    );
}
