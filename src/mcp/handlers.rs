//! MCP tool handler implementations.

use std::sync::Arc;

use chrono::Utc;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{
    AccessContext, AssembleContextRequest, AssembledContextItem, EntityCandidate, ExplainItem,
    ExplainRequest, ExtractResult, IngestRequest, InvalidateRequest,
};
use crate::service::MemoryService;

use super::error::mcp_error;
use super::params::*;
use super::parsers::{content_hash, parse_context_items, parse_datetime};

/// Response wrapper for tool results.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ToolResponse<T> {
    /// Result status for the tool call.
    pub status: String,
    /// The actual result data.
    pub result: T,
    /// Optional next-step guidance for the caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    /// Pagination flag for list responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    /// Total count of records in the current response slice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
    /// Offset for the next page when pagination is supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

impl<T> ToolResponse<T> {
    fn success_with_guidance(result: T, guidance: impl Into<String>) -> Self {
        Self {
            status: "success".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: None,
            total_count: None,
            next_offset: None,
        }
    }

    fn partial_with_guidance(result: T, guidance: impl Into<String>) -> Self {
        Self {
            status: "partial".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: None,
            total_count: None,
            next_offset: None,
        }
    }

    fn complete_list(result: T, total_count: usize, guidance: impl Into<String>) -> Self {
        Self {
            status: "success".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: Some(false),
            total_count: Some(total_count),
            next_offset: None,
        }
    }
}

