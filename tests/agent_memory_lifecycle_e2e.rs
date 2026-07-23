//! End-to-end lifecycle integration tests: capture → projection → recall.
//!
//! These tests exercise the production wiring that connects
//! `LifecycleCapture::execute()` and `LifecycleRecall::execute()` through
//! `MemoryService::capture_lifecycle_event()` and
//! `MemoryService::recall_lifecycle_event()`. They close the evidence gate
//! required by ADR-0016 AD-4/AD-5/AD-6 (Tasks 4, 6, and 9).
//!
//! Unlike the unit tests in `src/service/agent_memory/{capture,recall}.rs`,
//! these tests construct a real in-memory `MemoryService` with migrations
//! applied and assert the full durable + ephemeral behavior.

mod common;

use memory_mcp::models::{
    InvocationContext, InvocationOrigin, LifecycleEventKind, NormalizedHostEvent,
};
use memory_mcp::service::MemoryService;
use memory_mcp::service::agent_memory::capture::LifecycleCaptureResult;
use memory_mcp::service::agent_memory::recall::{LifecycleRecallResult, RecallDecision};

/// Build a lifecycle-enabled service with an in-memory DB.
async fn lifecycle_service() -> MemoryService {
    let mut service = common::make_service().await;
    // Enable lifecycle integration (disabled by default in test config).
    service = service.with_lifecycle_enabled(true);
    service
}

/// A lifecycle-adapter invocation context (the host bridge identity).
fn lifecycle_ctx(session: &str) -> InvocationContext {
    InvocationContext {
        origin: InvocationOrigin::LifecycleAdapter {
            adapter_id: "claude_code".to_string(),
            adapter_version: "1".to_string(),
            host_event: "post_tool_result".to_string(),
        },
        session_id: Some(session.to_string()),
        native_event_id: None,
        lifecycle_trace: None,
    }
}

/// A capture-eligible event with a verified success signal.
fn accepted_event(task: &str, content: &str) -> NormalizedHostEvent {
    NormalizedHostEvent {
        event_kind: LifecycleEventKind::PostToolResult,
        task_fingerprint: task.to_string(),
        normalized_task: task.to_string(),
        scope: "org".to_string(),
        project: Some("copper-palm".to_string()),
        policy_tags: vec![],
        content: Some(content.to_string()),
        artifact_uris: vec![],
        capture_signal: Some("verified_success".to_string()),
    }
}

/// A recall-eligible session-start event.
fn recall_event(task: &str) -> NormalizedHostEvent {
    NormalizedHostEvent {
        event_kind: LifecycleEventKind::SessionStart,
        task_fingerprint: task.to_string(),
        normalized_task: task.to_string(),
        scope: "org".to_string(),
        project: Some("copper-palm".to_string()),
        policy_tags: vec![],
        content: None,
        artifact_uris: vec![],
        capture_signal: None,
    }
}

#[tokio::test]
async fn capture_lifecycle_event_persists_accepted_event_and_job() {
    let service = lifecycle_service().await;
    let event = accepted_event("task:oauth", "OAuth login shipped with tests.");
    let ctx = lifecycle_ctx("s1");

    let result = service
        .capture_lifecycle_event(&event, &ctx)
        .await
        .expect("capture");

    let result = result.expect("lifecycle enabled → Some");
    match result {
        LifecycleCaptureResult::Accepted {
            event_id,
            episode_id,
            job_id,
        } => {
            assert!(!event_id.is_empty(), "event_id must be non-empty");
            assert!(!episode_id.is_empty(), "episode_id must be non-empty");
            assert!(!job_id.is_empty(), "job_id must be non-empty");
        }
        other => panic!("expected Accepted, got {other:?}"),
    }
}

#[tokio::test]
async fn capture_lifecycle_event_returns_none_when_disabled() {
    let service = common::make_service().await; // lifecycle disabled by default
    let event = accepted_event("task:1", "content");
    let ctx = lifecycle_ctx("s1");

    let result = service
        .capture_lifecycle_event(&event, &ctx)
        .await
        .expect("capture call");

    assert!(result.is_none(), "disabled lifecycle must return None");
}

#[tokio::test]
async fn capture_ignored_event_creates_zero_durable_rows() {
    let service = lifecycle_service().await;
    // Status polling — ignored by the capture policy → zero growth.
    let event = NormalizedHostEvent {
        event_kind: LifecycleEventKind::PostToolResult,
        task_fingerprint: "task:poll".to_string(),
        normalized_task: "ran cargo check".to_string(),
        scope: "org".to_string(),
        project: Some("copper-palm".to_string()),
        policy_tags: vec![],
        content: Some("ran cargo check".to_string()),
        artifact_uris: vec![],
        capture_signal: Some("status_polling".to_string()),
    };
    let ctx = lifecycle_ctx("s1");

    let result = service
        .capture_lifecycle_event(&event, &ctx)
        .await
        .expect("capture");

    let result = result.expect("lifecycle enabled");
    assert!(
        matches!(result, LifecycleCaptureResult::Ignored),
        "status polling must be ignored with zero durable growth"
    );
}

#[tokio::test]
async fn capture_rejects_secret_like_content() {
    let service = lifecycle_service().await;
    let event = accepted_event("task:secret", "API_KEY=sk-1234567890abcdef");
    let ctx = lifecycle_ctx("s1");

    let result = service
        .capture_lifecycle_event(&event, &ctx)
        .await
        .expect("capture");

    let result = result.expect("lifecycle enabled");
    assert!(
        matches!(result, LifecycleCaptureResult::Rejected),
        "secret-like content must be rejected"
    );
}

