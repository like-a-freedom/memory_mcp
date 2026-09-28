//! `explain` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExplainRequest};
use crate::tools::context::{ToolContext, ToolEvent};
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
pub async fn explain<T: ToolContext>(
    ctx: &T,
    params: ExplainParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("explain");
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
    ctx.record(ToolEvent {
        op: "explain.start",
        args: json!({"count": request.context_pack.len()}),
        result: json!({}),
        level: LogLevel::Info,
        request_id: Some(request_id.clone()),
        duration: None,
    });

    match ctx.explain(request, Some(access)).await {
        Ok(explanations) => {
            ctx.record(ToolEvent {
                op: "explain.done",
                args: json!({}),
                result: json!({"count": explanations.len()}),
                level: LogLevel::Info,
                request_id: Some(request_id.clone()),
                duration: Some(timer.elapsed()),
            });
            let count = explanations.len();
            operation_metrics.record_result("explanations", count);
            // Under compact mode, omit `quote` via the serde adapters reading
            // the thread-local CompactGuard. The guard is held across
            // serialization; `value` contains the compact form.
            let value = {
                let _guard = crate::tools::compact::set_compact(compact);
                serde_json::to_value(&explanations)
                    .map_err(|e| MemoryError::Transient(format!("serialize explain items: {e}")))?
            };
            operation_metrics.success();
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
            ctx.record(ToolEvent {
                op: "explain.error",
                args: json!({}),
                result: json!({"error": err.to_string()}),
                level: LogLevel::Warn,
                request_id: Some(request_id),
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}
