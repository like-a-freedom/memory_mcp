//! `assemble_context` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::AssembleContextRequest;
use crate::tools::context::{ToolContext, ToolEvent};
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
pub async fn assemble_context<T: ToolContext>(
    ctx: &T,
    params: AssembleContextParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let id = crate::logging::correlation::current().unwrap_or_else(next_request_id);
    crate::logging::correlation::scope(id, assemble_context_inner(ctx, params)).await
}

async fn assemble_context_inner<T: ToolContext>(
    ctx: &T,
    params: AssembleContextParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("assemble_context");
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
    ctx.record(ToolEvent {
        op: "assemble_context.start",
        args: json!({"query": request.query}),
        result: json!({}),
        level: LogLevel::Info,
        duration: None,
    });

    match ctx.assemble_context(request).await {
        Ok(results) => {
            ctx.record(ToolEvent {
                op: "assemble_context.done",
                args: json!({}),
                result: json!({"count": results.len()}),
                level: LogLevel::Info,
                duration: Some(timer.elapsed()),
            });
            let count = results.len();
            operation_metrics.record_result("items", count);
            // Under compact mode, omit `quote` and slim `rationale` via the
            // serde adapters reading the thread-local CompactGuard. The guard
            // is held across serialization; `value` contains the compact form.
            let value = {
                let _guard = crate::tools::compact::set_compact(compact);
                serde_json::to_value(&results)
                    .map_err(|e| MemoryError::Transient(format!("serialize context items: {e}")))?
            };
            operation_metrics.success();
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
            ctx.record(ToolEvent {
                op: "assemble_context.error",
                args: json!({}),
                result: json!({"error": err.to_string()}),
                level: err.log_level(),
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}
