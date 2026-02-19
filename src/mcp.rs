use std::sync::Arc;

use chrono::{DateTime, Utc};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ErrorCode, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;

use crate::logging::LogLevel;
use crate::models::{
    AccessContext, AssembleContextRequest, EntityCandidate, ExplainRequest, IngestRequest,
    InvalidateRequest,
};
use crate::service::{MemoryError, MemoryService};

/// Macro for instrumenting tool operations with logging.
///
/// This macro wraps an async block with start/done/error logging.
///
/// # Example
///
/// ```rust,ignore
/// let result = instrument_tool!(self, "ingest", args, {
///     self.service.ingest(request, access).await
/// });
/// ```
#[macro_export]
macro_rules! instrument_tool {
    ($self:expr, $op:expr, $args:expr, $body:expr) => {{
        $self.service.log_tool_event(
            concat!($op, ".start"),
            $args,
            json!({}),
            LogLevel::Info,
        );
        match $body {
            Ok(result) => {
                $self.service.log_tool_event(
                    concat!($op, ".done"),
                    json!({}),
                    json!({"result": &result}),
                    LogLevel::Info,
                );
                Ok(result)
            }
            Err(err) => {
                $self.service.log_tool_event(
                    concat!($op, ".error"),
                    json!({}),
                    json!({"error": err.to_string()}),
                    LogLevel::Warn,
                );
                Err(err)
            }
        }
    }};
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

/// Response wrapper for tool results.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ToolResponse<T> {
    /// The actual result data.
    pub result: T,
}

impl MemoryMcp {
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
    pub fn service(&self) -> Arc<MemoryService> {
        self.service.clone()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MemoryMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Memory MCP server: stores, extracts, resolves, and assembles long-term context."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
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
                ErrorCode::INVALID_PARAMS,
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

        // Tool-level start
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
            .map_err(|msg| ErrorData::new(ErrorCode::INVALID_PARAMS, msg, None))?;
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

    /// Shared implementation for extract operations (used by extract and extract_entities).
    /// Handles the common logic of extracting from episode_id or ingesting content first.
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
            let result = empty_extract_result("no_input", "episode_id or content/text is required");
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
                true, // enable logging
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
        // Parse aliases: support comma-separated string or JSON array
        let aliases: Vec<String> = if p.aliases.starts_with('[') {
            serde_json::from_str(&p.aliases).unwrap_or_default()
        } else {
            p.aliases
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
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
                ErrorCode::INVALID_PARAMS,
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
                ErrorCode::INVALID_PARAMS,
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
                false, // disable detailed logging for alias
            )
            .await?;
        Ok(Json(ToolResponse { result }))
    }

