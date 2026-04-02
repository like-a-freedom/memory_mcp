use chrono::{TimeZone, Utc};
use rmcp::handler::server::wrapper::Parameters;

use memory_mcp::mcp::MemoryMcp;

mod common;

#[tokio::test]
async fn e2e_open_memory_inspector_entity() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let t = Utc::now();
    let fact_id =
        common::seed_fact_at(&mcp.service(), "personal", "Alice works at Acme Corp.", t).await;

    let params = serde_json::json!({
        "scope": "personal",
        "target_type": "fact",
        "target_id": fact_id
    });

    let result = mcp
        .open_memory_inspector(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("open inspector should succeed")
        .0;

    assert_eq!(result.status, "success");
    assert!(result.result.get("fact").is_some());
}

#[tokio::test]
async fn e2e_open_memory_inspector_episode() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let episode_id =
        common::ingest_episode(&mcp.service(), "test-2", "Bob met with Carol yesterday.").await;

    let params = serde_json::json!({
        "scope": "personal",
        "target_type": "episode",
        "target_id": episode_id,
        "page_size": 10
    });

    let result = mcp
        .open_memory_inspector(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("open inspector should succeed")
        .0;

    assert_eq!(result.status, "success");
    assert!(result.result.get("episode").is_some());
}

#[tokio::test]
async fn e2e_close_session() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let episode_id =
        common::ingest_episode(&mcp.service(), "test-3", "Test content for session close.").await;

    let params = serde_json::json!({
        "scope": "personal",
        "target_type": "episode",
        "target_id": episode_id,
        "page_size": 10
    });

    let _open_result = mcp
        .open_memory_inspector(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("open inspector should succeed")
        .0;

    let close_params = serde_json::json!({
        "session_id": "nonexistent"
    });

    let close_result = mcp
        .close_session(Parameters(serde_json::from_value(close_params).unwrap()))
        .await;

    assert!(close_result.is_err());
}

#[tokio::test]
async fn e2e_open_ingestion_review() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let params = serde_json::json!({
        "scope": "personal",
        "source_text": "This is a test document about Acme Corp.",
        "ttl_seconds": 3600
    });

    let result = mcp
        .open_ingestion_review(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("open ingestion review should succeed")
        .0;

    assert_eq!(result.status, "success");
    assert!(result.result.get("draft_id").is_some());
    assert!(result.result.get("session_id").is_some());
}

#[tokio::test]
async fn e2e_cancel_ingestion_review() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let open_params = serde_json::json!({
        "scope": "personal",
        "source_text": "Test document"
    });

    let open_result = mcp
        .open_ingestion_review(Parameters(serde_json::from_value(open_params).unwrap()))
        .await
        .expect("open should succeed")
        .0;

    let session_id = open_result
        .result
        .get("session_id")
        .unwrap()
        .as_str()
        .unwrap();

    let cancel_params = serde_json::json!({
        "session_id": session_id
    });

    let cancel_result = mcp
        .cancel_ingestion_review(Parameters(serde_json::from_value(cancel_params).unwrap()))
        .await
        .expect("cancel should succeed")
        .0;

    assert_eq!(cancel_result.status, "success");
}

#[tokio::test]
async fn e2e_open_lifecycle_console() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let params = serde_json::json!({
        "scope": "personal",
        "filters": {
            "min_confidence": 0.5,
            "inactive_days": 30
        }
    });

    let result = mcp
        .open_lifecycle_console(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("open lifecycle console should succeed")
        .0;

    assert_eq!(result.status, "success");
    assert!(result.result.get("session_id").is_some());
}

#[tokio::test]
async fn e2e_lifecycle_operations_dry_run() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let open_params = serde_json::json!({
        "scope": "personal"
    });

    let open_result = mcp
        .open_lifecycle_console(Parameters(serde_json::from_value(open_params).unwrap()))
        .await
        .expect("open should succeed")
        .0;

    let session_id = open_result
        .result
        .get("session_id")
        .unwrap()
        .as_str()
        .unwrap();

    let recompute_params = serde_json::json!({
        "session_id": session_id,
        "dry_run": true
    });

    let recompute_result = mcp
        .recompute_decay(Parameters(
            serde_json::from_value(recompute_params).unwrap(),
        ))
        .await
        .expect("recompute should succeed")
        .0;

    assert_eq!(recompute_result.status, "success");
}

#[tokio::test]
async fn e2e_get_lifecycle_task_status() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let params = serde_json::json!({
        "task_id": "task:test-123"
    });

    let result = mcp
        .get_lifecycle_task_status(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("get status should succeed")
        .0;

    assert_eq!(result.status, "success");
}

#[tokio::test]
async fn e2e_open_graph_path_no_path() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let params = serde_json::json!({
        "scope": "personal",
        "from_entity_id": "entity:unknown-1",
        "to_entity_id": "entity:unknown-2",
        "max_depth": 2
    });

    let result = mcp
        .open_graph_path(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("open graph path should succeed")
        .0;

    assert_eq!(result.status, "success");
    assert!(result.result.get("path").is_some() || result.result.get("path_found").is_some());
}

#[tokio::test]
async fn e2e_open_temporal_diff() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();

    common::seed_fact_at(&mcp.service(), "personal", "Fact created in January", t1).await;

    let params = serde_json::json!({
        "scope": "personal",
        "target_type": "scope",
        "as_of_left": t1.to_rfc3339(),
        "as_of_right": t2.to_rfc3339(),
        "time_axis": "valid"
    });

    let result = mcp
        .open_temporal_diff(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("open temporal diff should succeed")
        .0;

    assert_eq!(result.status, "success");
    assert!(result.result.get("session_id").is_some());
}
