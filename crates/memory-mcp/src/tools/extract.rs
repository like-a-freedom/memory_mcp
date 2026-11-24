//! `extract` tool — protocol-agnostic.

use std::time::Instant;

use chrono::Utc;
use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{
    AccessPayload, ExtractResult, IngestRequest, MAX_EXTRACT_FACT_EVIDENCE_WIRE_BYTES,
};
use crate::service::build_extract_log_result;
use crate::tools::context::{ToolContext, ToolEvent};
use crate::tools::params::ExtractParams;
use crate::tools::parsers::{content_hash, normalize_optional_string, parse_datetime};
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

const FACT_EVIDENCE_RESPONSE_OVERHEAD_BYTES: usize = 40;
const FACT_EVIDENCE_ITEM_OVERHEAD_BYTES: usize = 32;

/// Extract entities, facts, and relationships from remembered content.
///
/// Handles extracting from `episode_id` or ingesting inline content first.
pub async fn extract<T: ToolContext>(
    ctx: &T,
    params: ExtractParams,
) -> Result<ToolResponse<ExtractResult>, MemoryError> {
    let id = crate::logging::correlation::current().unwrap_or_else(next_request_id);
    crate::logging::correlation::scope(id, extract_inner(ctx, params)).await
}

async fn extract_inner<T: ToolContext>(
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
    let include_fact_evidence = params.include_fact_evidence;
    let timer = Instant::now();

    ctx.record(ToolEvent {
        op: "extract.start",
        args: json!({
            "has_episode_id": episode_id.is_some(),
            "has_content": content.is_some() || text.is_some()
        }),
        result: json!({}),
        level: LogLevel::Info,
        duration: None,
    });

    if let Some(err) = validate_extract_inputs(&episode_id, &content, &text) {
        ctx.record(ToolEvent {
            op: "extract.invalid_input",
            args: json!({
                "has_episode_id": episode_id.is_some(),
                "has_content": err.has_content
            }),
            result: json!({"error": err.message}),
            level: LogLevel::Warn,
            duration: Some(timer.elapsed()),
        });
        return Err(MemoryError::Validation(err.message.to_string()));
    }

    let inline_content = content.or(text);

    if let Some(ref episode_id) = episode_id {
        match ctx
            .extract(episode_id, Some(access), zero_shot_labels.as_deref())
            .await
        {
            Ok(result) => {
                return record_extract_done(
                    ctx,
                    &mut operation_metrics,
                    timer,
                    episode_id,
                    result,
                    include_fact_evidence,
                )
                .await;
            }
            Err(err) => {
                ctx.record(ToolEvent {
                    op: "extract.error",
                    args: json!({}),
                    result: json!({"error_code": err.code()}),
                    level: err.log_level(),
                    duration: Some(timer.elapsed()),
                });
                if !is_retryable_extraction_failure(&err) {
                    return Err(err);
                }
                return Ok(extract_pending_response(episode_id));
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
                source_context: None,
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
                    record_extract_done(
                        ctx,
                        &mut operation_metrics,
                        timer,
                        &episode_id,
                        result,
                        include_fact_evidence,
                    )
                    .await
                }
                Err(err) => {
                    ctx.record(ToolEvent {
                        op: "extract.error",
                        args: json!({}),
                        result: json!({"error_code": err.code()}),
                        level: err.log_level(),
                        duration: Some(timer.elapsed()),
                    });
                    Ok(extract_pending_response(&episode_id))
                }
            }
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "extract.error",
                args: json!({}),
                result: json!({"error_code": err.code()}),
                level: err.log_level(),
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}

fn extract_pending_response(episode_id: &str) -> ToolResponse<ExtractResult> {
    ToolResponse::partial_with_guidance(
        ExtractResult {
            episode_id: episode_id.to_owned(),
            ..ExtractResult::empty()
        },
        "The source episode is stored, but extraction is pending. Retry extract with this episode_id; the episode write is idempotent, so retrying will not create a duplicate episode.",
    )
}

fn is_retryable_extraction_failure(error: &MemoryError) -> bool {
    matches!(
        error,
        MemoryError::Storage(_)
            | MemoryError::Transient(_)
            | MemoryError::Unavailable(_)
            | MemoryError::ModelNotReady(_)
    )
}

/// Refusal of an invalid `extract` argument combination: the exact message
/// and the `has_content` flag the `extract.invalid_input` event carries.
struct ExtractInputError {
    message: &'static str,
    has_content: bool,
}

/// The three invalid-input shapes, checked in a fixed order: dual-field
/// (content+text) wins over dual-source (episode_id+inline) wins over
/// neither-source. `None` means the combination is valid.
fn validate_extract_inputs(
    episode_id: &Option<String>,
    content: &Option<String>,
    text: &Option<String>,
) -> Option<ExtractInputError> {
    if content.is_some() && text.is_some() {
        return Some(ExtractInputError {
            message: "Invalid extract arguments: use only one inline snake_case field — `content` or `text` — not both. Do not wrap arguments in `payload`.",
            has_content: true,
        });
    }
    let inline_content = content.is_some() || text.is_some();
    if episode_id.is_some() && inline_content {
        return Some(ExtractInputError {
            message: "Invalid extract arguments: provide exactly one snake_case input source. Use `episode_id` for stored content, or `content`/`text` for inline text, but not both. Do not wrap arguments in `payload`.",
            has_content: true,
        });
    }
    if episode_id.is_none() && !inline_content {
        return Some(ExtractInputError {
            message: "Invalid extract arguments: provide exactly one snake_case input source — `episode_id` or non-empty `content`/`text`. Do not wrap arguments in `payload`.",
            has_content: false,
        });
    }
    None
}

/// Shared success tail of both extraction paths: record result metrics,
/// emit `extract.done` with the episode-shaped log result, and wrap the
/// outcome with path-appropriate guidance.
async fn record_extract_done<T: ToolContext>(
    ctx: &T,
    operation_metrics: &mut crate::observability::OperationMetrics,
    timer: Instant,
    episode_id: &str,
    mut result: ExtractResult,
    include_fact_evidence: bool,
) -> Result<ToolResponse<ExtractResult>, MemoryError> {
    record_extract_results(operation_metrics, &result);
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
        args: json!({}),
        result: log_result,
        level: LogLevel::Info,
        duration: Some(timer.elapsed()),
    });
    apply_fact_evidence_response_policy(&mut result, include_fact_evidence);
    let guidance = extract_guidance(&result);
    Ok(ToolResponse::success_with_guidance(result, guidance))
}

