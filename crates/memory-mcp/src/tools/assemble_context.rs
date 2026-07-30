//! `assemble_context` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::AssembleContextRequest;
use crate::service::MemoryError;
use crate::service::capabilities::assemble_context::AssembleContextCapability;
use crate::service::service_context::ServiceContext;
use crate::tools::params::AssembleContextParams;
use crate::tools::parsers::parse_datetime;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Assemble the most relevant active memory context for a query.
///
/// The result field is serialized to [`serde_json::Value`] inside this
/// function while the compact-mode guard is alive, and the outer
/// `ToolResponse<Value>` envelope is returned. Serialize happens on the same
/// thread and task as the guard; the guard is dropped before this returns.
pub async fn assemble_context(
    ctx: &ServiceContext,
    params: AssembleContextParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let compact = params.compact;
    let as_of = if params.as_of.trim().is_empty() {
        None
    } else {
        chrono::DateTime::parse_from_rfc3339(&params.as_of)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc))
    };
    let window_start = params.window_start.as_deref().and_then(parse_datetime);
    let window_end = params.window_end.as_deref().and_then(parse_datetime);
    let request = AssembleContextRequest {
        query: params.query,
        scope: params.scope,
        project: params.project,
        fact_types: params.fact_types,
        as_of,
        budget: params.budget,
        view_mode: params.view_mode,
        window_start,
        window_end,
        access: None,
        compact,
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    ctx.log_tool_event(
        "assemble_context.start",
        json!({"scope": request.scope, "query": request.query}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    match AssembleContextCapability::assemble_context(ctx, request).await {
        Ok(results) => {
            ctx.log_tool_event_with_duration(
                "assemble_context.done",
                json!({}),
                json!({"count": results.len()}),
                LogLevel::Info,
                timer.elapsed(),
                Some(&request_id),
            );
            let count = results.len();
            // Under compact mode, omit `quote` and slim `rationale` via the
            // serde adapters reading the thread-local CompactGuard. The guard
            // is held across serialization; `value` contains the compact form.
            let value = {
                let _guard = crate::tools::compact::set_compact(compact);
                serde_json::to_value(&results)
                    .map_err(|e| MemoryError::Transient(format!("serialize context items: {e}")))?
            };
            if compact {
                Ok(ToolResponse::complete_list_compact(
                    value,
                    count,
                    "Call explain if you need provenance-ready citations for selected items.",
                ))
            } else {
                Ok(ToolResponse::complete_list(
                    value,
                    count,
                    "Call explain if you need provenance-ready citations for selected items.",
                ))
            }
        }
        Err(err) => {
            ctx.log_tool_event_with_duration(
                "assemble_context.error",
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
