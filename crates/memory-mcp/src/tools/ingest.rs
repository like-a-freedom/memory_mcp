//! `ingest` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, IngestRequest};
use crate::service::MemoryError;
use crate::tools::context::{ToolContext, ToolEvent};
use crate::tools::params::IngestParams;
use crate::tools::parsers::parse_datetime;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Ingest an episode and return its `episode_id`.
///
/// Mirrors the previous `MemoryMcp::ingest` body exactly: same validation,
/// same `ingest.start` / `ingest.done` / `ingest.error` events, same
/// `ToolResponse::success_with_guidance` guidance string.
pub async fn ingest<T: ToolContext>(
    ctx: &T,
    params: IngestParams,
) -> Result<ToolResponse<String>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("ingest");
    let t_ref = parse_datetime(&params.t_ref).ok_or_else(|| {
        MemoryError::Validation(format!(
            "Invalid `t_ref` value: {}. \
             Provide a valid ISO 8601 timestamp with seconds, e.g. 2026-05-11T17:34:00Z or \
             2026-05-11T17:34:00+00:00.",
            params.t_ref
        ))
    })?;
    let t_ingested = params.t_ingested.as_ref().and_then(|s| parse_datetime(s));
    let access = AccessPayload::default();
    let request = IngestRequest {
        source_type: params.source_type,
        source_id: params.source_id,
        content: params.content,
        t_ref,
        t_ingested,
        policy_tags: params.policy_tags,
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    let source_id = request.source_id.clone();
    ctx.record(ToolEvent {
        op: "ingest.start",
        args: json!({"source_type": &request.source_type, "source_id": &source_id}),
        result: json!({}),
        level: LogLevel::Info,
        request_id: Some(request_id.clone()),
        duration: None,
    });

    match ctx.ingest(request, Some(access)).await {
        Ok(episode_id) => {
            operation_metrics.record_result("episodes", 1);
            operation_metrics.success();
            ctx.record(ToolEvent {
                op: "ingest.done",
                args: json!({"source_id": &source_id}),
                result: json!({"episode_id": &episode_id}),
                level: LogLevel::Info,
                request_id: Some(request_id),
                duration: Some(timer.elapsed()),
            });
            Ok(ToolResponse::success_with_guidance(
                episode_id,
                "Call extract next to derive entities and facts.",
            ))
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "ingest.error",
                args: json!({"source_id": &source_id}),
                result: json!({"error": err.to_string()}),
                level: LogLevel::Warn,
                request_id: Some(request_id),
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}
