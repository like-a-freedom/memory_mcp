//! Internal lifecycle-recall CLI command — hidden, consumed by hook scripts.
//!
//! Not a public tool (ADR-0016 AD-2). Constructs a `NormalizedHostEvent` +
//! `InvocationContext` from JSON arguments and delegates to
//! `MemoryService::recall_lifecycle_event`.

use crate::cli::args::LifecycleRecallArgs;
use crate::cli::commands::write_response;
use crate::models::{InvocationContext, NormalizedHostEvent};
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::service::agent_memory::recall::{LifecycleRecallResult, RecallDecision};

pub async fn run(service: &MemoryService, args: LifecycleRecallArgs) -> Result<(), MemoryError> {
    let event: NormalizedHostEvent = serde_json::from_str(&args.event)
        .map_err(|err| MemoryError::Validation(format!("invalid --event JSON: {err}")))?;
    let context: InvocationContext = serde_json::from_str(&args.context)
        .map_err(|err| MemoryError::Validation(format!("invalid --context JSON: {err}")))?;

    let result = service.recall_lifecycle_event(&event, &context).await?;

    let response = match result {
        Some(LifecycleRecallResult::Recalled { items, decision }) => {
            let decision_str = match decision {
                RecallDecision::Default => "default",
                RecallDecision::WakeUp => "wake_up",
                RecallDecision::Suppress => "suppress",
                RecallDecision::Force => "force",
            };
            serde_json::json!({
                "status": "recalled",
                "decision": decision_str,
                "count": items.len(),
            })
        }
        Some(LifecycleRecallResult::Suppressed) => {
            serde_json::json!({"status": "suppressed"})
        }
        None => serde_json::json!({"status": "disabled"}),
    };
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