    #[tool(description = "Find the canonical ID for an entity (alias for resolve).")]
    pub async fn resolve_entity(
        &self,
        params: Parameters<ResolveParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        let p = params.0;
        let access = AccessContext::default();
        // Parse aliases: try JSON array first, then comma-separated
        let aliases: Vec<String> = if p.aliases.starts_with('[') {
            serde_json::from_str(&p.aliases).unwrap_or_default()
        } else {
            p.aliases
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
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
                ErrorCode::INVALID_PARAMS,
                "start is required".to_string(),
                None,
            )
        })?;
        let end = parse_datetime(&p.end).ok_or_else(|| {
            ErrorData::new(
                ErrorCode::INVALID_PARAMS,
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

// === FLAT Params for OpenAI schema compatibility ===
// All *Params structs use primitive types only (no nested structs)

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IngestParams {
    /// Type of source (e.g., "email", "tfs_work_item", "document")
    pub source_type: String,
    /// Unique identifier for the source
    pub source_id: String,
    /// Content to ingest
    pub content: String,
    /// Reference timestamp (ISO 8601 format)
    pub t_ref: String,
    /// Scope (default: "org")
    #[serde(default = "default_scope")]
    pub scope: String,
    /// Ingestion timestamp (ISO 8601 format, optional)
    pub t_ingested: Option<String>,
    /// Visibility scope (optional)
    pub visibility_scope: Option<String>,
    /// Policy tags (optional)
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainParams {
    /// JSON array of context items to explain
    pub context_items: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractParams {
    /// Episode ID to extract from (optional if content provided)
    pub episode_id: Option<String>,
    /// Content to analyze (optional if episode_id provided)
    pub content: Option<String>,
    /// Alternative content field
    pub text: Option<String>,
    /// Source type (default: "ad-hoc")
    pub source_type: Option<String>,
    /// Source ID (optional)
    pub source_id: Option<String>,
    /// Reference timestamp (ISO 8601 format, optional)
    pub t_ref: Option<String>,
    /// Scope (default: "org")
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolveParams {
    /// Type of entity (e.g., "person", "project", "company")
    pub entity_type: String,
    /// Canonical name for the entity
    pub canonical_name: String,
    /// Known aliases (comma-separated or JSON array string)
    #[serde(default)]
    pub aliases: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvalidateParams {
    /// ID of the fact to invalidate
    pub fact_id: String,
    /// Reason for invalidation
    pub reason: String,
    /// Timestamp when fact became invalid (ISO 8601 format)
    pub t_invalid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssembleContextParams {
    pub query: String,
    pub scope: String,
    #[serde(default)]
    pub as_of: String,
    #[serde(default = "default_budget")]
    pub budget: i32,
}

fn default_budget() -> i32 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AliasIngestParams {
    /// Type of source
    pub source_type: String,
    /// Unique source ID
    pub source_id: String,
    /// Content to ingest
    pub content: String,
    /// Reference timestamp (ISO 8601)
    pub t_ref: String,
    /// Scope
    #[serde(default = "default_scope")]
    pub scope: String,
    /// Ingestion timestamp (optional)
    pub t_ingested: Option<String>,
    /// Visibility scope (optional)
    pub visibility_scope: Option<String>,
    /// Policy tags (optional)
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AliasExtractParams {
    /// Episode ID (optional if content provided)
    pub episode_id: Option<String>,
    /// Content to analyze (optional)
    pub content: Option<String>,
    /// Alternative content field
    pub text: Option<String>,
    /// Source type
    pub source_type: Option<String>,
    /// Source ID
    pub source_id: Option<String>,
    /// Reference timestamp (ISO 8601)
    pub t_ref: Option<String>,
    /// Scope
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskParams {
    /// Task title
    pub title: String,
    /// Due date (ISO 8601 format, optional)
    pub due_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SendMessageParams {
    /// Recipient
    pub to: Option<String>,
    /// Subject line
    pub subject: Option<String>,
    /// Message body
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleMeetingParams {
    /// Meeting title
    pub title: Option<String>,
    /// Start time (ISO 8601 format)
    pub start: String,
    /// End time (ISO 8601 format)
    pub end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateMetricParams {
    /// Metric name
    pub name: Option<String>,
    /// Metric value
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UiParams {
    #[serde(rename = "_")]
    marker: Option<Value>,
}

/// Parse `context_items` JSON string into `Vec<ExplainItem>`.
///
/// Accepted input formats (all must be a JSON array):
///   1. Strict: `[{"content":"…","quote":"…","source_episode":"episode:xxx"}]`
///   2. Array of id strings: `["episode:xxx","task:yyy"]`
///   3. Loose objects: `[{"content":"…","id":"task:xxx","source_type":"task"}]`
///      — `id` is used as `source_episode` when absent.
///      `quote` and `content` default to `""` when absent.
///   4. Mixed: any combination of strings and objects in one array.
pub(crate) fn parse_context_items(raw: &str) -> Result<Vec<crate::models::ExplainItem>, String> {
    let values: Vec<Value> =
        serde_json::from_str(raw).map_err(|e| format!("Invalid context_items JSON: {e}"))?;

    let items = values
        .into_iter()
        .map(|v| match v {
            Value::String(s) => crate::models::ExplainItem {
                content: String::new(),
                quote: String::new(),
                source_episode: s,
            },
            Value::Object(ref map) => {
                let content = map
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let quote = map
                    .get("quote")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let source_episode = map
                    .get("source_episode")
                    .or_else(|| map.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                crate::models::ExplainItem {
                    content,
                    quote,
                    source_episode,
                }
            }
            _ => crate::models::ExplainItem {
                content: String::new(),
                quote: String::new(),
                source_episode: String::new(),
            },
        })
        .collect();

    Ok(items)
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn empty_extract_result(status: &str, hint: &str) -> Value {
    json!({
        "status": status,
        "hint": hint,
        "entities": [],
        "facts": [],
        "links": [],
    })
}

fn content_hash(content: &str) -> String {
    let digest = sha2::Sha256::digest(content.as_bytes());
    hex::encode(digest)[..16].to_string()
}

fn default_scope() -> String {
    "org".to_string()
}

fn mcp_error(err: MemoryError) -> ErrorData {
    let code = match err {
        MemoryError::Validation(_) => ErrorCode::INVALID_PARAMS,
        MemoryError::NotFound(_) => ErrorCode::INVALID_PARAMS,
        MemoryError::ConfigMissing(_) => ErrorCode::INVALID_REQUEST,
        MemoryError::ConfigInvalid(_) => ErrorCode::INVALID_REQUEST,
        MemoryError::Storage(_) => ErrorCode::INTERNAL_ERROR,
    };
    ErrorData::new(code, err.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::parse_context_items;

    // ── Shape 1: strict ExplainItem objects ──────────────────────────

    #[test]
    fn parse_strict_explain_items() {
        let raw = r#"[{"content":"alpha","quote":"beta","source_episode":"episode:abc"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "alpha");
        assert_eq!(items[0].quote, "beta");
        assert_eq!(items[0].source_episode, "episode:abc");
    }

    // ── Shape 2: array of id strings ─────────────────────────────────

    #[test]
    fn parse_array_of_id_strings() {
        let raw = r#"["episode:111","task:222"]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source_episode, "episode:111");
        assert_eq!(items[0].content, "");
        assert_eq!(items[1].source_episode, "task:222");
    }

    // ── Shape 3: loose objects with `id` and no `quote` ──────────────

    #[test]
    fn parse_loose_objects_id_no_quote() {
        let raw = r#"[{"content":"Follow up on ARR deal","id":"task:e8g","source_type":"task"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "Follow up on ARR deal");
        assert_eq!(items[0].quote, "");
        assert_eq!(items[0].source_episode, "task:e8g");
    }

    #[test]
    fn parse_loose_objects_with_quote_and_id() {
        let raw = r#"[{"content":"data","quote":"q","id":"task:abc","source_type":"task"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].content, "data");
        assert_eq!(items[0].quote, "q");
        assert_eq!(items[0].source_episode, "task:abc");
    }

    // ── Shape 4: source_episode takes priority over id ───────────────

    #[test]
    fn parse_source_episode_preferred_over_id() {
        let raw =
            r#"[{"content":"x","quote":"y","source_episode":"episode:real","id":"task:alt"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].source_episode, "episode:real");
    }

    // ── Shape 5: mixed array (strings + objects) ─────────────────────

    #[test]
    fn parse_mixed_array() {
        let raw = r#"["episode:aaa",{"content":"c","id":"task:bbb"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source_episode, "episode:aaa");
        assert_eq!(items[0].content, "");
        assert_eq!(items[1].source_episode, "task:bbb");
        assert_eq!(items[1].content, "c");
    }

    // ── Shape 6: empty array ─────────────────────────────────────────

    #[test]
    fn parse_empty_array() {
        let items = parse_context_items("[]").unwrap();
        assert!(items.is_empty());
    }

    // ── Shape 7: object with no id at all ────────────────────────────

    #[test]
    fn parse_object_no_id_fields() {
        let raw = r#"[{"content":"only content","source_type":"email"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].content, "only content");
        assert_eq!(items[0].source_episode, "");
    }

    // ── Error: not valid JSON ────────────────────────────────────────

    #[test]
    fn parse_invalid_json_errors() {
        assert!(parse_context_items("not json").is_err());
    }

    // ── Error: not an array ──────────────────────────────────────────

    #[test]
    fn parse_non_array_errors() {
        assert!(parse_context_items(r#"{"content":"x"}"#).is_err());
    }

    // ── Real-world payload (exact reproduction of reported bug) ──────

    #[test]
    fn parse_real_world_payload_without_quote_and_source_episode() {
        let raw = r#"[{"content":"Follow up on ARR deal","id":"task:e8gsmlprfchnktf6js0p","source_type":"task"},{"content":"ASSIGNEE: Anton Solovey — Split listed requirements","id":"task:ha8caz3sb2fxr9ju2sbc","source_type":"task"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].content, "Follow up on ARR deal");
        assert_eq!(items[0].source_episode, "task:e8gsmlprfchnktf6js0p");
        assert_eq!(items[0].quote, "");
        assert_eq!(items[1].source_episode, "task:ha8caz3sb2fxr9ju2sbc");
    }

    #[test]
    fn parse_real_world_payload_with_quote_without_source_episode() {
        let raw = r#"[{"content":"Follow up on ARR deal","quote":"Follow up on ARR deal","id":"task:e8gsmlprfchnktf6js0p","source_type":"task"},{"content":"ASSIGNEE: Anton Solovey","quote":"ASSIGNEE: Anton Solovey","id":"task:ha8caz3sb2fxr9ju2sbc","source_type":"task"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].quote, "Follow up on ARR deal");
        assert_eq!(items[0].source_episode, "task:e8gsmlprfchnktf6js0p");
        assert_eq!(items[1].quote, "ASSIGNEE: Anton Solovey");
        assert_eq!(items[1].source_episode, "task:ha8caz3sb2fxr9ju2sbc");
    }
}
