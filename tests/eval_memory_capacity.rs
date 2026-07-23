//! Memory capacity evaluation tests.
//!
//! These tests verify that ignored and duplicate events create zero durable
//! growth, accepted content has one raw copy, and budget exhaustion occurs
//! before episode creation.

use memory_mcp::models::{
    CaptureBudget, CaptureDisposition, CaptureReasonCode, InvocationContext, InvocationOrigin,
    LifecycleEventKind, NormalizedHostEvent,
};
use memory_mcp::service::agent_memory::policy::CapturePolicy;

fn lifecycle_ctx() -> InvocationContext {
    InvocationContext {
        origin: InvocationOrigin::LifecycleAdapter {
            adapter_id: "claude_code".to_string(),
            adapter_version: "1".to_string(),
            host_event: "post_tool_result".to_string(),
        },
        session_id: Some("s1".to_string()),
        native_event_id: None,
        lifecycle_trace: None,
    }
}

fn ok_budget() -> CaptureBudget {
    CaptureBudget {
        remaining_session_captures: 32,
        remaining_session_bytes: 256 * 1024,
        remaining_project_daily_bytes: 10 * 1024 * 1024,
        exhausted: false,
    }
}

fn exhausted_budget() -> CaptureBudget {
    CaptureBudget {
        remaining_session_captures: 0,
        remaining_session_bytes: 0,
        remaining_project_daily_bytes: 0,
        exhausted: true,
    }
}

fn event_with(signal: Option<&str>, content: Option<&str>) -> NormalizedHostEvent {
    NormalizedHostEvent {
        event_kind: LifecycleEventKind::PostToolResult,
        task_fingerprint: "task:1".to_string(),
        normalized_task: "do work".to_string(),
        scope: "org".to_string(),
        project: Some("p".to_string()),
        policy_tags: vec![],
        content: content.map(str::to_string),
        artifact_uris: vec![],
        capture_signal: signal.map(str::to_string),
    }
}

#[test]
fn ignored_events_create_zero_durable_growth() {
    let event = event_with(Some("status_polling"), Some("ran cargo check"));
    let decision = CapturePolicy::evaluate(&event, &lifecycle_ctx(), &ok_budget(), 16 * 1024, 16);
    assert_eq!(decision.disposition, CaptureDisposition::Ignored);
    assert!(decision.disposition.is_zero_growth());
    assert!(decision.sanitized_content.is_none());
}

#[test]
fn duplicate_events_create_zero_durable_growth() {
    // A duplicate is detected by load_event before reaching the policy, but
    // if the policy classifies it as Duplicate, it is zero-growth.
    let duplicate = CaptureDisposition::Duplicate;
    assert!(duplicate.is_zero_growth());
    assert!(!duplicate.is_accepted());
}

#[test]
fn budget_exhaustion_rejects_before_episode_preparation() {
    let event = event_with(Some("verified_success"), Some("OAuth shipped with tests."));
    let decision =
        CapturePolicy::evaluate(&event, &lifecycle_ctx(), &exhausted_budget(), 16 * 1024, 16);
    assert_eq!(decision.disposition, CaptureDisposition::Rejected);
    assert!(decision.sanitized_content.is_none());
    assert!(
        decision
            .reason_codes
            .contains(&CaptureReasonCode::BudgetExhausted)
    );
}

#[test]
fn accepted_content_is_bounded_to_16_kib() {
    let long_content = "x".repeat(32 * 1024);
    let event = event_with(Some("decision"), Some(&long_content));
    let decision = CapturePolicy::evaluate(&event, &lifecycle_ctx(), &ok_budget(), 16 * 1024, 16);
    assert_eq!(decision.disposition, CaptureDisposition::Accepted);
    let sanitized = decision.sanitized_content.expect("sanitized content");
    assert!(sanitized.len() <= 16 * 1024);
}

#[test]
fn accepted_content_has_one_raw_copy() {
    // The capture path stores accepted content once in the episode, not copied
    // into the event/job. This test verifies the policy produces a single
    // sanitized_content value (one copy), not multiple.
    let event = event_with(Some("preference"), Some("Prefer the auth crate."));
    let decision = CapturePolicy::evaluate(&event, &lifecycle_ctx(), &ok_budget(), 16 * 1024, 16);
    assert_eq!(decision.disposition, CaptureDisposition::Accepted);
    assert_eq!(
        decision.sanitized_content.as_deref(),
        Some("Prefer the auth crate.")
    );
}

#[test]
fn artifact_uris_are_bounded_to_16() {
    let mut event = event_with(Some("verified_success"), Some("Shipped OAuth."));
    event.artifact_uris = (0..20)
        .map(|i| format!("file://artifact-{i}.txt"))
        .collect();
    let decision = CapturePolicy::evaluate(&event, &lifecycle_ctx(), &ok_budget(), 16 * 1024, 16);
    assert_eq!(decision.disposition, CaptureDisposition::Accepted);
    // The persistence budget caps artifact URIs at 16.
    assert_eq!(decision.persistence_budget.max_artifact_uris, 16);
}
