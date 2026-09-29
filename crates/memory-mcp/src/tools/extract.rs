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
                return Ok(ToolResponse::success_with_guidance(
                    result,
                    "Resolve canonical entities for any ambiguous names before creating manual links.",
                ));
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
            match ctx
                .extract(&episode_id, Some(access), zero_shot_labels.as_deref())
                .await
            {
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
                    Ok(ToolResponse::success_with_guidance(
                        result,
                        "Resolve canonical entities for any ambiguous names before creating manual links.",
                    ))
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

fn record_extract_results(
    metrics: &crate::observability::OperationMetrics,
    result: &ExtractResult,
) {
    metrics.record_result("entities", result.entities.len());
    metrics.record_result("facts", result.facts.len());
    metrics.record_result("links", result.links.len());
    metrics.record_result("warnings", result.warnings.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::models::{
        AssembleContextRequest, AssembledContextItem, EntityCandidate, Episode, ExplainItem,
        ExplainRequest, InvalidateRequest,
    };

    /// A `ToolContext` that scripts the three calls `extract` makes — `ingest`,
    /// `extract` and `find_episode` — and records the events the tool emits.
    /// The remaining capabilities are unreachable here and panic if called.
    struct StubContext {
        ingest_result: Result<String, MemoryError>,
        extract_result: Result<ExtractResult, MemoryError>,
        find_episode_result: Result<Option<Episode>, MemoryError>,
        events: Mutex<Vec<&'static str>>,
    }

    /// An extraction result with no entities, facts, links or warnings.
    fn empty_result(episode_id: &str) -> ExtractResult {
        let mut result = ExtractResult::empty();
        result.episode_id = episode_id.to_string();
        result
    }

    impl StubContext {
        /// A context whose extract succeeds and whose episode lookup misses.
        fn extracting(episode_id: &str) -> Self {
            Self {
                ingest_result: Ok(episode_id.to_string()),
                extract_result: Ok(empty_result(episode_id)),
                find_episode_result: Ok(None),
                events: Mutex::new(Vec::new()),
            }
        }

        /// A context whose `ingest` fails, for the inline-content path.
        fn failing_ingest(error: MemoryError) -> Self {
            Self {
                ingest_result: Err(error),
                extract_result: Ok(empty_result("episode:new")),
                find_episode_result: Ok(None),
                events: Mutex::new(Vec::new()),
            }
        }

        /// A context whose `extract` capability fails.
        fn failing_extract(error: MemoryError) -> Self {
            Self {
                ingest_result: Ok("episode:abc".to_string()),
                extract_result: Err(error),
                find_episode_result: Ok(None),
                events: Mutex::new(Vec::new()),
            }
        }

        /// A context whose `extract` succeeds but whose episode lookup errors,
        /// which must degrade the log rather than fail the call.
        fn failing_episode_lookup(error: MemoryError) -> Self {
            Self {
                ingest_result: Ok("episode:abc".to_string()),
                extract_result: Ok(empty_result("episode:abc")),
                find_episode_result: Err(error),
                events: Mutex::new(Vec::new()),
            }
        }

        fn recorded_ops(&self) -> Vec<&'static str> {
            self.events.lock().expect("event log").clone()
        }
    }

    impl ToolContext for StubContext {
        fn record(&self, event: ToolEvent) {
            self.events.lock().expect("event log").push(event.op);
        }

        async fn ingest(
            &self,
            _request: IngestRequest,
            _access: Option<AccessPayload>,
        ) -> Result<String, MemoryError> {
            self.ingest_result.clone()
        }

        async fn extract(
            &self,
            _episode_id: &str,
            _access: Option<AccessPayload>,
            _zero_shot_labels: Option<&[String]>,
        ) -> Result<ExtractResult, MemoryError> {
            self.extract_result.clone()
        }

        async fn resolve(
            &self,
            _candidate: EntityCandidate,
            _access: Option<AccessPayload>,
        ) -> Result<String, MemoryError> {
            panic!("extract must not reach resolve")
        }

        async fn explain(
            &self,
            _request: ExplainRequest,
            _access: Option<AccessPayload>,
        ) -> Result<Vec<ExplainItem>, MemoryError> {
            panic!("extract must not reach explain")
        }

        async fn invalidate(
            &self,
            _request: InvalidateRequest,
            _access: Option<AccessPayload>,
        ) -> Result<(), MemoryError> {
            panic!("extract must not reach invalidate")
        }

        async fn assemble_context(
            &self,
            _request: AssembleContextRequest,
        ) -> Result<Vec<AssembledContextItem>, MemoryError> {
            panic!("extract must not reach assemble_context")
        }

        async fn find_episode(&self, _episode_id: &str) -> Result<Option<Episode>, MemoryError> {
            self.find_episode_result.clone()
        }
    }

    /// Params with no input source at all.
    fn empty_params() -> ExtractParams {
        ExtractParams {
            episode_id: None,
            content: None,
            text: None,
            source_type: None,
            source_id: None,
            t_ref: None,
            zero_shot_labels: None,
        }
    }

    /// Run an async `extract` call on a current-thread runtime.
    fn run(
        ctx: &StubContext,
        params: ExtractParams,
    ) -> Result<ToolResponse<ExtractResult>, MemoryError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(extract(ctx, params))
    }

    #[test]
    fn rejects_content_and_text_together() {
        let ctx = StubContext::extracting("episode:abc");
        let params = ExtractParams {
            content: Some("alpha".to_string()),
            text: Some("beta".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params);

        assert!(
            matches!(observed, Err(MemoryError::Validation(_))),
            "only one inline field may be supplied"
        );
    }

    #[test]
    fn rejects_an_episode_id_and_inline_content_together() {
        let ctx = StubContext::extracting("episode:abc");
        let params = ExtractParams {
            episode_id: Some("episode:abc".to_string()),
            content: Some("alpha".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params);

        assert!(
            matches!(observed, Err(MemoryError::Validation(_))),
            "exactly one input source may be supplied"
        );
    }

    #[test]
    fn rejects_a_call_with_no_input_source() {
        let ctx = StubContext::extracting("episode:abc");

        let observed = run(&ctx, empty_params());

        assert!(
            matches!(observed, Err(MemoryError::Validation(_))),
            "extract needs an episode or inline content"
        );
    }

    #[test]
    fn treats_a_blank_episode_id_as_absent() {
        let ctx = StubContext::extracting("episode:abc");
        let params = ExtractParams {
            episode_id: Some("   ".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params);

        assert!(
            matches!(observed, Err(MemoryError::Validation(_))),
            "whitespace is not a usable episode id"
        );
    }

    #[test]
    fn records_an_invalid_input_event_for_a_rejected_call() {
        let ctx = StubContext::extracting("episode:abc");

        let _ = run(&ctx, empty_params());

        assert!(ctx.recorded_ops().contains(&"extract.invalid_input"));
    }

    #[test]
    fn returns_the_capability_result_for_a_stored_episode() {
        let ctx = StubContext::extracting("episode:abc");
        let params = ExtractParams {
            episode_id: Some("episode:abc".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params).expect("extract succeeds");

        assert_eq!(observed.result.episode_id, "episode:abc");
    }

    #[test]
    fn never_ingests_when_an_episode_id_is_given() {
        // A failing `ingest` makes an unwanted inline ingest observable: the
        // tool would propagate that error instead of returning the extraction.
        let ctx = StubContext::failing_ingest(MemoryError::Storage("ingest must not run".into()));
        let params = ExtractParams {
            episode_id: Some("episode:abc".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params);

        assert!(
            observed.is_ok(),
            "a stored episode must not trigger an ingest"
        );
    }

    #[test]
    fn ingests_then_extracts_for_inline_content() {
        let ctx = StubContext::extracting("episode:new");
        let params = ExtractParams {
            content: Some("alpha".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params).expect("extract succeeds");

        assert_eq!(observed.result.episode_id, "episode:new");
    }

    #[test]
    fn accepts_text_as_the_inline_field() {
        let ctx = StubContext::extracting("episode:new");
        let params = ExtractParams {
            text: Some("alpha".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params);

        assert!(
            observed.is_ok(),
            "`text` is the documented alias of `content`"
        );
    }

    #[test]
    fn propagates_an_ingest_failure() {
        let ctx = StubContext::failing_ingest(MemoryError::Storage("disk full".into()));
        let params = ExtractParams {
            content: Some("alpha".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params);

        assert!(observed.is_err(), "a failed inline ingest must surface");
    }

    #[test]
    fn records_an_error_event_when_ingest_fails() {
        let ctx = StubContext::failing_ingest(MemoryError::Storage("disk full".into()));
        let params = ExtractParams {
            content: Some("alpha".to_string()),
            ..empty_params()
        };

        let _ = run(&ctx, params);

        assert!(ctx.recorded_ops().contains(&"extract.error"));
    }

    #[test]
    fn propagates_an_extract_failure() {
        let ctx = StubContext::failing_extract(MemoryError::Storage("index offline".into()));
        let params = ExtractParams {
            episode_id: Some("episode:abc".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params);

        assert!(observed.is_err(), "a failed extraction must surface");
    }

    #[test]
    fn records_an_error_event_when_extraction_fails() {
        let ctx = StubContext::failing_extract(MemoryError::Storage("index offline".into()));
        let params = ExtractParams {
            episode_id: Some("episode:abc".to_string()),
            ..empty_params()
        };

        let _ = run(&ctx, params);

        assert!(ctx.recorded_ops().contains(&"extract.error"));
    }

    #[test]
    fn succeeds_even_when_the_episode_lookup_fails() {
        // The log is best-effort; a lookup failure must not fail an
        // extraction that already succeeded.
        let ctx = StubContext::failing_episode_lookup(MemoryError::Storage("lookup down".into()));
        let params = ExtractParams {
            episode_id: Some("episode:abc".to_string()),
            ..empty_params()
        };

        let observed = run(&ctx, params);

        assert!(observed.is_ok());
    }

    #[test]
    fn records_a_done_event_on_success() {
        let ctx = StubContext::extracting("episode:abc");
        let params = ExtractParams {
            episode_id: Some("episode:abc".to_string()),
            ..empty_params()
        };

        run(&ctx, params).expect("extract succeeds");

        assert_eq!(ctx.recorded_ops().last(), Some(&"extract.done"));
    }

    #[test]
    fn records_a_start_event_before_validating_arguments() {
        let ctx = StubContext::extracting("episode:abc");

        let _ = run(&ctx, empty_params());

        assert_eq!(ctx.recorded_ops().first(), Some(&"extract.start"));
    }
}
