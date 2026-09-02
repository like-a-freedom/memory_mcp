//! MCP tool handler implementations.

use std::sync::Arc;

use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, CreateTaskResult,
    GetTaskParams, GetTaskResult, ListResourceTemplatesResult, ListResourcesResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities,
    ServerInfo, UpdateTaskParams,
};
use rmcp::service::RequestContext;
use rmcp::task_manager::{TaskExit, TaskManager, TaskOptions};
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
#[cfg(feature = "mcp-apps")]
use serde_json::json;
#[cfg(feature = "streamable-http")]
use sha2::{Digest, Sha256};

#[cfg(feature = "mcp-apps")]
use crate::logging::LogLevel;
use crate::models::ExtractResult;
#[cfg(test)]
use crate::models::{AssembledContextItem, ExplainItem};
#[cfg(feature = "mcp-apps")]
use crate::service::AppCommandInput;
use crate::service::MemoryService;
#[cfg(all(feature = "mcp-apps", not(feature = "streamable-http")))]
use std::time::Instant;
#[cfg(feature = "streamable-http")]
use std::time::{Duration, Instant};

use super::error::mcp_error;
use super::params::*;
use super::response::{AppCommandResult, OpenAppResult, ToolResponse};
use super::session;
#[cfg(feature = "mcp-apps")]
use crate::service::apps::session::SessionManager;

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
/// The single protocol version advertised and accepted by the HTTP
/// profile. Stdio never references this.
/// Lives in this module (not http::transport) so non-HTTP feature
/// sets compile without depending on the http module.
pub const PROTOCOL_VERSION_2026_07_28: rmcp::model::ProtocolVersion =
    rmcp::model::ProtocolVersion::V_2026_07_28;

