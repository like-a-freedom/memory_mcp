//! `extract` tool — protocol-agnostic.

use std::time::Instant;

use chrono::Utc;
use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExtractResult, IngestRequest};
use crate::service::MemoryError;
use crate::service::build_extract_log_result;
use crate::service::capabilities::extract::ExtractCapability;
use crate::service::capabilities::ingest::IngestCapability;
use crate::service::episode_from_record;
use crate::service::service_context::ServiceContext;
use crate::tools::params::ExtractParams;
use crate::tools::parsers::{content_hash, normalize_optional_string, parse_datetime};
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Extract entities, facts, and relationships from remembered content.
///
/// Handles extracting from `episode_id` or ingesting inline content first.
pub async fn extract(
    ctx: &ServiceContext,
    params: ExtractParams,
) -> Result<ToolResponse<ExtractResult>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("extract");
    let access = AccessPayload::default();
    let episode_id = normalize_optional_string(params.episode_id);
    let content = normalize_optional_string(params.content);
    let text = normalize_optional_string(params.text);
    let source_type = params.source_type;
    let source_id = params.source_id;
    let t_ref = params.t_ref;
    let zero_shot_labels = params.zero_shot_labels;
    let timer = Instant::now();
    let request_id = next_request_id();

    ctx.log_tool_event(
        "extract.start",
        json!({"episode_id": &episode_id, "has_content": content.is_some() || text.is_some()}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    if content.is_some() && text.is_some() {
        let message = "Invalid extract arguments: use only one inline snake_case field — `content` or `text` — not both. Do not wrap arguments in `payload`.";
        ctx.log_tool_event_with_duration(
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
        ctx.log_tool_event_with_duration(
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
        ctx.log_tool_event_with_duration(
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
        match ExtractCapability::extract(ctx, episode_id, Some(access), zero_shot_labels.as_deref())
            .await
        {
            Ok(result) => {
                record_extract_results(&operation_metrics, &result);
                operation_metrics.success();
                let log_result = match ctx.find_episode_record(episode_id).await {
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

                ctx.log_tool_event_with_duration(
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
                ctx.log_tool_event_with_duration(
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

    let content = inline_content.ok_or_else(|| {
        MemoryError::Validation("inline extract content was unexpectedly missing".to_string())
    })?;

    let source_type = source_type.unwrap_or_else(|| "ad-hoc".to_string());
    let source_id = source_id.unwrap_or_else(|| content_hash(&content));
    let t_ref = t_ref
        .as_ref()
        .and_then(|s| parse_datetime(s))
        .unwrap_or_else(Utc::now);
    match IngestCapability::ingest(
        ctx,
        IngestRequest {
            source_type,
            source_id,
            content,
            t_ref,
            t_ingested: None,
            policy_tags: Vec::new(),
        },
        Some(access.clone()),
    )
    .await
    {
        Ok(episode_id) => {
            match ExtractCapability::extract(
                ctx,
                &episode_id,
                Some(access),
                zero_shot_labels.as_deref(),
            )
            .await
            {
                Ok(result) => {
                    record_extract_results(&operation_metrics, &result);
                    operation_metrics.success();
                    let log_result = match ctx.find_episode_record(&episode_id).await {
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

                    ctx.log_tool_event_with_duration(
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
                    ctx.log_tool_event_with_duration(
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
        Err(err) => {
            ctx.log_tool_event_with_duration(
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

fn record_extract_results(
    metrics: &crate::observability::OperationMetrics,
    result: &ExtractResult,
) {
    metrics.record_result("entities", result.entities.len());
    metrics.record_result("facts", result.facts.len());
    metrics.record_result("links", result.links.len());
    metrics.record_result("warnings", result.warnings.len());
}
