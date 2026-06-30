//! `assemble_context` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AssembleContextRequest, AssembledContextItem};
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::AssembleContextParams;
use crate::tools::parsers::parse_datetime;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Assemble the most relevant active memory context for a query.
pub async fn assemble_context(
    service: &MemoryService,
    params: AssembleContextParams,
) -> Result<ToolResponse<Vec<AssembledContextItem>>, MemoryError> {
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
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    service.log_tool_event(
        "assemble_context.start",
        json!({"scope": request.scope, "query": request.query}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    match service.assemble_context(request).await {
        Ok(results) => {
            service.log_tool_event_with_duration(
                "assemble_context.done",
                json!({}),
                json!({"count": results.len()}),
                LogLevel::Info,
                timer.elapsed(),
                Some(&request_id),
            );
            let count = results.len();
            Ok(ToolResponse::complete_list(
                results,
                count,
                "Call explain if you need provenance-ready citations for selected items.",
            ))
        }
        Err(err) => {
            service.log_tool_event_with_duration(
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