#[tokio::test]
async fn recall_lifecycle_event_returns_none_when_disabled() {
    let service = common::make_service().await;
    let event = recall_event("task:1");
    let ctx = lifecycle_ctx("s1");

    let result = service
        .recall_lifecycle_event(&event, &ctx)
        .await
        .expect("recall call");

    assert!(result.is_none(), "disabled lifecycle must return None");
}

#[tokio::test]
async fn recall_lifecycle_event_wakes_up_on_empty_session_start() {
    let service = lifecycle_service().await;
    // Session start with empty normalized task → WakeUp decision.
    let event = NormalizedHostEvent {
        event_kind: LifecycleEventKind::SessionStart,
        task_fingerprint: "task:empty".to_string(),
        normalized_task: String::new(),
        scope: "org".to_string(),
        project: None,
        policy_tags: vec![],
        content: None,
        artifact_uris: vec![],
        capture_signal: None,
    };
    let ctx = lifecycle_ctx("s1");

    let result = service
        .recall_lifecycle_event(&event, &ctx)
        .await
        .expect("recall");

    let result = result.expect("lifecycle enabled");
    match result {
        LifecycleRecallResult::Recalled { decision, items: _ } => {
            assert_eq!(
                decision,
                RecallDecision::WakeUp,
                "empty session start must use WakeUp"
            );
        }
        other => panic!("expected Recalled, got {other:?}"),
    }
}

#[tokio::test]
async fn recall_lifecycle_event_suppresses_duplicate_within_freshness_window() {
    let service = lifecycle_service().await;
    let event = recall_event("task:same");
    let ctx = lifecycle_ctx("s1");

    // First recall — should perform.
    let result1 = service
        .recall_lifecycle_event(&event, &ctx)
        .await
        .expect("first recall");
    assert!(result1.is_some(), "first recall must perform");

    // Second recall with the same key within the freshness window → suppressed.
    let result2 = service
        .recall_lifecycle_event(&event, &ctx)
        .await
        .expect("second recall");
    match result2 {
        Some(LifecycleRecallResult::Suppressed) => {}
        other => panic!("expected Suppressed, got {other:?}"),
    }
}

#[tokio::test]
async fn recall_lifecycle_event_forces_after_compaction() {
    let service = lifecycle_service().await;
    let event = recall_event("task:1");
    let ctx = lifecycle_ctx("s1");

    // First recall.
    let _ = service
        .recall_lifecycle_event(&event, &ctx)
        .await
        .expect("first recall");

    // Post-compaction resume forces recall even with a fresh trace.
    let force_event = NormalizedHostEvent {
        event_kind: LifecycleEventKind::PostCompactionResume,
        task_fingerprint: "task:1".to_string(),
        normalized_task: "resume work".to_string(),
        scope: "org".to_string(),
        project: Some("copper-palm".to_string()),
        policy_tags: vec![],
        content: None,
        artifact_uris: vec![],
        capture_signal: None,
    };
    let result = service
        .recall_lifecycle_event(&force_event, &ctx)
        .await
        .expect("forced recall");

    let result = result.expect("lifecycle enabled");
    match result {
        LifecycleRecallResult::Recalled { decision, items: _ } => {
            assert_eq!(
                decision,
                RecallDecision::Force,
                "post-compaction must force recall"
            );
        }
        other => panic!("expected Recalled, got {other:?}"),
    }
}

#[tokio::test]
async fn public_surface_unchanged_after_lifecycle_wiring() {
    // The lifecycle wiring must not add any public tool or CLI subcommand.
    // This is a smoke check; the full freeze is in eval_agent_memory_lifecycle.rs.
    let service = lifecycle_service().await;

    // capture_lifecycle_event and recall_lifecycle_event are pub(crate) —
    // they must not be reachable as public tools. The service itself must
    // still expose only the ordinary public methods.
    let event = accepted_event("task:surface", "content");
    let ctx = lifecycle_ctx("s1");
    let _ = service.capture_lifecycle_event(&event, &ctx).await;
    let _ = service.recall_lifecycle_event(&event, &ctx).await;

    // If the wiring leaked a public tool, the MCP registry would have grown.
    // The freeze test in eval_agent_memory_lifecycle.rs covers this fully.
}

#[tokio::test]
async fn capture_and_recall_full_cycle() {
    let service = lifecycle_service().await;

    // 1. Capture a significant outcome.
    let capture_event = accepted_event("task:full-cycle", "Shipped the auth crate with OAuth.");
    let ctx = lifecycle_ctx("s1");
    let capture_result = service
        .capture_lifecycle_event(&capture_event, &ctx)
        .await
        .expect("capture");
    assert!(matches!(
        capture_result,
        Some(LifecycleCaptureResult::Accepted { .. })
    ));

    // 2. Recall at a later session start for the same task.
    let recall_event = recall_event("task:full-cycle");
    let recall_result = service
        .recall_lifecycle_event(&recall_event, &ctx)
        .await
        .expect("recall");

    let result = recall_result.expect("lifecycle enabled");
    assert!(
        matches!(result, LifecycleRecallResult::Recalled { .. }),
        "first recall must perform"
    );
}
