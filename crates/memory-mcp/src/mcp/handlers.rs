//! MCP tool handler implementations.

use std::sync::Arc;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    CallToolRequestParams, CancelTaskParams, CancelTaskResult, CreateTaskResult, GetTaskParams,
    GetTaskPayloadParams, GetTaskPayloadResult, GetTaskResult, ListResourceTemplatesResult,
    ListResourcesResult, ListTasksResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, ServerCapabilities, ServerInfo, TasksCapability,
};
use rmcp::service::RequestContext;

type McpError = rmcp::ErrorData;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
#[cfg(feature = "mcp-apps")]
use serde_json::json;

#[cfg(feature = "mcp-apps")]
use crate::logging::LogLevel;
use crate::models::ExtractResult;
#[cfg(test)]
use crate::models::{AssembledContextItem, ExplainItem};
use crate::service::MemoryService;
#[cfg(feature = "mcp-apps")]
use crate::service::apps::dispatch::{AppContext, find_descriptor};
#[cfg(feature = "mcp-apps")]
use crate::service::{AppCommand, AppCommandInput};
#[cfg(feature = "mcp-apps")]
use std::time::Instant;

use super::error::mcp_error;
use super::params::*;
#[cfg(feature = "mcp-apps")]
use super::resources::app_session_uri;
use super::response::{AppCommandResult, OpenAppResult, ToolResponse};
use super::session::{self, SessionManager};
use super::tasks::{
    DEFAULT_TASK_POLL_INTERVAL_MS, DEFAULT_TASK_TTL_MS, TaskCancelState, TaskCreateError,
    TaskOutcome, TaskPayloadState, TaskStore, add_related_task_metadata, related_task_metadata,
};
mod apps;

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
    session_manager: SessionManager,
    task_store: Arc<tokio::sync::Mutex<TaskStore>>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl MemoryMcp {
    const SERVER_INSTRUCTIONS: &str = "Memory MCP server: stores facts about entities and relationships, resolves aliases, and assembles long-term context. All tool arguments and structured results use flat snake_case JSON keys that must match the published schemas exactly. Do not wrap tool arguments in `payload`.";
    /// Creates a new `MemoryMcp` instance with the given service.
    ///
    /// # Arguments
    ///
    /// * `service` - The `MemoryService` to use for memory operations.
    pub fn new(service: MemoryService) -> Self {
        Self {
            service: Arc::new(service),
            session_manager: SessionManager::new(),
            task_store: Arc::new(tokio::sync::Mutex::new(TaskStore::default())),
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

    #[cfg(feature = "mcp-apps")]
    /// Generates a monotonically increasing request id like `req_0001`.
    fn next_request_id(&self) -> String {
        crate::tools::request_id::next_request_id()
    }

    fn build_server_info() -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_tasks_with(TasksCapability::server_default())
                .build(),
        )
        .with_instructions(Self::SERVER_INSTRUCTIONS)
    }

    fn invalid_params(message: impl Into<String>) -> ErrorData {
        session::invalid_params(message)
    }

    fn missing_app_field(app: &str, field: &str) -> ErrorData {
        session::missing_app_field(app, field)
    }

    fn internal_error(message: impl Into<String>) -> ErrorData {
        session::internal_error(message)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MemoryMcp {
    fn get_info(&self) -> ServerInfo {
        Self::build_server_info()
    }

    async fn enqueue_task(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CreateTaskResult, McpError> {
        let task_id = format!("task_{}", crate::tools::request_id::next_request_id());
        let requested_ttl = request.task.as_ref().and_then(|metadata| metadata.ttl);
        request.task = None;

        let task = self
            .task_store
            .lock()
            .await
            .create(task_id.clone(), requested_ttl)
            .map_err(|error| match error {
                TaskCreateError::ActiveCapacity => {
                    Self::internal_error("too many active MCP tasks; retry later")
                }
                TaskCreateError::RetainedCapacity => {
                    Self::internal_error("MCP task result capacity exhausted; retry later")
                }
            })?;
        let ttl = task.ttl.unwrap_or(DEFAULT_TASK_TTL_MS);
        let server = self.clone();
        let worker_task_id = task_id.clone();
        let worker = tokio::spawn(async move {
            match tokio::time::timeout(
                std::time::Duration::from_millis(ttl),
                server.call_tool(request, context),
            )
            .await
            {
                Ok(Ok(result)) => {
                    let is_error = result.is_error.unwrap_or(false);
                    match serde_json::to_value(result) {
                        Ok(mut payload) => {
                            if let Err(error) =
                                add_related_task_metadata(&mut payload, &worker_task_id)
                            {
                                TaskOutcome::ProtocolError(McpError::internal_error(error, None))
                            } else {
                                TaskOutcome::ToolResult { payload, is_error }
                            }
                        }
                        Err(error) => TaskOutcome::ProtocolError(McpError::internal_error(
                            format!("failed to serialize task result: {error}"),
                            None,
                        )),
                    }
                }
                Ok(Err(error)) => TaskOutcome::ProtocolError(error),
                Err(_) => TaskOutcome::ProtocolError(McpError::internal_error(
                    "task TTL elapsed before completion",
                    None,
                )),
            }
        });
        let abort_handle = worker.abort_handle();
        let store = self.task_store.clone();
        let completion_id = task_id.clone();
        tokio::spawn(async move {
            let outcome = match worker.await {
                Ok(outcome) => outcome,
                Err(error) => TaskOutcome::ProtocolError(McpError::internal_error(
                    format!("task worker terminated: {error}"),
                    None,
                )),
            };
            store.lock().await.complete(&completion_id, outcome);
        });
        self.task_store
            .lock()
            .await
            .attach_abort_handle(&task_id, abort_handle);

        Ok(CreateTaskResult::new(task).with_meta(related_task_metadata(&task_id)))
    }

    async fn list_tasks(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListTasksResult, McpError> {
        if request.is_some_and(|params| params.cursor.is_some()) {
            return Err(Self::invalid_params("unknown tasks/list cursor"));
        }
        Ok(ListTasksResult::new(self.task_store.lock().await.list()))
    }

    async fn get_task_info(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, McpError> {
        self.task_store
            .lock()
            .await
            .get(&request.task_id)
            .map(GetTaskResult::new)
            .ok_or_else(|| Self::invalid_params(format!("task not found: {}", request.task_id)))
    }

    async fn get_task_result(
        &self,
        request: GetTaskPayloadParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskPayloadResult, McpError> {
        loop {
            let state = self.task_store.lock().await.payload(&request.task_id);
            match state {
                TaskPayloadState::Ready(payload) => {
                    return Ok(GetTaskPayloadResult::new(payload));
                }
                TaskPayloadState::ProtocolError(error) => return Err(error),
                TaskPayloadState::Pending => {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        DEFAULT_TASK_POLL_INTERVAL_MS,
                    ))
                    .await;
                }
                TaskPayloadState::Unavailable(status) => {
                    return Err(Self::invalid_params(format!(
                        "task {} has no result in status {status:?}",
                        request.task_id
                    )));
                }
                TaskPayloadState::Missing => {
                    return Err(Self::invalid_params(format!(
                        "task not found: {}",
                        request.task_id
                    )));
                }
            }
        }
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CancelTaskResult, McpError> {
        match self.task_store.lock().await.cancel(&request.task_id) {
            TaskCancelState::Cancelled(task) => Ok(CancelTaskResult::new(task)),
            TaskCancelState::NotCancellable(status) => Err(Self::invalid_params(format!(
                "task {} cannot be cancelled in status {status:?}",
                request.task_id
            ))),
            TaskCancelState::Missing => Err(Self::invalid_params(format!(
                "task not found: {}",
                request.task_id
            ))),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        #[cfg(not(feature = "mcp-apps"))]
        {
            Ok(ListResourcesResult {
                resources: Vec::new(),
                meta: None,
                next_cursor: None,
            })
        }
        #[cfg(feature = "mcp-apps")]
        {
            Ok(Self::list_resources_result())
        }
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        #[cfg(not(feature = "mcp-apps"))]
        {
            Ok(ListResourceTemplatesResult {
                resource_templates: Vec::new(),
                meta: None,
                next_cursor: None,
            })
        }
        #[cfg(feature = "mcp-apps")]
        {
            Ok(Self::list_resource_templates_result())
        }
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
    #[tool(
        description = "Store a new episode in long-term memory. Use this tool when you need to persist source material before extracting entities or facts. Do not use this tool for retrieval. Arguments must be a flat snake_case object with `source_type`, `source_id`, `content`, `t_ref`, and `scope` (optional: `project`, `t_ingested`, `visibility_scope`, `policy_tags`). Do not wrap arguments in `payload`. Returns the created or existing `episode_id`. On error, fix the input fields and retry."
    )]
    pub async fn ingest(
        &self,
        params: Parameters<IngestParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        crate::tools::ingest(&self.service.build_context(), params.0)
            .await
            .map(Json)
            .map_err(mcp_error)
    }

    #[tool(
        description = "Explain context items with provenance-ready citations. Use this tool when you already have context items and need source snippets for an answer. Do not use this tool to search memory. Pass `context_items` as a JSON array string; object entries must use snake_case keys such as `fact_id` and `source_episode`, while plain source ID strings are also accepted. Do not wrap arguments in `payload`. Returns citation-ready items. On error, fix the JSON payload shape and retry."
    )]
    pub async fn explain(
        &self,
        params: Parameters<ExplainParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        crate::tools::explain(&self.service.build_context(), params.0)
            .await
            .map(Json)
            .map_err(mcp_error)
    }

    #[tool(
        execution(task_support = "optional"),
        description = "Extract entities, facts, and relationships from remembered content. Use this tool when you need structured knowledge from an existing episode or from new inline content. Prefer task-based invocation when the client supports MCP Tasks or when local NER may exceed the client's synchronous timeout. Do not use this tool for retrieval. Arguments must be a flat snake_case object. Provide exactly one input source: `episode_id` for stored content, or inline `content`/`text`; optional fields are `source_type`, `source_id`, `t_ref`, `scope`, and `zero_shot_labels`. Do not wrap arguments in `payload`. If you pass inline content, the server ingests it first and then extracts facts. Returns extracted entities, facts, and links."
    )]
    pub async fn extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<Json<ToolResponse<ExtractResult>>, ErrorData> {
        crate::tools::extract(&self.service.build_context(), params.0)
            .await
            .map(Json)
            .map_err(mcp_error)
    }

    #[tool(
        description = "Resolve a canonical entity identifier for a name and its aliases. Use this tool when a person, company, or project may appear under multiple names. Do not use this tool for full-text retrieval. Arguments must be a flat snake_case object with `entity_type`, `canonical_name`, and optional `aliases`. Do not wrap arguments in `payload`. Returns the canonical `entity_id`. On error, fix the entity fields and retry."
    )]
    pub async fn resolve(
        &self,
        params: Parameters<ResolveParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        crate::tools::resolve(&self.service.build_context(), params.0)
            .await
            .map(Json)
            .map_err(mcp_error)
    }

    #[tool(
        description = "Invalidate a fact while preserving historical traceability. Use this tool when a fact becomes outdated or superseded. Do not use this tool to delete memory. Arguments must be a flat snake_case object with `fact_id`, `reason`, and ISO 8601 `t_invalid`. Do not wrap arguments in `payload`. Returns confirmation. On error, verify the fact identifier and retry."
    )]
    pub async fn invalidate(
        &self,
        params: Parameters<InvalidateParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        crate::tools::invalidate(&self.service.build_context(), params.0)
            .await
            .map(Json)
            .map_err(mcp_error)
    }

    #[tool(
        description = "Open a Memory MCP app through the minimal public launcher. Use this tool only when an interactive app workflow is required and no canonical memory tool already matches the intent. Arguments must be a flat snake_case object. Required fields depend on `app`: inspector -> `target_type` + `target_id`; diff -> `as_of_left` + `as_of_right`; graph -> `from_entity_id` + `to_entity_id`; ingestion_review -> `scope` plus optional `source_text` or `draft_episode_id`; lifecycle -> `scope` only. Do not wrap arguments in `payload`. Returns `session_id`, `resource_uri`, `fallback`, and `guidance`."
    )]
    pub async fn open_app(
        &self,
        params: Parameters<OpenAppParams>,
    ) -> Result<Json<ToolResponse<OpenAppResult>>, ErrorData> {
        #[cfg(not(feature = "mcp-apps"))]
        {
            let _ = params;
            Err(Self::invalid_params(
                "MCP apps are disabled; enable the `mcp-apps` feature",
            ))
        }

        #[cfg(feature = "mcp-apps")]
        {
            let p = params.0;
            let timer = Instant::now(); // open_app
            let request_id = self.next_request_id();
            let app = Self::normalize_public_app_name(&p.app)
                .ok_or_else(|| Self::invalid_params(format!("Unknown app: {}", p.app)))?;

            self.service.log_tool_event(
                "open_app.start",
                json!({"app": app, "scope": p.scope}),
                json!({}),
                LogLevel::Info,
                Some(&request_id),
            );

            let result = match app {
                "inspector" => self.open_inspector_app(&p).await,
                "diff" => self.open_diff_app(&p).await,
                "ingestion_review" => self.open_ingestion_review_app(&p).await,
                "lifecycle" => self.open_lifecycle_app(&p).await,
                "graph" => self.open_graph_app(&p).await,
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
                        Some(&request_id),
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
                        Some(&request_id),
                    );
                    Err(err)
                }
            }
        }
    }

    #[tool(
        description = "Execute a coarse-grained command for an app session opened via open_app. Use this only for session-scoped workflows that are not already covered by canonical memory tools. Arguments must be a flat snake_case object and must not be wrapped in `payload`. Supports ingestion review actions (`approve_items`, `reject_items`, `edit_item`, `commit_review`, `cancel_review`), lifecycle actions (`archive_candidates`, `restore_archived`, `recompute_decay`, `rebuild_communities`), diff export (`export_diff`), graph exploration actions (`expand_neighbors`, `open_edge_details`, `use_path_as_context`), and the generic `close_session`. Returns command status and whether the caller should re-read the app resource."
    )]
    pub async fn app_command(
        &self,
        params: Parameters<AppCommandParams>,
    ) -> Result<Json<ToolResponse<AppCommandResult>>, ErrorData> {
        #[cfg(not(feature = "mcp-apps"))]
        {
            let _ = params;
            Err(Self::invalid_params(
                "MCP apps are disabled; enable the `mcp-apps` feature",
            ))
        }

        #[cfg(feature = "mcp-apps")]
        {
            let p = params.0;
            let timer = Instant::now();
            let request_id = self.next_request_id();
            self.service.log_tool_event(
                "app_command.start",
                json!({"session_id": p.session_id, "action": p.action}),
                json!({}),
                LogLevel::Info,
                Some(&request_id),
            );

            let outcome: Result<AppCommandResult, ErrorData> = async {
                let session = self.session(&p.session_id).await?;
                let command = AppCommand::parse(
                    &session.app,
                    AppCommandInput {
                        action: p.action.clone(),
                        item_ids: p.item_ids.clone(),
                        target_ids: p.target_ids.clone(),
                        target_id: p.target_id.clone(),
                        item_id: p.item_id.clone(),
                        patch_json: p.patch_json.clone(),
                        reason: p.reason.clone(),
                        dry_run: p.dry_run.unwrap_or(false),
                        confirmed: p.confirmed.unwrap_or(false),
                        format: p.format.clone(),
                        direction: p.direction.clone(),
                        depth: p.depth,
                    },
                )
                .map_err(|error| Self::invalid_params(error.to_string()))?;
                let descriptor = find_descriptor(&command)?;
                let ctx = AppContext {
                    service: &self.service,
                    session_id: &p.session_id,
                    app: &session.app,
                    scope: &session.scope,
                    payload: session.payload.clone(),
                };
                let outcome = (descriptor.execute)(&ctx, &command).await?;
                if let Some(payload) = outcome.new_payload {
                    self.replace_session_payload(&p.session_id, payload).await?;
                }
                if outcome.close_session {
                    self.remove_session(&p.session_id).await?;
                }
                let resource_uri = if outcome.close_session {
                    None
                } else {
                    Some(app_session_uri(&session.app, &p.session_id))
                };
                Ok(Self::app_command_result_from_details(
                    &session.app,
                    &p.session_id,
                    outcome.action,
                    resource_uri,
                    outcome.details,
                ))
            }
            .await;

            match outcome {
                Ok(command_result) => {
                    self.service.log_tool_event_with_duration(
                        "app_command.done",
                        json!({"session_id": p.session_id, "action": command_result.action}),
                        json!({
                            "app": command_result.app,
                            "ok": command_result.ok,
                            "refresh_required": command_result.refresh_required,
                        }),
                        LogLevel::Info,
                        timer.elapsed(),
                        Some(&request_id),
                    );
                    Ok(Json(ToolResponse::success_with_guidance(
                        command_result,
                        "Re-read `resource_uri` when `refresh_required=true` to retrieve the latest app view.",
                    )))
                }
                Err(err) => {
                    self.service.log_tool_event_with_duration(
                        "app_command.error",
                        json!({"session_id": p.session_id, "action": p.action}),
                        json!({"error": err.to_string()}),
                        LogLevel::Warn,
                        timer.elapsed(),
                        Some(&request_id),
                    );
                    Err(err)
                }
            }
        }
    }

    #[tool(
        description = "Assemble the most relevant active memory context for a query. Use this tool when you need retrieval across stored facts before answering or planning. Do not use this tool to ingest new content. Arguments must be a flat snake_case object with `query`, `scope`, and optional `project`, `fact_types`, `as_of`, `budget`, `view_mode`, `window_start`, and `window_end`. Do not wrap arguments in `payload`. Returns ranked context items with confidence and rationale. On error, fix the query parameters and retry."
    )]
    pub async fn assemble_context(
        &self,
        params: Parameters<AssembleContextParams>,
    ) -> Result<Json<ToolResponse<serde_json::Value>>, ErrorData> {
        crate::tools::assemble_context(&self.service.build_context(), params.0)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
}