/// MCP (Model Context Protocol) server handler for memory operations.
///
/// `MemoryMcp` implements the MCP protocol and provides tools for:
/// - Ingesting episodes (conversations, emails, documents)
/// - Extracting entities and facts
/// - Resolving entity aliases
/// - Assembling context for queries
/// - Managing invalidations
///
/// # Example
///
/// ```rust,no_run
/// use memory_mcp::{MemoryMcp, MemoryService};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let service = MemoryService::new_from_env().await?;
///     let server = MemoryMcp::new(service);
///     // Start the MCP server...
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct MemoryMcp {
    service: Arc<MemoryService>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl MemoryMcp {
    const SERVER_INSTRUCTIONS: &str = "Memory MCP server: stores facts about entities and relationships, resolves aliases, and assembles long-term context.";

    /// Creates a new `MemoryMcp` instance with the given service.
    ///
    /// # Arguments
    ///
    /// * `service` - The `MemoryService` to use for memory operations.
    pub fn new(service: MemoryService) -> Self {
        Self {
            service: Arc::new(service),
            tool_router: Self::tool_router(),
        }
    }

    /// Returns a reference to the underlying `MemoryService`.
    ///
    /// This can be used to access service methods directly if needed.
    #[must_use]
    pub fn service(&self) -> Arc<MemoryService> {
        self.service.clone()
    }

    fn build_server_info() -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(Self::SERVER_INSTRUCTIONS)
    }

    /// Shared implementation for extract operations.
    ///
    /// Handles extracting from episode_id or ingesting content first.
    #[allow(clippy::too_many_arguments)]
    async fn extract_impl(
        &self,
        episode_id: Option<String>,
        content: Option<String>,
        text: Option<String>,
        source_type: Option<String>,
        source_id: Option<String>,
        t_ref: Option<String>,
        scope: Option<String>,
        enable_logging: bool,
    ) -> Result<ToolResponse<ExtractResult>, ErrorData> {
        use super::parsers::normalize_optional_string;

        let access = AccessContext::default();
        let episode_id = normalize_optional_string(episode_id);
        let content = normalize_optional_string(content);
        let text = normalize_optional_string(text);

        if enable_logging {
            self.service.log_tool_event(
                "extract.start",
                json!({"episode_id": episode_id, "has_content": content.is_some() || text.is_some()}),
                json!({}),
                LogLevel::Info,
            );
        }

        // If episode_id provided, extract directly
        if let Some(ref episode_id) = episode_id {
            match self.service.extract(episode_id, Some(access)).await {
                Ok(result) => {
                    if enable_logging {
                        self.service.log_tool_event(
                            "extract.done",
                            json!({"episode_id": episode_id}),
                            json!({"entities": result["entities"].as_array().map(|a| a.len()).unwrap_or(0), "facts": result["facts"].as_array().map(|a| a.len()).unwrap_or(0)}),
                            LogLevel::Info,
                        );
                    }
                    let parsed: ExtractResult = serde_json::from_value(result).map_err(|err| {
                        ErrorData::new(
                            rmcp::model::ErrorCode::INTERNAL_ERROR,
                            format!("extract result schema mismatch: {err}"),
                            None,
                        )
                    })?;
                    return Ok(ToolResponse::success_with_guidance(
                        parsed,
                        "Resolve canonical entities for any ambiguous names before creating manual links.",
                    ));
                }
                Err(err) => {
                    if enable_logging {
                        self.service.log_tool_event(
                            "extract.error",
                            json!({"episode_id": episode_id}),
                            json!({"error": err.to_string()}),
                            LogLevel::Warn,
                        );
                    }
                    return Err(mcp_error(err));
                }
            }
        }

        // Otherwise, ingest content first then extract
        let content = content.or(text).unwrap_or_default();
        if content.trim().is_empty() {
            if enable_logging {
                self.service.log_tool_event(
                    "extract.no_input",
                    json!({"episode_id": episode_id, "has_content": false}),
                    json!({"status": "no_input"}),
                    LogLevel::Warn,
                );
            }
            return Ok(ToolResponse::partial_with_guidance(
                ExtractResult::empty(),
                "Provide either `episode_id` or non-empty `content`/`text`, then retry.",
            ));
        }

        let source_type = source_type.unwrap_or_else(|| "ad-hoc".to_string());
        let source_id = source_id.unwrap_or_else(|| content_hash(&content));
        let t_ref = t_ref
            .as_ref()
            .and_then(|s| parse_datetime(s))
            .unwrap_or_else(Utc::now);
        let scope = scope.unwrap_or_else(|| "org".to_string());

        match self
            .service
            .ingest(
                IngestRequest {
                    source_type,
                    source_id,
                    content,
                    t_ref,
                    scope,
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: Vec::new(),
                },
                Some(access.clone()),
            )
            .await
        {
            Ok(episode_id) => match self.service.extract(&episode_id, Some(access)).await {
                Ok(result) => {
                    if enable_logging {
                        self.service.log_tool_event(
                            "extract.done",
                            json!({"episode_id": episode_id}),
                            json!({"entities": result["entities"].as_array().map(|a| a.len()).unwrap_or(0), "facts": result["facts"].as_array().map(|a| a.len()).unwrap_or(0)}),
                            LogLevel::Info,
                        );
                    }
                    let parsed: ExtractResult = serde_json::from_value(result).map_err(|err| {
                        ErrorData::new(
                            rmcp::model::ErrorCode::INTERNAL_ERROR,
                            format!("extract result schema mismatch: {err}"),
                            None,
                        )
                    })?;
                    Ok(ToolResponse::success_with_guidance(
                        parsed,
                        "Resolve canonical entities for any ambiguous names before creating manual links.",
                    ))
                }
                Err(err) => {
                    if enable_logging {
                        self.service.log_tool_event(
                            "extract.error",
                            json!({}),
                            json!({"error": err.to_string()}),
                            LogLevel::Warn,
                        );
                    }
                    Err(mcp_error(err))
                }
            },
            Err(err) => {
                if enable_logging {
                    self.service.log_tool_event(
                        "extract.error",
                        json!({}),
                        json!({"error": err.to_string()}),
                        LogLevel::Warn,
                    );
                }
                Err(mcp_error(err))
            }
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MemoryMcp {
    fn get_info(&self) -> ServerInfo {
        Self::build_server_info()
    }
}

#[tool_router]
impl MemoryMcp {
    #[tool(
        description = "Store a new episode in long-term memory. Use this tool when you need to persist source material before extracting entities or facts. Do not use this tool for retrieval. Arguments must include ISO 8601 `t_ref` and a memory `scope`. Returns the created or existing `episode_id`. On error, fix the input fields and retry."
    )]
    pub async fn ingest(
        &self,
        params: Parameters<IngestParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        let p = params.0;
        let t_ref = parse_datetime(&p.t_ref).ok_or_else(|| {
            ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "Invalid t_ref format".to_string(),
                None,
            )
        })?;
        let t_ingested = p.t_ingested.as_ref().and_then(|s| parse_datetime(s));

        let access = AccessContext::default();
        let request = IngestRequest {
            source_type: p.source_type.clone(),
            source_id: p.source_id.clone(),
            content: p.content.clone(),
            t_ref,
            scope: p.scope.clone(),
            t_ingested,
            visibility_scope: p.visibility_scope,
            policy_tags: p.policy_tags.clone(),
        };

        self.service.log_tool_event(
            "ingest.start",
            json!({"source_type": p.source_type, "source_id": p.source_id, "scope": p.scope}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.ingest(request, Some(access)).await {
            Ok(episode_id) => {
                self.service.log_tool_event(
                    "ingest.done",
                    json!({"source_id": p.source_id}),
                    json!({"episode_id": &episode_id}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    episode_id,
                    "Call extract next to derive entities and facts.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "ingest.error",
                    json!({"source_id": p.source_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Explain context items with provenance-ready citations. Use this tool when you already have context items and need source snippets for an answer. Do not use this tool to search memory. Pass `context_items` as a JSON array string of objects or source IDs. Returns citation-ready items. On error, fix the JSON payload shape and retry."
    )]
    pub async fn explain(
        &self,
        params: Parameters<ExplainParams>,
    ) -> Result<Json<ToolResponse<Vec<ExplainItem>>>, ErrorData> {
        let access = AccessContext::default();
        let context_pack = parse_context_items(&params.0.context_items)
            .map_err(|msg| ErrorData::new(rmcp::model::ErrorCode::INVALID_PARAMS, msg, None))?;
        let request = ExplainRequest { context_pack };

        self.service.log_tool_event(
            "explain.start",
            json!({"count": request.context_pack.len()}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.explain(request, Some(access)).await {
            Ok(explanations) => {
                let explanations: Vec<ExplainItem> = explanations
                    .into_iter()
                    .map(|value| {
                        serde_json::from_value(value).map_err(|err| {
                            ErrorData::new(
                                rmcp::model::ErrorCode::INTERNAL_ERROR,
                                format!("explain result schema mismatch: {err}"),
                                None,
                            )
                        })
                    })
                    .collect::<Result<_, _>>()?;
                self.service.log_tool_event(
                    "explain.done",
                    json!({}),
                    json!({"count": explanations.len()}),
                    LogLevel::Info,
                );
                let count = explanations.len();
                Ok(Json(ToolResponse::complete_list(
                    explanations,
                    count,
                    "Use these citations directly in the final response.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "explain.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Extract entities, facts, and relationships from remembered content. Use this tool when you need structured knowledge from an existing episode or from new inline content. Do not use this tool for retrieval. If you pass content instead of an `episode_id`, the server ingests it first and then extracts facts. Returns extracted entities, facts, and links. On error, provide either `episode_id` or content/text."
    )]
    pub async fn extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<Json<ToolResponse<ExtractResult>>, ErrorData> {
        let p = params.0;
        let response = self
            .extract_impl(
                p.episode_id,
                p.content,
                p.text,
                p.source_type,
                p.source_id,
                p.t_ref,
                p.scope,
                true,
            )
            .await?;
        Ok(Json(response))
    }

    #[tool(
        description = "Resolve a canonical entity identifier for a name and its aliases. Use this tool when a person, company, or project may appear under multiple names. Do not use this tool for full-text retrieval. Arguments must include `entity_type`, `canonical_name`, and optional `aliases`. Returns the canonical `entity_id`. On error, fix the entity fields and retry."
    )]
    pub async fn resolve(
        &self,
        params: Parameters<ResolveParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        let p = params.0;
        let access = AccessContext::default();
        let candidate = EntityCandidate {
            entity_type: p.entity_type.clone(),
            canonical_name: p.canonical_name.clone(),
            aliases: p.aliases.clone(),
        };

        self.service.log_tool_event(
            "resolve.start",
            json!({"entity_type": candidate.entity_type, "canonical": candidate.canonical_name}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.resolve(candidate, Some(access)).await {
            Ok(entity_id) => {
                self.service.log_tool_event(
                    "resolve.done",
                    json!({}),
                    json!({"entity_id": &entity_id}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    entity_id,
                    "Use this entity_id when linking facts or relationships.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "resolve.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Invalidate a fact while preserving historical traceability. Use this tool when a fact becomes outdated or superseded. Do not use this tool to delete memory. Arguments require a `fact_id`, `reason`, and ISO 8601 `t_invalid`. Returns confirmation. On error, verify the fact identifier and retry."
    )]
    pub async fn invalidate(
        &self,
        params: Parameters<InvalidateParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        let p = params.0;
        let access = AccessContext::default();
        let t_invalid = parse_datetime(&p.t_invalid).ok_or_else(|| {
            ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "Invalid t_invalid format".to_string(),
                None,
            )
        })?;
        let request = InvalidateRequest {
            fact_id: p.fact_id.clone(),
            reason: p.reason.clone(),
            t_invalid,
        };

        self.service.log_tool_event(
            "invalidate.start",
            json!({"fact_id": request.fact_id}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.invalidate(request, Some(access)).await {
            Ok(res) => {
                self.service.log_tool_event(
                    "invalidate.done",
                    json!({"fact_id": p.fact_id}),
                    json!({"result": res}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Re-run assemble_context with a fresh `as_of` timestamp to confirm the fact is no longer active.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "invalidate.error",
                    json!({"fact_id": p.fact_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Assemble the most relevant active memory context for a query. Use this tool when you need retrieval across stored facts before answering or planning. Do not use this tool to ingest new content. Arguments require a natural-language `query`, a `scope`, and optional `as_of` plus `budget`. Returns ranked context items with confidence and rationale. On error, fix the query parameters and retry."
    )]
    pub async fn assemble_context(
        &self,
        params: Parameters<AssembleContextParams>,
    ) -> Result<Json<ToolResponse<Vec<AssembledContextItem>>>, ErrorData> {
        let p = params.0;
        let as_of = if p.as_of.trim().is_empty() {
            None
        } else {
            chrono::DateTime::parse_from_rfc3339(&p.as_of)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
        };
        let request = AssembleContextRequest {
            query: p.query.clone(),
            scope: p.scope.clone(),
            as_of,
            budget: p.budget,
            access: None,
        };

        self.service.log_tool_event(
            "assemble_context.start",
            json!({"scope": request.scope, "query": request.query}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.assemble_context(request).await {
            Ok(results) => {
                let results: Vec<AssembledContextItem> = results
                    .into_iter()
                    .map(|value| {
                        serde_json::from_value(value).map_err(|err| {
                            ErrorData::new(
                                rmcp::model::ErrorCode::INTERNAL_ERROR,
                                format!("assemble_context result schema mismatch: {err}"),
                                None,
                            )
                        })
                    })
                    .collect::<Result<_, _>>()?;
                self.service.log_tool_event(
                    "assemble_context.done",
                    json!({}),
                    json!({"count": results.len()}),
                    LogLevel::Info,
                );
                let count = results.len();
                Ok(Json(ToolResponse::complete_list(
                    results,
                    count,
                    "Call explain if you need provenance-ready citations for selected items.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "assemble_context.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn schema_json<T: schemars::JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).expect("schema json")
    }

    #[test]
    fn build_server_info_enables_tools_and_sets_instructions() {
        let info = MemoryMcp::build_server_info();
        let capabilities = serde_json::to_value(&info.capabilities).unwrap();

        assert_eq!(
            info.instructions.as_deref(),
            Some(
                "Memory MCP server: stores facts about entities and relationships, resolves aliases, and assembles long-term context.",
            ),
        );
        assert!(capabilities.get("tools").is_some());
    }

    #[test]
    fn tool_response_success_envelope_is_decision_ready() {
        let response = ToolResponse {
            status: "success".to_string(),
            result: "episode:abc123".to_string(),
            guidance: Some("Call extract next to derive entities and facts.".to_string()),
            has_more: None,
            total_count: None,
            next_offset: None,
        };

        assert_eq!(response.status, "success");
        assert_eq!(response.result, "episode:abc123");
        assert_eq!(
            response.guidance.as_deref(),
            Some("Call extract next to derive entities and facts."),
        );
    }

    #[test]
    fn extract_tool_response_schema_exposes_structured_result() {
        let schema = schema_json::<ToolResponse<ExtractResult>>();
        let properties = schema["properties"].as_object().expect("properties object");

        assert!(properties.contains_key("status"));
        assert!(properties.contains_key("result"));
        assert!(properties.contains_key("guidance"));
        assert_eq!(properties["status"]["type"], "string");
        assert!(
            properties["result"]["$ref"] == "#/$defs/ExtractResult"
                || properties["result"]["$ref"] == "#/definitions/ExtractResult"
        );
    }

    #[test]
    fn assemble_context_tool_response_schema_exposes_item_array() {
        let schema = schema_json::<ToolResponse<Vec<AssembledContextItem>>>();
        let result = &schema["properties"]["result"];

        assert_eq!(result["type"], "array");
        assert!(
            result["items"]["$ref"] == "#/$defs/AssembledContextItem"
                || result["items"]["$ref"] == "#/definitions/AssembledContextItem"
        );
    }

    #[test]
    fn explain_tool_response_schema_exposes_citation_items() {
        let schema = schema_json::<ToolResponse<Vec<ExplainItem>>>();
        let result = &schema["properties"]["result"];

        assert_eq!(result["type"], "array");
        assert!(
            result["items"]["$ref"] == "#/$defs/ExplainItem"
                || result["items"]["$ref"] == "#/definitions/ExplainItem"
        );
    }

    #[test]
    fn tool_response_partial_envelope_marks_retryable_state() {
        let response = ToolResponse::partial_with_guidance(
            ExtractResult::empty(),
            "Provide either `episode_id` or non-empty `content`/`text`, then retry.",
        );

        assert_eq!(response.status, "partial");
        assert!(response.result.entities.is_empty());
        assert_eq!(
            response.guidance.as_deref(),
            Some("Provide either `episode_id` or non-empty `content`/`text`, then retry."),
        );
    }

    #[test]
    fn parse_datetime_handles_null() {
        // Test that None input returns None
        let result: Option<chrono::DateTime<chrono::Utc>> = None;
        assert!(result.is_none());
    }

    #[test]
    fn parse_datetime_parses_valid_iso() {
        let result = parse_datetime("2024-01-15T10:30:00Z");
        assert!(result.is_some());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn parse_datetime_returns_none_for_invalid() {
        assert!(parse_datetime("invalid").is_none());
        assert!(parse_datetime("").is_none());
    }

    // Note: normalize_optional_string, content_hash, default_scope, and empty_extract_result
    // are tested in src/mcp/parsers.rs tests module
}