#[derive(Clone)]
pub struct MemoryMcp {
    service: Arc<MemoryService>,
    #[cfg(feature = "mcp-apps")]
    session_manager: SessionManager,
    /// Durable app-session backend, wired only by the
    /// HTTP SaaS path. When `Some`, `open_app` and
    /// `app_command` route through `AppSessionStore`
    /// against the tenant's SurrealDB namespace; when
    /// `None`, the dispatch falls through to the
    /// in-memory `session_manager` (stdio and tests).
    #[cfg(all(feature = "mcp-apps", feature = "streamable-http"))]
    durable_app_sessions: Option<std::sync::Arc<crate::http::app_sessions::store::AppSessionStore>>,
    /// Durable task overlay. When `Some`, `get_task`,
    /// `cancel_task`, and the `extract` path dispatch
    /// through the `TaskStore` seam rather than the
    /// in-memory `TaskManager`.
    #[cfg(feature = "streamable-http")]
    durable_tasks: Option<std::sync::Arc<dyn crate::http::tasks::state::TaskStore>>,
    /// Durable subscription store. When `Some`, the
    /// handler advertises resource subscriptions and
    /// dispatches `subscriptions/listen` through the
    /// outbox-backed store.
    #[cfg(feature = "streamable-http")]
    subscription_store: Option<std::sync::Arc<dyn crate::http::subscriptions::SubscriptionStore>>,
    /// Request-scoped subscription principal. Set per
    /// request via `with_subscription_authorization`.
    #[cfg(feature = "streamable-http")]
    subscription_principal: Option<crate::http::principal::AuthenticatedPrincipal>,
    /// Request-scoped authenticator for subscription
    /// authorization rechecks.
    #[cfg(feature = "streamable-http")]
    subscription_authenticator: Option<std::sync::Arc<crate::http::principal::auth::Authenticator>>,
    /// Immutable tenant identity attached by the HTTP runtime. It is never
    /// accepted from MCP arguments and is absent for stdio.
    #[cfg(feature = "streamable-http")]
    tenant_id: Option<String>,
    /// Durable plan loaded from the control Registry for this tenant. It is
    /// attached by the runtime builder and never comes from MCP arguments.
    #[cfg(feature = "streamable-http")]
    tenant_plan: Option<crate::http::registry::plan::Plan>,
    /// Tenant-scoped extraction concurrency for synchronous tool calls.
    #[cfg(feature = "streamable-http")]
    extraction_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// Validated HTTP subscription delivery limits.
    #[cfg(feature = "streamable-http")]
    subscription_queue_capacity: usize,
    #[cfg(feature = "streamable-http")]
    subscription_auth_recheck_secs: u64,
    /// Maximum inline extraction size when the client did not advertise Tasks.
    #[cfg(feature = "streamable-http")]
    task_sync_max_bytes: usize,
    tasks: TaskManager,
    tool_router: ToolRouter<Self>,
    /// When true, advertise and negotiate only MCP 2026-07-28. Stdio
    /// constructors leave this false, preserving the frozen stdio
    /// behavior regardless of feature flags.
    modern_protocol_only: bool,
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
            #[cfg(feature = "mcp-apps")]
            session_manager: SessionManager::new(),
            #[cfg(all(feature = "mcp-apps", feature = "streamable-http"))]
            durable_app_sessions: None,
            #[cfg(feature = "streamable-http")]
            durable_tasks: None,
            #[cfg(feature = "streamable-http")]
            subscription_store: None,
            #[cfg(feature = "streamable-http")]
            subscription_principal: None,
            #[cfg(feature = "streamable-http")]
            subscription_authenticator: None,
            #[cfg(feature = "streamable-http")]
            tenant_id: None,
            #[cfg(feature = "streamable-http")]
            tenant_plan: None,
            #[cfg(feature = "streamable-http")]
            extraction_semaphore: None,
            #[cfg(feature = "streamable-http")]
            subscription_queue_capacity: crate::http::config::DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY,
            #[cfg(feature = "streamable-http")]
            subscription_auth_recheck_secs: crate::http::config::DEFAULT_SUBSCRIPTION_AUTH_RECHECK
                .as_secs(),
            #[cfg(feature = "streamable-http")]
            task_sync_max_bytes: crate::http::config::DEFAULT_TASK_SYNC_MAX_BYTES,
            tasks: TaskManager::new(),
            tool_router: Self::tool_router(),
            modern_protocol_only: false,
        }
    }

    /// HTTP SaaS profile constructor: modern protocol only.
    pub fn new_modern(service: MemoryService) -> Self {
        Self {
            modern_protocol_only: true,
            ..Self::new(service)
        }
    }

    /// Wire the durable `AppSessionStore` so `open_app`
    /// and `app_command` route through the tenant's
    /// SurrealDB namespace rather than the in-process
    /// store. The HTTP SaaS path calls this on
    /// `MemoryMcp::new_modern` before installing the
    /// handler into the request pipeline.
    #[cfg(all(feature = "mcp-apps", feature = "streamable-http"))]
    pub fn with_durable_app_sessions(
        mut self,
        store: std::sync::Arc<crate::http::app_sessions::store::AppSessionStore>,
    ) -> Self {
        self.durable_app_sessions = Some(store);
        self
    }

    /// Wire the durable `TaskStore` so `get_task` and
    /// `cancel_task` dispatch through the tenant's
    /// `tenant_task` table rather than the in-memory
    /// `TaskManager`. The HTTP SaaS path calls this on
    /// `MemoryMcp::new_modern` before the handler is
    /// installed into the request pipeline.
    #[cfg(feature = "streamable-http")]
    pub fn with_durable_tasks(
        mut self,
        tasks: std::sync::Arc<dyn crate::http::tasks::state::TaskStore>,
    ) -> Self {
        self.durable_tasks = Some(tasks);
        self
    }

    /// Wire the durable subscription store so the handler
    /// advertises resource subscriptions and dispatches
    /// `subscriptions/listen` through the outbox.
    #[cfg(feature = "streamable-http")]
    pub fn with_durable_subscriptions(
        mut self,
        store: std::sync::Arc<dyn crate::http::subscriptions::SubscriptionStore>,
    ) -> Self {
        self.subscription_store = Some(store);
        self
    }

    /// Attach request-scoped subscription authorization.
    /// Called per request in the transport handler before
    /// building the per-request `StreamableHttpService`.
    #[cfg(feature = "streamable-http")]
    pub fn with_subscription_authorization(
        mut self,
        principal: crate::http::principal::AuthenticatedPrincipal,
        authenticator: std::sync::Arc<crate::http::principal::auth::Authenticator>,
    ) -> Self {
        self.subscription_principal = Some(principal);
        self.subscription_authenticator = Some(authenticator);
        self
    }

    /// Attach the immutable tenant id to an HTTP runtime handler.
    #[cfg(feature = "streamable-http")]
    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Attach the durable plan selected by the control Registry. The setter is
    /// HTTP-only; stdio never carries a tenant plan.
    #[cfg(feature = "streamable-http")]
    pub fn with_tenant_plan(mut self, plan: crate::http::registry::plan::Plan) -> Self {
        let permits = match usize::try_from(plan.extraction_concurrency) {
            Ok(value) => value.max(1),
            Err(_) => usize::MAX,
        };
        self.extraction_semaphore = Some(Arc::new(tokio::sync::Semaphore::new(permits)));
        self.tenant_plan = Some(plan);
        self
    }

    /// Attach validated subscription delivery limits from HTTP config.
    #[cfg(feature = "streamable-http")]
    pub fn with_subscription_limits(
        mut self,
        queue_capacity: usize,
        auth_recheck: Duration,
    ) -> Self {
        self.subscription_queue_capacity = queue_capacity.max(1);
        self.subscription_auth_recheck_secs = auth_recheck.as_secs().max(1);
        self
    }

    /// Attach the bounded synchronous extraction policy for clients without
    /// the Tasks capability.
    #[cfg(feature = "streamable-http")]
    pub fn with_task_sync_max_bytes(mut self, max_bytes: usize) -> Self {
        self.task_sync_max_bytes = max_bytes.max(1);
        self
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
                .enable_tasks()
                .build(),
        )
        .with_instructions(Self::SERVER_INSTRUCTIONS)
    }

    /// HTTP profile server info: tools + tasks always; resources only
    /// when `mcp-apps` is compiled. Does not advertise MRTR, roots,
    /// sampling, elicitation, prompts-change, or tool-list-change.
    fn build_http_server_info(&self) -> ServerInfo {
        let builder = ServerCapabilities::builder().enable_tools().enable_tasks();
        #[cfg(feature = "mcp-apps")]
        let builder = {
            let b = builder.enable_resources();
            // Advertise resource subscriptions only when
            // the durable subscription backend is attached.
            #[cfg(feature = "streamable-http")]
            let b = if self.subscription_store.is_some() {
                b.enable_resources_subscribe()
            } else {
                b
            };
            b
        };
        ServerInfo::new(builder.build()).with_instructions(Self::SERVER_INSTRUCTIONS)
    }

    fn invalid_params(message: impl Into<String>) -> ErrorData {
        session::invalid_params(message)
    }

    #[cfg(feature = "mcp-apps")]
    fn missing_app_field(app: &str, field: &str) -> ErrorData {
        session::missing_app_field(app, field)
    }

    fn internal_error(message: impl Into<String>) -> ErrorData {
        session::internal_error(message)
    }
}

