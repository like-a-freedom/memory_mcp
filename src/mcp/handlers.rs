//! MCP tool handler implementations.

use std::sync::Arc;

use chrono::Utc;
use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ErrorCode, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, ResourceContents, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_router};
use serde_json::json;

use crate::logging::LogLevel;
use crate::models::{
    AccessContext, AssembleContextRequest, AssembledContextItem, EntityCandidate, ExplainItem,
    ExplainRequest, ExtractResult, IngestRequest, InvalidateRequest,
};
use crate::service::MemoryService;
use crate::timing::OperationTimer;

use super::error::mcp_error;
use super::params::*;
use super::parsers::{content_hash, parse_context_items, parse_datetime};
use super::resources::{
    APPS_INDEX_URI, app_catalog_resources, app_resource_templates, app_root_payload,
    app_session_html_document, app_session_uri, apps_index_payload, parse_app_root_uri,
    parse_app_session_uri,
};

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

/// Result payload returned by the public `open_app` launcher.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct OpenAppResult {
    /// Canonical public app identifier.
    pub app: String,
    /// Created or reused session identifier.
    pub session_id: String,
    /// Session-backed resource URI for reading the current view.
    pub resource_uri: String,
    /// Immediate JSON fallback payload for clients that do not read resources yet.
    pub fallback: serde_json::Value,
}

/// Result payload returned by the public `app_command` bridge.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct AppCommandResult {
    /// Canonical public app identifier.
    pub app: String,
    /// Target session identifier.
    pub session_id: String,
    /// Canonical action name.
    pub action: String,
    /// Whether the command completed successfully.
    pub ok: bool,
    /// Human-readable outcome message.
    pub message: String,
    /// Whether callers should re-read the session resource.
    pub refresh_required: bool,
    /// Resource URI to re-read when `refresh_required` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    /// Raw service details for clients that need extra metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
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
    const PUBLIC_TOOL_NAMES: [&str; 8] = [
        "assemble_context",
        "explain",
        "extract",
        "ingest",
        "invalidate",
        "resolve",
        "open_app",
        "app_command",
    ];

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
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_instructions(Self::SERVER_INSTRUCTIONS)
    }

    fn is_public_tool_name(name: &str) -> bool {
        Self::PUBLIC_TOOL_NAMES.contains(&name)
    }

    fn public_tools(&self) -> Vec<Tool> {
        self.tool_router
            .list_all()
            .into_iter()
            .filter(|tool| Self::is_public_tool_name(&tool.name))
            .collect()
    }

    fn invalid_params(message: impl Into<String>) -> ErrorData {
        ErrorData::new(ErrorCode::INVALID_PARAMS, message.into(), None)
    }

    fn missing_app_field(app: &str, field: &str) -> ErrorData {
        ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            format!(
                "`{field}` is required for {app}. Re-check the open_app contract for that app and retry."
            ),
            None,
        )
    }

    fn internal_error(message: impl Into<String>) -> ErrorData {
        ErrorData::new(ErrorCode::INTERNAL_ERROR, message.into(), None)
    }

    fn list_resources_result() -> ListResourcesResult {
        ListResourcesResult {
            resources: app_catalog_resources(),
            meta: None,
            next_cursor: None,
        }
    }

    fn list_resource_templates_result() -> ListResourceTemplatesResult {
        ListResourceTemplatesResult {
            resource_templates: app_resource_templates(),
            meta: None,
            next_cursor: None,
        }
    }

    fn normalize_public_app_name(app: &str) -> Option<&'static str> {
        match app {
            "inspector" | "memory_inspector" => Some("inspector"),
            "diff" | "temporal_diff" => Some("diff"),
            "ingestion_review" | "ingestion" => Some("ingestion_review"),
            "lifecycle" | "lifecycle_console" => Some("lifecycle"),
            "graph" | "graph_path" => Some("graph"),
            _ => None,
        }
    }

    fn session_matches_public_app(public_app: &str, app_id: &str) -> bool {
        match public_app {
            "inspector" => matches!(app_id, "inspector" | "memory_inspector"),
            "diff" => app_id == "temporal_diff",
            "ingestion_review" => app_id == "ingestion_review",
            "lifecycle" => app_id == "lifecycle_console",
            "graph" => app_id == "graph_path",
            _ => false,
        }
    }

    fn open_app_result(
        app: &str,
        session_id: impl Into<String>,
        fallback: serde_json::Value,
    ) -> OpenAppResult {
        let session_id = session_id.into();
        OpenAppResult {
            app: app.to_string(),
            resource_uri: app_session_uri(app, &session_id),
            session_id,
            fallback,
        }
    }

    fn app_command_result_from_details(
        app: &str,
        session_id: &str,
        action: &str,
        resource_uri: Option<String>,
        details: serde_json::Value,
    ) -> AppCommandResult {
        AppCommandResult {
            app: app.to_string(),
            session_id: session_id.to_string(),
            action: action.to_string(),
            ok: details
                .get("ok")
                .and_then(|value| value.as_bool())
                .unwrap_or(true),
            message: details
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("App command completed")
                .to_string(),
            refresh_required: details
                .get("refresh_required")
                .and_then(|value| value.as_bool())
                .unwrap_or(resource_uri.is_some()),
            resource_uri,
            details: Some(details),
        }
    }

    fn public_app_name_for_session_app_id(app_id: &str) -> Option<&'static str> {
        match app_id {
            "inspector" | "memory_inspector" => Some("inspector"),
            "temporal_diff" => Some("diff"),
            "ingestion_review" => Some("ingestion_review"),
            "lifecycle_console" => Some("lifecycle"),
            "graph_path" => Some("graph"),
            _ => None,
        }
    }

    fn ingestion_review_draft_id(session: &crate::models::AppSession) -> Result<&str, ErrorData> {
        if session.app_id != "ingestion_review" {
            return Err(Self::invalid_params(
                "Session is not an ingestion_review session",
            ));
        }

        session
            .target
            .get("draft_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| Self::internal_error("Ingestion review session missing draft_id"))
    }

    async fn load_ingestion_review_items(
        &self,
        session: &crate::models::AppSession,
    ) -> Result<Vec<crate::models::DraftItem>, ErrorData> {
        let draft_id = Self::ingestion_review_draft_id(session)?;
        let namespace = self
            .service
            .resolve_namespace_for_scope(&session.scope)
            .map_err(mcp_error)?;

        crate::service::apps::ingestion::IngestionReview::get_draft_items(
            draft_id,
            &*self.service.db_client,
            &namespace,
        )
        .await
        .map_err(mcp_error)
    }

    async fn load_inspector_view(
        &self,
        scope: &str,
        target_type: &str,
        target_id: &str,
        page_size: usize,
        cursor: Option<&str>,
    ) -> Result<serde_json::Value, ErrorData> {
        match target_type {
            "entity" => self
                .service
                .open_inspector_entity(target_id, scope, page_size, cursor)
                .await
                .map_err(mcp_error),
            "fact" => self
                .service
                .open_inspector_fact(target_id, scope)
                .await
                .map_err(mcp_error),
            "episode" => self
                .service
                .open_inspector_episode(target_id, scope, page_size, cursor)
                .await
                .map_err(mcp_error),
            _ => Err(Self::invalid_params(format!(
                "Invalid target_type: {target_type}. Must be entity, fact, or episode."
            ))),
        }
    }

    async fn open_inspector_app(&self, params: &OpenAppParams) -> Result<OpenAppResult, ErrorData> {
        let target_type = params
            .target_type
            .as_deref()
            .ok_or_else(|| Self::missing_app_field("inspector", "target_type"))?;
        let target_id = params
            .target_id
            .as_deref()
            .ok_or_else(|| Self::missing_app_field("inspector", "target_id"))?;
        let page_size = params.page_size.unwrap_or(20).max(1) as usize;
        let view = self
            .load_inspector_view(
                &params.scope,
                target_type,
                target_id,
                page_size,
                params.cursor.as_deref(),
            )
            .await?;

        let target = json!({
            "target_type": target_type,
            "target_id": target_id,
            "page_size": page_size,
            "cursor": params.cursor.clone(),
            "as_of": params.as_of.clone(),
            "view": view.clone(),
        });

        let session = self
            .service
            .app_session_manager
            .create_session(
                "inspector",
                &params.scope,
                json!({}),
                target.clone(),
                params.ttl_seconds,
            )
            .await
            .map_err(mcp_error)?;

        self.service
            .app_session_manager
            .update_session_state(&session.session_id, "ready", target)
            .await
            .map_err(mcp_error)?;

        Ok(Self::open_app_result(
            "inspector",
            &session.session_id,
            view,
        ))
    }

    async fn wrap_session_app_result(
        &self,
        app: &str,
        fallback: serde_json::Value,
    ) -> Result<OpenAppResult, ErrorData> {
        let session_id = fallback
            .get("session_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                Self::internal_error(format!(
                    "{app} opener did not return a `session_id` in its fallback payload"
                ))
            })?
            .to_string();

        let session = self
            .service
            .app_session_manager
            .get_session(&session_id)
            .await
            .map_err(mcp_error)?;

        self.service
            .app_session_manager
            .update_session_state(&session_id, "ready", session.target)
            .await
            .map_err(mcp_error)?;

        Ok(Self::open_app_result(app, session_id, fallback))
    }

    async fn read_app_resource_payload(
        &self,
        app: &str,
        session_id: &str,
    ) -> Result<serde_json::Value, ErrorData> {
        let public_app = Self::normalize_public_app_name(app)
            .ok_or_else(|| Self::invalid_params(format!("Unknown app resource: {app}")))?;
        let session = self
            .service
            .app_session_manager
            .get_session(session_id)
            .await
            .map_err(mcp_error)?;

        if !Self::session_matches_public_app(public_app, &session.app_id) {
            return Err(Self::invalid_params(format!(
                "Session {session_id} is not a {public_app} session"
            )));
        }

        let payload = match public_app {
            "inspector" => {
                let target_type = session
                    .target
                    .get("target_type")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| Self::internal_error("Inspector session missing target_type"))?;
                let target_id = session
                    .target
                    .get("target_id")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| Self::internal_error("Inspector session missing target_id"))?;
                let page_size = session
                    .target
                    .get("page_size")
                    .and_then(|value| value.as_i64())
                    .unwrap_or(20)
                    .max(1) as usize;
                let cursor = session
                    .target
                    .get("cursor")
                    .and_then(|value| value.as_str());
                let view = self
                    .load_inspector_view(&session.scope, target_type, target_id, page_size, cursor)
                    .await?;

                json!({
                    "app": "inspector",
                    "session_id": session.session_id,
                    "scope": session.scope,
                    "state": session.state,
                    "view": view,
                })
            }
            "diff" => json!({
                "app": "diff",
                "session_id": session.session_id,
                "scope": session.scope,
                "state": session.state,
                "view": session
                    .target
                    .get("result")
                    .cloned()
                    .unwrap_or_else(|| session.target.clone()),
            }),
            "ingestion_review" => json!({
                "app": "ingestion_review",
                "session_id": session.session_id,
                "scope": session.scope,
                "state": session.state,
                "draft_id": session.target.get("draft_id").cloned(),
                "summary": self.service.get_draft_summary(session_id).await.map_err(mcp_error)?,
                "items": self.load_ingestion_review_items(&session).await?,
            }),
            "lifecycle" => json!({
                "app": "lifecycle",
                "session_id": session.session_id,
                "scope": session.scope,
                "state": session.state,
                "view": self
                    .service
                    .get_lifecycle_dashboard(session_id)
                    .await
                    .map_err(mcp_error)?,
            }),
            "graph" => json!({
                "app": "graph",
                "session_id": session.session_id,
                "scope": session.scope,
                "state": session.state,
                "view": session
                    .target
                    .get("result")
                    .cloned()
                    .unwrap_or_else(|| session.target.clone()),
            }),
            _ => {
                return Err(Self::invalid_params(format!(
                    "Unknown app resource: {public_app}"
                )));
            }
        };

        self.service
            .app_session_manager
            .touch_session(session_id)
            .await
            .map_err(mcp_error)?;

        Ok(payload)
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
    ) -> Result<ToolResponse<ExtractResult>, ErrorData> {
        use super::parsers::normalize_optional_string;

        let access = AccessContext::default();
        let episode_id = normalize_optional_string(episode_id);
        let content = normalize_optional_string(content);
        let text = normalize_optional_string(text);

        self.service.log_tool_event(
            "extract.start",
            json!({"episode_id": &episode_id, "has_content": content.is_some() || text.is_some()}),
            json!({}),
            LogLevel::Info,
        );

        if let Some(ref episode_id) = episode_id {
            match self.service.extract(episode_id, Some(access)).await {
                Ok(result) => {
                    self.service.log_tool_event(
                        "extract.done",
                        json!({"episode_id": episode_id}),
                        json!({"entities": result.entities.len(), "facts": result.facts.len()}),
                        LogLevel::Info,
                    );
                    return Ok(ToolResponse::success_with_guidance(
                        result,
                        "Resolve canonical entities for any ambiguous names before creating manual links.",
                    ));
                }
                Err(err) => {
                    self.service.log_tool_event(
                        "extract.error",
                        json!({"episode_id": episode_id}),
                        json!({"error": err.to_string()}),
                        LogLevel::Warn,
                    );
                    return Err(mcp_error(err));
                }
            }
        }

        let content = content.or(text).unwrap_or_default();
        if content.trim().is_empty() {
            self.service.log_tool_event(
                "extract.no_input",
                json!({"episode_id": &episode_id, "has_content": false}),
                json!({"status": "no_input"}),
                LogLevel::Warn,
            );
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
                    self.service.log_tool_event(
                        "extract.done",
                        json!({"episode_id": &episode_id}),
                        json!({"entities": result.entities.len(), "facts": result.facts.len()}),
                        LogLevel::Info,
                    );
                    Ok(ToolResponse::success_with_guidance(
                        result,
                        "Resolve canonical entities for any ambiguous names before creating manual links.",
                    ))
                }
                Err(err) => {
                    self.service.log_tool_event(
                        "extract.error",
                        json!({}),
                        json!({"error": err.to_string()}),
                        LogLevel::Warn,
                    );
                    Err(mcp_error(err))
                }
            },
            Err(err) => {
                self.service.log_tool_event(
                    "extract.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(mcp_error(err))
            }
        }
    }
}

