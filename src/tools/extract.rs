//! `extract` tool — protocol-agnostic.

use std::time::Instant;

use chrono::Utc;
use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExtractResult, IngestRequest};
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::service::build_extract_log_result;
use crate::service::episode_from_record;
use crate::tools::params::ExtractParams;
use crate::tools::parsers::{content_hash, normalize_optional_string, parse_datetime};
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Extract entities, facts, and relationships from remembered content.
///
/// Handles extracting from `episode_id` or ingesting inline content first.
pub async fn extract(
    service: &MemoryService,
    params: ExtractParams,
) -> Result<ToolResponse<ExtractResult>, MemoryError> {
    let access = AccessPayload::default();
    let episode_id = normalize_optional_string(params.episode_id);
    let content = normalize_optional_string(params.content);
    let text = normalize_optional_string(params.text);
    let source_type = params.source_type;
    let source_id = params.source_id;
    let t_ref = params.t_ref;
    let scope = params.scope;
    let zero_shot_labels = params.zero_shot_labels;
    let timer = Instant::now();
    let request_id = next_request_id();

    service.log_tool_event(
        "extract.start",
        json!({"episode_id": &episode_id, "has_content": content.is_some() || text.is_some()}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    if content.is_some() && text.is_some() {
        let message = "Invalid extract arguments: use only one inline snake_case field — `content` or `text` — not both. Do not wrap arguments in `payload`.";
        service.log_tool_event_with_duration(
            "extract.invalid_input",
            json!({"episode_id": &episode_id, "has_content": true}),
            json!({"error": message}),
            LogLevel::Warn,
            timer.elapsed(),
            Some(&request_id),
        );
        return Err(MemoryError::Validation(message.to_string()));
    }

    let inline_content = content.or(text);

    if episode_id.is_some() && inline_content.is_some() {
        let message = "Invalid extract arguments: provide exactly one snake_case input source. Use `episode_id` for stored content, or `content`/`text` for inline text, but not both. Do not wrap arguments in `payload`.";
        service.log_tool_event_with_duration(
            "extract.invalid_input",
            json!({"episode_id": &episode_id, "has_content": true}),
            json!({"error": message}),
            LogLevel::Warn,
            timer.elapsed(),
            Some(&request_id),
        );
        return Err(MemoryError::Validation(message.to_string()));
    }

    if episode_id.is_none() && inline_content.is_none() {
        let message = "Invalid extract arguments: provide exactly one snake_case input source — `episode_id` or non-empty `content`/`text`. Do not wrap arguments in `payload`.";
        service.log_tool_event_with_duration(
            "extract.invalid_input",
            json!({"episode_id": &episode_id, "has_content": false}),
            json!({"error": message}),
            LogLevel::Warn,
            timer.elapsed(),
            Some(&request_id),
        );
        return Err(MemoryError::Validation(message.to_string()));
    }

    if let Some(ref episode_id) = episode_id {
        match service
            .extract(episode_id, Some(access), zero_shot_labels.as_deref())
            .await
        {
            Ok(result) => {
                let log_result = match service.find_episode_record(episode_id).await {
                    Ok((record, _)) => {
                        let episode = record.as_ref().and_then(episode_from_record);
                        build_extract_log_result(
                            episode.as_ref(),
                            result.entities.len(),
                            &result.facts,
                            result.links.len(),
                            result.warnings.len(),
                        )
                    }
                    Err(_) => build_extract_log_result(
                        None,
                        result.entities.len(),
                        &result.facts,
                        result.links.len(),
                        result.warnings.len(),
                    ),
                };

                service.log_tool_event_with_duration(
                    "extract.done",
                    json!({"episode_id": episode_id}),
                    log_result,
                    LogLevel::Info,
                    timer.elapsed(),
                    Some(&request_id),
                );
                return Ok(ToolResponse::success_with_guidance(
                    result,
                    "Resolve canonical entities for any ambiguous names before creating manual links.",
                ));
            }
            Err(err) => {
                service.log_tool_event_with_duration(
                    "extract.error",
                    json!({"episode_id": episode_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                    Some(&request_id),
                );
                return Err(err);
            }
        }
    }

    let content = inline_content.expect("validated extract inline content");

    let source_type = source_type.unwrap_or_else(|| "ad-hoc".to_string());
    let source_id = source_id.unwrap_or_else(|| content_hash(&content));
    let t_ref = t_ref
        .as_ref()
        .and_then(|s| parse_datetime(s))
        .unwrap_or_else(Utc::now);
    let scope = scope.unwrap_or_else(|| "org".to_string());

    match service
        .ingest(
            IngestRequest {
                source_type,
                source_id,
                content,
                t_ref,
                scope,
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: Vec::new(),
            },
            Some(access.clone()),
        )
        .await
    {
        Ok(episode_id) => match service
            .extract(&episode_id, Some(access), zero_shot_labels.as_deref())
            .await
        {
            Ok(result) => {
                let log_result = match service.find_episode_record(&episode_id).await {
                    Ok((record, _)) => {
                        let episode = record.as_ref().and_then(episode_from_record);
                        build_extract_log_result(
                            episode.as_ref(),
                            result.entities.len(),
                            &result.facts,
                            result.links.len(),
                            result.warnings.len(),
                        )
                    }
                    Err(_) => build_extract_log_result(
                        None,
                        result.entities.len(),
                        &result.facts,
                        result.links.len(),
                        result.warnings.len(),
                    ),
                };

                service.log_tool_event_with_duration(
                    "extract.done",
                    json!({"episode_id": &episode_id}),
                    log_result,
                    LogLevel::Info,
                    timer.elapsed(),
                    Some(&request_id),
                );
                Ok(ToolResponse::success_with_guidance(
                    result,
                    "Resolve canonical entities for any ambiguous names before creating manual links.",
                ))
            }
            Err(err) => {
                service.log_tool_event_with_duration(
                    "extract.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                    Some(&request_id),
                );
                Err(err)
            }
        },
        Err(err) => {
            service.log_tool_event_with_duration(
                "extract.error",
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
