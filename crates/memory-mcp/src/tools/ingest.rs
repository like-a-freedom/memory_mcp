//! `ingest` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, IngestRequest};
use crate::service::MemoryError;
use crate::service::capabilities::ingest::IngestCapability;
use crate::service::service_context::ServiceContext;
use crate::tools::params::IngestParams;
use crate::tools::parsers::parse_datetime;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Ingest an episode and return its `episode_id`.
///
/// Mirrors the previous `MemoryMcp::ingest` body exactly: same validation,
/// same `ingest.start` / `ingest.done` / `ingest.error` events, same
/// `ToolResponse::success_with_guidance` guidance string.
pub async fn ingest(
    ctx: &ServiceContext,
    params: IngestParams,
) -> Result<ToolResponse<String>, MemoryError> {
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
        scope: params.scope,
        project: params.project,
        t_ingested,
        visibility_scope: params.visibility_scope,
        policy_tags: params.policy_tags,
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    let source_id = request.source_id.clone();
    ctx.log_tool_event(
        "ingest.start",
        json!({"source_type": &request.source_type, "source_id": &source_id, "scope": &request.scope}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    match IngestCapability::ingest(ctx, request, Some(access)).await {
        Ok(episode_id) => {
            ctx.log_tool_event_with_duration(
                "ingest.done",
                json!({"source_id": &source_id}),
                json!({"episode_id": &episode_id}),
                LogLevel::Info,
                timer.elapsed(),
                Some(&request_id),
            );
            Ok(ToolResponse::success_with_guidance(
                episode_id,
                "Call extract next to derive entities and facts.",
            ))
        }
        Err(err) => {
            ctx.log_tool_event_with_duration(
                "ingest.error",
                json!({"source_id": &source_id}),
                json!({"error": err.to_string()}),
                LogLevel::Warn,
                timer.elapsed(),
                Some(&request_id),
            );
            Err(err)
        }
    }
}
