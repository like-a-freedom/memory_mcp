//! Integration tests for service module interactions.
//!
//! These tests verify that different service components work together correctly.

use chrono::{TimeZone, Utc};
use serde_json::json;

mod common;

#[tokio::test]
async fn test_service_ingest_and_extract_flow() {
    let service = common::make_service().await;

    let request = memory_mcp::models::IngestRequest {
        source_type: "meeting".to_string(),
        source_id: "integration-test-1".to_string(),
        content: "Meeting with Alice Inc and Bob Corp. Discussed ARR growth to $5M. Alice will deliver the prototype by Friday.".to_string(),
        t_ref: Utc::now(),
        scope: "org".to_string(),
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };

    let episode_id = service.ingest(request, None).await.unwrap();
    assert!(episode_id.starts_with("episode:"));

    let result = service.extract(&episode_id, None).await.unwrap();

    assert_eq!(result.episode_id, episode_id);
    assert!(!result.entities.is_empty());
    assert!(!result.facts.is_empty());
}

#[tokio::test]
async fn test_service_resolve_and_relate_entities() {
    let service = common::make_service().await;

    let alice_id = service.resolve_person("Alice Smith").await.unwrap();
    assert!(alice_id.starts_with("entity:"));

    let bob_id = service.resolve_person("Bob Jones").await.unwrap();
    assert!(bob_id.starts_with("entity:"));

    let alice_id_2 = service.resolve_person("Alice Smith").await.unwrap();
    assert_eq!(alice_id, alice_id_2);

    service.relate(&alice_id, "knows", &bob_id).await.unwrap();
}

#[tokio::test]
async fn test_service_add_fact_and_assemble_context() {
    let service = common::make_service().await;

    let t_valid = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

    let _fact_id = service
        .add_fact(
            "metric",
            "ARR reached $5M in Q4 2023",
            "ARR reached $5M",
            "episode:test",
            t_valid,
            "org",
            0.9,
            vec![],
            vec!["finance".to_string()],
            json!({"quarter": "Q4", "year": 2023}),
        )
        .await
        .unwrap();

    let request = memory_mcp::models::AssembleContextRequest {
        query: "ARR metric".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc::now()),
        budget: 10,
        access: None,
    };

    let context = service.assemble_context(request).await.unwrap();
    assert!(!context.is_empty());
    assert!(!context[0].fact_id.is_empty());
    assert!(!context[0].content.is_empty());
    assert!(context[0].confidence.is_finite());
}

#[tokio::test]
async fn test_service_fact_invalidation() {
    let service = common::make_service().await;

    let t_valid = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

    let fact_id = service
        .add_fact(
            "metric",
            "ARR $3M",
            "ARR $3M",
            "episode:test",
            t_valid,
            "org",
            0.9,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .unwrap();

    let request_before = memory_mcp::models::AssembleContextRequest {
        query: "ARR".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc::now()),
        budget: 10,
        access: None,
    };
    let context_before = service.assemble_context(request_before).await.unwrap();
    assert!(context_before.iter().any(|f| f.fact_id == fact_id));

    let t_invalid = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
    service
        .invalidate(
            memory_mcp::models::InvalidateRequest {
                fact_id: fact_id.clone(),
                reason: "Superseded by new value".to_string(),
                t_invalid,
            },
            None,
        )
        .await
        .unwrap();

    let request_after = memory_mcp::models::AssembleContextRequest {
        query: "ARR".to_string(),
        scope: "org".to_string(),
        as_of: Some(Utc.with_ymd_and_hms(2024, 12, 1, 0, 0, 0).unwrap()),
        budget: 10,
        access: None,
    };
    let context_after = service.assemble_context(request_after).await.unwrap();
    assert!(!context_after.iter().any(|f| f.fact_id == fact_id));
}

#[tokio::test]
async fn test_service_cache_behavior() {
    let service = common::make_service().await;

    let t_valid = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

    service
        .add_fact(
            "note",
            "Test content for caching",
            "Test quote",
            "episode:cache-test",
            t_valid,
            "org",
            0.8,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .unwrap();

    let request = memory_mcp::models::AssembleContextRequest {
        query: "Test content".to_string(),
        scope: "org".to_string(),
        as_of: None,
        budget: 5,
        access: None,
    };
    let result1 = service.assemble_context(request.clone()).await.unwrap();
    assert!(!result1.is_empty());

    let result2 = service.assemble_context(request).await.unwrap();
    assert_eq!(result1.len(), result2.len());
}

#[tokio::test]
async fn test_service_scope_isolation() {
    let service = common::make_service().await;

    let t_valid = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

    service
        .add_fact(
            "note",
            "Org scope fact",
            "Org quote",
            "episode:org",
            t_valid,
            "org",
            0.9,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .unwrap();

    service
        .add_fact(
            "note",
            "Personal scope fact",
            "Personal quote",
            "episode:personal",
            t_valid,
            "personal",
            0.9,
            vec![],
            vec![],
            json!({}),
        )
        .await
        .unwrap();

    let request_org = memory_mcp::models::AssembleContextRequest {
        query: "scope fact".to_string(),
        scope: "org".to_string(),
        as_of: None,
        budget: 10,
        access: None,
    };
    let org_results = service.assemble_context(request_org).await.unwrap();
    assert!(org_results.iter().all(|r| { r.content.contains("Org") }));
}
