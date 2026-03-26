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

mod common;

use chrono::Utc;
use memory_mcp::MemoryService;
use memory_mcp::models::{ExplainItem, ExplainRequest};
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
            fact_id: None,
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
        !item.all_sources.is_empty(),
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
            fact_id: None,
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
        !result[0].all_sources.is_empty(),
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
            fact_id: None,
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

#[tokio::test]
async fn explain_includes_linked_episodes_via_shared_entity() {
    let service = common::make_service().await;
    let t_ref = Utc::now();
    let scope = "org";

    // Shared entity
    let entity_id = memory_mcp::service::deterministic_entity_id("person", "Alice Smith");

    // Episode A
    let episode_a_id =
        memory_mcp::service::deterministic_episode_id("email", "linked-ep-a", t_ref, scope);
    service
        .ingest(
            memory_mcp::models::IngestRequest {
                source_type: "email".into(),
                source_id: "linked-ep-a".into(),
                content: "Alice Smith closed a deal".into(),
                t_ref,
                scope: scope.into(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest A");

    // Fact A: explicitly linked to entity
    let fact_a_id = service
        .add_fact(
            "metric",
            "Alice Smith closed $5M deal",
            "Alice Smith closed a $5M deal",
            &episode_a_id,
            t_ref,
            scope,
            0.9,
            vec![entity_id.clone()],
            vec![],
            json!({"source_episode": episode_a_id}),
        )
        .await
        .expect("add fact A");

    // Episode B
    let episode_b_id =
        memory_mcp::service::deterministic_episode_id("email", "linked-ep-b", t_ref, scope);
    service
        .ingest(
            memory_mcp::models::IngestRequest {
                source_type: "email".into(),
                source_id: "linked-ep-b".into(),
                content: "Alice Smith presented results".into(),
                t_ref,
                scope: scope.into(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest B");

    // Fact B: also linked to same entity
    let fact_b_id = service
        .add_fact(
            "fact",
            "Alice Smith presented quarterly results",
            "Alice Smith presented quarterly results",
            &episode_b_id,
            t_ref,
            scope,
            0.85,
            vec![entity_id.clone()],
            vec![],
            json!({"source_episode": episode_b_id}),
        )
        .await
        .expect("add fact B");

    // Create involved_in edges: entity → fact A, entity → fact B
    let now = memory_mcp::service::now();
    for (fact_id, _edge_suffix) in [(&fact_a_id, "a"), (&fact_b_id, "b")] {
        let edge_id =
            memory_mcp::service::deterministic_edge_id(&entity_id, "involved_in", fact_id, t_ref);
        service
            .db_client
            .relate_edge(
                scope,
                &edge_id,
                &entity_id,
                fact_id,
                json!({
                    "edge_id": edge_id,
                    "relation": "involved_in",
                    "strength": 0.8,
                    "confidence": 0.85,
                    "t_valid": memory_mcp::service::normalize_dt(t_ref),
                    "t_ingested": memory_mcp::service::normalize_dt(now),
                }),
            )
            .await
            .expect("relate edge");
    }

    // Explain fact A — should include episode B as linked source via entity
    let request = ExplainRequest {
        context_pack: vec![ExplainItem {
            fact_id: Some(fact_a_id),
            content: "Alice Smith closed $5M deal".into(),
            quote: "Alice Smith closed a $5M deal".into(),
            source_episode: episode_a_id.clone(),
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: json!({"source_episode": episode_a_id}),
            citation_context: None,
            all_sources: vec![],
        }],
    };

    let result = service
        .explain(request, None)
        .await
        .expect("explain completed");

    assert!(!result.is_empty(), "Should return explain results");
    let item = &result[0];

    // Must have at least 2 sources: direct (episode A) + linked (episode B)
    assert!(
        item.all_sources.len() >= 2,
        "Should have direct + linked sources, got {} sources: {:?}",
        item.all_sources.len(),
        item.all_sources
    );

    let has_direct = item.all_sources.iter().any(|s| s.relationship == "direct");
    let has_linked = item.all_sources.iter().any(|s| s.relationship == "linked");
    assert!(has_direct, "Should have a direct provenance source");
    assert!(has_linked, "Should have a linked provenance source");
}
