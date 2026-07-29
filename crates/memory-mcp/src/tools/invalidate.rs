//! `invalidate` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, InvalidateRequest};
use crate::service::MemoryError;
use crate::service::capabilities::invalidate::InvalidateCapability;
use crate::service::service_context::ServiceContext;
use crate::tools::params::InvalidateParams;
use crate::tools::parsers::parse_datetime;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Invalidate a fact while preserving historical traceability.
pub async fn invalidate(
    ctx: &ServiceContext,
    params: InvalidateParams,
) -> Result<ToolResponse<String>, MemoryError> {
    let access = AccessPayload::default();
    let t_invalid = parse_datetime(&params.t_invalid).ok_or_else(|| {
        MemoryError::Validation(format!(
            "Invalid t_invalid format. \
             Provide a valid ISO 8601 timestamp indicating when the fact became invalid, e.g. \
             2026-05-11T17:34:00Z. \
             Could not parse `t_invalid` as an ISO 8601 datetime: {}. \
             Accepted formats: 2026-05-11T17:34:00Z, 2026-05-11T17:34:00+05:00.",
            params.t_invalid
        ))
    })?;
    let request = InvalidateRequest {
        fact_id: params.fact_id,
        reason: params.reason,
        t_invalid,
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    let fact_id = request.fact_id.clone();
    ctx.log_tool_event(
        "invalidate.start",
        json!({"fact_id": &fact_id}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    match InvalidateCapability::invalidate(ctx, request, Some(access)).await {
        Ok(()) => {
            ctx.log_tool_event_with_duration(
                "invalidate.done",
                json!({"fact_id": &fact_id}),
                json!({"status": "invalidated"}),
                LogLevel::Info,
                timer.elapsed(),
                Some(&request_id),
            );
            Ok(ToolResponse::success_with_guidance(
                "invalidated".to_string(),
                "Re-run assemble_context with a fresh `as_of` timestamp to confirm the fact is no longer active.",
            ))
        }
        Err(err) => {
            ctx.log_tool_event_with_duration(
                "invalidate.error",
                json!({"fact_id": &fact_id}),
                json!({"error": err.to_string()}),
                LogLevel::Warn,
                timer.elapsed(),
                Some(&request_id),
            );
            Err(err)
        }
    }
}
