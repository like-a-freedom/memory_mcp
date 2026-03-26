//! Integration tests for multi-source provenance in explain().
//!
//! These tests verify that explain() returns complete provenance information
//! including direct and linked episode sources.
//!
//! **Note:** These tests require `--test-threads=1` due to embedded SurrealDB
//! LOCK file contention. Run with:
//! ```bash
//! cargo test --test explain_provenance -- --test-threads=1
//! ```

use chrono::Utc;
use memory_mcp::models::{ExplainItem, ExplainRequest};
use memory_mcp::MemoryService;
use serde_json::json;

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn explain_returns_direct_provenance_source() {
    // Setup: Create episode and fact
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let episode_id = "episode:direct_provenance_integration";
    let episode_content = "Integration test: Alice promised to deliver the report";

    service
        .add_fact(
            "promise",
            "Alice will deliver the report",
            episode_content,
            episode_id,
            Utc::now(),
            "test_provenance_integration",
            0.9,
            vec![],
            vec![],
            json!({"source_episode": episode_id}),
        )
        .await
        .expect("fact added");

    // Act: Call explain
    let request = ExplainRequest {
        context_pack: vec![ExplainItem {
            content: "Alice will deliver the report".to_string(),
            quote: episode_content.to_string(),
            source_episode: episode_id.to_string(),
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: json!({"source_episode": episode_id}),
            citation_context: None,
            all_sources: vec![],
        }],
    };

    let result = service
        .explain(request, None)
        .await
        .expect("explain completed");

    // Assert: Verify direct provenance source is populated
    assert!(!result.is_empty(), "Should return at least one result");
    let item = &result[0];

    assert!(
        item.all_sources.len() >= 1,
        "Should have at least one provenance source (direct), got {}",
        item.all_sources.len()
    );

    let direct_source = &item.all_sources[0];
    assert_eq!(
        direct_source.relationship, "direct",
        "First source should be direct, got {}",
        direct_source.relationship
    );
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn explain_backward_compatible_with_empty_all_sources() {
    // Setup
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let episode_id = "episode:compat_provenance_test";

    service
        .add_fact(
            "metric",
            "Backward compatibility provenance test",
            "Backward compatibility provenance test",
            episode_id,
            Utc::now(),
            "test_provenance_compat",
            0.8,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .expect("fact added");

    // Act: Call explain with minimal request (backward compatible)
    let request = ExplainRequest {
        context_pack: vec![ExplainItem {
            content: String::new(),
            quote: String::new(),
            source_episode: episode_id.to_string(),
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: json!({}),
            citation_context: None,
            all_sources: vec![], // Empty as old code would have
        }],
    };

    let result = service
        .explain(request, None)
        .await
        .expect("explain completed");

    // Assert: Verify backward compatibility
    assert!(!result.is_empty(), "Should return results");
    assert!(
        result[0].all_sources.len() >= 1,
        "Should populate all_sources with at least direct source"
    );
}

#[tokio::test]
#[ignore = "requires --test-threads=1 due to embedded SurrealDB LOCK"]
async fn explain_populates_all_sources_field() {
    // Setup
    let service = MemoryService::new_from_env()
        .await
        .expect("service created");

    let episode_id = "episode:all_sources_integration";

    service
        .add_fact(
            "task",
            "Task completed for all_sources test",
            "Task completed",
            episode_id,
            Utc::now(),
            "test_provenance_sources",
            0.95,
            vec![],
            vec![],
            json!({"source_episode": episode_id}),
        )
        .await
        .expect("fact added");

    // Act
    let request = ExplainRequest {
        context_pack: vec![ExplainItem {
            content: "Task completed".to_string(),
            quote: "Task completed".to_string(),
            source_episode: episode_id.to_string(),
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: json!({}),
            citation_context: None,
            all_sources: vec![],
        }],
    };

    let result = service
        .explain(request, None)
        .await
        .expect("explain completed");

    // Assert
    assert!(!result.is_empty());
    let item = &result[0];

    // Verify all_sources is populated (not empty)
    assert!(
        !item.all_sources.is_empty(),
        "all_sources should be populated, got {} sources",
        item.all_sources.len()
    );

    // Verify structure of provenance sources
    for source in &item.all_sources {
        assert!(
            !source.episode_id.is_empty(),
            "Episode ID should not be empty"
        );
        assert!(
            ["direct", "linked"].contains(&source.relationship.as_str()),
            "Relationship should be 'direct' or 'linked', got {}",
            source.relationship
        );
    }
}
