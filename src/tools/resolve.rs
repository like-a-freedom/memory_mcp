//! `resolve` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, EntityCandidate};
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::ResolveParams;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Resolve a canonical entity identifier for a name and its aliases.
pub async fn resolve(
    service: &MemoryService,
    params: ResolveParams,
) -> Result<ToolResponse<String>, MemoryError> {
    let access = AccessPayload::default();
    let candidate = EntityCandidate {
        entity_type: params.entity_type,
        canonical_name: params.canonical_name,
        aliases: params.aliases,
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    service.log_tool_event(
        "resolve.start",
        json!({"entity_type": candidate.entity_type, "canonical": candidate.canonical_name}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    match service.resolve(candidate, Some(access)).await {
        Ok(entity_id) => {
            service.log_tool_event_with_duration(
                "resolve.done",
                json!({}),
                json!({"entity_id": &entity_id}),
                LogLevel::Info,
                timer.elapsed(),
                Some(&request_id),
            );
            Ok(ToolResponse::success_with_guidance(
                entity_id,
                "Use this entity_id when linking facts or relationships.",
            ))
        }
        Err(err) => {
            service.log_tool_event_with_duration(
                "resolve.error",
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
