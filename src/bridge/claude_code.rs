//! Claude Code host adapter.
//!
//! Pins the Claude Code hook contract and maps supported hook events to
//! internal lifecycle invocations. Unsupported or renamed events are explicit
//! degraded cases.

use super::{BridgeInvocation, BridgePlan, BridgeReason, HostAdapter, NormalizedHostEventInput};
use crate::models::{InvocationContext, InvocationOrigin, LifecycleEventKind, NormalizedHostEvent};

/// The pinned Claude Code host version.
pub const CLAUDE_CODE_VERSION: &str = "1.0";

/// The adapter identifier.
pub const ADAPTER_ID: &str = "claude_code";

/// Supported Claude Code hook event names.
pub const SUPPORTED_EVENTS: &[&str] = &[
    "session_start",
    "user_prompt",
    "pre_tool",
    "post_tool",
    "pre_compaction",
    "post_compaction",
    "task_stop",
];

/// Claude Code host adapter.
pub struct ClaudeCodeAdapter;

impl ClaudeCodeAdapter {
    /// Create a new adapter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Map a Claude Code event name to an internal lifecycle event kind.
    fn map_event_kind(event_name: &str) -> Option<LifecycleEventKind> {
        match event_name {
            "session_start" => Some(LifecycleEventKind::SessionStart),
            "user_prompt" => Some(LifecycleEventKind::UserPrompt),
            "pre_tool" => Some(LifecycleEventKind::PreToolBoundary),
            "post_tool" => Some(LifecycleEventKind::PostToolResult),
            "pre_compaction" => Some(LifecycleEventKind::PreCompaction),
            "post_compaction" => Some(LifecycleEventKind::PostCompactionResume),
            "task_stop" => Some(LifecycleEventKind::TaskStop),
            _ => None,
        }
    }
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostAdapter for ClaudeCodeAdapter {
    fn adapter_id(&self) -> &str {
        ADAPTER_ID
    }

    fn adapter_version(&self) -> &str {
        CLAUDE_CODE_VERSION
    }

    fn normalize(&self, input: &NormalizedHostEventInput) -> BridgePlan {
        // Validate the event is supported.
        let Some(event_kind) = Self::map_event_kind(&input.event_name) else {
            return BridgePlan::ignored(BridgeReason::UnsupportedEvent);
        };

        // Build the invocation context.
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

        // Build the normalized host event.
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

        // Session start: recall only (no capture unless there is content).
        if matches!(event_kind, LifecycleEventKind::SessionStart) {
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

        // Post-tool result, pre-compaction, task stop: capture only.
        if matches!(
            event_kind,
            LifecycleEventKind::PostToolResult
                | LifecycleEventKind::PreCompaction
                | LifecycleEventKind::TaskStop
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

        // User prompt: recall when task changes, capture only explicit signals.
        if matches!(event_kind, LifecycleEventKind::UserPrompt) {
            let mut invocations = Vec::new();
            invocations.push(BridgeInvocation::Recall(super::NormalizedRecall {
                task_fingerprint: input.task_fingerprint.clone(),
                normalized_task: input.normalized_task.clone(),
                scope: input.scope.clone(),
                project: input.project.clone(),
                policy_tags: input.policy_tags.clone(),
            }));
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

        // Pre-tool boundary: recall only.
        if matches!(event_kind, LifecycleEventKind::PreToolBoundary) {
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

        // Post-compaction resume: force recall.
        BridgePlan {
            invocations: vec![BridgeInvocation::Recall(super::NormalizedRecall {
                task_fingerprint: input.task_fingerprint.clone(),
                normalized_task: input.normalized_task.clone(),
                scope: input.scope.clone(),
                project: input.project.clone(),
                policy_tags: input.policy_tags.clone(),
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
            host: "claude_code".to_string(),
            host_version: CLAUDE_CODE_VERSION.to_string(),
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
        let adapter = ClaudeCodeAdapter::new();
        let plan = adapter.normalize(&make_input("session_start"));
        assert_eq!(plan.invocations.len(), 1);
        assert!(matches!(plan.invocations[0], BridgeInvocation::Recall(_)));
    }

    #[test]
    fn post_tool_produces_capture_only() {
        let adapter = ClaudeCodeAdapter::new();
        let plan = adapter.normalize(&make_input("post_tool"));
        assert_eq!(plan.invocations.len(), 1);
        assert!(matches!(plan.invocations[0], BridgeInvocation::Capture(_)));
    }

    #[test]
    fn unsupported_event_is_ignored() {
        let adapter = ClaudeCodeAdapter::new();
        let plan = adapter.normalize(&make_input("nonexistent_event"));
        assert!(plan.invocations.is_empty());
        assert_eq!(plan.ignored_reason, Some(BridgeReason::UnsupportedEvent));
    }

    #[test]
    fn user_prompt_with_signal_produces_recall_and_capture() {
        let adapter = ClaudeCodeAdapter::new();
        let mut input = make_input("user_prompt");
        input.content = Some("Prefer the auth crate.".to_string());
        input.capture_signal = Some("preference".to_string());
        let plan = adapter.normalize(&input);
        assert_eq!(plan.invocations.len(), 2);
    }

    #[test]
    fn adapter_id_and_version_are_pinned() {
        let adapter = ClaudeCodeAdapter::new();
        assert_eq!(adapter.adapter_id(), "claude_code");
        assert_eq!(adapter.adapter_version(), CLAUDE_CODE_VERSION);
    }
}