#[cfg(feature = "streamable-http")]
fn durable_task_fingerprint(tool_name: &str, params: &serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool_name.as_bytes());
    hasher.update([0]);
    hasher.update(serde_json::to_vec(params).unwrap_or_default());
    format!("{tool_name}:{}", hex::encode(hasher.finalize()))
}

#[cfg(feature = "streamable-http")]
async fn ensure_subscription_authorization(
    authenticator: &crate::http::principal::auth::Authenticator,
    principal: &crate::http::principal::AuthenticatedPrincipal,
) -> Result<(), ErrorData> {
    match tokio::time::timeout(Duration::from_secs(5), authenticator.is_current(principal)).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ErrorData::internal_error(
            "subscription authorization expired",
            None,
        )),
        Err(_) => Err(ErrorData::internal_error(
            "subscription authorization recheck timed out",
            None,
        )),
    }
}

async fn extract_response(
    service: Arc<MemoryService>,
    params: ExtractParams,
) -> Result<ToolResponse<ExtractResult>, ErrorData> {
    crate::tools::extract(&service.build_context(), params)
        .await
        .map_err(mcp_error)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MemoryMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = if self.modern_protocol_only {
            self.build_http_server_info()
        } else {
            Self::build_server_info()
        };
        if self.modern_protocol_only {
            // Pin the negotiation fallback: rmcp falls back to this
            // version when a client requests one we do not support.
            info = info.with_protocol_version(PROTOCOL_VERSION_2026_07_28);
        }
        info
    }

    fn supported_protocol_versions(
        &self,
    ) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
        if self.modern_protocol_only {
            std::borrow::Cow::Owned(vec![PROTOCOL_VERSION_2026_07_28])
        } else {
            std::borrow::Cow::Borrowed(rmcp::model::ProtocolVersion::KNOWN_VERSIONS)
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let client_supports_tasks = context
            .client_capabilities()
            .is_some_and(|caps| caps.supports_tasks());

        #[cfg(feature = "streamable-http")]
        if request.name == "extract" && !client_supports_tasks {
            let inline_bytes = request
                .arguments
                .as_ref()
                .and_then(|arguments| {
                    arguments
                        .get("content")
                        .or_else(|| arguments.get("text"))
                        .and_then(|value| value.as_str())
                })
                .map(str::len);
            if inline_bytes.is_some_and(|bytes| bytes > self.task_sync_max_bytes) {
                return Err(ErrorData::invalid_params(
                    "inline extract exceeds the synchronous limit; advertise MCP Tasks",
                    None,
                ));
            }
        }

        if request.name == "extract" && client_supports_tasks {
            let params: ExtractParams = serde_json::from_value(serde_json::Value::Object(
                request.arguments.clone().unwrap_or_default(),
            ))
            .map_err(|error| {
                ErrorData::invalid_params(
                    format!("failed to deserialize parameters: {error}"),
                    None,
                )
            })?;

            // Durable backend: enqueue via TaskStore and return a seed
            // CreateTaskResult. The worker picks up the task asynchronously.
            #[cfg(feature = "streamable-http")]
            if let Some(task_store) = self.durable_tasks.as_ref() {
                let task_params = serde_json::to_value(&params).map_err(|error| {
                    ErrorData::internal_error(
                        format!("failed to serialize extract task parameters: {error}"),
                        None,
                    )
                })?;
                let fingerprint = durable_task_fingerprint(&request.name, &task_params);
                let task_id = task_store
                    .enqueue(&fingerprint, task_params)
                    .await
                    .map_err(|error| {
                        ErrorData::internal_error(
                            format!("failed to enqueue extract task: {error}"),
                            None,
                        )
                    })?;
                let now = chrono::Utc::now().to_rfc3339();
                let seed = rmcp::model::Task::new(
                    task_id,
                    rmcp::model::TaskStatus::Working,
                    now.clone(),
                    now,
                )
                .with_status_message("Task accepted");
                return Ok(CallToolResponse::Task(CreateTaskResult::new(seed)));
            }

            // In-memory fallback (stdio, tests without durable overlay).
            let service = Arc::clone(&self.service);
            let task = self.tasks.spawn(
                TaskOptions::new().with_status_message("Task accepted"),
                move |ctx| {
                    Box::pin(async move {
                        tokio::select! {
                            _ = ctx.cancelled() => Err(TaskExit::Cancelled),
                            result = extract_response(service, params) => {
                                let response = result.map_err(TaskExit::Error)?;
                                let structured = serde_json::to_value(response).map_err(|error| {
                                    TaskExit::Error(ErrorData::internal_error(
                                        format!("failed to serialize extract task result: {error}"),
                                        None,
                                    ))
                                })?;
                                Ok(CallToolResult::structured(structured))
                            }
                        }
                    })
                },
            );
            return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
        }

        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        // Durable overlay: try TaskStore first, fall back to in-memory.
        #[cfg(feature = "streamable-http")]
        if let Some(task_store) = self.durable_tasks.as_ref()
            && let Some(record) = task_store.load(&request.task_id).await.map_err(|error| {
                ErrorData::internal_error(format!("failed to load task: {error}"), None)
            })?
        {
            let status = match record.state {
                crate::http::tasks::state::TaskState::Queued
                | crate::http::tasks::state::TaskState::Running => rmcp::model::TaskStatus::Working,
                crate::http::tasks::state::TaskState::Completed => {
                    rmcp::model::TaskStatus::Completed
                }
                crate::http::tasks::state::TaskState::Failed => rmcp::model::TaskStatus::Failed,
                _ => rmcp::model::TaskStatus::Cancelled,
            };
            let now = chrono::Utc::now().to_rfc3339();
            let task = rmcp::model::Task::new(
                request.task_id,
                status,
                record.created_at.to_rfc3339(),
                now,
            );
            let payload = match status {
                rmcp::model::TaskStatus::Working => rmcp::model::TaskPayload::Working,
                rmcp::model::TaskStatus::Completed => {
                    let result = record
                        .result
                        .unwrap_or_else(|| serde_json::json!({"status": "completed"}));
                    rmcp::model::TaskPayload::Completed {
                        result: result.as_object().cloned().unwrap_or_default(),
                    }
                }
                rmcp::model::TaskStatus::Failed => {
                    let error = record.error.unwrap_or_else(
                        || serde_json::json!({"code": -1, "message": "task failed"}),
                    );
                    rmcp::model::TaskPayload::Failed {
                        error: error.as_object().cloned().unwrap_or_default(),
                    }
                }
                _ => rmcp::model::TaskPayload::Cancelled,
            };
            return Ok(GetTaskResult::new(rmcp::model::DetailedTask::new(
                task, payload,
            )));
        }
        Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        // Durable overlay: v1 tasks don't support input
        // requests, so update_task is invalid.
        #[cfg(feature = "streamable-http")]
        if self.durable_tasks.is_some() {
            return Err(ErrorData::invalid_params(
                "durable tasks do not support update_task in v1",
                None,
            ));
        }
        self.tasks
            .update_task(&request.task_id, request.input_responses)
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        // Durable overlay: delegate to TaskStore::set_cancellation_intent.
        #[cfg(feature = "streamable-http")]
        if let Some(task_store) = self.durable_tasks.as_ref() {
            task_store
                .set_cancellation_intent(&request.task_id)
                .await
                .map_err(|error| {
                    ErrorData::internal_error(format!("failed to cancel task: {error}"), None)
                })?;
            return Ok(());
        }
        self.tasks.cancel_task(&request.task_id)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        #[cfg(not(feature = "mcp-apps"))]
        {
            Ok(ListResourcesResult::with_all_items(Vec::new()))
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
            Ok(ListResourceTemplatesResult::with_all_items(Vec::new()))
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
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.read_resource_result(request).await.map(Into::into)
    }

    #[cfg(feature = "streamable-http")]
    fn accepted_subscription_filter(
        &self,
        requested: &rmcp::model::SubscriptionFilter,
    ) -> Option<rmcp::model::SubscriptionFilter> {
        self.subscription_store.as_ref()?;
        let tenant_id = self.tenant_id.as_deref()?;
        crate::http::subscriptions::ValidatedSubscriptionFilter::for_tenant(tenant_id, requested)
            .ok()
            .map(|filter| filter.to_rmcp_filter())
    }

    #[cfg(feature = "streamable-http")]
    fn listen(
        &self,
        context: rmcp::service::SubscriptionContext,
    ) -> impl std::future::Future<Output = Result<(), rmcp::ErrorData>>
    + rmcp::service::MaybeSendFuture
    + '_ {
        let store = self.subscription_store.clone();
        let principal = self.subscription_principal.clone();
        let authenticator = self.subscription_authenticator.clone();
        let tenant_id = self.tenant_id.clone();
        async move {
            let Some(store) = store else {
                return Err(rmcp::ErrorData::method_not_found::<
                    rmcp::model::SubscriptionsListenRequestMethod,
                >());
            };
            let Some(principal) = principal else {
                return Err(rmcp::ErrorData::internal_error(
                    "subscription principal missing",
                    None,
                ));
            };
            let Some(authenticator) = authenticator else {
                return Err(rmcp::ErrorData::internal_error(
                    "subscription authenticator missing",
                    None,
                ));
            };
            let Some(tenant_id) = tenant_id else {
                return Err(rmcp::ErrorData::internal_error(
                    "subscription tenant binding missing",
                    None,
                ));
            };
            let filter = crate::http::subscriptions::ValidatedSubscriptionFilter::for_tenant(
                tenant_id,
                context.accepted(),
            )
            .map_err(|error| rmcp::ErrorData::invalid_params(error.to_string(), None))?;

            ensure_subscription_authorization(&authenticator, &principal).await?;
            store
                .validate_filter(&filter)
                .await
                .map_err(|error| rmcp::ErrorData::invalid_params(error.to_string(), None))?;
            let mut cursor = tokio::time::timeout(Duration::from_secs(5), store.current_sequence())
                .await
                .map_err(|_| {
                    rmcp::ErrorData::internal_error("subscription sequence read timed out", None)
                })?
                .map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;

            let mut last_auth_check = Instant::now();
            let mut queue = crate::http::subscriptions::stream::CoalescingQueue::new(
                self.subscription_queue_capacity,
            );
            let mut poll_tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = context.cancelled() => return Ok(()),
                    _ = poll_tick.tick() => {
                        if last_auth_check.elapsed()
                            >= Duration::from_secs(self.subscription_auth_recheck_secs)
                        {
                            ensure_subscription_authorization(&authenticator, &principal).await?;
                            last_auth_check = Instant::now();
                        }
                        let batch = tokio::time::timeout(
                            Duration::from_secs(5),
                            store.next_batch(cursor, &filter),
                        )
                        .await
                        .map_err(|_| rmcp::ErrorData::internal_error(
                            "subscription outbox read timed out",
                            None,
                        ))?
                        .map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;
                        for event in batch {
                            cursor = cursor.max(event.sequence);
                            queue.push(event).map_err(|_| {
                                rmcp::ErrorData::internal_error(
                                    "subscription disconnected: slow consumer",
                                    None,
                                )
                            })?;
                        }
                        while let Some(event) = queue.pop_front() {
                            if last_auth_check.elapsed()
                                >= Duration::from_secs(self.subscription_auth_recheck_secs)
                            {
                                ensure_subscription_authorization(&authenticator, &principal).await?;
                                last_auth_check = Instant::now();
                            }
                            tokio::select! {
                                _ = context.cancelled() => return Ok(()),
                                result = crate::http::subscriptions::stream::send_invalidation_with_timeout(
                                    context.sink(),
                                    event,
                                ) => {
                                    result.map_err(|error| rmcp::ErrorData::internal_error(
                                        error.to_string(),
                                        None,
                                    ))?;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[tool_router]
impl MemoryMcp {
    #[tool(
        description = "Store a new episode in long-term memory. Use this tool when you need to persist source material before extracting entities or facts. Do not use this tool for retrieval. Arguments must be a flat snake_case object with `source_type`, `source_id`, `content`, `t_ref`, and optional `t_ingested` and `policy_tags`. Do not wrap arguments in `payload`. Returns the created or existing `episode_id`. On error, fix the input fields and retry."
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
        description = "Extract entities, facts, and relationships from remembered content. Use this tool when you need structured knowledge from an existing episode or from new inline content. Prefer task-based invocation when the client supports MCP Tasks or when local NER may exceed the client's synchronous timeout. Do not use this tool for retrieval. Arguments must be a flat snake_case object. Provide exactly one input source: `episode_id` for stored content, or inline `content`/`text`; optional fields are `source_type`, `source_id`, `t_ref`, and `zero_shot_labels`. Do not wrap arguments in `payload`. If you pass inline content, the server ingests it first and then extracts facts. Returns extracted entities, facts, and links."
    )]
    pub async fn extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<Json<ToolResponse<ExtractResult>>, ErrorData> {
        #[cfg(feature = "streamable-http")]
        let _permit =
            match self.extraction_semaphore.as_ref() {
                Some(semaphore) => Some(semaphore.clone().acquire_owned().await.map_err(|_| {
                    Self::internal_error("extraction concurrency is shutting down")
                })?),
                None => None,
            };
        extract_response(Arc::clone(&self.service), params.0)
            .await
            .map(Json)
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
        description = "Open a Memory MCP app through the minimal public launcher. Use this tool only when an interactive app workflow is required and no canonical memory tool already matches the intent. Arguments must be a flat snake_case object. Required fields depend on `app`: inspector -> `target_type` + `target_id`; diff -> `as_of_left` + `as_of_right`; graph -> `from_entity_id` + `to_entity_id`; ingestion_review -> optional `source_text` or `draft_episode_id`; lifecycle -> no partition field. Do not wrap arguments in `payload`. Returns `session_id`, `resource_uri`, `fallback`, and `guidance`."
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
                json!({"app": app}),
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

            let input = AppCommandInput {
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
            };
            let outcome = {
                #[cfg(all(feature = "streamable-http", feature = "mcp-apps"))]
                if self.durable_app_sessions.is_some() {
                    self.execute_durable_app_command(&p.session_id, input).await
                } else {
                    crate::service::apps::session_lifecycle::execute_app_command(
                        &self.service,
                        &self.session_manager,
                        &p.session_id,
                        input,
                    )
                    .await
                    .map_err(mcp_error)
                }
                #[cfg(not(feature = "streamable-http"))]
                {
                    crate::service::apps::session_lifecycle::execute_app_command(
                        &self.service,
                        &self.session_manager,
                        &p.session_id,
                        input,
                    )
                    .await
                    .map_err(mcp_error)
                }
            };

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
        description = "Assemble the most relevant active memory context for a query. Use this tool when you need retrieval across stored facts before answering or planning. Do not use this tool to ingest new content. Arguments must be a flat snake_case object with `query`, `fact_types`, `as_of`, `budget`, `view_mode`, `window_start`, and `window_end`. Do not wrap arguments in `payload`. Returns ranked context items with confidence and rationale. On error, fix the query parameters and retry."
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
    #[cfg(feature = "mcp-apps")]
    use crate::service::apps::session::enrich_session_payload;
    #[cfg(feature = "mcp-apps")]
    use crate::service::capabilities::ingest::IngestCapability;
    #[cfg(feature = "mcp-apps")]
    use crate::service::capabilities::resolve::ResolveCapability;
    use crate::service::edge_neighbor;
    use crate::service::{DisabledEmbeddingProvider, EntityExtractor, GlinerEntityExtractor};
    use crate::storage::{DbClient, SurrealDbClient};
    use crate::tools::params::{ExtractParams, IngestParams};
    use chrono::Datelike;
    #[cfg(feature = "mcp-apps")]
    use chrono::{TimeZone, Utc};
    use rmcp::model::ProtocolVersion;
    #[cfg(feature = "mcp-apps")]
    use rmcp::model::{ReadResourceRequestParams, ResourceContents};
    use serde_json::{Value, json};
    use std::path::Path;

    async fn test_db_client() -> Arc<SurrealDbClient> {
        let namespaces = vec!["org".to_string()];
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

        db_client
    }

    async fn create_test_mcp() -> MemoryMcp {
        let service = MemoryService::new(
            test_db_client().await,
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("create test service");
        MemoryMcp::new(service)
    }

    async fn create_test_mcp_with_extractor(
        entity_extractor: Arc<dyn EntityExtractor>,
    ) -> MemoryMcp {
        let service = MemoryService::new_with_embedding_provider(
            test_db_client().await,
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(DisabledEmbeddingProvider::new(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            )),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            entity_extractor,
        )
        .expect("create test service with custom extractor");
        MemoryMcp::new(service)
    }

    #[cfg(feature = "mcp-apps")]
    async fn create_test_entity(mcp: &MemoryMcp, canonical_name: &str) -> String {
        ResolveCapability::resolve(
            &mcp.service().build_context(),
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
    fn protocol_version_2026_07_28_is_canonical() {
        assert_eq!(PROTOCOL_VERSION_2026_07_28, ProtocolVersion::V_2026_07_28);
    }

    #[cfg(feature = "streamable-http")]
    #[test]
    fn durable_task_fingerprint_includes_parameters() {
        let first = durable_task_fingerprint("extract", &json!({"episode_id": "ep_a"}));
        let second = durable_task_fingerprint("extract", &json!({"episode_id": "ep_b"}));
        assert_ne!(first, second);
        assert!(first.starts_with("extract:"));
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
            capabilities["extensions"]["io.modelcontextprotocol/tasks"].is_object(),
            "tasks extension capability missing: {}",
            capabilities
        );
        assert!(capabilities.get("tasks").is_none());
    }

    #[tokio::test]
    async fn tool_router_still_exposes_extract_and_non_task_tools() {
        let mcp = create_test_mcp().await;
        assert!(mcp.get_tool("extract").is_some());
        assert!(mcp.get_tool("ingest").is_some());
    }

    #[tokio::test]
    async fn mcp_extract_with_empty_gliner_labels_skips_model_loading() {
        let extractor = Arc::new(
            GlinerEntityExtractor::new(
                Path::new("/path/to/a/nonexistent/gliner/model"),
                vec!["person".to_string()],
                0.2,
            )
            .expect("GLiNER runtime configuration should not load model files"),
        ) as Arc<dyn EntityExtractor>;
        let mcp = create_test_mcp_with_extractor(extractor).await;

        let ingest = mcp
            .ingest(Parameters(IngestParams {
                source_type: "test".to_string(),
                source_id: "empty-gliner-labels".to_string(),
                content: "Alice works at Acme".to_string(),
                t_ref: "2026-01-01T00:00:00Z".to_string(),
                t_ingested: None,
                policy_tags: Vec::new(),
            }))
            .await
            .expect("MCP ingest should succeed")
            .0;

        let extraction = mcp
            .extract(Parameters(ExtractParams {
                episode_id: Some(ingest.result),
                content: None,
                text: None,
                source_type: None,
                source_id: None,
                t_ref: None,
                zero_shot_labels: Some(Vec::new()),
            }))
            .await
            .expect("MCP extract with empty GLiNER labels should succeed")
            .0;

        assert_eq!(extraction.status, "success");
        assert!(extraction.result.entities.is_empty());
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
            "t_ref",
            "t_ingested",
            "provenance",
            "citation_context",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }

        assert!(
            !properties.contains_key("scope"),
            "ExplainItem schema must not expose the legacy scope field"
        );

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
        let stale_episode_id = IngestCapability::ingest(
            &mcp.service().build_context(),
            IngestRequest {
                source_type: "meeting".to_string(),
                source_id: "stale-lifecycle-episode".to_string(),
                content: "Legacy launch plan that should be archived.".to_string(),
                t_ref: Utc.with_ymd_and_hms(2025, 1, 10, 9, 0, 0).unwrap(),
                t_ingested: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest stale episode");

        let open_result = mcp
            .open_app(Parameters(OpenAppParams {
                app: "lifecycle".to_string(),
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

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn enrich_session_payload_adds_meta_with_expiry() {
        let payload = json!({"data": "value"});
        let enriched = enrich_session_payload("inspector", "ses:1", Some(3600), payload);
        assert_eq!(enriched["app"], "inspector");
        assert_eq!(enriched["session_id"], "ses:1");

        assert!(enriched["meta"]["expires_at"].is_string());
        assert_eq!(enriched["meta"]["ttl_seconds"], 3600);
        assert_eq!(enriched["data"], "value");
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn enrich_session_payload_handles_no_ttl() {
        let payload = json!({});
        let enriched = enrich_session_payload("diff", "ses:2", None, payload);
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
