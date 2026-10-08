//! `explain` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessPayload, ExplainRequest};
use crate::tools::context::{ToolContext, ToolEvent};
use crate::tools::params::ExplainParams;
use crate::tools::parsers::parse_context_items;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Explain context items with provenance-ready citations.
///
/// The result field is serialized to [`serde_json::Value`] inside this
/// function while the compact-mode guard is alive, and the outer
/// `ToolResponse<Value>` envelope is returned. Serialize happens on the same
/// thread and task as the guard; the guard is dropped before this returns.
pub async fn explain<T: ToolContext>(
    ctx: &T,
    params: ExplainParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let id = crate::logging::correlation::current().unwrap_or_else(next_request_id);
    crate::logging::correlation::scope(id, explain_inner(ctx, params)).await
}

async fn explain_inner<T: ToolContext>(
    ctx: &T,
    params: ExplainParams,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("explain");
    let access = AccessPayload::default();
    let context_pack =
        parse_context_items(&params.context_items).map_err(MemoryError::Validation)?;
    let compact = params.compact;
    let request = ExplainRequest {
        context_pack,
        compact,
    };

    let timer = Instant::now();
    ctx.record(ToolEvent {
        op: "explain.start",
        args: json!({"count": request.context_pack.len()}),
        result: json!({}),
        level: LogLevel::Info,
        duration: None,
    });

    match ctx.explain(request, Some(access)).await {
        Ok(explanations) => {
            ctx.record(ToolEvent {
                op: "explain.done",
                args: json!({}),
                result: json!({"count": explanations.len()}),
                level: LogLevel::Info,
                duration: Some(timer.elapsed()),
            });
            let count = explanations.len();
            operation_metrics.record_result("explanations", count);
            // Under compact mode, omit `quote` via the serde adapters reading
            // the thread-local CompactGuard. The guard is held across
            // serialization; `value` contains the compact form.
            let value = {
                let _guard = crate::tools::compact::set_compact(compact);
                serde_json::to_value(&explanations)
                    .map_err(|e| MemoryError::Transient(format!("serialize explain items: {e}")))?
            };
            operation_metrics.success();
            if compact {
                Ok(ToolResponse::complete_list_compact(
                    value,
                    count,
                    "Use these citations directly in the final response.",
                ))
            } else {
                Ok(ToolResponse::complete_list(
                    value,
                    count,
                    "Use these citations directly in the final response.",
                ))
            }
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "explain.error",
                args: json!({}),
                result: json!({"error": err.to_string()}),
                level: LogLevel::Warn,
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::models::{
        AssembleContextRequest, AssembledContextItem, EntityCandidate, Episode, ExplainItem,
        ExtractResult, IngestRequest, InvalidateRequest,
    };

    /// A `ToolContext` that answers `explain` from a canned script and records
    /// the events the tool emits. Every other capability is unreachable from
    /// this test and panics if it is ever called, so the double cannot quietly
    /// grow into a second implementation of the trait.
    struct StubContext {
        explanations: Result<Vec<ExplainItem>, MemoryError>,
        events: Mutex<Vec<&'static str>>,
    }

    impl StubContext {
        fn returning(explanations: Vec<ExplainItem>) -> Self {
            Self {
                explanations: Ok(explanations),
                events: Mutex::new(Vec::new()),
            }
        }

        fn failing(error: MemoryError) -> Self {
            Self {
                explanations: Err(error),
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
            panic!("explain must not reach ingest")
        }

        async fn extract(
            &self,
            _episode_id: &str,
            _access: Option<AccessPayload>,
            _zero_shot_labels: Option<&[String]>,
        ) -> Result<ExtractResult, MemoryError> {
            panic!("explain must not reach extract")
        }

        async fn resolve(
            &self,
            _candidate: EntityCandidate,
            _access: Option<AccessPayload>,
        ) -> Result<String, MemoryError> {
            panic!("explain must not reach resolve")
        }

        async fn explain(
            &self,
            _request: ExplainRequest,
            _access: Option<AccessPayload>,
        ) -> Result<Vec<ExplainItem>, MemoryError> {
            self.explanations.clone()
        }

        async fn invalidate(
            &self,
            _request: InvalidateRequest,
            _access: Option<AccessPayload>,
        ) -> Result<(), MemoryError> {
            panic!("explain must not reach invalidate")
        }

        async fn assemble_context(
            &self,
            _request: AssembleContextRequest,
        ) -> Result<Vec<AssembledContextItem>, MemoryError> {
            panic!("explain must not reach assemble_context")
        }

        async fn find_episode(&self, _episode_id: &str) -> Result<Option<Episode>, MemoryError> {
            panic!("explain must not reach find_episode")
        }
    }

    /// One explained fact with the given content, as the capability returns it.
    fn item(content: &str) -> ExplainItem {
        ExplainItem {
            content: content.to_string(),
            source_episode: "episode:abc".to_string(),
            ..ExplainItem::default()
        }
    }

    /// Params carrying a single context item and the requested compact mode.
    fn params(compact: bool) -> ExplainParams {
        ExplainParams {
            context_items: r#"[{"content":"alpha","source_episode":"episode:abc"}]"#.to_string(),
            compact,
        }
    }

    /// Run an async `explain` call on a current-thread runtime.
    fn run(
        ctx: &StubContext,
        params: ExplainParams,
    ) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(explain(ctx, params))
    }

    #[test]
    fn returns_explanations_from_the_capability() {
        let ctx = StubContext::returning(vec![item("alpha")]);

        let observed = run(&ctx, params(false)).expect("explain succeeds");

        assert_eq!(observed.result[0]["content"], "alpha");
    }

    #[test]
    fn reports_the_number_of_explanations_as_total_count() {
        let ctx = StubContext::returning(vec![item("alpha"), item("beta")]);

        let observed = run(&ctx, params(false)).expect("explain succeeds");

        assert_eq!(observed.total_count, Some(2));
    }

    #[test]
    fn reports_a_success_status() {
        let ctx = StubContext::returning(vec![item("alpha")]);

        let observed = run(&ctx, params(false)).expect("explain succeeds");

        assert_eq!(observed.status, "success");
    }

    #[test]
    fn omits_the_quote_when_compact_is_requested() {
        let ctx = StubContext::returning(vec![item("alpha")]);

        let observed = run(&ctx, params(true)).expect("explain succeeds");

        assert!(
            observed.result[0].get("quote").is_none(),
            "compact mode drops the quote because it duplicates the content"
        );
    }

    #[test]
    fn keeps_the_quote_when_compact_is_not_requested() {
        let ctx = StubContext::returning(vec![item("alpha")]);

        let observed = run(&ctx, params(false)).expect("explain succeeds");

        assert_eq!(observed.result[0]["quote"], "");
    }

    #[test]
    fn records_a_start_event_before_the_capability_call() {
        let ctx = StubContext::returning(vec![item("alpha")]);

        run(&ctx, params(false)).expect("explain succeeds");

        assert_eq!(ctx.recorded_ops().first(), Some(&"explain.start"));
    }

    #[test]
    fn records_a_done_event_after_the_capability_call() {
        let ctx = StubContext::returning(vec![item("alpha")]);

        run(&ctx, params(false)).expect("explain succeeds");

        assert_eq!(ctx.recorded_ops().last(), Some(&"explain.done"));
    }

    #[test]
    fn records_an_error_event_when_the_capability_fails() {
        let ctx = StubContext::failing(MemoryError::Storage("index offline".into()));

        let _ = run(&ctx, params(false));

        assert!(ctx.recorded_ops().contains(&"explain.error"));
    }

    #[test]
    fn propagates_the_capability_error() {
        let ctx = StubContext::failing(MemoryError::Storage("index offline".into()));

        let observed = run(&ctx, params(false));

        assert!(
            observed.is_err(),
            "a capability failure must not be swallowed"
        );
    }

    #[test]
    fn rejects_context_items_that_are_not_valid_json() {
        let ctx = StubContext::returning(vec![]);
        let params = ExplainParams {
            context_items: "not json at all".to_string(),
            compact: false,
        };

        let observed = run(&ctx, params);

        assert!(matches!(observed, Err(MemoryError::Validation(_))));
    }

    #[test]
    fn never_calls_the_capability_when_context_items_are_invalid() {
        let ctx = StubContext::failing(MemoryError::Storage("must not run".into()));
        let params = ExplainParams {
            context_items: "not json at all".to_string(),
            compact: false,
        };

        let _ = run(&ctx, params);

        assert!(
            ctx.recorded_ops().is_empty(),
            "a parse failure must be reported before any capability call"
        );
    }
}
