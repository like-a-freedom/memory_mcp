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
    let mut operation_metrics = crate::observability::OperationMetrics::new("resolve");
    let access = AccessPayload::default();
    let candidate = EntityCandidate {
        entity_type: params.entity_type,
        canonical_name: params.canonical_name,
        aliases: params.aliases,
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    ctx.record(ToolEvent {
        op: "resolve.start",
        args: json!({"entity_type": candidate.entity_type, "canonical": candidate.canonical_name}),
        result: json!({}),
        level: LogLevel::Info,
        request_id: Some(request_id.clone()),
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
                request_id: Some(request_id),
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
                request_id: Some(request_id),
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
        AssembleContextRequest, AssembledContextItem, Episode, ExplainItem, ExplainRequest,
        ExtractResult, IngestRequest, InvalidateRequest,
    };

    /// A `ToolContext` that answers `resolve` from a canned result and records
    /// the events the tool emits. Every other capability is unreachable from
    /// this test and panics if called.
    struct StubContext {
        resolve_result: Result<String, MemoryError>,
        events: Mutex<Vec<&'static str>>,
    }

    impl StubContext {
        fn returning(entity_id: &str) -> Self {
            Self {
                resolve_result: Ok(entity_id.to_string()),
                events: Mutex::new(Vec::new()),
            }
        }

        fn failing(error: MemoryError) -> Self {
            Self {
                resolve_result: Err(error),
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
            panic!("resolve must not reach ingest")
        }

        async fn extract(
            &self,
            _episode_id: &str,
            _access: Option<AccessPayload>,
            _zero_shot_labels: Option<&[String]>,
        ) -> Result<ExtractResult, MemoryError> {
            panic!("resolve must not reach extract")
        }

        async fn resolve(
            &self,
            _candidate: EntityCandidate,
            _access: Option<AccessPayload>,
        ) -> Result<String, MemoryError> {
            self.resolve_result.clone()
        }

        async fn explain(
            &self,
            _request: ExplainRequest,
            _access: Option<AccessPayload>,
        ) -> Result<Vec<ExplainItem>, MemoryError> {
            panic!("resolve must not reach explain")
        }

        async fn invalidate(
            &self,
            _request: InvalidateRequest,
            _access: Option<AccessPayload>,
        ) -> Result<(), MemoryError> {
            panic!("resolve must not reach invalidate")
        }

        async fn assemble_context(
            &self,
            _request: AssembleContextRequest,
        ) -> Result<Vec<AssembledContextItem>, MemoryError> {
            panic!("resolve must not reach assemble_context")
        }

        async fn find_episode(&self, _episode_id: &str) -> Result<Option<Episode>, MemoryError> {
            panic!("resolve must not reach find_episode")
        }
    }

    /// Params for a `person` entity with one alias.
    fn params() -> ResolveParams {
        ResolveParams {
            entity_type: "person".to_string(),
            canonical_name: "Ada Lovelace".to_string(),
            aliases: vec!["Ada".to_string()],
        }
    }

    /// Run an async `resolve` call on a current-thread runtime.
    fn run(ctx: &StubContext, params: ResolveParams) -> Result<ToolResponse<String>, MemoryError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(resolve(ctx, params))
    }

    #[test]
    fn returns_the_entity_id_from_the_capability() {
        let ctx = StubContext::returning("entity:1");

        let observed = run(&ctx, params()).expect("resolve succeeds");

        assert_eq!(observed.result, "entity:1");
    }

    #[test]
    fn reports_a_success_status() {
        let ctx = StubContext::returning("entity:1");

        let observed = run(&ctx, params()).expect("resolve succeeds");

        assert_eq!(observed.status, "success");
    }

    #[test]
    fn records_a_start_event_before_the_capability_call() {
        let ctx = StubContext::returning("entity:1");

        run(&ctx, params()).expect("resolve succeeds");

        assert_eq!(ctx.recorded_ops().first(), Some(&"resolve.start"));
    }

    #[test]
    fn records_a_done_event_after_the_capability_call() {
        let ctx = StubContext::returning("entity:1");

        run(&ctx, params()).expect("resolve succeeds");

        assert_eq!(ctx.recorded_ops().last(), Some(&"resolve.done"));
    }

    #[test]
    fn records_an_error_event_when_the_capability_fails() {
        let ctx = StubContext::failing(MemoryError::Conflict("ambiguous".into()));

        let _ = run(&ctx, params());

        assert!(ctx.recorded_ops().contains(&"resolve.error"));
    }

    #[test]
    fn propagates_the_capability_error() {
        let ctx = StubContext::failing(MemoryError::Conflict("ambiguous".into()));

        let observed = run(&ctx, params());

        assert!(
            observed.is_err(),
            "a resolution failure must not be swallowed"
        );
    }

    #[test]
    fn an_entity_with_no_aliases_still_resolves() {
        let ctx = StubContext::returning("entity:1");
        let params = ResolveParams {
            aliases: Vec::new(),
            ..params()
        };

        let observed = run(&ctx, params).expect("resolve succeeds");

        assert_eq!(observed.result, "entity:1");
    }
}
