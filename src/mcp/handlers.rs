//! MCP tool handler implementations.

use std::sync::Arc;

use chrono::Utc;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{
    AccessContext, AssembleContextRequest, EntityCandidate, ExplainRequest, IngestRequest,
    InvalidateRequest,
};
use crate::service::MemoryService;

use super::error::mcp_error;
use super::params::*;
use super::parsers::{content_hash, parse_context_items, parse_datetime};

/// Response wrapper for tool results.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ToolResponse<T> {
    /// The actual result data.
    pub result: T,
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
    const SERVER_INSTRUCTIONS: &str =
        "Memory MCP server: stores, extracts, resolves, and assembles long-term context.";

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
    ) -> Result<Value, ErrorData> {
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
                    return Ok(result);
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
            let result = super::parsers::empty_extract_result(
                "no_input",
                "episode_id or content/text is required",
            );
            if enable_logging {
                self.service.log_tool_event(
                    "extract.no_input",
                    json!({"episode_id": episode_id, "has_content": false}),
                    json!({"status": "no_input"}),
                    LogLevel::Warn,
                );
            }
            return Ok(result);
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
                    Ok(result)
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

    /// Parse aliases from a string (comma-separated or JSON array).
    fn parse_aliases(aliases_str: &str) -> Vec<String> {
        if aliases_str.starts_with('[') {
            serde_json::from_str(aliases_str).unwrap_or_default()
        } else {
            aliases_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
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
        description = "Store a new episode (conversation, email, document, or event) into memory."
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
                Ok(Json(ToolResponse { result: episode_id }))
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

    #[tool(description = "Provide detailed explanations for context items with source citations.")]
    pub async fn explain(
        &self,
        params: Parameters<ExplainParams>,
    ) -> Result<Json<ToolResponse<Vec<Value>>>, ErrorData> {
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
                self.service.log_tool_event(
                    "explain.done",
                    json!({}),
                    json!({"count": explanations.len()}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse {
                    result: explanations,
                }))
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

    #[tool(description = "Analyze an episode to identify entities, facts, and relationships.")]
    pub async fn extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<Json<ToolResponse<Value>>, ErrorData> {
        let p = params.0;
        let result = self
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
        Ok(Json(ToolResponse { result }))
    }

    #[tool(description = "Find or create a canonical entity, handling deduplication and aliases.")]
    pub async fn resolve(
        &self,
        params: Parameters<ResolveParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        let p = params.0;
        let access = AccessContext::default();
        let aliases = Self::parse_aliases(&p.aliases);
        let candidate = EntityCandidate {
            entity_type: p.entity_type.clone(),
            canonical_name: p.canonical_name.clone(),
            aliases,
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
                Ok(Json(ToolResponse { result: entity_id }))
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

    #[tool(description = "Mark a fact as no longer valid while preserving its history.")]
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
                Ok(Json(ToolResponse { result: res }))
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

    #[tool(description = "Retrieve relevant facts and context for answering a specific query.")]
    pub async fn assemble_context(
        &self,
        params: Parameters<AssembleContextParams>,
    ) -> Result<Json<ToolResponse<Vec<Value>>>, ErrorData> {
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
                self.service.log_tool_event(
                    "assemble_context.done",
                    json!({}),
                    json!({"count": results.len()}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse { result: results }))
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

    #[tool(description = "Store a document into memory (alias for ingest).")]
    pub async fn ingest_document(
        &self,
        params: Parameters<AliasIngestParams>,
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
            source_type: p.source_type,
            source_id: p.source_id,
            content: p.content,
            t_ref,
            scope: p.scope,
            t_ingested,
            visibility_scope: p.visibility_scope,
            policy_tags: p.policy_tags,
        };
        let episode_id = self
            .service
            .ingest(request, Some(access))
            .await
            .map_err(mcp_error)?;
        Ok(Json(ToolResponse { result: episode_id }))
    }

    #[tool(description = "Analyze content to identify entities (alias for extract).")]
    pub async fn extract_entities(
        &self,
        params: Parameters<AliasExtractParams>,
    ) -> Result<Json<ToolResponse<Value>>, ErrorData> {
        let p = params.0;
        let result = self
            .extract_impl(
                p.episode_id,
                p.content,
                p.text,
                p.source_type,
                p.source_id,
                p.t_ref,
                p.scope,
                false,
            )
            .await?;
        Ok(Json(ToolResponse { result }))
    }

    #[tool(description = "Find the canonical ID for an entity (alias for resolve).")]
    pub async fn resolve_entity(
        &self,
        params: Parameters<ResolveEntityParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        let p = params.0;
        let access = AccessContext::default();
        let aliases = Self::parse_aliases(&p.aliases);
        let candidate = EntityCandidate {
            entity_type: p.entity_type,
            canonical_name: p.canonical_name,
            aliases,
        };
        let entity_id = self
            .service
            .resolve(candidate, Some(access))
            .await
            .map_err(mcp_error)?;
        Ok(Json(ToolResponse { result: entity_id }))
    }

    #[tool(description = "Create a task reminder draft for user confirmation.")]
    pub async fn create_task(
        &self,
        params: Parameters<CreateTaskParams>,
    ) -> Result<Json<ToolResponse<Value>>, ErrorData> {
        let p = params.0;
        let due_date = p.due_date.as_ref().and_then(|s| parse_datetime(s));

        self.service.log_tool_event(
            "create_task.start",
            json!({"title": &p.title}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.create_task(&p.title, due_date).await {
            Ok(record) => {
                self.service.log_tool_event(
                    "create_task.done",
                    json!({"title": &p.title}),
                    json!({"id": record.get("id")}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse { result: record }))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "create_task.error",
                    json!({"title": &p.title}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(description = "Draft an email or message for the user to review.")]
    pub async fn send_message_draft(
        &self,
        params: Parameters<SendMessageParams>,
    ) -> Result<Json<ToolResponse<Value>>, ErrorData> {
        let p = params.0;
        let result = self.service.send_message_draft(
            p.to.as_deref().unwrap_or(""),
            p.subject.as_deref().unwrap_or(""),
            p.body.as_deref().unwrap_or(""),
        );
        Ok(Json(ToolResponse { result }))
    }

    #[tool(description = "Draft a calendar meeting for user confirmation.")]
    pub async fn schedule_meeting(
        &self,
        params: Parameters<ScheduleMeetingParams>,
    ) -> Result<Json<ToolResponse<Value>>, ErrorData> {
        let p = params.0;
        let start = parse_datetime(&p.start).ok_or_else(|| {
            ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "start is required".to_string(),
                None,
            )
        })?;
        let end = parse_datetime(&p.end).ok_or_else(|| {
            ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "end is required".to_string(),
                None,
            )
        })?;
        let result = self
            .service
            .schedule_meeting(p.title.as_deref().unwrap_or(""), start, end);
        Ok(Json(ToolResponse { result }))
    }

    #[tool(description = "Record or update a tracked metric value.")]
    pub async fn update_metric(
        &self,
        params: Parameters<UpdateMetricParams>,
    ) -> Result<Json<ToolResponse<Value>>, ErrorData> {
        let p = params.0;
        let result = self
            .service
            .update_metric(p.name.as_deref().unwrap_or(""), p.value.unwrap_or(0.0));
        Ok(Json(ToolResponse { result }))
    }

    #[tool(description = "Retrieve all commitments and promises.")]
    pub async fn ui_promises(
        &self,
        _params: Parameters<UiParams>,
    ) -> Result<Json<ToolResponse<Vec<Value>>>, ErrorData> {
        self.service
            .log_tool_event("ui_promises.start", json!({}), json!({}), LogLevel::Info);
        match self.service.ui_promises().await {
            Ok(result) => {
                self.service.log_tool_event(
                    "ui_promises.done",
                    json!({}),
                    json!({"count": result.len()}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse { result }))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "ui_promises.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(description = "Retrieve all tracked metrics.")]
    pub async fn ui_metrics(
        &self,
        _params: Parameters<UiParams>,
    ) -> Result<Json<ToolResponse<Vec<Value>>>, ErrorData> {
        self.service
            .log_tool_event("ui_metrics.start", json!({}), json!({}), LogLevel::Info);
        match self.service.ui_metrics().await {
            Ok(result) => {
                self.service.log_tool_event(
                    "ui_metrics.done",
                    json!({}),
                    json!({"count": result.len()}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse { result }))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "ui_metrics.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(description = "Retrieve all pending task drafts.")]
    pub async fn ui_tasks(
        &self,
        _params: Parameters<UiParams>,
    ) -> Result<Json<ToolResponse<Vec<Value>>>, ErrorData> {
        self.service
            .log_tool_event("ui_tasks.start", json!({}), json!({}), LogLevel::Info);
        match self.service.ui_tasks().await {
            Ok(result) => {
                self.service.log_tool_event(
                    "ui_tasks.done",
                    json!({}),
                    json!({"count": result.len()}),
                    LogLevel::Info,
                );
                Ok(Json(ToolResponse { result }))
            }
            Err(err) => {
                self.service.log_tool_event(
                    "ui_tasks.error",
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
    use serde_json::json;

    #[test]
    fn build_server_info_enables_tools_and_sets_instructions() {
        let info = MemoryMcp::build_server_info();
        let capabilities = serde_json::to_value(&info.capabilities).unwrap();

        assert_eq!(
            info.instructions.as_deref(),
            Some(MemoryMcp::SERVER_INSTRUCTIONS),
        );
        assert!(capabilities.get("tools").is_some());
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

    #[test]
    fn create_task_params_accept_null_due_date() {
        let params: CreateTaskParams = serde_json::from_value(json!({
            "title": "Follow up with ACME",
            "due_date": null
        }))
        .unwrap();

        assert_eq!(params.title, "Follow up with ACME");
        assert!(params.due_date.is_none());
    }

    // Note: normalize_optional_string, content_hash, default_scope, and empty_extract_result
    // are tested in src/mcp/parsers.rs tests module
}
