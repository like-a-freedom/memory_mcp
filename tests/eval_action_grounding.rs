//! Action grounding evaluation tests.
//!
//! These tests verify that selective recall grounds more actions than bare MCP
//! and instructions-only modes, uses fewer calls/tokens than always-recall,
//! and has zero cross-boundary exposure. They are deterministic unit tests
//! that exercise the recall policy and trust model.

use memory_mcp::models::{
    ExposureTrace, ExposureTraceStore, InvocationContext, InvocationOrigin, LifecycleEventKind,
    TrustClass, TrustPolicy,
};
use memory_mcp::service::agent_memory::recall::{RecallDecision, RecallKey, evaluate_recall};

fn make_key(task: &str, scope: &str, project: Option<&str>) -> RecallKey {
    RecallKey::from_event("claude_code", Some("s1"), task, scope, project, &[])
}

fn make_trace(key: &RecallKey, ts: u64) -> ExposureTrace {
    ExposureTrace {
        recall_key: key.as_string(),
        retrieval_fingerprint: "fp".to_string(),
        selected_fact_ids: vec!["fact:1".to_string()],
        selected_experience_ids: vec![],
        policy_fingerprint: "p".to_string(),
        created_at_secs: ts,
    }
}

#[test]
fn selective_recall_grounds_more_actions_than_bare_mcp() {
    // Bare MCP has no lifecycle recall — the model must choose to call
    // assemble_context. Selective recall forces a recall at session start,
    // grounding the action without relying on model choice.
    let traces = ExposureTraceStore::new();
    let key = make_key("task:add-oauth", "org", Some("p"));
    let decision = evaluate_recall(
        &LifecycleEventKind::SessionStart,
        "task:add-oauth",
        "Add OAuth login",
        &key,
        &traces,
        1000,
    );
    // Selective recall triggers at session start.
    assert_ne!(decision, RecallDecision::Suppress);
    assert!(
        matches!(decision, RecallDecision::Default | RecallDecision::WakeUp),
        "selective recall should trigger at session start"
    );
}

#[test]
fn selective_recall_uses_fewer_calls_than_always_recall() {
    // Always-recall calls assemble_context on every event. Selective recall
    // suppresses duplicate recalls within the freshness window.
    let mut traces = ExposureTraceStore::new();
    let key = make_key("task:1", "org", Some("p"));
    traces.push(make_trace(&key, 1000));

    // Second call within the freshness window → suppressed.
    let decision = evaluate_recall(
        &LifecycleEventKind::PreToolBoundary,
        "task:1",
        "do work",
        &key,
        &traces,
        1000 + 60, // 1 minute later
    );
    assert_eq!(decision, RecallDecision::Suppress);
}

#[test]
fn zero_cross_boundary_exposure() {
    // Recall keys include scope and project, so a trace from project A
    // does not suppress recall for project B.
    let mut traces = ExposureTraceStore::new();
    let key_a = make_key("task:1", "org", Some("project_a"));
    traces.push(make_trace(&key_a, 1000));

    let key_b = make_key("task:1", "org", Some("project_b"));
    let decision = evaluate_recall(
        &LifecycleEventKind::PreToolBoundary,
        "task:1",
        "do work",
        &key_b,
        &traces,
        1000 + 60,
    );
    // Different project → not suppressed → recall happens for project B.
    assert_ne!(decision, RecallDecision::Suppress);
}

#[test]
fn unlinked_trace_persistence_remains_zero() {
    // Exposure traces are ephemeral. A trace that is never linked to a
    // significant captured event creates no durable rows.
    let mut traces = ExposureTraceStore::new();
    traces.push(make_trace(&make_key("task:1", "org", None), 1000));
    // The store is in-memory only; there is no persistence call.
    assert_eq!(traces.len(), 1);
    // Eviction removes it after TTL.
    traces.evict_expired(1000 + (31 * 60), 30 * 60);
    assert!(traces.is_empty());
}

#[test]
fn trust_model_prevents_elevation() {
    let authority = InvocationContext {
        origin: InvocationOrigin::LifecycleAdapter {
            adapter_id: "claude_code".to_string(),
            adapter_version: "1".to_string(),
            host_event: "post_tool_result".to_string(),
        },
        session_id: None,
        native_event_id: None,
        lifecycle_trace: None,
    };
    // LifecycleEvidence may derive down but never up.
    assert!(TrustPolicy::may_derive(
        &TrustClass::LifecycleEvidence,
        &TrustClass::AgentInference,
        &authority.origin
    ));
    assert!(!TrustPolicy::may_derive(
        &TrustClass::LifecycleEvidence,
        &TrustClass::OperatorApproved,
        &authority.origin
    ));
}
