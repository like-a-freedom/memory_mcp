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
    let mut context_pack =
        parse_context_items(&params.context_items).map_err(MemoryError::Validation)?;
    let total = context_pack.len();
    let start = params.item_offset.unwrap_or_default();
    if let Some(offset) = params.item_offset {
        if offset > total {
            return Err(MemoryError::Validation(
                "item_offset exceeds the item count".into(),
            ));
        }
        context_pack = context_pack.into_iter().skip(offset).take(1).collect();
    }
    if (params.source_offset.is_some()
        || params.source_limit_scalars.is_some()
        || params.provenance_offset.is_some())
        && context_pack.len() != 1
    {
        return Err(MemoryError::Validation(
            "source and provenance continuations require exactly one selected item".into(),
        ));
    }
    if params
        .source_limit_scalars
        .is_some_and(|limit| params.source_offset.is_none() || limit > 8000)
    {
        return Err(MemoryError::Validation(
            "source_limit_scalars requires source_offset and must be at most 8000".into(),
        ));
    }
    if context_pack.iter().any(|item| {
        item.source_episode.len() > 1024 || item.fact_id.as_ref().is_some_and(|id| id.len() > 1024)
    }) {
        return Err(MemoryError::Validation(
            "explain item identifiers must be at most 1024 bytes".into(),
        ));
    }
    let compact = params.compact;
    let supplied_content: Vec<_> = context_pack
        .iter()
        .map(|item| !item.content.is_empty())
        .collect();
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
            let response =
                bounded_response(explanations, &supplied_content, &params, start, total)?;
            operation_metrics.success();
            Ok(response)
        }
        Err(err) => {
            ctx.record(ToolEvent {
                op: "explain.error",
                args: json!({}),
                result: json!({"error_code": err.code()}),
                level: err.log_level(),
                duration: Some(timer.elapsed()),
            });
            Err(err)
        }
    }
}

const EXPLAIN_MAX_BYTES: usize = 32 * 1024;

fn bounded_response(
    explanations: Vec<crate::models::ExplainItem>,
    supplied_content: &[bool],
    params: &ExplainParams,
    start: usize,
    requested_total: usize,
) -> Result<ToolResponse<serde_json::Value>, MemoryError> {
    let compact = params.compact;
    let total = if params.item_offset.is_some() {
        requested_total
    } else {
        explanations.len()
    };
    let end = start + explanations.len();
    let mut response = if compact && params.item_offset.is_none() {
        ToolResponse::complete_list_compact(
            json!([]),
            total,
            "Use these citations in the final response.",
        )
    } else {
        ToolResponse::complete_list(
            json!([]),
            total,
            "Use these citations in the final response.",
        )
    };
    if end < total {
        mark_omitted(&mut response, total, end, false);
    }
    for (index, mut item) in explanations.into_iter().enumerate() {
        let (citation, reasons) = crate::memory::explanation::public_citation(
            &mut item,
            supplied_content.get(index).copied().unwrap_or_default(),
            params.source_offset,
            params.source_limit_scalars,
            params.provenance_offset,
        )?;
        let quote = std::mem::take(&mut item.quote);
        let mut value = {
            let _guard = crate::tools::compact::set_compact(compact);
            serde_json::to_value(item).map_err(serialization_error)?
        };
        if let Some(fields) = value.as_object_mut() {
            if let serde_json::Value::Object(citation) = citation {
                fields.extend(citation);
            }
            if !quote.is_empty() || !compact {
                fields.insert("quote".into(), json!(quote));
            }
        }
        let previous_reasons = response.partial_reasons.clone();
        response.partial_reasons.extend(reasons);
        refresh_status(&mut response);
        let Some(items) = response.result.as_array_mut() else {
            return Err(MemoryError::Transient(
                "explain results are not an array".into(),
            ));
        };
        items.push(value);
        if envelope_fits(&response)? {
            continue;
        }
        if index == 0 && trim_first_item(&mut response)? {
            continue;
        }
        if let Some(items) = response.result.as_array_mut() {
            items.pop();
        }
        response.partial_reasons = previous_reasons;
        mark_omitted(&mut response, total, start + index, true);
        if !envelope_fits(&response)? {
            response.result = json!([]);
            response.partial_reasons = vec!["items_omitted", "response_budget_applied"];
            response.next_offset = Some(start);
        }
        break;
    }
    Ok(response)
}

fn envelope_fits(response: &ToolResponse<serde_json::Value>) -> Result<bool, MemoryError> {
    Ok(serde_json::to_vec(response)
        .map_err(serialization_error)?
        .len()
        <= EXPLAIN_MAX_BYTES)
}

fn refresh_status(response: &mut ToolResponse<serde_json::Value>) {
    response.partial_reasons.sort_unstable();
    response.partial_reasons.dedup();
    if !response.partial_reasons.is_empty() {
        response.status = "partial".into();
        response.guidance = Some("Use the returned evidence and request remaining items, source text, or provenance by offset.".into());
    }
}

