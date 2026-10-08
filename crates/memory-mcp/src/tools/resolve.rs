//! `resolve` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessPayload, EntityCandidate};
use crate::tools::context::{ToolContext, ToolEvent};
use crate::tools::params::ResolveParams;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Resolve a canonical entity identifier for a name and its aliases.
pub async fn resolve<T: ToolContext>(
    ctx: &T,
    params: ResolveParams,
) -> Result<ToolResponse<String>, MemoryError> {
    let id = crate::logging::correlation::current().unwrap_or_else(next_request_id);
    crate::logging::correlation::scope(id, resolve_inner(ctx, params)).await
}

async fn resolve_inner<T: ToolContext>(
    ctx: &T,
    params: ResolveParams,
) -> Result<ToolResponse<String>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("resolve");
    let access = AccessPayload::default();
    let candidate = EntityCandidate {
        entity_type: params.entity_type,
        canonical_name: params.canonical_name,
        aliases: params.aliases,
    };

    let timer = Instant::now();
    ctx.record(ToolEvent {
        op: "resolve.start",
        args: json!({"entity_type": candidate.entity_type, "canonical": candidate.canonical_name}),
        result: json!({}),
        level: LogLevel::Info,
        duration: None,
    });

    match ctx.resolve(candidate, Some(access)).await {
        Ok(entity_id) => {
            operation_metrics.record_result("entities", 1);
            operation_metrics.success();
            ctx.record(ToolEvent {
                op: "resolve.done",
                args: json!({}),
                result: json!({"entity_id": &entity_id}),
                level: LogLevel::Info,
                duration: Some(timer.elapsed()),
            });
            Ok(ToolResponse::success_with_guidance(
                entity_id,
                "Use this entity_id when linking facts or relationships.",
            ))
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "resolve.error",
                args: json!({}),
                result: json!({"error": err.to_string()}),
                level: LogLevel::Warn,
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}