/// Retain complete statement/quote pairs only when the complete evidence
/// addition fits the wire-size budget. Evidence is all-or-none because a
/// partial set could make a caller believe a particular correction was
/// verified when its fact was silently skipped.
fn apply_fact_evidence_response_policy(result: &mut ExtractResult, include: bool) {
    if !include {
        clear_fact_evidence(result);
        result.fact_evidence_omitted = None;
        return;
    }

    let evidence_is_complete = result.facts.iter().all(|fact| {
        fact.statement
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            && fact
                .source_quote
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    });
    let evidence_bytes =
        result
            .facts
            .iter()
            .fold(FACT_EVIDENCE_RESPONSE_OVERHEAD_BYTES, |total, fact| {
                total
                    .saturating_add(FACT_EVIDENCE_ITEM_OVERHEAD_BYTES)
                    .saturating_add(
                        fact.statement
                            .as_deref()
                            .map_or(0, json_encoded_string_bytes),
                    )
                    .saturating_add(
                        fact.source_quote
                            .as_deref()
                            .map_or(0, json_encoded_string_bytes),
                    )
            });

    if evidence_is_complete && evidence_bytes <= MAX_EXTRACT_FACT_EVIDENCE_WIRE_BYTES {
        result.fact_evidence_omitted = Some(false);
    } else {
        clear_fact_evidence(result);
        result.fact_evidence_omitted = Some(!result.facts.is_empty());
    }
}

fn clear_fact_evidence(result: &mut ExtractResult) {
    for fact in &mut result.facts {
        fact.statement = None;
        fact.source_quote = None;
    }
}

/// Exact UTF-8 byte length of a JSON string value, including its quotes.
/// Control characters are escaped before they reach the MCP wire response.
fn json_encoded_string_bytes(value: &str) -> usize {
    value.chars().fold(2usize, |bytes, character| {
        bytes.saturating_add(match character {
            '"' | '\\' | '\u{08}' | '\u{0C}' | '\n' | '\r' | '\t' => 2,
            control if control <= '\u{1F}' => 6,
            plain => plain.len_utf8(),
        })
    })
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
