//! Codex host adapter.
//!
//! Pins the Codex hook contract and maps supported hook events to internal
//! lifecycle invocations. A mapping that exists for Codex is not assumed to
//! exist for Claude Code.

use super::{BridgeInvocation, BridgePlan, BridgeReason, HostAdapter, NormalizedHostEventInput};
use crate::models::{InvocationContext, InvocationOrigin, LifecycleEventKind, NormalizedHostEvent};

/// The pinned Codex host version.
pub const CODEX_VERSION: &str = "1.0";

/// The adapter identifier.
pub const ADAPTER_ID: &str = "codex";

/// Supported Codex hook event names.
pub const SUPPORTED_EVENTS: &[&str] = &[
    "session_start",
    "user_turn",
    "tool_call",
    "tool_result",
    "compaction",
    "turn_complete",
];

/// Codex host adapter.
pub struct CodexAdapter;

impl CodexAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn map_event_kind(event_name: &str) -> Option<LifecycleEventKind> {
        match event_name {
            "session_start" => Some(LifecycleEventKind::SessionStart),
            "user_turn" => Some(LifecycleEventKind::UserPrompt),
            "tool_call" => Some(LifecycleEventKind::PreToolBoundary),
            "tool_result" => Some(LifecycleEventKind::PostToolResult),
            "compaction" => Some(LifecycleEventKind::PreCompaction),
            "turn_complete" => Some(LifecycleEventKind::TaskStop),
            _ => None,
        }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostAdapter for CodexAdapter {
    fn adapter_id(&self) -> &str {
        ADAPTER_ID
    }

    fn adapter_version(&self) -> &str {
        CODEX_VERSION
    }

    fn normalize(&self, input: &NormalizedHostEventInput) -> BridgePlan {
        let Some(event_kind) = Self::map_event_kind(&input.event_name) else {
            return BridgePlan::ignored(BridgeReason::UnsupportedEvent);
        };

        let context = InvocationContext {
            origin: InvocationOrigin::LifecycleAdapter {
                adapter_id: self.adapter_id().to_string(),
                adapter_version: self.adapter_version().to_string(),
                host_event: input.event_name.clone(),
            },
            session_id: input.session_id.clone(),
            native_event_id: input.native_event_id.clone(),
            lifecycle_trace: None,
        };

        let event = NormalizedHostEvent {
            event_kind: event_kind.clone(),
            task_fingerprint: input.task_fingerprint.clone(),
            normalized_task: input.normalized_task.clone(),
            scope: input.scope.clone(),
            project: input.project.clone(),
            policy_tags: input.policy_tags.clone(),
            content: input.content.clone(),
            artifact_uris: input.artifact_uris.clone(),
            capture_signal: input.capture_signal.clone(),
        };

        // Session start and turn boundaries: recall only.
        if matches!(
            event_kind,
            LifecycleEventKind::SessionStart | LifecycleEventKind::PreToolBoundary
        ) {
            return BridgePlan {
                invocations: vec![BridgeInvocation::Recall(super::NormalizedRecall {
                    task_fingerprint: input.task_fingerprint.clone(),
                    normalized_task: input.normalized_task.clone(),
                    scope: input.scope.clone(),
                    project: input.project.clone(),
                    policy_tags: input.policy_tags.clone(),
                })],
                ignored_reason: None,
                degraded_reason: None,
            };
        }

        // Tool result and turn complete: capture only.
        if matches!(
            event_kind,
            LifecycleEventKind::PostToolResult | LifecycleEventKind::TaskStop
        ) {
            return BridgePlan {
                invocations: vec![BridgeInvocation::Capture(super::NormalizedCapture {
                    event,
                    context,
                    budget: default_budget(),
                })],
                ignored_reason: None,
                degraded_reason: None,
            };
        }

        // User turn: recall + optional capture.
        if matches!(event_kind, LifecycleEventKind::UserPrompt) {
            let mut invocations = vec![BridgeInvocation::Recall(super::NormalizedRecall {
                task_fingerprint: input.task_fingerprint.clone(),
                normalized_task: input.normalized_task.clone(),
                scope: input.scope.clone(),
                project: input.project.clone(),
                policy_tags: input.policy_tags.clone(),
            })];
            if input.content.is_some() && input.capture_signal.is_some() {
                invocations.push(BridgeInvocation::Capture(super::NormalizedCapture {
                    event,
                    context,
                    budget: default_budget(),
                }));
            }
            return BridgePlan {
                invocations,
                ignored_reason: None,
                degraded_reason: None,
            };
        }

        // Compaction: capture checkpoint.
        BridgePlan {
            invocations: vec![BridgeInvocation::Capture(super::NormalizedCapture {
                event,
                context,
                budget: default_budget(),
            })],
            ignored_reason: None,
            degraded_reason: None,
        }
    }
}

fn default_budget() -> crate::models::CaptureBudget {
    crate::models::CaptureBudget {
        remaining_session_captures: 32,
        remaining_session_bytes: 256 * 1024,
        remaining_project_daily_bytes: 10 * 1024 * 1024,
        exhausted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(event_name: &str) -> NormalizedHostEventInput {
        NormalizedHostEventInput {
            host: "codex".to_string(),
            host_version: CODEX_VERSION.to_string(),
            event_name: event_name.to_string(),
            session_id: Some("s1".to_string()),
            native_event_id: Some("e1".to_string()),
            task_fingerprint: "task:1".to_string(),
            normalized_task: "do work".to_string(),
            scope: "org".to_string(),
            project: Some("p".to_string()),
            policy_tags: vec![],
            content: None,
            artifact_uris: vec![],
            capture_signal: None,
        }
    }

    #[test]
    fn session_start_produces_recall_only() {
        let adapter = CodexAdapter::new();
        let plan = adapter.normalize(&make_input("session_start"));
        assert_eq!(plan.invocations.len(), 1);
        assert!(matches!(plan.invocations[0], BridgeInvocation::Recall(_)));
    }

    #[test]
    fn tool_result_produces_capture_only() {
        let adapter = CodexAdapter::new();
        let plan = adapter.normalize(&make_input("tool_result"));
        assert_eq!(plan.invocations.len(), 1);
        assert!(matches!(plan.invocations[0], BridgeInvocation::Capture(_)));
    }

    #[test]
    fn unsupported_event_is_ignored() {
        let adapter = CodexAdapter::new();
        let plan = adapter.normalize(&make_input("nonexistent"));
        assert!(plan.invocations.is_empty());
        assert_eq!(plan.ignored_reason, Some(BridgeReason::UnsupportedEvent));
    }

    #[test]
    fn codex_does_not_support_post_compaction_resume() {
        // Codex has no post_compaction event; this is unsupported, not degraded.
        let adapter = CodexAdapter::new();
        let plan = adapter.normalize(&make_input("post_compaction"));
        assert!(plan.invocations.is_empty());
        assert_eq!(plan.ignored_reason, Some(BridgeReason::UnsupportedEvent));
    }
}