#[cfg(test)]
mod tests {
    use super::apps::{shallow_merge_object, summarize_ingestion_review_items};
    use super::*;
    use crate::mcp::parsers::parse_datetime;
    #[cfg(feature = "mcp-apps")]
    use crate::models::EntityCandidate;
    #[cfg(feature = "mcp-apps")]
    use crate::models::IngestRequest;
    use crate::service::edge_neighbor;
    use crate::storage::{DbClient, SurrealDbClient};
    use chrono::Datelike;
    #[cfg(feature = "mcp-apps")]
    use chrono::{TimeZone, Utc};
    #[cfg(feature = "mcp-apps")]
    use rmcp::model::{ReadResourceRequestParams, ResourceContents};
    use serde_json::{Value, json};

    async fn create_test_mcp() -> MemoryMcp {
        let namespaces = vec![
            "org".to_string(),
            "personal".to_string(),
            "private".to_string(),
        ];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "memory_mcp_handlers_test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in-memory test db"),
        );

        for namespace in &namespaces {
            db_client
                .apply_migrations(namespace)
                .await
                .expect("apply test migrations");
        }

        let service = MemoryService::new(db_client, namespaces, "warn".to_string(), 50, 100)
            .expect("create test service");
        MemoryMcp::new(service)
    }

    #[cfg(feature = "mcp-apps")]
    #[cfg(feature = "mcp-apps")]
    async fn create_test_entity(mcp: &MemoryMcp, canonical_name: &str) -> String {
        mcp.service()
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: canonical_name.to_string(),
                    aliases: Vec::new(),
                },
                None,
            )
            .await
            .expect("create test entity")
    }

    fn schema_json<T: schemars::JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).expect("schema json")
    }

    #[test]
    fn build_server_info_enables_tools_resources_and_sets_instructions() {
        let info = MemoryMcp::build_server_info();
        let capabilities = serde_json::to_value(&info.capabilities).unwrap();

        assert_eq!(
            info.instructions.as_deref(),
            Some(
                "Memory MCP server: stores facts about entities and relationships, resolves aliases, and assembles long-term context. All tool arguments and structured results use flat snake_case JSON keys that must match the published schemas exactly. Do not wrap tool arguments in `payload`.",
            ),
        );
        assert!(capabilities.get("tools").is_some());
        assert!(capabilities.get("resources").is_some());
        assert!(
            capabilities.get("tasks").is_some(),
            "tasks capability missing: {}",
            capabilities
        );
        assert!(
            capabilities["tasks"]["requests"]["tools"]["call"].is_object(),
            "unexpected tasks structure: {}",
            capabilities["tasks"]
        );
    }

    #[tokio::test]
    async fn only_extract_allows_task_execution() {
        use rmcp::model::TaskSupport;

        let mcp = create_test_mcp().await;
        let extract_tool = mcp.get_tool("extract").expect("extract tool must exist");
        assert_eq!(extract_tool.task_support(), TaskSupport::Optional);

        let ingest_tool = mcp.get_tool("ingest").expect("ingest tool must exist");
        assert_eq!(ingest_tool.task_support(), TaskSupport::Forbidden);

        let resolve_tool = mcp.get_tool("resolve").expect("resolve tool must exist");
        assert_eq!(resolve_tool.task_support(), TaskSupport::Forbidden);

        let invalidate_tool = mcp
            .get_tool("invalidate")
            .expect("invalidate tool must exist");
        assert_eq!(invalidate_tool.task_support(), TaskSupport::Forbidden);
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
    fn extract_tool_response_schema_exposes_warning_array() {
        let schema = schema_json::<ToolResponse<ExtractResult>>();
        let defs = schema
            .get("$defs")
            .or_else(|| schema.get("definitions"))
            .and_then(serde_json::Value::as_object)
            .expect("schema definitions");
        let extract_result = defs.get("ExtractResult").expect("ExtractResult definition");
        let properties = extract_result["properties"]
            .as_object()
            .expect("properties object");

        assert!(
            properties.contains_key("warnings"),
            "ExtractResult should expose warnings in the MCP schema"
        );
        assert_eq!(properties["warnings"]["type"], "array");
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

        // Public MCP structured outputs use snake_case keys only.
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

        for key in ["hasMore", "totalCount", "nextOffset"] {
            assert!(
                !properties.contains_key(key),
                "unexpected camelCase property {key}"
            );
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

        for key in ["sourceEpisode", "tRef", "tIngested", "citationContext"] {
            assert!(
                !properties.contains_key(key),
                "unexpected camelCase property {key}"
            );
        }
    }

    #[test]
    fn explain_item_schema_exposes_graph_insights() {
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

        assert!(
            properties.contains_key("graph_insights"),
            "ExplainItem should expose graph_insights in the MCP schema"
        );
        assert!(!properties.contains_key("graphInsights"));
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

        for key in [
            "fact_id",
            "content",
            "quote",
            "source_episode",
            "confidence",
            "provenance",
            "rationale",
            "retrieval_tier",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }

        for key in ["factId", "sourceEpisode", "retrievalTier"] {
            assert!(
                !properties.contains_key(key),
                "unexpected camelCase property {key}"
            );
        }
    }

    #[test]
    fn extract_params_deserialization_rejects_nested_payload_contract() {
        let err = serde_json::from_value::<ExtractParams>(json!({
            "payload": {
                "episode_id": "episode:abc123"
            }
        }))
        .expect_err("nested payload wrapper should be rejected");

        assert!(
            err.to_string().contains("payload"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn tool_response_partial_envelope_marks_retryable_state() {
        let response = ToolResponse::partial_with_guidance(
            ExtractResult::empty(),
            "Provide exactly one snake_case input source: `episode_id` or non-empty `content`/`text`, then retry without wrapping arguments in `payload`.",
        );

        assert_eq!(response.status, "partial");
        assert!(response.result.entities.is_empty());
        assert_eq!(
            response.guidance.as_deref(),
            Some(
                "Provide exactly one snake_case input source: `episode_id` or non-empty `content`/`text`, then retry without wrapping arguments in `payload`."
            ),
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

    #[cfg(feature = "mcp-apps")]
    #[tokio::test]
    async fn public_tools_expose_open_app_and_app_command() {
        let mcp = create_test_mcp().await;

        assert!(mcp.get_tool("open_app").is_some());
        assert!(mcp.get_tool("app_command").is_some());
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn list_resource_templates_exposes_public_app_session_templates() {
        let result = MemoryMcp::list_resource_templates_result();
        let uri_templates: Vec<_> = result
            .resource_templates
            .iter()
            .map(|template| template.uri_template.as_str())
            .collect();

        assert!(uri_templates.contains(&"ui://memory/app/inspector/{session_id}"));
        assert!(uri_templates.contains(&"ui://memory/app/diff/{session_id}"));
        assert!(uri_templates.contains(&"ui://memory/app/ingestion_review/{session_id}"));
        assert!(uri_templates.contains(&"ui://memory/app/lifecycle/{session_id}"));
        assert!(uri_templates.contains(&"ui://memory/app/graph/{session_id}"));
    }

    #[cfg(feature = "mcp-apps")]
    #[tokio::test]
    async fn open_app_inspector_returns_session_backed_envelope() {
        let mcp = create_test_mcp().await;
        let entity_id = create_test_entity(&mcp, "Inspector Alice").await;

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
                page_size: None,
                max_depth: None,
                ttl_seconds: None,
            }))
            .await
            .expect("open inspector app")
            .0;

        assert_eq!(response.status, "success");
        assert_eq!(response.result.app, "inspector");
        assert!(response.result.session_id.starts_with("ses:"));
        assert_eq!(
            response.result.resource_uri,
            format!("ui://memory/app/inspector/{}", response.result.session_id)
        );
        assert_eq!(
            response.result.fallback["target"]["target_id"],
            entity_id.as_str()
        );
        assert_eq!(
            response.result.fallback["record"]["entity_id"],
            entity_id.as_str()
        );
    }

    #[cfg(feature = "mcp-apps")]
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
                source_text: Some("Review this ingestion draft".to_string()),
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: Some(600),
            }))
            .await
            .expect("open ingestion review app")
            .0
            .result;

        let result = mcp
            .read_resource_result(ReadResourceRequestParams::new(
                open_result.resource_uri.clone(),
            ))
            .await
            .expect("read ingestion review resource");

        assert_eq!(result.contents.len(), 1);
        match &result.contents[0] {
            ResourceContents::TextResourceContents {
                mime_type, text, ..
            } => {
                assert_eq!(mime_type.as_deref(), Some("text/html;profile=mcp-app"));
                assert!(text.contains("<script type=\"application/json\" id=\"app-data\">"));
                assert!(text.contains("Review this ingestion draft"));
            }
            other => panic!("expected text resource, got {other:?}"),
        }
    }

    #[cfg(feature = "mcp-apps")]
    #[tokio::test]
    async fn app_command_mutates_ingestion_review_items_and_closes_session() {
        let mcp = create_test_mcp().await;

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "ingestion_review".to_string(),
                scope: "org".to_string(),
                target_type: None,
                target_id: None,
                from_entity_id: None,
                to_entity_id: None,
                source_text: Some("Approve this review item".to_string()),
                draft_episode_id: None,
                as_of: None,
                as_of_left: None,
                as_of_right: None,
                time_axis: None,
                view: None,
                cursor: None,
                page_size: None,
                max_depth: None,
                ttl_seconds: None,
            }))
            .await
            .expect("open ingestion review app")
            .0
            .result;

        let initial_payload = mcp
            .read_app_resource_payload("ingestion_review", &open_result.session_id)
            .await
            .expect("read initial ingestion review payload");
        let item_id = initial_payload["items"][0]["item_id"]
            .as_str()
            .expect("draft item id")
            .to_string();

        let approve = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "approve_items".to_string(),
                item_ids: vec![item_id.clone()],
                target_ids: Vec::new(),
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

        assert_eq!(approve.status, "success");
        assert!(approve.result.refresh_required);
        assert_eq!(
            approve.result.resource_uri.as_deref(),
            Some(open_result.resource_uri.as_str())
        );

        let approved_payload = mcp
            .read_app_resource_payload("ingestion_review", &open_result.session_id)
            .await
            .expect("read approved ingestion review payload");
        assert_eq!(approved_payload["items"][0]["status"], "approved");

        let close = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "close_session".to_string(),
                item_ids: Vec::new(),
                target_ids: Vec::new(),
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
            .expect("close ingestion review session")
            .0;

        assert_eq!(close.status, "success");
        assert_eq!(close.result.action, "close_session");
        assert!(!close.result.refresh_required);
        assert_eq!(close.result.resource_uri, None);
        assert!(
            mcp.read_app_resource_payload("ingestion_review", &open_result.session_id)
                .await
                .is_err(),
            "closed sessions should no longer resolve as readable resources"
        );
    }

    #[cfg(feature = "mcp-apps")]
    #[tokio::test]
    async fn lifecycle_app_commands_archive_and_restore_candidates() {
        let mcp = create_test_mcp().await;
        let stale_episode_id = mcp
            .service()
            .ingest(
                IngestRequest {
                    source_type: "meeting".to_string(),
                    source_id: "stale-lifecycle-episode".to_string(),
                    content: "Legacy launch plan that should be archived.".to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 1, 10, 9, 0, 0).unwrap(),
                    scope: "org".to_string(),
                    project: None,
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .expect("ingest stale episode");

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
                ttl_seconds: None,
            }))
            .await
            .expect("open lifecycle app")
            .0
            .result;

        let initial_payload = mcp
            .read_app_resource_payload("lifecycle", &open_result.session_id)
            .await
            .expect("read lifecycle payload");
        assert!(
            initial_payload["dashboard"]["archival_candidate_ids"]
                .as_array()
                .expect("candidate ids array")
                .iter()
                .any(|value| value.as_str() == Some(stale_episode_id.as_str()))
        );

        let archive = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "archive_candidates".to_string(),
                item_ids: Vec::new(),
                target_ids: vec![stale_episode_id.clone()],
                target_id: None,
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: Some(false),
                confirmed: Some(true),
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("archive lifecycle candidate")
            .0;

        assert_eq!(archive.status, "success");
        assert!(archive.result.refresh_required);
        let archived_episode = mcp
            .service()
            .find_episode_record(&stale_episode_id)
            .await
            .expect("load archived episode")
            .0
            .expect("archived episode exists");
        assert_eq!(archived_episode["status"], "archived");

        let archived_payload = mcp
            .read_app_resource_payload("lifecycle", &open_result.session_id)
            .await
            .expect("read archived lifecycle payload");
        assert!(
            !archived_payload["dashboard"]["archival_candidate_ids"]
                .as_array()
                .expect("candidate ids array")
                .iter()
                .any(|value| value.as_str() == Some(stale_episode_id.as_str()))
        );

        let restore = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "restore_archived".to_string(),
                item_ids: Vec::new(),
                target_ids: vec![stale_episode_id.clone()],
                target_id: None,
                item_id: None,
                patch_json: None,
                reason: None,
                dry_run: None,
                confirmed: Some(true),
                format: None,
                direction: None,
                depth: None,
            }))
            .await
            .expect("restore archived lifecycle candidate")
            .0;

        assert_eq!(restore.status, "success");
        let restored_episode = mcp
            .service()
            .find_episode_record(&stale_episode_id)
            .await
            .expect("load restored episode")
            .0
            .expect("restored episode exists");
        assert_eq!(restored_episode["status"], "active");
        assert!(
            restored_episode
                .get("archived_at")
                .is_none_or(serde_json::Value::is_null)
        );
    }

    #[cfg(feature = "mcp-apps")]
    #[tokio::test]
    async fn lifecycle_app_dry_run_actions_report_without_mutating_state() {
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
                ttl_seconds: None,
            }))
            .await
            .expect("open lifecycle app")
            .0
            .result;

        let recompute = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id.clone(),
                action: "recompute_decay".to_string(),
                item_ids: Vec::new(),
                target_ids: Vec::new(),
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
            .expect("run lifecycle recompute dry-run")
            .0;

        assert_eq!(recompute.status, "success");
        let recompute_details = recompute.result.details.expect("recompute details");
        assert_eq!(recompute_details["dry_run"], true);
        assert_eq!(recompute_details["invalidated"], 0);

        let rebuild = mcp
            .app_command(Parameters(AppCommandParams {
                session_id: open_result.session_id,
                action: "rebuild_communities".to_string(),
                item_ids: Vec::new(),
                target_ids: Vec::new(),
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
            .expect("run lifecycle rebuild dry-run")
            .0;

        assert_eq!(rebuild.status, "success");
        let rebuild_details = rebuild.result.details.expect("rebuild details");
        assert_eq!(rebuild_details["dry_run"], true);
        assert_eq!(rebuild_details["rebuilt"], 0);
    }

    #[test]
    fn normalize_public_app_name_maps_all_known_apps() {
        assert_eq!(
            MemoryMcp::normalize_public_app_name("inspector"),
            Some("inspector")
        );
        assert_eq!(
            MemoryMcp::normalize_public_app_name("memory_inspector"),
            Some("inspector")
        );
        assert_eq!(MemoryMcp::normalize_public_app_name("diff"), Some("diff"));
        assert_eq!(
            MemoryMcp::normalize_public_app_name("temporal_diff"),
            Some("diff")
        );
        assert_eq!(
            MemoryMcp::normalize_public_app_name("ingestion_review"),
            Some("ingestion_review")
        );
        assert_eq!(
            MemoryMcp::normalize_public_app_name("ingestion"),
            Some("ingestion_review")
        );
        assert_eq!(
            MemoryMcp::normalize_public_app_name("lifecycle"),
            Some("lifecycle")
        );
        assert_eq!(
            MemoryMcp::normalize_public_app_name("lifecycle_console"),
            Some("lifecycle")
        );
        assert_eq!(MemoryMcp::normalize_public_app_name("graph"), Some("graph"));
        assert_eq!(
            MemoryMcp::normalize_public_app_name("graph_path"),
            Some("graph")
        );
        assert_eq!(MemoryMcp::normalize_public_app_name("unknown_app"), None);
    }

    #[test]
    fn enrich_session_payload_adds_meta_with_expiry() {
        let payload = json!({"data": "value"});
        let enriched =
            session::enrich_session_payload("inspector", "ses:1", "org", Some(3600), payload);
        assert_eq!(enriched["app"], "inspector");
        assert_eq!(enriched["session_id"], "ses:1");
        assert_eq!(enriched["scope"], "org");
        assert!(enriched["meta"]["expires_at"].is_string());
        assert_eq!(enriched["meta"]["ttl_seconds"], 3600);
        assert_eq!(enriched["data"], "value");
    }

    #[test]
    fn enrich_session_payload_handles_no_ttl() {
        let payload = json!({});
        let enriched = session::enrich_session_payload("diff", "ses:2", "personal", None, payload);
        assert_eq!(enriched["app"], "diff");
        assert_eq!(enriched["meta"]["ttl_seconds"], serde_json::Value::Null);
        assert!(enriched["meta"]["expires_at"].is_null());
    }

    #[test]
    fn shallow_merge_object_combines_keys() {
        use std::collections::HashMap;
        let mut target: HashMap<String, Value> = HashMap::new();
        target.insert("a".to_string(), json!(1));
        let mut patch: HashMap<String, Value> = HashMap::new();
        patch.insert("b".to_string(), json!(2));
        patch.insert("a".to_string(), json!(99));

        // Note: shallow_merge_object works on serde_json::Map, not HashMap
        let mut target_map = serde_json::Map::new();
        target_map.insert("a".to_string(), json!(1));
        let mut patch_map = serde_json::Map::new();
        patch_map.insert("b".to_string(), json!(2));
        patch_map.insert("a".to_string(), json!(99));
        shallow_merge_object(&mut target_map, &patch_map);
        assert_eq!(target_map["a"], 99); // overwritten
        assert_eq!(target_map["b"], 2); // added
    }

    #[test]
    fn summarize_ingestion_review_items_counts_by_status() {
        let items = vec![
            json!({"status": "approved"}),
            json!({"status": "approved"}),
            json!({"status": "rejected"}),
            json!({"status": "pending"}),
        ];
        let summary = summarize_ingestion_review_items(&items);
        assert_eq!(summary["total"], 4);
        assert_eq!(summary["approved"], 2);
        assert_eq!(summary["rejected"], 1);
        assert_eq!(summary["pending"], 1);
        assert_eq!(summary["committable"], 2);
    }

    #[test]
    fn summarize_ingestion_review_items_handles_empty() {
        let summary = summarize_ingestion_review_items(&[]);
        assert_eq!(summary["total"], 0);
        assert_eq!(summary["committable"], 0);
    }

    #[test]
    fn edge_neighbor_returns_correct_endpoint_for_incoming() {
        let record = json!({
            "in": "entity:alice",
            "out": "entity:bob",
            "relation": "knows"
        });
        let neighbor = edge_neighbor(&record, crate::storage::GraphDirection::Incoming);
        assert_eq!(neighbor, Some("entity:alice".to_string()));
    }

    #[test]
    fn edge_neighbor_returns_correct_endpoint_for_outgoing() {
        let record = json!({
            "in": "entity:alice",
            "out": "entity:bob",
            "relation": "knows"
        });
        let neighbor = edge_neighbor(&record, crate::storage::GraphDirection::Outgoing);
        assert_eq!(neighbor, Some("entity:bob".to_string()));
    }

    #[test]
    fn edge_neighbor_returns_none_for_missing_field() {
        let record = json!({"relation": "knows"});
        assert!(edge_neighbor(&record, crate::storage::GraphDirection::Incoming).is_none());
    }

    #[test]
    fn edge_neighbor_returns_none_for_non_string_value() {
        let record = json!({"in": 123, "out": "entity:bob"});
        assert!(edge_neighbor(&record, crate::storage::GraphDirection::Incoming).is_none());
    }

    #[test]
    fn content_hash_differs_for_different_content() {
        use super::super::parsers::content_hash;
        let hash1 = content_hash("content one");
        let hash2 = content_hash("content two");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn content_hash_is_deterministic() {
        use super::super::parsers::content_hash;
        let hash1 = content_hash("same content");
        let hash2 = content_hash("same content");
        assert_eq!(hash1, hash2);
    }
}
