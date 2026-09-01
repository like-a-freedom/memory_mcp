//! Internal lifecycle-capture CLI command — hidden, consumed by hook scripts.
//!
//! Not a public tool. Constructs a `NormalizedHostEvent` +
//! `InvocationContext` from JSON arguments and delegates to
//! `MemoryService::capture_lifecycle_event`.

use crate::cli::args::LifecycleCaptureArgs;
use crate::cli::commands::write_response;
use crate::models::{InvocationContext, NormalizedHostEvent};
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::service::agent_memory::capture::LifecycleCaptureResult;

pub async fn run(service: &MemoryService, args: LifecycleCaptureArgs) -> Result<(), MemoryError> {
    let event: NormalizedHostEvent = serde_json::from_str(&args.event)
        .map_err(|err| MemoryError::Validation(format!("invalid --event JSON: {err}")))?;
    let context: InvocationContext = serde_json::from_str(&args.context)
        .map_err(|err| MemoryError::Validation(format!("invalid --context JSON: {err}")))?;

    let result = service.capture_lifecycle_event(&event, &context).await?;

    let response = match result {
        Some(LifecycleCaptureResult::Accepted {
            event_id,
            episode_id,
            job_id,
        }) => serde_json::json!({
            "status": "accepted",
            "event_id": event_id,
            "episode_id": episode_id,
            "job_id": job_id,
        }),
        Some(LifecycleCaptureResult::Duplicate { event_id }) => serde_json::json!({
            "status": "duplicate",
            "event_id": event_id,
        }),
        Some(LifecycleCaptureResult::Ignored) => serde_json::json!({"status": "ignored"}),
        Some(LifecycleCaptureResult::Quarantined { event_id }) => serde_json::json!({
            "status": "quarantined",
            "event_id": event_id,
        }),
        Some(LifecycleCaptureResult::Rejected) => serde_json::json!({"status": "rejected"}),
        Some(LifecycleCaptureResult::Degraded) => serde_json::json!({"status": "degraded"}),
        None => serde_json::json!({"status": "disabled"}),
    };
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
