//! `invalidate` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, InvalidateRequest};
use crate::service::MemoryError;
use crate::tools::context::{ToolContext, ToolEvent};
use crate::tools::params::InvalidateParams;
use crate::tools::parsers::parse_datetime;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Invalidate a fact while preserving historical traceability.
pub async fn invalidate<T: ToolContext>(
    ctx: &T,
    params: InvalidateParams,
) -> Result<ToolResponse<String>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("invalidate");
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
    ctx.record(ToolEvent {
        op: "invalidate.start",
        args: json!({"fact_id": &fact_id}),
        result: json!({}),
        level: LogLevel::Info,
        request_id: Some(request_id.clone()),
        duration: None,
    });

    match ctx.invalidate(request, Some(access)).await {
        Ok(()) => {
            operation_metrics.record_result("invalidations", 1);
            operation_metrics.success();
            ctx.record(ToolEvent {
                op: "invalidate.done",
                args: json!({"fact_id": &fact_id}),
                result: json!({"status": "invalidated"}),
                level: LogLevel::Info,
                request_id: Some(request_id),
                duration: Some(timer.elapsed()),
            });
            Ok(ToolResponse::success_with_guidance(
                "invalidated".to_string(),
                "Re-run assemble_context with a fresh `as_of` timestamp to confirm the fact is no longer active.",
            ))
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "invalidate.error",
                args: json!({"fact_id": &fact_id}),
                result: json!({"error": err.to_string()}),
                level: LogLevel::Warn,
                request_id: Some(request_id),
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}
