//! `assemble_context` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AssembleContextRequest, ContextAssemblyResult, TemporalContextQuery};
use crate::tools::context::{ToolContext, ToolEvent};
use crate::tools::params::AssembleContextParams;
use crate::tools::parsers::parse_datetime;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

fn context_guidance(has_items: bool) -> &'static str {
    if has_items {
        "Call explain if you need provenance-ready citations for selected items."
    } else {
        "No sufficient matching context was found. Refine the query or verify primary sources."
    }
}

fn parse_temporal_query(
    params: &AssembleContextParams,
) -> Result<Option<TemporalContextQuery>, MemoryError> {
    let as_of_text = (!params.as_of.trim().is_empty()).then_some(params.as_of.as_str());
    let has_new_axis =
        params.valid_at.is_some() || params.known_at.is_some() || params.known_revision.is_some();
    if as_of_text.is_some() && has_new_axis {
        return Err(MemoryError::Validation(
            "as_of cannot be combined with valid_at, known_at, or known_revision".into(),
        ));
    }
    if params.known_at.is_some() && params.known_revision.is_some() {
        return Err(MemoryError::Validation(
            "known_at and known_revision are mutually exclusive".into(),
        ));
    }
    if params.known_revision.is_some_and(|revision| revision < 0) {
        return Err(MemoryError::Validation(
            "known_revision must be zero or greater".into(),
        ));
    }

    let parse = |name: &str,
                 value: Option<&str>|
     -> Result<Option<chrono::DateTime<chrono::Utc>>, MemoryError> {
        value
            .map(|value| {
                chrono::DateTime::parse_from_rfc3339(value)
                    .map(|date| date.with_timezone(&chrono::Utc))
                    .map_err(|_| {
                        MemoryError::Validation(format!(
                            "{name} must be a valid RFC 3339 timestamp"
                        ))
                    })
            })
            .transpose()
    };
    let as_of = parse("as_of", as_of_text)?;
    let temporal = TemporalContextQuery {
        valid_at: parse("valid_at", params.valid_at.as_deref())?.or(as_of),
        known_at: parse("known_at", params.known_at.as_deref())?.or(as_of),
        known_revision: params.known_revision,
    };
    Ok(temporal.is_explicit().then_some(temporal))
}

/// Assemble the most relevant active memory context for a query.
///
/// The result field is serialized to [`serde_json::Value`] inside this
/// function while the compact-mode guard is alive, and the outer
/// `ToolResponse<Value>` envelope is returned. Serialize happens on the same
/// thread and task as the guard; the guard is dropped before this returns.
pub async fn assemble_context<T: ToolContext>(
    ctx: &T,
    params: AssembleContextParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let id = crate::logging::correlation::current().unwrap_or_else(next_request_id);
    crate::logging::correlation::scope(id, assemble_context_inner(ctx, params)).await
}

