//! Direct unit tests of the shared protocol-agnostic tool layer (src/tools/*).
//!
//! These tests exercise the tool functions directly with a MemoryService backed
//! by an in-memory SurrealDB, verifying ToolResponse shape, log event emission,
//! and error handling without going through the MCP or CLI adapter layers.

use chrono::Utc;
use memory_mcp::service::MemoryError;
use memory_mcp::tools::params::{
    AssembleContextParams, ExplainParams, ExtractParams, IngestParams, InvalidateParams,
    ResolveParams,
};

mod common;

#[tokio::test]
async fn tools_ingest_returns_validation_error_for_bad_t_ref() {
    let service = common::make_service().await;

    let params = IngestParams {
        source_type: "test".to_string(),
        source_id: "t-1".to_string(),
        content: "hello".to_string(),
        t_ref: "not-a-date".to_string(),
        scope: "org".to_string(),
        project: None,
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };

    let result = memory_mcp::tools::ingest(&service, params).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MemoryError::Validation(msg) => {
            assert!(msg.contains("Invalid `t_ref` value"), "msg: {msg}");
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[tokio::test]
async fn tools_extract_rejects_both_episode_and_inline() {
    let service = common::make_service().await;

    let params = ExtractParams {
        episode_id: Some("episode:test".to_string()),
        content: Some("inline content".to_string()),
        text: None,
        source_type: None,
        source_id: None,
        t_ref: None,
        scope: None,
        zero_shot_labels: None,
    };

    let result = memory_mcp::tools::extract(&service, params).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MemoryError::Validation(msg) => {
            assert!(
                msg.contains("not both"),
                "expected 'not both' in msg: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[tokio::test]
async fn tools_extract_rejects_no_input() {
    let service = common::make_service().await;

    let params = ExtractParams {
        episode_id: None,
        content: None,
        text: None,
        source_type: None,
        source_id: None,
        t_ref: None,
        scope: None,
        zero_shot_labels: None,
    };

    let result = memory_mcp::tools::extract(&service, params).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MemoryError::Validation(msg) => {
            assert!(
                msg.contains("exactly one"),
                "expected 'exactly one' in msg: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[tokio::test]
async fn tools_ingest_and_extract_happy_path() {
    let service = common::make_service().await;

    let params = IngestParams {
        source_type: "test".to_string(),
        source_id: "t-2".to_string(),
        content: "Alice works at Acme Corp and promised to deliver the API by Friday.".to_string(),
        t_ref: Utc::now().to_rfc3339(),
        scope: "org".to_string(),
        project: None,
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };

    let response = memory_mcp::tools::ingest(&service, params)
        .await
        .expect("ingest should succeed");
    assert_eq!(response.status, "success");
    assert!(
        response.result.starts_with("episode:"),
        "result: {:?}",
        response.result
    );
    assert_eq!(
        response.guidance.as_deref(),
        Some("Call extract next to derive entities and facts."),
    );

    // Now extract from the ingested episode
    let extract_params = ExtractParams {
        episode_id: Some(response.result),
        content: None,
        text: None,
        source_type: None,
        source_id: None,
        t_ref: None,
        scope: None,
        zero_shot_labels: None,
    };

    let extract_response = memory_mcp::tools::extract(&service, extract_params)
        .await
        .expect("extract should succeed");
    assert_eq!(extract_response.status, "success");
    assert!(
        !extract_response.result.entities.is_empty(),
        "expected entities"
    );
}

#[tokio::test]
async fn tools_resolve_creates_canonical_entity() {
    let service = common::make_service().await;

    let params = ResolveParams {
        entity_type: "person".to_string(),
        canonical_name: "Alice Smith".to_string(),
        aliases: vec!["Alice".to_string(), "A. Smith".to_string()],
    };

    let response = memory_mcp::tools::resolve(&service, params)
        .await
        .expect("resolve should succeed");
    assert_eq!(response.status, "success");
    assert!(
        response.result.starts_with("entity:"),
        "result: {:?}",
        response.result
    );
    assert_eq!(
        response.guidance.as_deref(),
        Some("Use this entity_id when linking facts or relationships."),
    );
}

#[tokio::test]
async fn tools_invalidate_validates_t_invalid() {
    let service = common::make_service().await;

    let params = InvalidateParams {
        fact_id: "fact:nonexistent".to_string(),
        reason: "test".to_string(),
        t_invalid: "bad-date".to_string(),
    };

    let result = memory_mcp::tools::invalidate(&service, params).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MemoryError::Validation(msg) => {
            assert!(
                msg.contains("t_invalid"),
                "expected t_invalid in msg: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[tokio::test]
async fn tools_assemble_context_returns_empty_for_empty_db() {
    let service = common::make_service().await;

    let params = AssembleContextParams {
        query: "nothing relevant".to_string(),
        scope: "personal".to_string(),
        project: None,
        fact_types: vec![],
        as_of: String::new(),
        budget: 5,
        view_mode: None,
        window_start: None,
        window_end: None,
    };

    let response = memory_mcp::tools::assemble_context(&service, params)
        .await
        .expect("assemble_context should succeed");
    assert_eq!(response.status, "success");
    assert_eq!(response.total_count, Some(0));
    assert!(response.result.is_empty());
}

#[tokio::test]
async fn tools_explain_rejects_bad_json() {
    let service = common::make_service().await;

    let params = ExplainParams {
        context_items: "not valid json".to_string(),
    };

    let result = memory_mcp::tools::explain(&service, params).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        MemoryError::Validation(msg) => {
            assert!(
                msg.contains("Invalid") || msg.contains("parse"),
                "expected validation msg about invalid JSON, got: {msg}"
            );
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }
}
