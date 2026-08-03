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
use memory_mcp::models::{ExplainItem, ExplainRequest, IngestRequest, Provenance};
use memory_mcp::service::capabilities::explain::ExplainCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::storage::DbClient;
use serde_json::json;

#[tokio::test]
async fn explain_returns_direct_provenance_source() {
    let (service, _db_client) = common::make_service_with_client().await;
    let episode_id = common::ingest_episode(
        &service,
        "direct-provenance-integration",
        "Integration test: Alice promised to deliver the report",
    )
    .await;
    let episode_content = "Integration test: Alice promised to deliver the report";

    service
        .add_fact(
            "promise",
            "Alice will deliver the report",
            episode_content,
            &episode_id,
            Utc::now(),
            "personal",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .expect("fact added");

    // Act: Call explain
    let request = ExplainRequest {
        context_pack: vec![ExplainItem {
            fact_id: None,
            content: "Alice will deliver the report".to_string(),
            quote: episode_content.to_string(),
            source_episode: episode_id.clone(),
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: json!({"source_episode": episode_id}),
            citation_context: None,
            all_sources: vec![],
            graph_insights: None,
            fact_age_days: None,
            decayed_confidence: None,
            ingestion_method: None,
        }],
        compact: false,
    };

    let result = ExplainCapability::explain(&service.build_context(), request, None)
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
async fn explain_backward_compatible_with_empty_all_sources() {
    let (service, _db_client) = common::make_service_with_client().await;
    let episode_id = common::ingest_episode(
        &service,
        "compat-provenance-test",
        "Backward compatibility provenance test",
    )
    .await;

    service
        .add_fact(
            "metric",
            "Backward compatibility provenance test",
            "Backward compatibility provenance test",
            &episode_id,
            Utc::now(),
            "personal",
            0.8,
            vec![],
            vec![],
            Provenance::manual(),
        )
        .await
        .expect("fact added");

    // Act: Call explain with minimal request (backward compatible)
    let request = ExplainRequest {
        context_pack: vec![ExplainItem {
            fact_id: None,
            content: String::new(),
            quote: String::new(),
            source_episode: episode_id,
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: json!({}),
            citation_context: None,
            all_sources: vec![], // Empty as old code would have
            graph_insights: None,
            fact_age_days: None,
            decayed_confidence: None,
            ingestion_method: None,
        }],
        compact: false,
    };

    let result = ExplainCapability::explain(&service.build_context(), request, None)
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
async fn explain_populates_all_sources_field() {
    let (service, _db_client) = common::make_service_with_client().await;
    let episode_id = common::ingest_episode(
        &service,
        "all-sources-integration",
        "Task completed for all_sources test",
    )
    .await;

    service
        .add_fact(
            "task",
            "Task completed for all_sources test",
            "Task completed",
            &episode_id,
            Utc::now(),
            "personal",
            0.95,
            vec![],
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .expect("fact added");

    // Act
    let request = ExplainRequest {
        context_pack: vec![ExplainItem {
            fact_id: None,
            content: "Task completed".to_string(),
            quote: "Task completed".to_string(),
            source_episode: episode_id,
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: json!({}),
            citation_context: None,
            all_sources: vec![],
            graph_insights: None,
            fact_age_days: None,
            decayed_confidence: None,
            ingestion_method: None,
        }],
        compact: false,
    };

    let result = ExplainCapability::explain(&service.build_context(), request, None)
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
    let (service, db_client) = common::make_service_with_client().await;
    let t_ref = Utc::now();
    let scope = "org";

    // Shared entity
    let entity_id = memory_mcp::service::deterministic_entity_id("person", "Alice Smith");

    // Episode A
    let episode_a_id =
        memory_mcp::service::deterministic_episode_id("email", "linked-ep-a", t_ref, scope);
    IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "email".into(),
            source_id: "linked-ep-a".into(),
            content: "Alice Smith closed a deal".into(),
            t_ref,
            scope: scope.into(),
            project: None,
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
            Provenance::agent_observation(&episode_a_id),
        )
        .await
        .expect("add fact A");

    // Episode B
    let episode_b_id =
        memory_mcp::service::deterministic_episode_id("email", "linked-ep-b", t_ref, scope);
    IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "email".into(),
            source_id: "linked-ep-b".into(),
            content: "Alice Smith presented results".into(),
            t_ref,
            scope: scope.into(),
            project: None,
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
            Provenance::agent_observation(&episode_b_id),
        )
        .await
        .expect("add fact B");

    // Create involved_in edges: entity → fact A, entity → fact B
    let now = memory_mcp::service::now();
    for (fact_id, _edge_suffix) in [(&fact_a_id, "a"), (&fact_b_id, "b")] {
        let edge_id =
            memory_mcp::service::deterministic_edge_id(&entity_id, "involved_in", fact_id, t_ref);
        memory_mcp::storage::EpisodeStoreClient::new(db_client.clone())
            .relate_edge(
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
                scope,
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
            graph_insights: None,
            fact_age_days: None,
            decayed_confidence: None,
            ingestion_method: None,
        }],
        compact: false,
    };

    let result = ExplainCapability::explain(&service.build_context(), request, None)
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

#[tokio::test]
async fn explain_when_fact_is_cited_then_access_count_increases() {
    let (service, db_client) = common::make_service_with_client().await;

    let fact_id = service
        .add_fact(
            "note",
            "Explain access boost note",
            "Explain access boost note",
            "episode:explain-boost",
            Utc::now(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:explain-boost"),
        )
        .await
        .expect("add fact");

    let result = ExplainCapability::explain(
        &service.build_context(),
        ExplainRequest {
            context_pack: vec![ExplainItem {
                fact_id: Some(fact_id.clone()),
                content: "Explain access boost note".to_string(),
                quote: "Explain access boost note".to_string(),
                source_episode: "episode:explain-boost".to_string(),
                scope: None,
                t_ref: None,
                t_ingested: None,
                provenance: json!({"source_episode": "episode:explain-boost"}),
                citation_context: None,
                all_sources: vec![],
                graph_insights: None,
                fact_age_days: None,
                decayed_confidence: None,
                ingestion_method: None,
            }],
            compact: false,
        },
        None,
    )
    .await
    .expect("explain completed");

    assert_eq!(result.len(), 1);

    let stored = db_client
        .select_one(&fact_id, "org")
        .await
        .expect("select fact")
        .expect("stored fact");

    assert_eq!(
        stored.get("access_count").and_then(|value| value.as_i64()),
        Some(3)
    );
    assert!(stored.get("last_accessed").is_some());
}

#[tokio::test]
async fn explain_with_empty_source_episode_returns_validation_error() {
    let (service, _db_client) = common::make_service_with_client().await;

    // Act: Call explain with empty source_episode (the bug scenario)
    let request = ExplainRequest {
        context_pack: vec![ExplainItem {
            fact_id: Some("fact:52f9d92d20d829840f24294f".to_string()),
            content: "Some content".to_string(),
            quote: "Some quote".to_string(),
            source_episode: String::new(), // empty — triggers the bug
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: serde_json::Value::Null,
            citation_context: None,
            all_sources: vec![],
            graph_insights: None,
            fact_age_days: None,
            decayed_confidence: None,
            ingestion_method: None,
        }],
        compact: false,
    };

    let result = ExplainCapability::explain(&service.build_context(), request, None).await;

    // Assert: Should return a validation error, not a SurrealDB parse error
    assert!(result.is_err(), "Expected error for empty source_episode");
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("source_episode is required"),
        "Unexpected error message: {err_msg}"
    );
}

#[tokio::test]
async fn explain_with_context_items_missing_source_episode_returns_validation_error() {
    let (service, _db_client) = common::make_service_with_client().await;

    // Simulates the exact bug payload: objects with fact_id but no source_episode
    let request = ExplainRequest {
        context_pack: vec![
            ExplainItem {
                fact_id: Some("fact:52f9d92d20d829840f24294f".to_string()),
                content: String::new(),
                quote: String::new(),
                source_episode: String::new(), // missing from original JSON
                scope: None,
                t_ref: None,
                t_ingested: None,
                provenance: serde_json::Value::Null,
                citation_context: None,
                all_sources: vec![],
                graph_insights: None,
                fact_age_days: None,
                decayed_confidence: None,
                ingestion_method: None,
            },
            ExplainItem {
                fact_id: Some("fact:3440abb2c00eb317567d3148".to_string()),
                content: String::new(),
                quote: String::new(),
                source_episode: String::new(), // missing from original JSON
                scope: None,
                t_ref: None,
                t_ingested: None,
                provenance: serde_json::Value::Null,
                citation_context: None,
                all_sources: vec![],
                graph_insights: None,
                fact_age_days: None,
                decayed_confidence: None,
                ingestion_method: None,
            },
        ],
        compact: false,
    };

    let result = ExplainCapability::explain(&service.build_context(), request, None).await;

    assert!(result.is_err(), "Expected error for empty source_episode");
    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("source_episode is required"),
        "Unexpected error message: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// Batch graph insights sharing
// ---------------------------------------------------------------------------

/// Verify that explain with multiple items sharing the same entity produces
/// identical graph_insights across all items (proving batch computation).
#[tokio::test]
async fn explain_batch_shares_graph_insights() {
    let (service, _db_client) = common::make_service_with_client().await;
    let t_ref = Utc::now();

    let alice_id = service
        .resolve_entity("person", "Alice Shared")
        .await
        .expect("alice");
    let bob_id = service
        .resolve_entity("person", "Bob Shared")
        .await
        .expect("bob");
    service
        .relate(&alice_id, "knows", &bob_id)
        .await
        .expect("edge");

    // Episode A → fact with Alice
    let ep_a = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "note".into(),
            source_id: "batch-ep-a".into(),
            content: "Alice met Bob".into(),
            t_ref,
            scope: "org".into(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest a");
    let fact_a = service
        .add_fact(
            "note",
            "Alice met Bob",
            "Alice met Bob",
            &ep_a,
            t_ref,
            "org",
            0.9,
            vec![alice_id.clone(), bob_id.clone()],
            vec![],
            Provenance::agent_observation(&ep_a),
        )
        .await
        .expect("add fact a");

    // Episode B → fact with same entity set (Alice + Bob)
    let ep_b = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "note".into(),
            source_id: "batch-ep-b".into(),
            content: "Bob talked to Alice again".into(),
            t_ref,
            scope: "org".into(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest b");
    let fact_b = service
        .add_fact(
            "note",
            "Bob talked to Alice again",
            "Bob talked to Alice again",
            &ep_b,
            t_ref,
            "org",
            0.9,
            vec![bob_id.clone(), alice_id.clone()],
            vec![],
            Provenance::agent_observation(&ep_b),
        )
        .await
        .expect("add fact b");

    // Explain both facts in a single batch
    let result = ExplainCapability::explain(
        &service.build_context(),
        ExplainRequest {
            context_pack: vec![
                ExplainItem {
                    fact_id: Some(fact_a),
                    content: "Alice met Bob".into(),
                    quote: "Alice met Bob".into(),
                    source_episode: ep_a.clone(),
                    ..Default::default()
                },
                ExplainItem {
                    fact_id: Some(fact_b),
                    content: "Bob talked to Alice again".into(),
                    quote: "Bob talked to Alice again".into(),
                    source_episode: ep_b.clone(),
                    ..Default::default()
                },
            ],
            compact: false,
        },
        None,
    )
    .await
    .expect("explain batch");

    assert_eq!(result.len(), 2, "should return 2 explain items");

    // Both items should have the same graph_insights (shared batch computation)
    let insights_a = serde_json::to_value(&result[0])
        .unwrap()
        .get("graph_insights")
        .cloned();
    let insights_b = serde_json::to_value(&result[1])
        .unwrap()
        .get("graph_insights")
        .cloned();

    assert_eq!(
        insights_a, insights_b,
        "graph_insights must be identical across batch items (shared computation)"
    );
}

/// Verify that explain correctly handles items without fact_id (no entity_links).
#[tokio::test]
async fn explain_batch_mixed_with_and_without_fact_ids() {
    let (service, _db_client) = common::make_service_with_client().await;
    let t_ref = Utc::now();

    // Episode with fact
    let ep_with = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "note".into(),
            source_id: "mixed-ep-1".into(),
            content: "Fact content here".into(),
            t_ref,
            scope: "org".into(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest");
    let alice_id = service
        .resolve_entity("person", "Alice Mixed")
        .await
        .expect("alice");
    let fact_id = service
        .add_fact(
            "note",
            "Fact content here",
            "Fact content here",
            &ep_with,
            t_ref,
            "org",
            0.9,
            vec![alice_id],
            vec![],
            Provenance::agent_observation(&ep_with),
        )
        .await
        .expect("add fact");

    // Episode without fact (raw episode explain)
    let ep_without = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "note".into(),
            source_id: "mixed-ep-2".into(),
            content: "Just an episode, no fact".into(),
            t_ref,
            scope: "org".into(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest");

    let result = ExplainCapability::explain(
        &service.build_context(),
        ExplainRequest {
            context_pack: vec![
                ExplainItem {
                    fact_id: Some(fact_id),
                    content: "Fact content here".into(),
                    quote: "Fact content here".into(),
                    source_episode: ep_with.clone(),
                    ..Default::default()
                },
                ExplainItem {
                    fact_id: None, // no fact — will have no entity_links
                    content: "".into(),
                    quote: "".into(),
                    source_episode: ep_without.clone(),
                    ..Default::default()
                },
            ],
            compact: false,
        },
        None,
    )
    .await
    .expect("explain mixed batch");

    assert_eq!(result.len(), 2);
    // First item has fact_id → may have graph_insights
    let item0 = serde_json::to_value(&result[0]).unwrap();
    // Second item has no fact_id → should still get the shared batch graph_insights
    let item1 = serde_json::to_value(&result[1]).unwrap();
    assert_eq!(
        item0.get("graph_insights"),
        item1.get("graph_insights"),
        "both items should share the same batch graph_insights"
    );
}

/// Verify that explain with empty context_pack returns an empty vec without error.
#[tokio::test]
async fn explain_empty_context_pack() {
    let service = common::make_service().await;

    let result = ExplainCapability::explain(
        &service.build_context(),
        ExplainRequest {
            context_pack: vec![],
            compact: false,
        },
        None,
    )
    .await
    .expect("explain empty");

    assert!(
        result.is_empty(),
        "empty context_pack should yield empty result"
    );
}

/// Verify that explain skips items with unknown source_episode gracefully.
#[tokio::test]
async fn explain_skips_unknown_episode() {
    let service = common::make_service().await;

    let result = ExplainCapability::explain(
        &service.build_context(),
        ExplainRequest {
            context_pack: vec![ExplainItem {
                source_episode: "episode:nonexistent-99999".into(),
                content: "some content".into(),
                quote: "some quote".into(),
                fact_age_days: None,
                decayed_confidence: None,
                ingestion_method: None,
                ..Default::default()
            }],
            compact: false,
        },
        None,
    )
    .await
    .expect("explain unknown episode");

    assert_eq!(result.len(), 1);
    // Should return the item as-is (no enrichment)
    assert_eq!(result[0].source_episode, "episode:nonexistent-99999");
    assert_eq!(result[0].content, "some content");
    assert_eq!(result[0].scope, None);
    assert_eq!(result[0].all_sources.len(), 0);
}