async fn assemble_context_inner<T: ToolContext>(
    ctx: &T,
    params: AssembleContextParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("assemble_context");
    let compact = params.compact;
    let temporal = parse_temporal_query(&params)?;
    let window_start = params.window_start.as_deref().and_then(parse_datetime);
    let window_end = params.window_end.as_deref().and_then(parse_datetime);
    let request = AssembleContextRequest {
        query: params.query,
        fact_types: params.fact_types,
        as_of: None,
        budget: params.budget,
        view_mode: params.view_mode,
        window_start,
        window_end,
        access: None,
        compact,
    };

    let timer = Instant::now();
    ctx.record(ToolEvent {
        op: "assemble_context.start",
        args: json!({"query_chars": request.query.chars().count()}),
        result: json!({}),
        level: LogLevel::Info,
        duration: None,
    });

    let assembly = match temporal {
        Some(temporal) => ctx.assemble_context_temporal(request, temporal).await,
        None => ctx
            .assemble_context(request)
            .await
            .map(ContextAssemblyResult::current),
    };
    match assembly {
        Ok(assembly) => {
            let results = assembly.items;
            ctx.record(ToolEvent {
                op: "assemble_context.done",
                args: json!({}),
                result: json!({"count": results.len()}),
                level: LogLevel::Info,
                duration: Some(timer.elapsed()),
            });
            let count = results.len();
            let guidance = if let Some(metadata) = assembly.metadata.as_ref() {
                let is_partial =
                    metadata.completeness == crate::models::ContextCompleteness::Partial;
                if is_partial && count == 0 && metadata.candidate_scan_truncated {
                    "The historical candidate scan was truncated and returned no items. You may retry once with a narrower query. Preserve the exact snapshot by setting `valid_at` and `known_revision` from the returned `context.snapshot`, leaving all other selectors unchanged and omitting `as_of` and `known_at`. An empty retry still does not establish absence; verify against the source."
                } else if is_partial && count == 0 {
                    "No matching items were returned from this partial historical snapshot. You may retry once with a more specific query. Preserve the exact snapshot by setting `valid_at` and `known_revision` from the returned `context.snapshot`, leaving all other selectors unchanged and omitting `as_of` and `known_at`. A second empty result still does not establish absence because some selector channels were omitted; verify against the source."
                } else if !is_partial {
                    context_guidance(count > 0)
                } else if metadata
                    .omitted_channels
                    .iter()
                    .any(|channel| channel.starts_with("claim relations"))
                {
                    "Historical context is partial; use the returned snapshot and verify omitted selector channels before relying on an empty or reduced result."
                } else {
                    "Historical context is partial; use the returned snapshot and typed relation roles, then verify omitted selector channels before relying on an empty or reduced result."
                }
            } else {
                context_guidance(count > 0)
            };
            operation_metrics.record_result("items", count);
            // Under compact mode, omit `quote` and slim `rationale` via the
            // serde adapters reading the thread-local CompactGuard. The guard
            // is held across serialization; `value` contains the compact form.
            let value = {
                let _guard = crate::tools::compact::set_compact(compact);
                serde_json::to_value(&results)
                    .map_err(|e| MemoryError::Transient(format!("serialize context items: {e}")))?
            };
            operation_metrics.success();
            if compact {
                Ok(ToolResponse::complete_context_list_compact(
                    value,
                    count,
                    guidance,
                    assembly.metadata,
                ))
            } else {
                Ok(ToolResponse::complete_context_list(
                    value,
                    count,
                    guidance,
                    assembly.metadata,
                ))
            }
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "assemble_context.error",
                args: json!({}),
                result: json!({"error_code": err.code()}),
                level: err.log_level(),
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> AssembleContextParams {
        AssembleContextParams {
            query: "q".into(),
            fact_types: Vec::new(),
            as_of: String::new(),
            valid_at: None,
            known_at: None,
            known_revision: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            compact: true,
        }
    }

    fn timestamp(value: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(value)
            .expect("test timestamp")
            .with_timezone(&chrono::Utc)
    }

    struct PartialHistoricalContext {
        candidate_scan_truncated: bool,
    }

    impl ToolContext for PartialHistoricalContext {
        fn record(&self, _event: ToolEvent) {}

        async fn ingest(
            &self,
            _request: crate::models::IngestRequest,
            _access: Option<crate::models::AccessPayload>,
        ) -> Result<String, MemoryError> {
            panic!("assemble_context must not reach ingest")
        }

        async fn extract(
            &self,
            _episode_id: &str,
            _access: Option<crate::models::AccessPayload>,
            _zero_shot_labels: Option<&[String]>,
        ) -> Result<crate::models::ExtractResult, MemoryError> {
            panic!("assemble_context must not reach extract")
        }

        async fn resolve(
            &self,
            _candidate: crate::models::EntityCandidate,
            _access: Option<crate::models::AccessPayload>,
        ) -> Result<String, MemoryError> {
            panic!("assemble_context must not reach resolve")
        }

        async fn explain(
            &self,
            _request: crate::models::ExplainRequest,
            _access: Option<crate::models::AccessPayload>,
        ) -> Result<Vec<crate::models::ExplainItem>, MemoryError> {
            panic!("assemble_context must not reach explain")
        }

        async fn invalidate(
            &self,
            _request: crate::models::InvalidateRequest,
            _access: Option<crate::models::AccessPayload>,
        ) -> Result<(), MemoryError> {
            panic!("assemble_context must not reach invalidate")
        }

        async fn assemble_context(
            &self,
            _request: AssembleContextRequest,
        ) -> Result<Vec<crate::models::AssembledContextItem>, MemoryError> {
            panic!("historical parameters must use assemble_context_temporal")
        }

        async fn assemble_context_temporal(
            &self,
            _request: AssembleContextRequest,
            _temporal: TemporalContextQuery,
        ) -> Result<ContextAssemblyResult, MemoryError> {
            Ok(ContextAssemblyResult {
                items: Vec::new(),
                metadata: Some(crate::models::ContextAssemblyMetadata {
                    snapshot: crate::models::ContextSnapshot {
                        valid_at: timestamp("2026-11-21T00:00:00Z"),
                        known_at: timestamp("2026-11-25T00:00:00Z"),
                        known_revision: 0,
                    },
                    completeness: crate::models::ContextCompleteness::Partial,
                    candidate_scan_truncated: self.candidate_scan_truncated,
                    omitted_channels: vec!["historical fact scan".into()],
                }),
            })
        }

        async fn find_episode(
            &self,
            _episode_id: &str,
        ) -> Result<Option<crate::models::Episode>, MemoryError> {
            panic!("assemble_context must not reach find_episode")
        }
    }

    #[tokio::test]
    async fn truncated_empty_partial_historical_read_authorizes_only_bounded_retry() {
        let mut params = params();
        params.valid_at = Some("2026-11-21T00:00:00Z".into());
        params.known_revision = Some(0);

        let response = assemble_context(
            &PartialHistoricalContext {
                candidate_scan_truncated: true,
            },
            params,
        )
        .await
        .expect("return the bounded historical result");

        assert!(response.result.as_array().is_some_and(Vec::is_empty));
        assert_eq!(
            response.guidance.as_deref(),
            Some(
                "The historical candidate scan was truncated and returned no items. You may retry once with a narrower query. Preserve the exact snapshot by setting `valid_at` and `known_revision` from the returned `context.snapshot`, leaving all other selectors unchanged and omitting `as_of` and `known_at`. An empty retry still does not establish absence; verify against the source."
            )
        );
    }

    #[tokio::test]
    async fn empty_partial_historical_read_authorizes_one_query_refinement() {
        let mut params = params();
        params.valid_at = Some("2026-11-21T00:00:00Z".into());
        params.known_revision = Some(0);

        let response = assemble_context(
            &PartialHistoricalContext {
                candidate_scan_truncated: false,
            },
            params,
        )
        .await
        .expect("return the bounded historical result");

        assert!(response.result.as_array().is_some_and(Vec::is_empty));
        assert_eq!(
            response.guidance.as_deref(),
            Some(
                "No matching items were returned from this partial historical snapshot. You may retry once with a more specific query. Preserve the exact snapshot by setting `valid_at` and `known_revision` from the returned `context.snapshot`, leaving all other selectors unchanged and omitting `as_of` and `known_at`. A second empty result still does not establish absence because some selector channels were omitted; verify against the source."
            )
        );
    }

    #[test]
    fn temporal_query_defaults_to_current_retrieval() {
        assert_eq!(parse_temporal_query(&params()).unwrap(), None);
    }

    #[test]
    fn as_of_maps_to_both_temporal_axes() {
        let mut params = params();
        params.as_of = "2026-11-01T00:00:00Z".into();

        let temporal = parse_temporal_query(&params).unwrap().unwrap();

        assert_eq!(temporal.valid_at, Some(timestamp("2026-11-01T00:00:00Z")));
        assert_eq!(temporal.known_at, Some(timestamp("2026-11-01T00:00:00Z")));
        assert_eq!(temporal.known_revision, None);
    }

    #[test]
    fn valid_at_only_leaves_knowledge_time_at_request_default() {
        let mut params = params();
        params.valid_at = Some("2026-11-01T00:00:00Z".into());

        let temporal = parse_temporal_query(&params).unwrap().unwrap();

        assert_eq!(temporal.valid_at, Some(timestamp("2026-11-01T00:00:00Z")));
        assert_eq!(temporal.known_at, None);
        assert_eq!(temporal.known_revision, None);
    }

    #[test]
    fn known_at_only_leaves_valid_time_at_request_default() {
        let mut params = params();
        params.known_at = Some("2026-11-15T00:00:00Z".into());

        let temporal = parse_temporal_query(&params).unwrap().unwrap();

        assert_eq!(temporal.valid_at, None);
        assert_eq!(temporal.known_at, Some(timestamp("2026-11-15T00:00:00Z")));
        assert_eq!(temporal.known_revision, None);
    }

    #[test]
    fn temporal_query_rejects_malformed_timestamp() {
        let mut params = params();
        params.valid_at = Some("not a timestamp".into());

        assert!(matches!(
            parse_temporal_query(&params),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn temporal_query_rejects_ambiguous_as_of_and_new_axes() {
        let mut params = params();
        params.as_of = "2026-11-01T00:00:00Z".into();
        params.valid_at = Some("2026-11-01T00:00:00Z".into());

        assert!(matches!(
            parse_temporal_query(&params),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn temporal_query_accepts_each_independent_axis_and_exact_revision() {
        let mut params = params();
        params.valid_at = Some("2026-11-01T00:00:00Z".into());
        params.known_at = Some("2026-11-15T00:00:00Z".into());
        let both = parse_temporal_query(&params).unwrap().unwrap();
        assert_eq!(both.valid_at, Some(timestamp("2026-11-01T00:00:00Z")));
        assert_eq!(both.known_at, Some(timestamp("2026-11-15T00:00:00Z")));

        params.known_at = None;
        params.known_revision = Some(12);
        let exact = parse_temporal_query(&params).unwrap().unwrap();
        assert_eq!(exact.known_revision, Some(12));
    }

    #[test]
    fn temporal_query_rejects_timestamp_and_exact_revision_for_same_axis() {
        let mut params = params();
        params.known_at = Some("2026-11-15T00:00:00Z".into());
        params.known_revision = Some(12);

        assert!(matches!(
            parse_temporal_query(&params),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn temporal_query_rejects_negative_exact_revision() {
        let mut params = params();
        params.known_revision = Some(-1);

        assert!(matches!(
            parse_temporal_query(&params),
            Err(MemoryError::Validation(_))
        ));
    }
}
