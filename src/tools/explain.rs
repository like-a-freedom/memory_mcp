//! `explain` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExplainItem, ExplainRequest};
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::ExplainParams;
use crate::tools::parsers::parse_context_items;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Explain context items with provenance-ready citations.
pub async fn explain(
    service: &MemoryService,
    params: ExplainParams,
) -> Result<ToolResponse<Vec<ExplainItem>>, MemoryError> {
    let access = AccessPayload::default();
    let context_pack =
        parse_context_items(&params.context_items).map_err(MemoryError::Validation)?;
    let request = ExplainRequest { context_pack };

    let timer = Instant::now();
    let request_id = next_request_id();
    service.log_tool_event(
        "explain.start",
        json!({"count": request.context_pack.len()}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    match service.explain(request, Some(access)).await {
        Ok(explanations) => {
            service.log_tool_event_with_duration(
                "explain.done",
                json!({}),
                json!({"count": explanations.len()}),
                LogLevel::Info,
                timer.elapsed(),
                Some(&request_id),
            );
            let count = explanations.len();
            Ok(ToolResponse::complete_list(
                explanations,
                count,
                "Use these citations directly in the final response.",
            ))
        }
        Err(err) => {
            service.log_tool_event_with_duration(
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