fn mark_omitted(
    response: &mut ToolResponse<serde_json::Value>,
    total: usize,
    offset: usize,
    budget: bool,
) {
    response.has_more = Some(true);
    response.total_count = Some(total);
    response.next_offset = Some(offset);
    response.partial_reasons.push("items_omitted");
    if budget {
        response.partial_reasons.push("response_budget_applied");
    }
    refresh_status(response);
}

fn trim_first_item(response: &mut ToolResponse<serde_json::Value>) -> Result<bool, MemoryError> {
    const DIAGNOSTICS: &[&str] = &[
        "graph_insights",
        "fact_age_days",
        "decayed_confidence",
        "ingestion_method",
        "provenance",
    ];
    let Some(item) = response.result[0].as_object_mut() else {
        return Ok(false);
    };
    for key in DIAGNOSTICS {
        item.remove(*key);
    }
    mark_item_partial(response, "response_budget_applied");
    if envelope_fits(response)? {
        return Ok(true);
    }
    if response.result[0]["all_sources"]
        .as_array()
        .is_some_and(|sources| !sources.is_empty())
    {
        let count = response.result[0]["all_sources"]
            .as_array()
            .map_or(0, Vec::len);
        let next = response.result[0]["provenance_next_offset"]
            .as_u64()
            .or_else(|| response.result[0]["provenance_total_count"].as_u64());
        response.result[0]["all_sources"] = json!([]);
        response.result[0]["provenance_next_offset"] =
            json!(next.map(|offset| offset.saturating_sub(count as u64)));
        mark_item_partial(response, "provenance_omitted_by_budget");
        if envelope_fits(response)? {
            return Ok(true);
        }
    }
    if response.result[0]["content"]
        .as_str()
        .is_some_and(|text| !text.is_empty())
    {
        response.result[0]["content"] = json!("");
        response.result[0]["content_truncated"] = json!(true);
        mark_item_partial(response, "content_truncated");
        if envelope_fits(response)? {
            return Ok(true);
        }
    }
    if fit_citation_prefix(response)? {
        return Ok(true);
    }
    if let Some(item) = response.result[0].as_object_mut()
        && item.remove("quote").is_some()
    {
        item.insert("quote_returned".into(), json!(false));
        item.insert("quote_complete".into(), json!(false));
        mark_item_partial(response, "quote_incomplete");
        mark_item_partial(response, "quote_omitted_by_budget");
    }
    envelope_fits(response)
}

fn fit_citation_prefix(
    response: &mut ToolResponse<serde_json::Value>,
) -> Result<bool, MemoryError> {
    let item = &response.result[0];
    let Some(window) = item["citation_context"]
        .as_str()
        .filter(|text| !text.is_empty())
    else {
        return Ok(false);
    };
    let window = window.to_owned();
    let mut boundaries: Vec<_> = window.char_indices().map(|(index, _)| index).collect();
    boundaries.push(window.len());
    let explicit = item.get("citation_context_next_offset").is_some();
    let minimum = usize::from(explicit);
    let start = item["citation_context_start_offset"]
        .as_u64()
        .unwrap_or_default();
    let original = item.clone();
    let previous_reasons = response.partial_reasons.clone();
    mark_item_partial(response, "citation_window_incomplete");
    if explicit {
        mark_item_partial(response, "source_window_incomplete");
    }

    let mut low = minimum;
    let mut high = boundaries.len() - 2;
    let mut best = None;
    while low <= high {
        let count = low + (high - low) / 2;
        let prefix = &window[..boundaries[count]];
        let item = &mut response.result[0];
        item["citation_context"] = json!(prefix);
        item["citation_context_scalars_returned"] = json!(count);
        item["citation_context_covers_entire_source"] = json!(false);
        item["quote_contained_in_window"] = json!(
            item["quote"]
                .as_str()
                .is_some_and(|quote| !quote.is_empty() && prefix.contains(quote))
        );
        if explicit {
            item["citation_context_next_offset"] = json!(start + count as u64);
        }
        if envelope_fits(response)? {
            best = Some(response.result[0].clone());
            low = count + 1;
        } else if count == 0 {
            break;
        } else {
            high = count - 1;
        }
    }
    if let Some(item) = best {
        response.result[0] = item;
        Ok(true)
    } else {
        response.result[0] = original;
        response.partial_reasons = previous_reasons;
        Ok(false)
    }
}

fn mark_item_partial(response: &mut ToolResponse<serde_json::Value>, reason: &'static str) {
    if let Some(reasons) = response.result[0]["partial_reasons"].as_array_mut() {
        let value = json!(reason);
        if !reasons.contains(&value) {
            reasons.push(value);
        }
    }
    response.partial_reasons.push(reason);
    refresh_status(response);
}

fn serialization_error(error: serde_json::Error) -> MemoryError {
    MemoryError::Transient(format!("serialize explain response: {error}"))
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
            ..ExplainParams::default()
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
            ..ExplainParams::default()
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
            ..ExplainParams::default()
        };

        let _ = run(&ctx, params);

        assert!(
            ctx.recorded_ops().is_empty(),
            "a parse failure must be reported before any capability call"
        );
    }
}
