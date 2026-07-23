//! Host adapter bridge: normalizes versioned host events to internal
//! lifecycle invocations.
//!
//! The bridge maps a host boundary event to zero or more internal
//! `BridgeInvocation`s (recall and/or capture). It does not call storage
//! directly — it produces a `BridgePlan` that the server executes.
//!
//! An event absent from the installed host contract is unsupported, not
//! silently substituted. Each adapter documents its exact subset.

pub mod claude_code;
pub mod codex;
pub mod transport;

use crate::models::{CaptureBudget, InvocationContext, NormalizedHostEvent};

/// A normalized host event ready for lifecycle processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedHostEventInput {
    pub host: String,
    pub host_version: String,
    pub event_name: String,
    pub session_id: Option<String>,
    pub native_event_id: Option<String>,
    pub task_fingerprint: String,
    pub normalized_task: String,
    pub scope: String,
    pub project: Option<String>,
    pub policy_tags: Vec<String>,
    pub content: Option<String>,
    pub artifact_uris: Vec<String>,
    pub capture_signal: Option<String>,
}

/// An internal invocation derived from a host event.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeInvocation {
    Recall(NormalizedRecall),
    Capture(NormalizedCapture),
}

/// Normalized recall request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRecall {
    pub task_fingerprint: String,
    pub normalized_task: String,
    pub scope: String,
    pub project: Option<String>,
    pub policy_tags: Vec<String>,
}

/// Normalized capture request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedCapture {
    pub event: NormalizedHostEvent,
    pub context: InvocationContext,
    pub budget: CaptureBudget,
}

/// The reason a bridge invocation was ignored or degraded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BridgeReason {
    UnsupportedEvent,
    VersionMismatch,
    EmptyEvent,
    ReadOnlyNoise,
}

/// A plan produced by the bridge for one host event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgePlan {
    pub invocations: Vec<BridgeInvocation>,
    pub ignored_reason: Option<BridgeReason>,
    pub degraded_reason: Option<BridgeReason>,
}

impl BridgePlan {
    /// An empty plan that ignores the event.
    #[must_use]
    pub fn ignored(reason: BridgeReason) -> Self {
        Self {
            invocations: Vec::new(),
            ignored_reason: Some(reason),
            degraded_reason: None,
        }
    }

    /// A degraded plan that never pretends enforcement succeeded.
    #[must_use]
    pub fn degraded(reason: BridgeReason) -> Self {
        Self {
            invocations: Vec::new(),
            ignored_reason: None,
            degraded_reason: Some(reason),
        }
    }
}

/// Trait for host adapters that normalize host events to internal invocations.
pub trait HostAdapter: Send + Sync {
    /// The adapter identifier (e.g., "claude_code", "codex").
    fn adapter_id(&self) -> &str;

    /// The adapter version.
    fn adapter_version(&self) -> &str;

    /// Normalize a host event input to a bridge plan.
    fn normalize(&self, input: &NormalizedHostEventInput) -> BridgePlan;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_plan_has_no_invocations() {
        let plan = BridgePlan::ignored(BridgeReason::UnsupportedEvent);
        assert!(plan.invocations.is_empty());
        assert_eq!(plan.ignored_reason, Some(BridgeReason::UnsupportedEvent));
        assert!(plan.degraded_reason.is_none());
    }

    #[test]
    fn degraded_plan_has_no_invocations() {
        let plan = BridgePlan::degraded(BridgeReason::VersionMismatch);
        assert!(plan.invocations.is_empty());
        assert!(plan.ignored_reason.is_none());
        assert_eq!(plan.degraded_reason, Some(BridgeReason::VersionMismatch));
    }
}
