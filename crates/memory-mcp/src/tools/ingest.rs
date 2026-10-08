//! `ingest` tool — protocol-agnostic.

use std::time::Instant;

use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessPayload, IngestRequest};
use crate::tools::context::{ToolContext, ToolEvent};
use crate::tools::params::IngestParams;
use crate::tools::parsers::parse_datetime;
use crate::tools::request_id::next_request_id;
use crate::tools::response::ToolResponse;

/// Ingest an episode and return its `episode_id`.
///
/// Mirrors the previous `MemoryMcp::ingest` body exactly: same validation,
/// same `ingest.start` / `ingest.done` / `ingest.error` events, same
/// `ToolResponse::success_with_guidance` guidance string.
pub async fn ingest<T: ToolContext>(
    ctx: &T,
    params: IngestParams,
) -> Result<ToolResponse<String>, MemoryError> {
    let id = crate::logging::correlation::current().unwrap_or_else(next_request_id);
    crate::logging::correlation::scope(id, ingest_inner(ctx, params)).await
}

async fn ingest_inner<T: ToolContext>(
    ctx: &T,
    params: IngestParams,
) -> Result<ToolResponse<String>, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("ingest");
    let t_ref = parse_datetime(&params.t_ref).ok_or_else(|| {
        MemoryError::Validation(format!(
            "Invalid `t_ref` value: {}. \
             Provide a valid ISO 8601 timestamp with seconds, e.g. 2026-05-11T17:34:00Z or \
             2026-05-11T17:34:00+00:00.",
            params.t_ref
        ))
    })?;
    let t_ingested = params.t_ingested.as_ref().and_then(|s| parse_datetime(s));
    let access = AccessPayload::default();
    let request = IngestRequest {
        source_type: params.source_type,
        source_id: params.source_id,
        content: params.content,
        t_ref,
        t_ingested,
        policy_tags: params.policy_tags,
    };

    let timer = Instant::now();
    let source_id = request.source_id.clone();
    ctx.record(ToolEvent {
        op: "ingest.start",
        args: json!({"source_type": &request.source_type, "source_id": &source_id}),
        result: json!({}),
        level: LogLevel::Info,
        duration: None,
    });

    // The write is the whole cost of this tool, so it is the stage worth
    // naming: an `ingest` that got slower can be traced to this histogram
    // rather than to the operation total, which moves for any reason at all.
    let outcome = {
        let _stage = crate::shared::observability::StageTimer::new("ingest", "store_write");
        ctx.ingest(request, Some(access)).await
    };

    match outcome {
        Ok(episode_id) => {
            operation_metrics.record_result("episodes", 1);
            operation_metrics.success();
            ctx.record(ToolEvent {
                op: "ingest.done",
                args: json!({"source_id": &source_id}),
                result: json!({"episode_id": &episode_id}),
                level: LogLevel::Info,
                duration: Some(timer.elapsed()),
            });
            Ok(ToolResponse::success_with_guidance(
                episode_id,
                "Call extract next to derive entities and facts. Pass the episode_id you just \
                 received back exactly as it appears, including the 'episode:' prefix — do not \
                 strip or re-add it.",
            ))
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "ingest.error",
                args: json!({"source_id": &source_id}),
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

    use crate::models::Episode;
    use crate::tools::context::ToolContext;

    /// The one capability `ingest` reaches; every other one panics if
    /// the tool ever grows a call to it.
    struct StubContext {
        episode_id: String,
        /// `(op, the correlation id in force when the tool recorded it)`.
        events: Mutex<Vec<(&'static str, Option<String>)>>,
    }

    impl ToolContext for StubContext {
        fn record(&self, event: ToolEvent) {
            self.events
                .lock()
                .expect("event log")
                .push((event.op, crate::logging::correlation::current()));
        }

        async fn ingest(
            &self,
            _request: IngestRequest,
            _access: Option<AccessPayload>,
        ) -> Result<String, MemoryError> {
            Ok(self.episode_id.clone())
        }

        async fn extract(
            &self,
            _episode_id: &str,
            _access: Option<AccessPayload>,
            _zero_shot_labels: Option<&[String]>,
        ) -> Result<crate::models::ExtractResult, MemoryError> {
            panic!("ingest must not reach extract")
        }

        async fn resolve(
            &self,
            _candidate: crate::models::EntityCandidate,
            _access: Option<AccessPayload>,
        ) -> Result<String, MemoryError> {
            panic!("ingest must not reach resolve")
        }

        async fn explain(
            &self,
            _request: crate::models::ExplainRequest,
            _access: Option<AccessPayload>,
        ) -> Result<Vec<crate::models::ExplainItem>, MemoryError> {
            panic!("ingest must not reach explain")
        }

        async fn invalidate(
            &self,
            _request: crate::models::InvalidateRequest,
            _access: Option<AccessPayload>,
        ) -> Result<(), MemoryError> {
            panic!("ingest must not reach invalidate")
        }

        async fn assemble_context(
            &self,
            _request: crate::models::AssembleContextRequest,
        ) -> Result<Vec<crate::models::AssembledContextItem>, MemoryError> {
            panic!("ingest must not reach assemble_context")
        }

        async fn find_episode(&self, _episode_id: &str) -> Result<Option<Episode>, MemoryError> {
            panic!("ingest must not reach find_episode")
        }
    }

    fn params() -> IngestParams {
        IngestParams {
            source_type: "ad-hoc".to_string(),
            source_id: "guidance-roundtrip".to_string(),
            content: "content".to_string(),
            t_ref: "2026-01-01T00:00:00Z".to_string(),
            t_ingested: None,
            policy_tags: Vec::new(),
        }
    }

    fn run(ctx: &StubContext) -> Result<ToolResponse<String>, MemoryError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(ingest(ctx, params()))
    }

    #[test]
    fn guidance_tells_the_caller_to_pass_the_id_back_unchanged() {
        // `ingest` returns the id as a bare string and then tells the
        // caller to use it in the next call. A caller that re-types it
        // and drops the `episode:` prefix gets a validation error on the
        // next tool, so the guidance is the only place that can prevent
        // the mistake.
        let ctx = StubContext {
            episode_id: "episode:d1a2438bcfb3380ffb913ec4".to_string(),
            events: Mutex::new(Vec::new()),
        };

        let response = run(&ctx).expect("ingest should succeed");
        let guidance = response.guidance.expect("ingest must carry guidance");

        assert!(
            guidance.contains("episode:"),
            "guidance must name the canonical prefix, got: {guidance}"
        );
        assert!(
            guidance.contains("exact") || guidance.contains("unchanged"),
            "guidance must require the id be passed back verbatim, got: {guidance}"
        );
        assert!(
            guidance.to_lowercase().contains("strip"),
            "guidance must warn against stripping the prefix, got: {guidance}"
        );
        assert!(
            guidance.contains("extract"),
            "guidance must still name the next tool, got: {guidance}"
        );
    }

    #[test]
    fn guidance_names_the_same_id_the_result_carries() {
        // The guidance is only actionable if it agrees with the result
        // it sits next to; a caller that reads one and compares the
        // other must not be told to send something else.
        let episode_id = "episode:d1a2438bcfb3380ffb913ec4";
        let ctx = StubContext {
            episode_id: episode_id.to_string(),
            events: Mutex::new(Vec::new()),
        };

        let response = run(&ctx).expect("ingest should succeed");

        assert_eq!(
            response.result, episode_id,
            "the result must be the canonical id the guidance refers to"
        );
    }

    /// A tool adopts the ambient correlation id, so its events join the request
    /// that is already in flight instead of minting a second, unjoinable id.
    #[tokio::test]
    async fn a_tool_adopts_an_ambient_request_id() {
        let ctx = StubContext {
            episode_id: "episode:d1a2438bcfb3380ffb913ec4".to_string(),
            events: Mutex::new(Vec::new()),
        };

        crate::logging::correlation::scope("req_x", ingest(&ctx, params()))
            .await
            .expect("ingest should succeed");

        let events = ctx.events.lock().expect("event log");
        assert!(!events.is_empty(), "the tool records lifecycle events");
        assert!(
            events.iter().all(|(_, id)| id.as_deref() == Some("req_x")),
            "every event must carry the ambient id: {events:?}"
        );
    }

    /// With no ambient id — stdio, a direct call in a test — the tool mints a
    /// per-call `req_NNNN`, preserving the existing stdio behaviour.
    #[tokio::test]
    async fn a_tool_mints_an_id_when_none_is_ambient() {
        let ctx = StubContext {
            episode_id: "episode:d1a2438bcfb3380ffb913ec4".to_string(),
            events: Mutex::new(Vec::new()),
        };

        ingest(&ctx, params()).await.expect("ingest should succeed");

        let events = ctx.events.lock().expect("event log");
        assert!(!events.is_empty());
        assert!(
            events
                .iter()
                .all(|(_, id)| id.as_deref().is_some_and(|id| id.starts_with("req_"))),
            "a tool with no ambient id must mint one: {events:?}"
        );
    }
}
