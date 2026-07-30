//! `explain` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExplainRequest};
use crate::service::MemoryError;
use crate::service::capabilities::explain::ExplainCapability;
use crate::service::service_context::ServiceContext;
use crate::tools::params::ExplainParams;
use crate::tools::parsers::parse_context_items;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Explain context items with provenance-ready citations.
///
/// The result field is serialized to [`serde_json::Value`] inside this
/// function while the compact-mode guard is alive, and the outer
/// `ToolResponse<Value>` envelope is returned. Serialize happens on the same
/// thread and task as the guard; the guard is dropped before this returns.
pub async fn explain(
    ctx: &ServiceContext,
    params: ExplainParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let access = AccessPayload::default();
    let context_pack =
        parse_context_items(&params.context_items).map_err(MemoryError::Validation)?;
    let compact = params.compact;
    let request = ExplainRequest {
        context_pack,
        compact,
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    ctx.log_tool_event(
        "explain.start",
        json!({"count": request.context_pack.len()}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    match ExplainCapability::explain(ctx, request, Some(access)).await {
        Ok(explanations) => {
            ctx.log_tool_event_with_duration(
                "explain.done",
                json!({}),
                json!({"count": explanations.len()}),
                LogLevel::Info,
                timer.elapsed(),
                Some(&request_id),
            );
            let count = explanations.len();
            // Under compact mode, omit `quote` via the serde adapters reading
            // the thread-local CompactGuard. The guard is held across
            // serialization; `value` contains the compact form.
            let value = {
                let _guard = crate::tools::compact::set_compact(compact);
                serde_json::to_value(&explanations)
                    .map_err(|e| MemoryError::Transient(format!("serialize explain items: {e}")))?
            };
            if compact {
                Ok(ToolResponse::complete_list_compact(
                    value,
                    count,
                    "Use these citations directly in the final response.",
                ))
            } else {
                Ok(ToolResponse::complete_list(
                    value,
                    count,
                    "Use these citations directly in the final response.",
                ))
            }
        }
        Err(err) => {
            ctx.log_tool_event_with_duration(
                "explain.error",
                json!({}),
                json!({"error": err.to_string()}),
                LogLevel::Warn,
                timer.elapsed(),
                Some(&request_id),
            );
            Err(err)
        }
    }
}