impl ServerHandler for MemoryMcp {
    fn get_info(&self) -> ServerInfo {
        Self::build_server_info()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if !Self::is_public_tool_name(&request.name) {
            return Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("Unknown tool: {}", request.name),
                None,
            ));
        }

        let correlation_id = crate::correlation::CorrelationId::new();
        let logger = self.service.logger.with_correlation_id(correlation_id);

        logger.log(
            crate::log_event!("tool_call", "start", "tool" => &request.name),
            crate::logging::LogLevel::Debug,
        );

        let tool_context = ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_context).await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.public_tools(),
            meta: None,
            next_cursor: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Self::is_public_tool_name(name)
            .then(|| self.tool_router.get(name).cloned())
            .flatten()
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(Self::list_resources_result())
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(Self::list_resource_templates_result())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.read_resource_result(request).await
    }
}

#[tool_router]
impl MemoryMcp {
    async fn read_resource_result(
        &self,
        request: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, ErrorData> {
        if request.uri == APPS_INDEX_URI {
            let body = serde_json::to_string_pretty(&apps_index_payload())
                .map_err(|error| Self::internal_error(error.to_string()))?;

            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(body, request.uri).with_mime_type("application/json"),
            ]));
        }

        if let Some(app) = parse_app_root_uri(&request.uri) {
            let payload = app_root_payload(&app).ok_or_else(|| {
                Self::invalid_params(format!("Unknown app root resource: {}", request.uri))
            })?;
            let body = serde_json::to_string_pretty(&payload)
                .map_err(|error| Self::internal_error(error.to_string()))?;

            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(body, request.uri).with_mime_type("application/json"),
            ]));
        }

        if let Some((app, session_id)) = parse_app_session_uri(&request.uri) {
            let payload = self.read_app_resource_payload(&app, &session_id).await?;
            let body = app_session_html_document(&app, &payload);

            return Ok(ReadResourceResult::new(vec![
                ResourceContents::text(body, request.uri)
                    .with_mime_type("text/html;profile=mcp-app"),
            ]));
        }

        Err(Self::invalid_params(format!(
            "Unknown resource URI: {}",
            request.uri
        )))
    }

    #[tool(
        description = "Store a new episode in long-term memory. Use this tool when you need to persist raw source text as memory before any downstream extraction. Do NOT use this tool for retrieval. Arguments must include ISO 8601 `t_ref` and a memory `scope`. Returns the created or existing `episode_id`, plus `guidance` telling the agent what to do next."
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

        let timer = OperationTimer::new("ingest");
        self.service.log_tool_event(
            "ingest.start",
            json!({"source_type": p.source_type, "source_id": p.source_id, "scope": p.scope}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.ingest(request, Some(access)).await {
            Ok(episode_id) => {
                self.service.log_tool_event_with_duration(
                    "ingest.done",
                    json!({"source_id": p.source_id}),
                    json!({"episode_id": &episode_id}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    episode_id,
                    "Call extract next to derive entities and facts.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "ingest.error",
                    json!({"source_id": p.source_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Explain context items with provenance-ready citations. Use this tool when you already have selected context items and need source snippets for a final answer. Do NOT use this tool to search memory. `context_items` must be a JSON-encoded array string. Accepted item forms: source ID strings, objects with `source_episode`, or objects with `id`. Returns citation-ready items and `guidance`."
    )]
    pub async fn explain(
        &self,
        params: Parameters<ExplainParams>,
    ) -> Result<Json<ToolResponse<Vec<ExplainItem>>>, ErrorData> {
        let access = AccessContext::default();
        let context_pack = parse_context_items(&params.0.context_items)
            .map_err(|msg| ErrorData::new(rmcp::model::ErrorCode::INVALID_PARAMS, msg, None))?;
        let request = ExplainRequest { context_pack };

        let timer = OperationTimer::new("explain");
        self.service.log_tool_event(
            "explain.start",
            json!({"count": request.context_pack.len()}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.explain(request, Some(access)).await {
            Ok(explanations) => {
                self.service.log_tool_event_with_duration(
                    "explain.done",
                    json!({}),
                    json!({"count": explanations.len()}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                let count = explanations.len();
                Ok(Json(ToolResponse::complete_list(
                    explanations,
                    count,
                    "Use these citations directly in the final response.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "explain.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Extract entities, facts, and relationships from remembered content. Use this tool when you need structured information from an existing `episode_id` or from new inline `content`/`text`. Do NOT use this tool for retrieval. If inline content is provided, the server ingests it first and then extracts. Returns extracted entities, facts, links, and `guidance` for the next step."
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

        let timer = OperationTimer::new("resolve");
        self.service.log_tool_event(
            "resolve.start",
            json!({"entity_type": candidate.entity_type, "canonical": candidate.canonical_name}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.resolve(candidate, Some(access)).await {
            Ok(entity_id) => {
                self.service.log_tool_event_with_duration(
                    "resolve.done",
                    json!({}),
                    json!({"entity_id": &entity_id}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    entity_id,
                    "Use this entity_id when linking facts or relationships.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "resolve.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
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

        let timer = OperationTimer::new("invalidate");
        self.service.log_tool_event(
            "invalidate.start",
            json!({"fact_id": request.fact_id}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.invalidate(request, Some(access)).await {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "invalidate.done",
                    json!({"fact_id": p.fact_id}),
                    json!({"result": res}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Re-run assemble_context with a fresh `as_of` timestamp to confirm the fact is no longer active.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "invalidate.error",
                    json!({"fact_id": p.fact_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
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
        let window_start = p.window_start.as_deref().and_then(parse_datetime);
        let window_end = p.window_end.as_deref().and_then(parse_datetime);
        let request = AssembleContextRequest {
            query: p.query.clone(),
            scope: p.scope.clone(),
            as_of,
            budget: p.budget,
            view_mode: p.view_mode.clone(),
            window_start,
            window_end,
            access: None,
        };

        let timer = OperationTimer::new("assemble_context");
        self.service.log_tool_event(
            "assemble_context.start",
            json!({"scope": request.scope, "query": request.query}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.assemble_context(request).await {
            Ok(results) => {
                self.service.log_tool_event_with_duration(
                    "assemble_context.done",
                    json!({}),
                    json!({"count": results.len()}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                let count = results.len();
                Ok(Json(ToolResponse::complete_list(
                    results,
                    count,
                    "Call explain if you need provenance-ready citations for selected items.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "assemble_context.error",
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Open a Memory MCP app through the minimal public launcher. Use this tool only when an interactive app workflow is required and no canonical memory tool already matches the intent. Required fields depend on `app`: inspector -> `target_type` + `target_id`; diff -> `as_of_left` + `as_of_right`; graph -> `from_entity_id` + `to_entity_id`; ingestion_review -> `scope` plus optional `source_text` or `draft_episode_id`; lifecycle -> `scope` only. Returns `session_id`, `resource_uri`, `fallback`, and `guidance`."
    )]
    pub async fn open_app(
        &self,
        params: Parameters<OpenAppParams>,
    ) -> Result<Json<ToolResponse<OpenAppResult>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("open_app");
        let app = Self::normalize_public_app_name(&p.app)
            .ok_or_else(|| Self::invalid_params(format!("Unknown app: {}", p.app)))?;

        self.service.log_tool_event(
            "open_app.start",
            json!({"app": app, "scope": p.scope}),
            json!({}),
            LogLevel::Info,
        );

        let result = match app {
            "inspector" => self.open_inspector_app(&p).await,
            "diff" => {
                let target_type = p
                    .target_type
                    .as_deref()
                    .unwrap_or(if p.target_id.is_some() {
                        "entity"
                    } else {
                        "scope"
                    });
                let as_of_left = p
                    .as_of_left
                    .as_deref()
                    .ok_or_else(|| Self::missing_app_field("diff", "as_of_left"))?;
                let as_of_right = p
                    .as_of_right
                    .as_deref()
                    .ok_or_else(|| Self::missing_app_field("diff", "as_of_right"))?;
                let fallback = self
                    .service
                    .open_temporal_diff(
                        &p.scope,
                        target_type,
                        p.target_id.as_deref(),
                        as_of_left,
                        as_of_right,
                        p.time_axis.as_deref().unwrap_or("valid"),
                        None,
                    )
                    .await
                    .map_err(mcp_error)?;
                self.wrap_session_app_result("diff", fallback).await
            }
            "ingestion_review" => {
                let fallback = self
                    .service
                    .open_ingestion_review(
                        &p.scope,
                        p.source_text.as_deref(),
                        p.draft_episode_id.as_deref(),
                        p.ttl_seconds,
                    )
                    .await
                    .map_err(mcp_error)?;
                self.wrap_session_app_result("ingestion_review", fallback)
                    .await
            }
            "lifecycle" => {
                let fallback = self
                    .service
                    .open_lifecycle_console(&p.scope, None)
                    .await
                    .map_err(mcp_error)?;
                self.wrap_session_app_result("lifecycle", fallback).await
            }
            "graph" => {
                let from_entity_id = p
                    .from_entity_id
                    .as_deref()
                    .ok_or_else(|| Self::missing_app_field("graph", "from_entity_id"))?;
                let to_entity_id = p
                    .to_entity_id
                    .as_deref()
                    .ok_or_else(|| Self::missing_app_field("graph", "to_entity_id"))?;
                let fallback = self
                    .service
                    .open_graph_path(
                        &p.scope,
                        from_entity_id,
                        to_entity_id,
                        p.as_of.as_deref(),
                        p.max_depth.unwrap_or(4).max(1),
                    )
                    .await
                    .map_err(mcp_error)?;
                self.wrap_session_app_result("graph", fallback).await
            }
            _ => Err(Self::invalid_params(format!("Unknown app: {}", p.app))),
        };

        match result {
            Ok(opened) => {
                self.service.log_tool_event_with_duration(
                    "open_app.done",
                    json!({"app": app}),
                    json!({"session_id": opened.session_id, "resource_uri": opened.resource_uri}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    opened,
                    "Read the returned `resource_uri` to retrieve the current app view. Prefer canonical memory tools when the business intent already matches them.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "open_app.error",
                    json!({"app": app}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(err)
            }
        }
    }

    #[tool(
        description = "Execute a coarse-grained command for an app session opened via open_app. Use this only for session-scoped workflows that are not already covered by canonical memory tools. Supports ingestion review actions (`approve_items`, `reject_items`, `edit_item`, `commit_review`, `cancel_review`), lifecycle actions (`archive_candidates`, `restore_archived`, `recompute_decay`, `rebuild_communities`), diff export (`export_diff`), graph exploration actions (`expand_neighbors`, `open_edge_details`, `use_path_as_context`), and the generic `close_session`. Returns command status and whether the caller should re-read the app resource."
    )]
    pub async fn app_command(
        &self,
        params: Parameters<AppCommandParams>,
    ) -> Result<Json<ToolResponse<AppCommandResult>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("app_command");
        self.service.log_tool_event(
            "app_command.start",
            json!({"session_id": p.session_id, "action": p.action}),
            json!({}),
            LogLevel::Info,
        );

        let session = self
            .service
            .app_session_manager
            .get_session(&p.session_id)
            .await
            .map_err(mcp_error)?;
        let app = Self::public_app_name_for_session_app_id(&session.app_id).ok_or_else(|| {
            Self::invalid_params(format!(
                "Session {} belongs to unsupported app {}",
                p.session_id, session.app_id
            ))
        })?;

        let result = match p.action.as_str() {
            "approve_items" | "approve_ingestion_items" => {
                if app != "ingestion_review" {
                    Err(Self::invalid_params(
                        "approve_items is only supported for ingestion_review sessions",
                    ))
                } else if p.item_ids.is_empty() {
                    Err(Self::invalid_params(
                        "`item_ids` is required for approve_items",
                    ))
                } else {
                    let details = self
                        .service
                        .approve_ingestion_items(&p.session_id, &p.item_ids)
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "approve_items",
                        Some(app_session_uri(app, &p.session_id)),
                        details,
                    ))
                }
            }
            "reject_items" | "reject_ingestion_items" => {
                if app != "ingestion_review" {
                    Err(Self::invalid_params(
                        "reject_items is only supported for ingestion_review sessions",
                    ))
                } else if p.item_ids.is_empty() {
                    Err(Self::invalid_params(
                        "`item_ids` is required for reject_items",
                    ))
                } else {
                    let details = self
                        .service
                        .reject_ingestion_items(&p.session_id, &p.item_ids, p.reason.as_deref())
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "reject_items",
                        Some(app_session_uri(app, &p.session_id)),
                        details,
                    ))
                }
            }
            "edit_item" => {
                if app != "ingestion_review" {
                    Err(Self::invalid_params(
                        "edit_item is only supported for ingestion_review sessions",
                    ))
                } else {
                    let item_id = p
                        .item_id
                        .as_deref()
                        .ok_or_else(|| Self::missing_app_field("edit_item", "item_id"))?;
                    let patch_json = p
                        .patch_json
                        .as_deref()
                        .ok_or_else(|| Self::missing_app_field("edit_item", "patch_json"))?;
                    let patch_value: serde_json::Value =
                        serde_json::from_str(patch_json).map_err(|err| {
                            Self::invalid_params(format!(
                                "`patch_json` must be a valid JSON object: {err}"
                            ))
                        })?;
                    if !patch_value.is_object() {
                        return Err(Self::invalid_params(
                            "`patch_json` must encode a JSON object",
                        ));
                    }

                    let details = self
                        .service
                        .edit_ingestion_item(&p.session_id, item_id, &patch_value)
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "edit_item",
                        Some(app_session_uri(app, &p.session_id)),
                        details,
                    ))
                }
            }
            "commit_review" | "commit_ingestion_review" => {
                if app != "ingestion_review" {
                    Err(Self::invalid_params(
                        "commit_review is only supported for ingestion_review sessions",
                    ))
                } else {
                    let details = self
                        .service
                        .commit_ingestion_review(&p.session_id)
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "commit_review",
                        None,
                        details,
                    ))
                }
            }
            "cancel_review" | "cancel_ingestion_review" => {
                if app != "ingestion_review" {
                    Err(Self::invalid_params(
                        "cancel_review is only supported for ingestion_review sessions",
                    ))
                } else {
                    let details = self
                        .service
                        .cancel_ingestion_review(&p.session_id)
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "cancel_review",
                        None,
                        details,
                    ))
                }
            }
            "archive_candidates" => {
                if app != "lifecycle" {
                    Err(Self::invalid_params(
                        "archive_candidates is only supported for lifecycle sessions",
                    ))
                } else if p.target_ids.is_empty() {
                    Err(Self::invalid_params(
                        "`target_ids` is required for archive_candidates",
                    ))
                } else if !p.dry_run.unwrap_or(false) && !p.confirmed.unwrap_or(false) {
                    Err(Self::invalid_params(
                        "archive_candidates requires `confirmed=true` unless `dry_run=true`",
                    ))
                } else {
                    let details = self
                        .service
                        .archive_candidates(
                            &p.session_id,
                            &p.target_ids,
                            p.dry_run.unwrap_or(false),
                        )
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "archive_candidates",
                        Some(app_session_uri(app, &p.session_id)),
                        details,
                    ))
                }
            }
            "restore_archived" => {
                if app != "lifecycle" {
                    Err(Self::invalid_params(
                        "restore_archived is only supported for lifecycle sessions",
                    ))
                } else if p.target_ids.is_empty() {
                    Err(Self::invalid_params(
                        "`target_ids` is required for restore_archived",
                    ))
                } else if !p.confirmed.unwrap_or(false) {
                    Err(Self::invalid_params(
                        "restore_archived requires `confirmed=true`",
                    ))
                } else {
                    let details = self
                        .service
                        .restore_archived(&p.session_id, &p.target_ids)
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "restore_archived",
                        Some(app_session_uri(app, &p.session_id)),
                        details,
                    ))
                }
            }
            "recompute_decay" => {
                if app != "lifecycle" {
                    Err(Self::invalid_params(
                        "recompute_decay is only supported for lifecycle sessions",
                    ))
                } else if !p.dry_run.unwrap_or(false) && !p.confirmed.unwrap_or(false) {
                    Err(Self::invalid_params(
                        "recompute_decay requires `confirmed=true` unless `dry_run=true`",
                    ))
                } else {
                    let details = self
                        .service
                        .recompute_decay(
                            &p.session_id,
                            (!p.target_ids.is_empty()).then_some(p.target_ids.as_slice()),
                            p.dry_run.unwrap_or(false),
                        )
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "recompute_decay",
                        Some(app_session_uri(app, &p.session_id)),
                        details,
                    ))
                }
            }
            "rebuild_communities" => {
                if app != "lifecycle" {
                    Err(Self::invalid_params(
                        "rebuild_communities is only supported for lifecycle sessions",
                    ))
                } else if !p.dry_run.unwrap_or(false) && !p.confirmed.unwrap_or(false) {
                    Err(Self::invalid_params(
                        "rebuild_communities requires `confirmed=true` unless `dry_run=true`",
                    ))
                } else {
                    let details = self
                        .service
                        .rebuild_communities(&p.session_id, p.dry_run.unwrap_or(false))
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "rebuild_communities",
                        Some(app_session_uri(app, &p.session_id)),
                        details,
                    ))
                }
            }
            "export_diff" => {
                if app != "diff" {
                    Err(Self::invalid_params(
                        "export_diff is only supported for diff sessions",
                    ))
                } else {
                    let format = p
                        .format
                        .as_deref()
                        .ok_or_else(|| Self::missing_app_field("export_diff", "format"))?;
                    let details = self
                        .service
                        .export_temporal_diff(&p.session_id, format)
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "export_diff",
                        Some(app_session_uri(app, &p.session_id)),
                        details,
                    ))
                }
            }
            "expand_neighbors" => {
                if app != "graph" {
                    Err(Self::invalid_params(
                        "expand_neighbors is only supported for graph sessions",
                    ))
                } else {
                    let target_id = p
                        .target_id
                        .as_deref()
                        .ok_or_else(|| Self::missing_app_field("expand_neighbors", "target_id"))?;
                    let direction = p
                        .direction
                        .as_deref()
                        .ok_or_else(|| Self::missing_app_field("expand_neighbors", "direction"))?;
                    let details = self
                        .service
                        .expand_graph_neighbors(
                            &p.session_id,
                            target_id,
                            direction,
                            p.depth.unwrap_or(1),
                        )
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "expand_neighbors",
                        None,
                        details,
                    ))
                }
            }
            "open_edge_details" => {
                if app != "graph" {
                    Err(Self::invalid_params(
                        "open_edge_details is only supported for graph sessions",
                    ))
                } else {
                    let target_id = p
                        .target_id
                        .as_deref()
                        .ok_or_else(|| Self::missing_app_field("open_edge_details", "target_id"))?;
                    let details = self
                        .service
                        .open_edge_details(&p.session_id, target_id)
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "open_edge_details",
                        None,
                        details,
                    ))
                }
            }
            "use_path_as_context" => {
                if app != "graph" {
                    Err(Self::invalid_params(
                        "use_path_as_context is only supported for graph sessions",
                    ))
                } else {
                    let path_id = p.target_id.as_deref().unwrap_or("current");
                    let details = self
                        .service
                        .use_path_as_context(&p.session_id, path_id)
                        .await
                        .map_err(mcp_error)?;
                    Ok(Self::app_command_result_from_details(
                        app,
                        &p.session_id,
                        "use_path_as_context",
                        None,
                        details,
                    ))
                }
            }
            "close_session" => {
                self.service
                    .close_app_session(&p.session_id)
                    .await
                    .map_err(mcp_error)?;
                Ok(AppCommandResult {
                    app: app.to_string(),
                    session_id: p.session_id.clone(),
                    action: "close_session".to_string(),
                    ok: true,
                    message: "Session closed".to_string(),
                    refresh_required: false,
                    resource_uri: None,
                    details: None,
                })
            }
            _ => Err(Self::invalid_params(format!(
                "Unsupported app action: {}. Supported actions: approve_items, reject_items, edit_item, commit_review, cancel_review, archive_candidates, restore_archived, recompute_decay, rebuild_communities, export_diff, expand_neighbors, open_edge_details, use_path_as_context, close_session.",
                p.action
            ))),
        };

        match result {
            Ok(command_result) => {
                self.service.log_tool_event_with_duration(
                    "app_command.done",
                    json!({"session_id": p.session_id, "action": command_result.action}),
                    json!({
                        "app": command_result.app,
                        "refresh_required": command_result.refresh_required,
                    }),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                let guidance = if command_result.refresh_required {
                    "Re-read the returned `resource_uri` to get the updated app view."
                } else {
                    "No refresh is required; the session is complete or has been closed."
                };
                Ok(Json(ToolResponse::success_with_guidance(
                    command_result,
                    guidance,
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "app_command.error",
                    json!({"session_id": p.session_id, "action": p.action}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(err)
            }
        }
    }

    // --- MCP Apps: APP-01 Memory Inspector ---

    #[tool(
        description = "Inspect a memory entity, fact, or episode with full temporal state and provenance. Use when you need to examine one piece of memory in detail. Do not use for bulk search. Arguments require `scope`, `target_type` (entity|fact|episode), and `target_id`. Returns a detailed view with state badges and pagination. On error, verify the target exists."
    )]
    pub async fn open_memory_inspector(
        &self,
        params: Parameters<OpenMemoryInspectorParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let page_size = p.page_size.unwrap_or(20) as usize;

        let timer = OperationTimer::new("open_memory_inspector");
        self.service.log_tool_event(
            "open_memory_inspector.start",
            json!({"scope": p.scope, "target_type": p.target_type, "target_id": p.target_id}),
            json!({}),
            LogLevel::Info,
        );

        let result = self
            .load_inspector_view(
                &p.scope,
                &p.target_type,
                &p.target_id,
                page_size,
                p.cursor.as_deref(),
            )
            .await;

        match result {
            Ok(view) => {
                self.service.log_tool_event_with_duration(
                    "open_memory_inspector.done",
                    json!({}),
                    json!({"target_type": p.target_type}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    view,
                    "Read the inspector app resource for current state. Use canonical memory tools for supported writes, then re-read the resource.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "open_memory_inspector.error",
                    json!({"target_id": p.target_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(err)
            }
        }
    }

    #[tool(
        description = "Refresh the Memory Inspector view by reloading data from the session. Use when you want to get the latest state after modifications. Arguments require `session_id`. Returns refreshed view. On error, verify the session exists."
    )]
    pub async fn refresh_memory_inspector(
        &self,
        params: Parameters<RefreshMemoryInspectorParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("refresh_memory_inspector");

        let session = match self
            .service
            .app_session_manager
            .get_session(&p.session_id)
            .await
        {
            Ok(s) => s,
            Err(err) => return Err(mcp_error(err)),
        };

        if session.app_id != "memory_inspector" && session.app_id != "inspector" {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "Session is not a Memory Inspector session",
                None,
            ));
        }

        let target_id = session
            .target
            .get("target_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    rmcp::model::ErrorCode::INVALID_PARAMS,
                    "target_id not found in session".to_string(),
                    None,
                )
            })?;

        let target_type = session
            .target
            .get("target_type")
            .and_then(|v| v.as_str())
            .unwrap_or("entity");

        let scope = &session.scope;
        let result = self
            .load_inspector_view(scope, target_type, target_id, 20, None)
            .await;

        match result {
            Ok(view) => {
                self.service.log_tool_event_with_duration(
                    "refresh_memory_inspector.done",
                    json!({}),
                    json!({"target_type": target_type}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    view,
                    "View refreshed with latest data.",
                )))
            }
            Err(err) => Err(err),
        }
    }

    #[tool(
        description = "Invalidate a fact while preserving historical traceability. Use when a fact becomes outdated. Do not use to delete memory. Arguments require `session_id`, `fact_id`, and `confirmed: true`. Returns confirmation. On error, verify the fact identifier."
    )]
    pub async fn invalidate_fact(
        &self,
        params: Parameters<InvalidateFactParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("invalidate_fact");

        if !p.confirmed {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "This action requires confirmed: true. Set confirmed: true to invalidate the fact.",
                None,
            ));
        }

        let t_invalid = chrono::Utc::now();
        let request = InvalidateRequest {
            fact_id: p.fact_id.clone(),
            reason: p
                .reason
                .unwrap_or_else(|| "Manual invalidation via inspector".to_string()),
            t_invalid,
        };

        self.service.log_tool_event(
            "invalidate_fact.start",
            json!({"fact_id": &p.fact_id}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.invalidate(request, None).await {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "invalidate_fact.done",
                    json!({"fact_id": p.fact_id}),
                    json!({"result": &res}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    json!({"status": res}),
                    "Fact invalidated. Re-read the relevant app resource to see updated state.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "invalidate_fact.error",
                    json!({"fact_id": p.fact_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Archive an episode, hiding it from active retrieval. Use when an episode is no longer relevant. Do not use for fact-level invalidation. Arguments require `session_id`, `episode_id`, and `confirmed: true`. Returns confirmation. On error, verify the episode identifier."
    )]
    pub async fn archive_episode(
        &self,
        params: Parameters<ArchiveEpisodeParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("archive_episode");

        if !p.confirmed {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "This action requires confirmed: true. Set confirmed: true to archive the episode.",
                None,
            ));
        }

        self.service.log_tool_event(
            "archive_episode.start",
            json!({"episode_id": &p.episode_id}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.archive_episode(&p.episode_id).await {
            Ok(()) => {
                self.service.log_tool_event_with_duration(
                    "archive_episode.done",
                    json!({"episode_id": p.episode_id}),
                    json!({"status": "archived"}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    json!({"status": "archived"}),
                    "Episode archived. Re-read the relevant app resource to see updated state.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "archive_episode.error",
                    json!({"episode_id": p.episode_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Close an MCP App session. Use to free server resources after finishing work with an app. Arguments require `session_id`. Returns confirmation. On error, the session may already be closed or expired."
    )]
    pub async fn close_session(
        &self,
        params: Parameters<CloseSessionParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        self.service.log_tool_event(
            "close_session.start",
            json!({"session_id": &p.session_id}),
            json!({}),
            LogLevel::Info,
        );

        match self.service.close_app_session(&p.session_id).await {
            Ok(()) => Ok(Json(ToolResponse::success_with_guidance(
                json!({"status": "closed"}),
                "Session closed. Open a new session to continue.",
            ))),
            Err(err) => Err(mcp_error(err)),
        }
    }

    // --- MCP Apps: APP-02 Temporal Diff ---

    #[tool(
        description = "Compare memory state between two points in time. Use when you need to see what changed in a scope, entity, or episode over time. Arguments require `scope`, `target_type`, `as_of_left`, and `as_of_right`. Returns a diff with added, removed, and changed items. On error, verify the target exists and timestamps are valid."
    )]
    pub async fn open_temporal_diff(
        &self,
        params: Parameters<OpenTemporalDiffParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("open_temporal_diff");
        self.service.log_tool_event(
            "open_temporal_diff.start",
            json!({"scope": p.scope, "target_type": p.target_type}),
            json!({}),
            LogLevel::Info,
        );

        let time_axis = p.time_axis.unwrap_or_else(|| "valid".to_string());

        let filters_value = p
            .filters
            .as_ref()
            .map(|f| serde_json::to_value(f).unwrap_or(serde_json::Value::Null));

        let result = self
            .service
            .open_temporal_diff(
                &p.scope,
                &p.target_type,
                p.target_id.as_deref(),
                &p.as_of_left,
                &p.as_of_right,
                &time_axis,
                filters_value.as_ref(),
            )
            .await;

        match result {
            Ok(view) => {
                self.service.log_tool_event_with_duration(
                    "open_temporal_diff.done",
                    json!({}),
                    json!({"target_type": p.target_type}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    view,
                    "Read the diff app resource for results, or use open_app on inspector to drill down into specific items.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "open_temporal_diff.error",
                    json!({"scope": p.scope}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Export temporal diff results in a specific format. Use when you need to share or save the diff results. Arguments require `session_id` and `format` (json or markdown). Returns the exported content. On error, verify the session exists."
    )]
    pub async fn export_temporal_diff(
        &self,
        params: Parameters<ExportTemporalDiffParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("export_temporal_diff");
        self.service.log_tool_event(
            "export_temporal_diff.start",
            json!({"session_id": &p.session_id, "format": p.format}),
            json!({}),
            LogLevel::Info,
        );

        let result = self
            .service
            .export_temporal_diff(&p.session_id, &p.format)
            .await;

        match result {
            Ok(export) => {
                self.service.log_tool_event_with_duration(
                    "export_temporal_diff.done",
                    json!({}),
                    json!({"format": p.format}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    export,
                    "Diff exported successfully.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "export_temporal_diff.error",
                    json!({"session_id": p.session_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Open Memory Inspector from a temporal diff item. Use when you need to examine a specific item from the diff in detail. Arguments require `session_id`, `target_id`, and `target_type`. Returns the inspector view. On error, verify the target exists in the diff."
    )]
    pub async fn open_memory_inspector_from_diff(
        &self,
        params: Parameters<OpenMemoryInspectorFromDiffParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("open_memory_inspector_from_diff");
        self.service.log_tool_event(
            "open_memory_inspector_from_diff.start",
            json!({"session_id": &p.session_id, "target_id": p.target_id}),
            json!({}),
            LogLevel::Info,
        );

        let result = self
            .service
            .open_memory_inspector_from_diff(&p.session_id, &p.target_id, &p.target_type)
            .await;

        match result {
            Ok(view) => {
                self.service.log_tool_event_with_duration(
                    "open_memory_inspector_from_diff.done",
                    json!({}),
                    json!({"target_type": p.target_type}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    view,
                    "View the inspector for detailed information about this item.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "open_memory_inspector_from_diff.error",
                    json!({"session_id": p.session_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    // --- MCP Apps: APP-03 Ingestion Review ---

    #[tool(
        description = "Open an ingestion review session for human-in-the-loop validation. Use when you need to review and approve extracted entities, facts, and edges before committing to the main store. Arguments require `scope`. Returns a session with draft_id. On error, verify the scope exists."
    )]
    pub async fn open_ingestion_review(
        &self,
        params: Parameters<OpenIngestionReviewParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("open_ingestion_review");
        self.service.log_tool_event(
            "open_ingestion_review.start",
            json!({"scope": p.scope}),
            json!({}),
            LogLevel::Info,
        );

        let result = self
            .service
            .open_ingestion_review(
                &p.scope,
                p.source_text.as_deref(),
                p.draft_episode_id.as_deref(),
                p.ttl_seconds,
            )
            .await;

        match result {
            Ok(view) => {
                self.service.log_tool_event_with_duration(
                    "open_ingestion_review.done",
                    json!({}),
                    json!({"scope": p.scope}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    view,
                    "Read the ingestion_review app resource to inspect candidates, then use app_command to edit, approve, reject, or commit them.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "open_ingestion_review.error",
                    json!({"scope": p.scope}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Get summary of draft items for review. Use to see candidate counts by type and status. Arguments require `session_id`. Returns draft summary. On error, verify the session exists."
    )]
    pub async fn get_draft_summary(
        &self,
        params: Parameters<GetDraftSummaryParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("get_draft_summary");
        self.service.log_tool_event(
            "get_draft_summary.start",
            json!({"session_id": &p.session_id}),
            json!({}),
            LogLevel::Info,
        );

        let result = self.service.get_draft_summary(&p.session_id).await;

        match result {
            Ok(summary) => {
                self.service.log_tool_event_with_duration(
                    "get_draft_summary.done",
                    json!({}),
                    json!({}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    summary,
                    "Use app_command to approve, reject, edit, or commit the reviewed candidates.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "get_draft_summary.error",
                    json!({"session_id": p.session_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Approve ingestion items for commit. Use when you want to mark items as approved. Arguments require `session_id` and `item_ids`. Returns confirmation. On error, verify the items exist in the draft."
    )]
    pub async fn approve_ingestion_items(
        &self,
        params: Parameters<ApproveIngestionItemsParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("approve_ingestion_items");
        self.service.log_tool_event(
            "approve_ingestion_items.start",
            json!({"session_id": &p.session_id, "count": p.item_ids.len()}),
            json!({}),
            LogLevel::Info,
        );

        let result = self
            .service
            .approve_ingestion_items(&p.session_id, &p.item_ids)
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "approve_ingestion_items.done",
                    json!({}),
                    json!({"count": p.item_ids.len()}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Re-read the ingestion_review app resource to see updated counts.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "approve_ingestion_items.error",
                    json!({"session_id": p.session_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Reject ingestion items. Use when you want to mark items as rejected. Arguments require `session_id` and `item_ids`. Returns confirmation. On error, verify the items exist in the draft."
    )]
    pub async fn reject_ingestion_items(
        &self,
        params: Parameters<RejectIngestionItemsParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("reject_ingestion_items");
        self.service.log_tool_event(
            "reject_ingestion_items.start",
            json!({"session_id": &p.session_id, "count": p.item_ids.len()}),
            json!({}),
            LogLevel::Info,
        );

        let result = self
            .service
            .reject_ingestion_items(&p.session_id, &p.item_ids, p.reason.as_deref())
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "reject_ingestion_items.done",
                    json!({}),
                    json!({"count": p.item_ids.len()}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Re-read the ingestion_review app resource to see updated counts.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "reject_ingestion_items.error",
                    json!({"session_id": p.session_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Cancel an ingestion review and discard all candidates. Use when you want to abort the review without committing. Arguments require `session_id`. Returns confirmation. On error, verify the session exists."
    )]
    pub async fn cancel_ingestion_review(
        &self,
        params: Parameters<CancelIngestionReviewParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("cancel_ingestion_review");
        self.service.log_tool_event(
            "cancel_ingestion_review.start",
            json!({"session_id": &p.session_id}),
            json!({}),
            LogLevel::Info,
        );

        let result = self.service.cancel_ingestion_review(&p.session_id).await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "cancel_ingestion_review.done",
                    json!({}),
                    json!({}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Ingestion review cancelled. Open a new review to continue.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "cancel_ingestion_review.error",
                    json!({"session_id": p.session_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Commit an ingestion review and finalize approved items. Use when you have reviewed and approved candidates and want to persist them to the main store. Arguments require `session_id` and `confirmed: true`. Returns confirmation with commit summary. On error, verify you have approved items."
    )]
    pub async fn commit_ingestion_review(
        &self,
        params: Parameters<CommitIngestionReviewParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("commit_ingestion_review");

        if !p.confirmed {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "This action requires confirmed: true. Set confirmed: true to commit.",
                None,
            ));
        }

        let result = self.service.commit_ingestion_review(&p.session_id).await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "commit_ingestion_review.done",
                    json!({}),
                    json!({}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Ingestion review committed successfully.",
                )))
            }
            Err(err) => Err(mcp_error(err)),
        }
    }

    // --- MCP Apps: APP-04 Lifecycle Console ---

    #[tool(
        description = "Open lifecycle console for memory hygiene operations. Use when you need to manage decay, archival, and community rebuilding. Arguments require `scope`. Returns a session. On error, verify the scope exists."
    )]
    pub async fn open_lifecycle_console(
        &self,
        params: Parameters<OpenLifecycleConsoleParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("open_lifecycle_console");
        self.service.log_tool_event(
            "open_lifecycle_console.start",
            json!({"scope": p.scope}),
            json!({}),
            LogLevel::Info,
        );

        let filters_value = p
            .filters
            .as_ref()
            .map(|f| serde_json::to_value(f).unwrap_or(serde_json::Value::Null));

        let result = self
            .service
            .open_lifecycle_console(&p.scope, filters_value.as_ref())
            .await;

        match result {
            Ok(view) => {
                self.service.log_tool_event_with_duration(
                    "open_lifecycle_console.done",
                    json!({}),
                    json!({"scope": p.scope}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    view,
                    "Read the lifecycle app resource to review the dashboard, then use app_command for lifecycle actions.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "open_lifecycle_console.error",
                    json!({"scope": p.scope}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Archive memory candidates. Use when you want to archive episodes or facts. Arguments require `session_id`, `candidate_ids`, and `confirmed: true`. Returns confirmation. On error, verify the candidates exist."
    )]
    pub async fn archive_candidates(
        &self,
        params: Parameters<ArchiveCandidatesParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("archive_candidates");
        self.service.log_tool_event(
            "archive_candidates.start",
            json!({"session_id": &p.session_id, "count": p.candidate_ids.len()}),
            json!({}),
            LogLevel::Info,
        );

        if !p.confirmed {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "This action requires confirmed: true. Set confirmed: true to archive.",
                None,
            ));
        }

        let result = self
            .service
            .archive_candidates(&p.session_id, &p.candidate_ids, p.dry_run.unwrap_or(false))
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "archive_candidates.done",
                    json!({}),
                    json!({"count": p.candidate_ids.len()}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Archive operation completed.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "archive_candidates.error",
                    json!({"session_id": &p.session_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Restore archived episodes. Use when you want to restore archived memory. Arguments require `session_id`, `episode_ids`, and `confirmed: true`. Returns confirmation. On error, verify the episodes exist."
    )]
    pub async fn restore_archived(
        &self,
        params: Parameters<RestoreArchivedParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("restore_archived");

        if !p.confirmed {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "This action requires confirmed: true. Set confirmed: true to restore.",
                None,
            ));
        }

        let result = self
            .service
            .restore_archived(&p.session_id, &p.episode_ids)
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "restore_archived.done",
                    json!({}),
                    json!({"count": p.episode_ids.len()}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Restore operation completed.",
                )))
            }
            Err(err) => Err(mcp_error(err)),
        }
    }

    #[tool(
        description = "Recompute decay for facts. Use when you want to recalculate confidence decay. Arguments require `session_id`. Optional `dry_run` returns preview without changes. Returns confirmation. On error, verify the session exists."
    )]
    pub async fn recompute_decay(
        &self,
        params: Parameters<RecomputeDecayParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("recompute_decay");
        self.service.log_tool_event(
            "recompute_decay.start",
            json!({"session_id": &p.session_id}),
            json!({}),
            LogLevel::Info,
        );

        let result = self
            .service
            .recompute_decay(
                &p.session_id,
                p.target_ids.as_deref(),
                p.dry_run.unwrap_or(false),
            )
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "recompute_decay.done",
                    json!({}),
                    json!({}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Decay recomputed.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "recompute_decay.error",
                    json!({"session_id": &p.session_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Rebuild community detection. Use when you want to rebuild entity communities. Arguments require `session_id` and `confirmed: true`. Optional `dry_run` returns preview. Returns confirmation. On error, verify the session exists."
    )]
    pub async fn rebuild_communities(
        &self,
        params: Parameters<RebuildCommunitiesParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("rebuild_communities");

        if !p.confirmed {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "This action requires confirmed: true. Set confirmed: true to rebuild communities.",
                None,
            ));
        }

        let result = self
            .service
            .rebuild_communities(&p.session_id, p.dry_run.unwrap_or(false))
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "rebuild_communities.done",
                    json!({}),
                    json!({}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Communities rebuilt.",
                )))
            }
            Err(err) => Err(mcp_error(err)),
        }
    }

    #[tool(
        description = "Get status of a lifecycle task. Use to check progress of async operations. Arguments require `task_id`. Returns task status. On error, verify the task_id is correct."
    )]
    pub async fn get_lifecycle_task_status(
        &self,
        params: Parameters<GetLifecycleTaskStatusParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("get_lifecycle_task_status");

        let result = self.service.get_lifecycle_task_status(&p.task_id).await;

        match result {
            Ok(status) => {
                self.service.log_tool_event_with_duration(
                    "get_lifecycle_task_status.done",
                    json!({}),
                    json!({}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    status,
                    "Task status retrieved.",
                )))
            }
            Err(err) => Err(mcp_error(err)),
        }
    }

    // --- MCP Apps: APP-05 Graph Path Explorer ---

    #[tool(
        description = "Explore graph path between two entities. Use when you need to find how two entities are connected. Arguments require `scope`, `from_entity_id`, and `to_entity_id`. Returns the path if found. On error, verify the entities exist."
    )]
    pub async fn open_graph_path(
        &self,
        params: Parameters<OpenGraphPathParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("open_graph_path");
        self.service.log_tool_event(
            "open_graph_path.start",
            json!({"scope": p.scope, "from": p.from_entity_id, "to": p.to_entity_id}),
            json!({}),
            LogLevel::Info,
        );

        let result = self
            .service
            .open_graph_path(
                &p.scope,
                &p.from_entity_id,
                &p.to_entity_id,
                p.as_of.as_deref(),
                p.max_depth.unwrap_or(4),
            )
            .await;

        match result {
            Ok(view) => {
                self.service.log_tool_event_with_duration(
                    "open_graph_path.done",
                    json!({}),
                    json!({"path_found": view.get("path_found").is_some()}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    view,
                    "Read the graph app resource for the current path view, or use app_command with expand_neighbors, open_edge_details, or use_path_as_context for session-scoped graph exploration.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "open_graph_path.error",
                    json!({"from": p.from_entity_id, "to": p.to_entity_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Expand neighbors of a graph node. Use when you want to explore connections from a specific entity. Arguments require `session_id`, `entity_id`, and `direction`. Returns neighbor list. On error, verify the entity exists."
    )]
    pub async fn expand_graph_neighbors(
        &self,
        params: Parameters<ExpandGraphNeighborsParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("expand_graph_neighbors");

        let result = self
            .service
            .expand_graph_neighbors(
                &p.session_id,
                &p.entity_id,
                &p.direction,
                p.depth.unwrap_or(1),
            )
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "expand_graph_neighbors.done",
                    json!({"entity_id": p.entity_id, "direction": p.direction}),
                    json!({"count": res.get("nodes").and_then(|n| n.as_array()).map(|a| a.len()).unwrap_or(0)}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Neighbors expanded.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "expand_graph_neighbors.error",
                    json!({"entity_id": p.entity_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Open edge details from a graph path. Use when you need to see details about a specific connection. Arguments require `session_id` and `edge_id`. Returns edge details. On error, verify the edge exists."
    )]
    pub async fn open_edge_details(
        &self,
        params: Parameters<OpenEdgeDetailsParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("open_edge_details");

        let result = self
            .service
            .open_edge_details(&p.session_id, &p.edge_id)
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "open_edge_details.done",
                    json!({"edge_id": p.edge_id}),
                    json!({}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Edge details retrieved.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "open_edge_details.error",
                    json!({"edge_id": p.edge_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Use a path as context for further queries. Use when you want to use a found path as context. Arguments require `session_id` and `path_id`. Returns serialized path. On error, verify the path exists."
    )]
    pub async fn use_path_as_context(
        &self,
        params: Parameters<UsePathAsContextParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        let p = params.0;
        let timer = OperationTimer::new("use_path_as_context");

        let result = self
            .service
            .use_path_as_context(&p.session_id, &p.path_id)
            .await;

        match result {
            Ok(res) => {
                self.service.log_tool_event_with_duration(
                    "use_path_as_context.done",
                    json!({"path_id": p.path_id}),
                    json!({}),
                    LogLevel::Info,
                    timer.elapsed(),
                );
                Ok(Json(ToolResponse::success_with_guidance(
                    res,
                    "Path ready for use as context.",
                )))
            }
            Err(err) => {
                self.service.log_tool_event_with_duration(
                    "use_path_as_context.error",
                    json!({"path_id": p.path_id}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                    timer.elapsed(),
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
    use std::sync::Arc;

    use crate::service::{AnnoEntityExtractor, DisabledEmbeddingProvider};
    use crate::storage::{DbClient, SurrealDbClient};

    fn schema_json<T: schemars::JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).expect("schema json")
    }

    fn schema_properties(
        schema: &serde_json::Value,
    ) -> &serde_json::Map<String, serde_json::Value> {
        schema["properties"].as_object().expect("properties object")
    }

    fn json_object_schema(
        schema: &Arc<serde_json::Map<String, serde_json::Value>>,
    ) -> serde_json::Value {
        serde_json::Value::Object((**schema).clone())
    }

    fn resolve_schema_properties<'a>(
        root_schema: &'a serde_json::Value,
        schema: &'a serde_json::Value,
    ) -> &'a serde_json::Map<String, serde_json::Value> {
        fn referenced_properties<'a>(
            root_schema: &'a serde_json::Value,
            reference: &str,
        ) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
            let definition_name = reference.rsplit('/').next()?;

            root_schema
                .get("definitions")
                .and_then(|definitions| definitions.get(definition_name))
                .or_else(|| {
                    root_schema
                        .get("$defs")
                        .and_then(|definitions| definitions.get(definition_name))
                })
                .and_then(|definition| resolve_schema_properties_optional(root_schema, definition))
        }

        fn resolve_schema_properties_optional<'a>(
            root_schema: &'a serde_json::Value,
            schema: &'a serde_json::Value,
        ) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
            if let Some(reference) = schema.get("$ref").and_then(|value| value.as_str()) {
                return referenced_properties(root_schema, reference);
            }

            if let Some(properties) = schema.get("properties").and_then(|value| value.as_object()) {
                return Some(properties);
            }

            for key in ["allOf", "anyOf", "oneOf"] {
                if let Some(branches) = schema.get(key).and_then(|value| value.as_array()) {
                    for branch in branches {
                        if let Some(properties) =
                            resolve_schema_properties_optional(root_schema, branch)
                        {
                            return Some(properties);
                        }
                    }
                }
            }

            None
        }

        resolve_schema_properties_optional(root_schema, schema)
            .expect("schema properties should be discoverable")
    }

    async fn create_test_mcp() -> MemoryMcp {
        let db_client: Arc<dyn DbClient> = Arc::new(
            SurrealDbClient::connect_in_memory("testdb_mcp_handlers", "org", "warn")
                .await
                .expect("in-memory surrealdb"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply test migrations");
        let service = MemoryService::new_with_embedding_provider(
            db_client,
            vec!["org".to_string()],
            "warn".to_string(),
            100,
            100,
            Arc::new(DisabledEmbeddingProvider::new(Some(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            ))),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("memory service");

        MemoryMcp::new(service)
    }

    #[test]
    fn build_server_info_enables_tools_resources_and_sets_instructions() {
        let info = MemoryMcp::build_server_info();
        let capabilities = serde_json::to_value(&info.capabilities).unwrap();

        assert_eq!(
            info.instructions.as_deref(),
            Some(
                "Memory MCP server: stores facts about entities and relationships, resolves aliases, and assembles long-term context.",
            ),
        );
        assert!(capabilities.get("tools").is_some());
        assert!(capabilities.get("resources").is_some());
    }

    #[test]
    fn public_tool_names_match_canonical_memory_surface() {
        assert_eq!(
            MemoryMcp::PUBLIC_TOOL_NAMES.as_slice(),
            [
                "assemble_context",
                "explain",
                "extract",
                "ingest",
                "invalidate",
                "resolve",
                "open_app",
                "app_command",
            ]
            .as_slice(),
        );
    }

    #[test]
    fn public_tool_name_filter_hides_mcp_apps() {
        for tool_name in [
            "open_memory_inspector",
            "refresh_memory_inspector",
            "open_temporal_diff",
            "open_ingestion_review",
            "open_lifecycle_console",
            "open_graph_path",
            "close_session",
        ] {
            assert!(
                !MemoryMcp::is_public_tool_name(tool_name),
                "{tool_name} should not be publicly advertised"
            );
        }

        for tool_name in MemoryMcp::PUBLIC_TOOL_NAMES {
            assert!(
                MemoryMcp::is_public_tool_name(tool_name),
                "{tool_name} should remain public"
            );
        }
    }

    #[tokio::test]
    async fn get_tool_exposes_only_canonical_public_surface() {
        let mcp = create_test_mcp().await;

        for tool_name in MemoryMcp::PUBLIC_TOOL_NAMES {
            assert!(
                mcp.get_tool(tool_name).is_some(),
                "{tool_name} should be discoverable"
            );
        }

        for tool_name in [
            "open_memory_inspector",
            "open_temporal_diff",
            "open_ingestion_review",
            "open_lifecycle_console",
            "open_graph_path",
            "close_session",
        ] {
            assert!(
                mcp.get_tool(tool_name).is_none(),
                "{tool_name} should be hidden from the public MCP surface"
            );
        }
    }

    #[tokio::test]
    async fn get_tool_exposes_open_app_launcher() {
        let mcp = create_test_mcp().await;

        assert!(
            mcp.get_tool("open_app").is_some(),
            "open_app should be discoverable from the public MCP surface"
        );
    }

    #[tokio::test]
    async fn get_tool_exposes_app_command_bridge() {
        let mcp = create_test_mcp().await;

        assert!(
            mcp.get_tool("app_command").is_some(),
            "app_command should be discoverable from the public MCP surface once mutation-heavy app flows are exposed"
        );
    }

    #[tokio::test]
    async fn list_resource_templates_exposes_public_app_session_templates() {
        let _mcp = create_test_mcp().await;
        let result = MemoryMcp::list_resource_templates_result();
        let uri_templates: Vec<_> = result
            .resource_templates
            .iter()
            .map(|template| template.raw.uri_template.as_str())
            .collect();

        assert!(uri_templates.contains(&"ui://memory/app/inspector/{session_id}"));
        assert!(uri_templates.contains(&"ui://memory/app/diff/{session_id}"));
        assert!(uri_templates.contains(&"ui://memory/app/ingestion_review/{session_id}"));
        assert!(uri_templates.contains(&"ui://memory/app/lifecycle/{session_id}"));
        assert!(uri_templates.contains(&"ui://memory/app/graph/{session_id}"));
    }

    #[tokio::test]
    async fn public_tool_schemas_publish_output_schema_for_all_public_tools() {
        let mcp = create_test_mcp().await;

        for tool_name in MemoryMcp::PUBLIC_TOOL_NAMES {
            let tool = mcp
                .get_tool(tool_name)
                .unwrap_or_else(|| panic!("{tool_name} should be discoverable"));
            assert!(
                tool.output_schema.is_some(),
                "{tool_name} should publish an output schema"
            );
        }
    }

    #[tokio::test]
    async fn open_app_tool_schema_matches_current_public_contract() {
        let mcp = create_test_mcp().await;
        let tool = mcp.get_tool("open_app").expect("open_app tool");

        let input_schema = json_object_schema(&tool.input_schema);
        let input_properties = schema_properties(&input_schema);
        for key in [
            "app",
            "scope",
            "target_type",
            "target_id",
            "from_entity_id",
            "to_entity_id",
            "source_text",
            "draft_episode_id",
            "as_of",
            "as_of_left",
            "as_of_right",
            "time_axis",
            "view",
            "cursor",
            "page_size",
            "max_depth",
            "ttl_seconds",
        ] {
            assert!(input_properties.contains_key(key), "missing property {key}");
        }

        let output_schema = json_object_schema(
            tool.output_schema
                .as_ref()
                .expect("open_app output schema should be published"),
        );
        let output_properties = schema_properties(&output_schema);
        for key in ["status", "result", "guidance"] {
            assert!(
                output_properties.contains_key(key),
                "missing property {key}"
            );
        }

        let result_properties =
            resolve_schema_properties(&output_schema, &output_properties["result"]);
        for key in ["app", "session_id", "resource_uri", "fallback"] {
            assert!(
                result_properties.contains_key(key),
                "missing property {key}"
            );
        }
    }

    #[tokio::test]
    async fn app_command_tool_schema_matches_current_public_contract() {
        let mcp = create_test_mcp().await;
        let tool = mcp.get_tool("app_command").expect("app_command tool");

        let input_schema = json_object_schema(&tool.input_schema);
        let input_properties = schema_properties(&input_schema);
        for key in [
            "session_id",
            "action",
            "item_ids",
            "target_ids",
            "target_id",
            "item_id",
            "patch_json",
            "reason",
            "dry_run",
            "confirmed",
            "format",
            "direction",
            "depth",
        ] {
            assert!(input_properties.contains_key(key), "missing property {key}");
        }

        let output_schema = json_object_schema(
            tool.output_schema
                .as_ref()
                .expect("app_command output schema should be published"),
        );
        let output_properties = schema_properties(&output_schema);
        for key in ["status", "result", "guidance"] {
            assert!(
                output_properties.contains_key(key),
                "missing property {key}"
            );
        }

        let result_properties =
            resolve_schema_properties(&output_schema, &output_properties["result"]);
        for key in [
            "app",
            "session_id",
            "action",
            "ok",
            "message",
            "refresh_required",
            "resource_uri",
            "details",
        ] {
            assert!(
                result_properties.contains_key(key),
                "missing property {key}"
            );
        }
    }

    #[tokio::test]
    async fn open_app_inspector_returns_session_backed_envelope() {
        let mcp = create_test_mcp().await;
        let entity_id = mcp
            .service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Alice Example".to_string(),
                    aliases: vec!["Alice".to_string()],
                },
                Some(AccessContext::default()),
            )
            .await
            .expect("create test entity");

        let response = mcp
            .open_app(Parameters(OpenAppParams {
                app: "inspector".to_string(),
                scope: "org".to_string(),
                target_type: Some("entity".to_string()),
                target_id: Some(entity_id.clone()),
                from_entity_id: None,
                to_entity_id: None,
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: Some(10),
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open inspector app");

        let result = response.0.result;
        assert_eq!(result.app, "inspector");
        assert!(result.session_id.starts_with("ses:"));
        assert_eq!(
            result.resource_uri,
            format!("ui://memory/app/inspector/{}", result.session_id)
        );
        assert_eq!(result.fallback["target_type"], "entity");
        assert_eq!(result.fallback["target_id"], entity_id);
    }

    #[tokio::test]
    async fn read_app_resource_payload_returns_live_inspector_view() {
        let mcp = create_test_mcp().await;
        let entity_id = mcp
            .service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Bob Example".to_string(),
                    aliases: vec!["Bobby".to_string()],
                },
                Some(AccessContext::default()),
            )
            .await
            .expect("create test entity");

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "inspector".to_string(),
                scope: "org".to_string(),
                target_type: Some("entity".to_string()),
                target_id: Some(entity_id.clone()),
                from_entity_id: None,
                to_entity_id: None,
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: Some(5),
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open inspector app")
            .0
            .result;

        let payload = mcp
            .read_app_resource_payload("inspector", &open_result.session_id)
            .await
            .expect("read inspector resource payload");

        assert_eq!(payload["app"], "inspector");
        assert_eq!(payload["session_id"], open_result.session_id);
        assert_eq!(payload["view"]["target_type"], "entity");
        assert_eq!(payload["view"]["target_id"], entity_id);
        assert_eq!(payload["view"]["entity"]["canonical_name"], "Bob Example");
    }

    #[tokio::test]
    async fn read_app_resource_payload_exposes_ingestion_review_items() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "ingestion_review".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: Some("Alice works at Acme".to_string()),
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open ingestion review app")
            .0
            .result;

        let payload = mcp
            .read_app_resource_payload("ingestion_review", &open_result.session_id)
            .await
            .expect("read ingestion review payload");

        let items = payload["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["status"], "pending");
        assert!(items[0].get("item_id").is_some());
        assert_eq!(payload["summary"]["by_status"]["pending"], 1);
    }

    #[tokio::test]
    async fn read_resource_returns_public_ingestion_review_session_html_document() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "ingestion_review".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: Some("Alice works at Acme".to_string()),
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open ingestion review app")
            .0
            .result;

        let result = mcp
            .read_resource_result(ReadResourceRequestParams::new(app_session_uri(
                "ingestion_review",
                &open_result.session_id,
            )))
            .await
            .expect("resource read should succeed");

        assert_eq!(result.contents.len(), 1);
        let body = match &result.contents[0] {
            ResourceContents::TextResourceContents {
                text, mime_type, ..
            } => {
                assert_eq!(mime_type.as_deref(), Some("text/html;profile=mcp-app"));
                text
            }
            other => panic!("expected text resource, got {other:?}"),
        };
        assert!(body.contains("Memory App: ingestion_review"));
        assert!(body.contains("<script type=\"application/json\" id=\"app-data\">"));
        assert!(body.contains(&open_result.session_id));
        assert!(body.contains("\"app\": \"ingestion_review\""));
    }

    #[tokio::test]
    async fn app_command_approve_items_refreshes_ingestion_review_resource() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "ingestion_review".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: Some("Alice works at Acme".to_string()),
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open ingestion review app")
            .0
            .result;

        let payload = mcp
            .read_app_resource_payload("ingestion_review", &open_result.session_id)
            .await
            .expect("read ingestion review payload");
        let item_id = payload["items"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item.get("item_id"))
            .and_then(|value| value.as_str())
            .expect("draft item id")
            .to_string();

        let response = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "approve_items".to_string(),
                item_ids: vec![item_id],
                target_ids: vec![],
                target_id: None,
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: None,
                confirmed: None,
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("approve ingestion review item")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.app, "ingestion_review");
        assert_eq!(response.result.action, "approve_items");
        assert!(response.result.refresh_required);
        assert_eq!(
            response.result.resource_uri,
            Some(format!(
                "ui://memory/app/ingestion_review/{}",
                open_result.session_id
            ))
        );

        let refreshed = mcp
            .read_app_resource_payload("ingestion_review", &open_result.session_id)
            .await
            .expect("read refreshed ingestion review payload");
        assert_eq!(refreshed["summary"]["by_status"]["pending"], 0);
        assert_eq!(refreshed["summary"]["by_status"]["approved"], 1);
        assert_eq!(refreshed["items"][0]["status"], "approved");
    }

    #[tokio::test]
    async fn app_command_edit_item_refreshes_ingestion_review_resource() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "ingestion_review".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: Some("Alice works at Acme".to_string()),
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open ingestion review app")
            .0
            .result;

        let payload = mcp
            .read_app_resource_payload("ingestion_review", &open_result.session_id)
            .await
            .expect("read ingestion review payload");
        let item_id = payload["items"][0]["item_id"]
            .as_str()
            .expect("draft item id")
            .to_string();

        let response = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "edit_item".to_string(),
                item_ids: vec![],
                target_ids: vec![],
                target_id: None,
                item_id: Some(item_id),
                patch_json: Some(
                    serde_json::json!({
                        "content": "Alice joined Acme Corporation",
                        "fact_type": "employment",
                        "policy_tags": ["public"]
                    })
                    .to_string(),
                ),
                reason: None,
                dry_run: None,
                confirmed: None,
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("edit ingestion review item")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.action, "edit_item");
        assert!(response.result.refresh_required);

        let refreshed = mcp
            .read_app_resource_payload("ingestion_review", &open_result.session_id)
            .await
            .expect("read refreshed ingestion review payload");
        assert_eq!(refreshed["summary"]["by_status"]["edited"], 1);
        assert_eq!(refreshed["items"][0]["status"], "edited");
        assert_eq!(
            refreshed["items"][0]["payload"]["content"],
            "Alice joined Acme Corporation"
        );
    }

    #[tokio::test]
    async fn app_command_commit_review_closes_ingestion_review_session() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "ingestion_review".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: Some("Alice works at Acme".to_string()),
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open ingestion review app")
            .0
            .result;

        let payload = mcp
            .read_app_resource_payload("ingestion_review", &open_result.session_id)
            .await
            .expect("read ingestion review payload");
        let item_id = payload["items"][0]["item_id"]
            .as_str()
            .expect("draft item id")
            .to_string();

        mcp.app_command(Parameters(AppCommandParams {
            session_id: open_result.session_id.clone(),
            action: "approve_items".to_string(),
            item_ids: vec![item_id],
            target_ids: vec![],
            target_id: None,
            item_id: None,
            patch_json: None,
            reason: None,
            dry_run: None,
            confirmed: None,
            format: None,
            direction: None,
            depth: None,
        }))
        .await
        .expect("approve ingestion review item");

        let response = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "commit_review".to_string(),
                item_ids: vec![],
                target_ids: vec![],
                target_id: None,
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: None,
                confirmed: None,
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("commit ingestion review")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.action, "commit_review");
        assert_eq!(response.result.resource_uri, None);
        assert_eq!(
            response.result.details.as_ref().unwrap()["commit_summary"]["facts"],
            1
        );
        assert!(
            mcp.read_app_resource_payload("ingestion_review", &open_result.session_id)
                .await
                .is_err(),
            "committed ingestion review session should be closed"
        );
    }

    #[tokio::test]
    async fn open_app_lifecycle_fallback_contains_live_dashboard_data() {
        let mcp = create_test_mcp().await;

        let response = mcp
            .open_app(Parameters(OpenAppParams {
                app: "lifecycle".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open lifecycle app")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.app, "lifecycle");
        assert!(response.result.fallback["view"]["low_confidence_facts"].is_array());
        assert!(response.result.fallback["view"]["archival_candidates"].is_array());
        assert!(response.result.fallback["view"]["archived_episodes"].is_array());
        assert!(response.result.fallback["view"]["stale_communities"].is_array());
    }

    #[tokio::test]
    async fn read_app_resource_payload_exposes_live_lifecycle_dashboard() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "lifecycle".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open lifecycle app")
            .0
            .result;

        let payload = mcp
            .read_app_resource_payload("lifecycle", &open_result.session_id)
            .await
            .expect("read lifecycle payload");

        assert_eq!(payload["app"], "lifecycle");
        assert!(payload["view"]["dashboard"]["low_confidence_facts"].is_array());
        assert!(payload["view"]["dashboard"]["archival_candidates"].is_array());
        assert!(payload["view"]["dashboard"]["archived_episodes"].is_array());
        assert!(payload["view"]["dashboard"]["stale_communities"].is_array());
    }

    #[tokio::test]
    async fn app_command_recompute_decay_supports_lifecycle_dry_run() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "lifecycle".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open lifecycle app")
            .0
            .result;

        let response = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "recompute_decay".to_string(),
                item_ids: vec![],
                target_ids: vec![],
                target_id: None,
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: Some(true),
                confirmed: None,
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("recompute lifecycle decay in dry-run mode")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.app, "lifecycle");
        assert_eq!(response.result.action, "recompute_decay");
        assert!(response.result.refresh_required);
        assert_eq!(response.result.details.as_ref().unwrap()["dry_run"], true);
    }

    #[tokio::test]
    async fn app_command_export_diff_returns_current_export_payload() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "diff".to_string(),
                scope: "org".to_string(),
                target_type: Some("scope".to_string()),
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: Some("2026-01-01T00:00:00Z".to_string()),
                as_of_right: Some("2026-03-01T00:00:00Z".to_string()),
                time_axis: Some("valid".to_string()),
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open diff app")
            .0
            .result;

        let response = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "export_diff".to_string(),
                item_ids: vec![],
                target_ids: vec![],
                target_id: None,
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: None,
                confirmed: None,
                format: Some("markdown".to_string()),
                direction: None,
                depth: None,
            }))
            .await
            .expect("export diff through public app command")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.app, "diff");
        assert_eq!(response.result.action, "export_diff");
        assert!(response.result.refresh_required);
        assert_eq!(
            response.result.resource_uri,
            Some(format!("ui://memory/app/diff/{}", open_result.session_id))
        );
        assert_eq!(response.result.details.as_ref().unwrap()["ok"], true);
        assert_eq!(
            response.result.details.as_ref().unwrap()["export"]["format"],
            "markdown"
        );
        assert!(
            response.result.details.as_ref().unwrap()["export"]["content"]
                .as_str()
                .expect("markdown export content")
                .contains("# Temporal Diff")
        );
    }

    #[tokio::test]
    async fn app_command_close_session_closes_public_session() {
        let mcp = create_test_mcp().await;
        let entity_id = mcp
            .service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Close Me".to_string(),
                    aliases: vec![],
                },
                Some(AccessContext::default()),
            )
            .await
            .expect("create close-session entity");

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "inspector".to_string(),
                scope: "org".to_string(),
                target_type: Some("entity".to_string()),
                target_id: Some(entity_id),
                from_entity_id: None,
                to_entity_id: None,
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: Some(5),
                max_depth: None,
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open inspector app")
            .0
            .result;

        let response = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "close_session".to_string(),
                item_ids: vec![],
                target_ids: vec![],
                target_id: None,
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: None,
                confirmed: None,
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("close public session")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.app, "inspector");
        assert_eq!(response.result.action, "close_session");
        assert!(!response.result.refresh_required);
        assert_eq!(response.result.resource_uri, None);
        assert!(
            mcp.read_app_resource_payload("inspector", &open_result.session_id)
                .await
                .is_err(),
            "closed session resource should no longer be readable"
        );
    }

    #[tokio::test]
    async fn app_command_expand_neighbors_supports_public_graph_sessions() {
        let mcp = create_test_mcp().await;

        let alice = mcp
            .service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Alice Graph".to_string(),
                    aliases: vec![],
                },
                Some(AccessContext::default()),
            )
            .await
            .expect("create alice entity");
        let bob = mcp
            .service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Bob Graph".to_string(),
                    aliases: vec![],
                },
                Some(AccessContext::default()),
            )
            .await
            .expect("create bob entity");
        let carol = mcp
            .service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Carol Graph".to_string(),
                    aliases: vec![],
                },
                Some(AccessContext::default()),
            )
            .await
            .expect("create carol entity");

        mcp.service()
            .relate(&alice, "works_with", &bob)
            .await
            .expect("relate alice to bob");
        mcp.service()
            .relate(&alice, "manages", &carol)
            .await
            .expect("relate alice to carol");

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "graph".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: Some(alice.clone()),
                to_entity_id: Some(bob.clone()),
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: Some(3),
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open graph app")
            .0
            .result;

        let response = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "expand_neighbors".to_string(),
                item_ids: vec![],
                target_ids: vec![],
                target_id: Some(alice.clone()),
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: None,
                confirmed: None,
                format: None,
                direction: Some("out".to_string()),
                depth: Some(1),
            }))
            .await
            .expect("expand graph neighbors")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.app, "graph");
        assert_eq!(response.result.action, "expand_neighbors");
        assert!(!response.result.refresh_required);
        let neighbors = response.result.details.as_ref().unwrap()["neighbors"]["neighbors"]
            .as_array()
            .expect("neighbors array");
        assert!(!neighbors.is_empty(), "expected graph expansion results");
    }

    #[tokio::test]
    async fn app_command_graph_edge_details_and_context_are_public() {
        let mcp = create_test_mcp().await;

        let alice = mcp
            .service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Alice Edge".to_string(),
                    aliases: vec![],
                },
                Some(AccessContext::default()),
            )
            .await
            .expect("create alice entity");
        let bob = mcp
            .service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Bob Edge".to_string(),
                    aliases: vec![],
                },
                Some(AccessContext::default()),
            )
            .await
            .expect("create bob entity");

        mcp.service()
            .relate(&alice, "works_with", &bob)
            .await
            .expect("relate alice to bob");

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "graph".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: Some(alice.clone()),
                to_entity_id: Some(bob.clone()),
                source_text: None,
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: Some(3),
                ttl_seconds: Some(300),
            }))
            .await
            .expect("open graph app")
            .0
            .result;

        let edge_id = open_result.fallback["path"]
            .as_array()
            .and_then(|nodes| nodes.first())
            .and_then(|node| node.get("edges"))
            .and_then(|edges| edges.as_array())
            .and_then(|edges| edges.first())
            .and_then(|edge| edge.get("edge_id"))
            .and_then(|value| value.as_str())
            .expect("graph path edge id")
            .to_string();

        let edge_details = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "open_edge_details".to_string(),
                item_ids: vec![],
                target_ids: vec![],
                target_id: Some(edge_id),
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: None,
                confirmed: None,
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("open graph edge details")
            .0;

        assert_eq!(edge_details.result.action, "open_edge_details");
        assert!(edge_details.result.details.as_ref().unwrap()["details"].is_object());

        let context_response = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "use_path_as_context".to_string(),
                item_ids: vec![],
                target_ids: vec![],
                target_id: Some("current".to_string()),
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: None,
                confirmed: None,
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("use graph path as context")
            .0;

        assert_eq!(context_response.result.action, "use_path_as_context");
        assert!(
            context_response.result.details.as_ref().unwrap()["path_serialized"]
                .as_str()
                .expect("serialized path")
                .contains(&alice)
        );
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
    fn tool_response_schema_exposes_list_pagination_contract() {
        let schema = schema_json::<ToolResponse<Vec<AssembledContextItem>>>();
        let properties = schema["properties"].as_object().expect("properties object");

        // Fields use snake_case for MCP client compatibility
        for key in [
            "status",
            "result",
            "guidance",
            "has_more",
            "total_count",
            "next_offset",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }
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
    fn explain_item_schema_exposes_enriched_citation_fields() {
        let schema = schema_json::<ToolResponse<Vec<ExplainItem>>>();
        let defs = schema
            .get("$defs")
            .or_else(|| schema.get("definitions"))
            .and_then(serde_json::Value::as_object)
            .expect("schema definitions");
        let explain_item = defs.get("ExplainItem").expect("ExplainItem definition");
        let properties = explain_item["properties"]
            .as_object()
            .expect("properties object");

        // Fields use snake_case for MCP client compatibility
        for key in [
            "content",
            "quote",
            "source_episode",
            "scope",
            "t_ref",
            "t_ingested",
            "provenance",
            "citation_context",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }
    }

    #[test]
    fn assembled_context_item_schema_exposes_rationale_and_provenance() {
        let schema = schema_json::<ToolResponse<Vec<AssembledContextItem>>>();
        let defs = schema
            .get("$defs")
            .or_else(|| schema.get("definitions"))
            .and_then(serde_json::Value::as_object)
            .expect("schema definitions");
        let context_item = defs
            .get("AssembledContextItem")
            .expect("AssembledContextItem definition");
        let properties = context_item["properties"]
            .as_object()
            .expect("properties object");

        // Fields use snake_case for MCP client compatibility
        for key in [
            "fact_id",
            "content",
            "quote",
            "source_episode",
            "confidence",
            "provenance",
            "rationale",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }
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
}
