//! `extract` tool — protocol-agnostic.

use std::time::Instant;

use chrono::Utc;
use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExtractResult, IngestRequest};
use crate::service::build_extract_log_result;
use crate::tools::context::{ToolContext, ToolEvent};
use crate::tools::params::ExtractParams;
use crate::tools::parsers::{content_hash, normalize_optional_string, parse_datetime};
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Extract entities, facts, and relationships from remembered content.
///
/// Handles extracting from `episode_id` or ingesting inline content first.
pub async fn extract<T: ToolContext>(
    ctx: &T,
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

    ctx.record(ToolEvent {
        op: "extract.start",
        args: json!({"episode_id": &episode_id, "has_content": content.is_some() || text.is_some()}),
        result: json!({}),
        level: LogLevel::Info,
        request_id: Some(request_id.clone()),
        duration: None,
    });

    if content.is_some() && text.is_some() {
        let message = "Invalid extract arguments: use only one inline snake_case field — `content` or `text` — not both. Do not wrap arguments in `payload`.";
        ctx.record(ToolEvent {
            op: "extract.invalid_input",
            args: json!({"episode_id": &episode_id, "has_content": true}),
            result: json!({"error": message}),
            level: LogLevel::Warn,
            request_id: Some(request_id.clone()),
            duration: Some(timer.elapsed()),
        });
        return Err(MemoryError::Validation(message.to_string()));
    }

    let inline_content = content.or(text);

    if episode_id.is_some() && inline_content.is_some() {
        let message = "Invalid extract arguments: provide exactly one snake_case input source. Use `episode_id` for stored content, or `content`/`text` for inline text, but not both. Do not wrap arguments in `payload`.";
        ctx.record(ToolEvent {
            op: "extract.invalid_input",
            args: json!({"episode_id": &episode_id, "has_content": true}),
            result: json!({"error": message}),
            level: LogLevel::Warn,
            request_id: Some(request_id.clone()),
            duration: Some(timer.elapsed()),
        });
        return Err(MemoryError::Validation(message.to_string()));
    }

    if episode_id.is_none() && inline_content.is_none() {
        let message = "Invalid extract arguments: provide exactly one snake_case input source — `episode_id` or non-empty `content`/`text`. Do not wrap arguments in `payload`.";
        ctx.record(ToolEvent {
            op: "extract.invalid_input",
            args: json!({"episode_id": &episode_id, "has_content": false}),
            result: json!({"error": message}),
            level: LogLevel::Warn,
            request_id: Some(request_id.clone()),
            duration: Some(timer.elapsed()),
        });
        return Err(MemoryError::Validation(message.to_string()));
    }

    if let Some(ref episode_id) = episode_id {
        match ctx
            .extract(episode_id, Some(access), zero_shot_labels.as_deref())
            .await
        {
            Ok(result) => {
                record_extract_results(&operation_metrics, &result);
                operation_metrics.success();
                let log_result = match ctx.find_episode(episode_id).await {
                    Ok(episode) => build_extract_log_result(
                        episode.as_ref(),
                        result.entities.len(),
                        &result.facts,
                        result.links.len(),
                        result.warnings.len(),
                    ),
                    Err(_) => build_extract_log_result(
                        None,
                        result.entities.len(),
                        &result.facts,
                        result.links.len(),
                        result.warnings.len(),
                    ),
                };

                ctx.record(ToolEvent {
                    op: "extract.done",
                    args: json!({"episode_id": episode_id}),
                    result: log_result,
                    level: LogLevel::Info,
                    request_id: Some(request_id.clone()),
                    duration: Some(timer.elapsed()),
                });
                let guidance = extract_guidance(&result);
                return Ok(ToolResponse::success_with_guidance(result, guidance));
            }
            Err(err) => {
                ctx.record(ToolEvent {
                    op: "extract.error",
                    args: json!({"episode_id": episode_id}),
                    result: json!({"error": err.to_string()}),
                    level: LogLevel::Warn,
                    request_id: Some(request_id.clone()),
                    duration: Some(timer.elapsed()),
                });
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
    match ctx
        .ingest(
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
            // The extraction itself is the whole cost of this tool, so it is
            // the only stage worth naming: an `extract` that got slower can be
            // traced to this histogram rather than to the operation total,
            // which would move for any reason at all.
            let extraction = {
                let _stage = crate::shared::observability::StageTimer::new("extract", "extraction");
                ctx.extract(&episode_id, Some(access), zero_shot_labels.as_deref())
                    .await
            };
            match extraction {
                Ok(result) => {
                    record_extract_results(&operation_metrics, &result);
                    operation_metrics.success();
                    let log_result = match ctx.find_episode(&episode_id).await {
                        Ok(episode) => build_extract_log_result(
                            episode.as_ref(),
                            result.entities.len(),
                            &result.facts,
                            result.links.len(),
                            result.warnings.len(),
                        ),
                        Err(_) => build_extract_log_result(
                            None,
                            result.entities.len(),
                            &result.facts,
                            result.links.len(),
                            result.warnings.len(),
                        ),
                    };

                    ctx.record(ToolEvent {
                        op: "extract.done",
                        args: json!({"episode_id": &episode_id}),
                        result: log_result,
                        level: LogLevel::Info,
                        request_id: Some(request_id.clone()),
                        duration: Some(timer.elapsed()),
                    });
                    let guidance = extract_guidance(&result);
                    Ok(ToolResponse::success_with_guidance(result, guidance))
                }
                Err(err) => {
                    ctx.record(ToolEvent {
                        op: "extract.error",
                        args: json!({}),
                        result: json!({"error": err.to_string()}),
                        level: LogLevel::Warn,
                        request_id: Some(request_id.clone()),
                        duration: Some(timer.elapsed()),
                    });
                    Err(err)
                }
            }
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "extract.error",
                args: json!({}),
                result: json!({"error": err.to_string()}),
                level: LogLevel::Warn,
                request_id: Some(request_id),
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}

/// Next-step guidance for an `extract` result.
///
/// A successful extraction that produced no durable facts leaves the source as
/// `episode-only`. Staying silent there let a capture that followed the
/// documented SOP look complete while storing nothing recallable, so the empty
/// case names the outcome and the structured-capture path that yields facts.
fn extract_guidance(result: &ExtractResult) -> &'static str {
    if result.facts.is_empty() {
        "episode-only: the content was stored but no durable facts were extracted. To capture recallable facts, ingest a structured summary — markdown headings such as `## Decisions`, `## Facts` or `## Pending items` with bullet items, or `Decision: …` / `Fact: …` lines. Alternatively open the `ingestion_review` app and approve a draft note."
    } else {
        "Resolve canonical entities for any ambiguous names before creating manual links."
    }
}

fn record_extract_results(
    operation_metrics: &crate::observability::OperationMetrics,
    result: &ExtractResult,
) {
    operation_metrics.record_result("entities", result.entities.len());
    operation_metrics.record_result("facts", result.facts.len());
    operation_metrics.record_result("links", result.links.len());
    operation_metrics.record_result("warnings", result.warnings.len());
}
