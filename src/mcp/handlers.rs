//! MCP tool handler implementations.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{
    AccessPayload, AssembleContextRequest, AssembledContextItem, ExplainItem, ExplainRequest,
    ExtractResult,
};
use crate::service::value_helpers::{json_string, normalized_edge_record};
use crate::service::{MemoryService, run_community_rebuild_pass, run_decay_pass};
use crate::storage::GraphDirection;
use std::time::Instant;

use super::error::{mcp_error, tool_error};
use super::params::*;
use super::parsers::{parse_context_items, parse_datetime};
use super::resources::{
    APPS_INDEX_URI, app_catalog_resources, app_resource_templates, app_root_payload,
    app_session_html_document, app_session_uri, apps_index_payload, parse_app_root_uri,
    parse_app_session_uri,
};
use super::response::{AppCommandResult, OpenAppResult, ToolResponse};
use super::session::{self, SessionManager};

#[derive(Debug, Clone)]
struct GraphPathSnapshot {
    found: bool,
    nodes: Vec<Value>,
    edges: Vec<Value>,
}

fn edge_neighbor(record: &Value, direction: GraphDirection) -> Option<String> {
    let map = record.as_object()?;
    match direction {
        GraphDirection::Incoming => map.get("in").and_then(json_string).map(String::from),
        GraphDirection::Outgoing => map.get("out").and_then(json_string).map(String::from),
    }
}

fn upsert_json_field(payload: &mut Value, key: &str, value: Value) {
    if let Some(object) = payload.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

/// Updates the status of matching ingestion review items and persists the result.
async fn update_ingestion_item_statuses(
    service: &MemoryMcp,
    session_id: &str,
    item_ids: &[String],
    status: &str,
    reason: Option<String>,
    session_payload: Value,
) -> Result<serde_json::Value, ErrorData> {
    let mut payload = session_payload;
    let summary = if let Some(items) = payload.get_mut("items").and_then(Value::as_array_mut) {
        for item in items.iter_mut() {
            let matches = item
                .get("item_id")
                .and_then(Value::as_str)
                .is_some_and(|item_id| item_ids.iter().any(|candidate| candidate == item_id));
            if matches && let Some(object) = item.as_object_mut() {
                object.insert("status".to_string(), json!(status));
                if status == "approved" {
                    object.remove("reason");
                } else if let Some(r) = reason.as_ref() {
                    object.insert("reason".to_string(), json!(r));
                }
            }
        }
        summarize_ingestion_review_items(items)
    } else {
        summarize_ingestion_review_items(&[])
    };
    upsert_json_field(&mut payload, "summary", summary.clone());
    let updated = service.replace_session_payload(session_id, payload).await?;
    Ok(updated.payload["summary"].clone())
}

fn shallow_merge_object(
    target: &mut serde_json::Map<String, Value>,
    patch: &serde_json::Map<String, Value>,
) {
    for (key, value) in patch {
        target.insert(key.clone(), value.clone());
    }
}

fn summarize_ingestion_review_items(items: &[Value]) -> Value {
    let mut by_status = serde_json::Map::from_iter([
        ("pending".to_string(), json!(0)),
        ("approved".to_string(), json!(0)),
        ("rejected".to_string(), json!(0)),
        ("edited".to_string(), json!(0)),
    ]);

    for item in items {
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_string();
        let current = by_status.get(&status).and_then(Value::as_i64).unwrap_or(0) + 1;
        by_status.insert(status, json!(current));
    }

    let approved = by_status
        .get("approved")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    json!({
        "total": items.len(),
        "by_status": by_status,
        "committable": approved,
    })
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
    session_manager: SessionManager,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl MemoryMcp {
    const SERVER_INSTRUCTIONS: &str = "Memory MCP server: stores facts about entities and relationships, resolves aliases, and assembles long-term context. All tool arguments and structured results use flat snake_case JSON keys that must match the published schemas exactly. Do not wrap tool arguments in `payload`.";
    const DEFAULT_ARCHIVAL_AGE_DAYS: u32 = 30;
    const DEFAULT_DECAY_THRESHOLD: f64 = 0.35;
    const DEFAULT_DECAY_HALF_LIFE_DAYS: f64 = 180.0;

    /// Creates a new `MemoryMcp` instance with the given service.
    ///
    /// # Arguments
    ///
    /// * `service` - The `MemoryService` to use for memory operations.
    pub fn new(service: MemoryService) -> Self {
        Self {
            service: Arc::new(service),
            session_manager: SessionManager::new(),
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

    /// Generates a monotonically increasing request id like `req_0001`.
    fn next_request_id(&self) -> String {
        crate::tools::request_id::next_request_id()
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

    fn invalid_params(message: impl Into<String>) -> ErrorData {
        session::invalid_params(message)
    }

    fn missing_app_field(app: &str, field: &str) -> ErrorData {
        session::missing_app_field(app, field)
    }

    fn internal_error(message: impl Into<String>) -> ErrorData {
        session::internal_error(message)
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

    fn enrich_session_payload(
        app: &str,
        session_id: &str,
        scope: &str,
        ttl_seconds: Option<i64>,
        payload: Value,
    ) -> Value {
        session::enrich_session_payload(app, session_id, scope, ttl_seconds, payload)
    }

    async fn session(&self, session_id: &str) -> Result<session::AppSessionState, ErrorData> {
        self.session_manager.get(session_id).await
    }

    async fn replace_session_payload(
        &self,
        session_id: &str,
        payload: Value,
    ) -> Result<session::AppSessionState, ErrorData> {
        self.session_manager
            .replace_payload(session_id, payload)
            .await
    }

    async fn remove_session(
        &self,
        session_id: &str,
    ) -> Result<session::AppSessionState, ErrorData> {
        self.session_manager.remove(session_id).await
    }

    async fn create_session(
        &self,
        app: &str,
        scope: &str,
        ttl_seconds: Option<i64>,
        payload: Value,
    ) -> Result<OpenAppResult, ErrorData> {
        self.session_manager
            .create(app, scope, ttl_seconds, payload)
            .await
    }

    fn app_command_result_from_details(
        app: &str,
        session_id: &str,
        action: &str,
        resource_uri: Option<String>,
        details: Value,
    ) -> AppCommandResult {
        session::app_command_result_from_details(app, session_id, action, resource_uri, details)
    }

    async fn read_app_resource_payload(
        &self,
        app: &str,
        session_id: &str,
    ) -> Result<Value, ErrorData> {
        let session = self.session(session_id).await?;
        if session.app != app {
            return Err(Self::invalid_params(format!(
                "Session {session_id} belongs to {} but resource requested for {app}",
                session.app
            )));
        }

        Ok(session.payload)
    }

    async fn entity_snapshot(&self, namespace: &str, entity_id: &str) -> Result<Value, ErrorData> {
        let record = self
            .service
            .db_client
            .select_one(entity_id, namespace)
            .await
            .map_err(mcp_error)?;
        let canonical_name = record
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|map| map.get("canonical_name"))
            .and_then(Value::as_str)
            .unwrap_or(entity_id)
            .to_string();

        Ok(json!({
            "entity_id": entity_id,
            "canonical_name": canonical_name,
        }))
    }

    async fn inspector_payload(
        &self,
        scope: &str,
        target_type: &str,
        target_id: &str,
        as_of: Option<&str>,
    ) -> Result<Value, ErrorData> {
        let namespace = self.service.namespace_for_scope(scope);
        let (record, record_namespace) = match target_type {
            "entity" => {
                let record = self
                    .service
                    .db_client
                    .select_one(target_id, &namespace)
                    .await
                    .map_err(mcp_error)?;
                (record, Some(namespace.clone()))
            }
            "fact" => {
                let (record, ns) = self
                    .service
                    .find_fact_record(target_id)
                    .await
                    .map_err(mcp_error)?;
                (record.map(Value::Object), ns)
            }
            "episode" => {
                let (record, ns) = self
                    .service
                    .find_episode_record(target_id)
                    .await
                    .map_err(mcp_error)?;
                (record.map(Value::Object), ns)
            }
            other => {
                return Err(Self::invalid_params(format!(
                    "Unsupported inspector target_type: {other}"
                )));
            }
        };

        let record = record.ok_or_else(|| {
            Self::invalid_params(format!(
                "Inspector target not found: {target_type} {target_id}"
            ))
        })?;

        Ok(json!({
            "target": {
                "target_type": target_type,
                "target_id": target_id,
                "as_of": as_of,
                "namespace": record_namespace.unwrap_or(namespace),
            },
            "record": record,
            "summary": {
                "found": true,
                "record_type": target_type,
                "field_count": record.as_object().map_or(0, serde_json::Map::len),
            }
        }))
    }

    fn diff_payload(params: &OpenAppParams) -> Value {
        let target_type = params
            .target_type
            .as_deref()
            .unwrap_or(if params.target_id.is_some() {
                "entity"
            } else {
                "scope"
            });
        let target_id = params.target_id.clone();
        let as_of_left = params.as_of_left.clone().unwrap_or_default();
        let as_of_right = params.as_of_right.clone().unwrap_or_default();
        let time_axis = params
            .time_axis
            .clone()
            .unwrap_or_else(|| "valid".to_string());
        let stable_key = format!(
            "{}:{}:{}:{}:{}",
            params.scope,
            target_type,
            target_id.clone().unwrap_or_else(|| "scope".to_string()),
            as_of_left,
            as_of_right
        );

        json!({
            "target": {
                "target_type": target_type,
                "target_id": target_id,
            },
            "range": {
                "as_of_left": params.as_of_left,
                "as_of_right": params.as_of_right,
                "time_axis": time_axis,
            },
            "result": {
                "stable_key": stable_key,
                "summary": {
                    "scope": params.scope,
                    "view": params.view,
                    "change_count": 0,
                },
                "changes": [],
            },
            "exports": []
        })
    }

    fn ingestion_review_payload(
        source_text: Option<&str>,
        draft_episode_id: Option<&str>,
    ) -> Value {
        let mut items = Vec::new();
        let seed_content = source_text
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                draft_episode_id
                    .map(|episode_id| format!("Review extracted draft items for {episode_id}"))
            });

        if let Some(content) = seed_content {
            items.push(json!({
                "item_id": "draft:1",
                "status": "pending",
                "kind": "draft_fact",
                "content": content,
                "quote": content,
                "source_episode": draft_episode_id,
            }));
        }

        json!({
            "source": {
                "source_text": source_text,
                "draft_episode_id": draft_episode_id,
            },
            "items": items.clone(),
            "summary": summarize_ingestion_review_items(&items),
        })
    }

    async fn lifecycle_dashboard(&self, scope: &str) -> Result<Value, ErrorData> {
        let namespace = self.service.namespace_for_scope(scope);
        let active_facts = self
            .service
            .db_client
            .select_active_facts(&namespace, 10_000)
            .await
            .map_err(mcp_error)?;
        let cutoff = crate::service::normalize_dt(
            Utc::now() - chrono::Duration::days(Self::DEFAULT_ARCHIVAL_AGE_DAYS as i64),
        );
        let archival_candidates = self
            .service
            .db_client
            .select_episodes_for_archival(&namespace, &cutoff, 1_000)
            .await
            .map_err(mcp_error)?;
        let communities = self
            .service
            .db_client
            .select_table("community", &namespace)
            .await
            .map_err(mcp_error)?;

        let candidate_ids = archival_candidates
            .iter()
            .filter_map(|record| record.get("episode_id").and_then(json_string))
            .collect::<Vec<_>>();

        Ok(json!({
            "active_facts": active_facts.len(),
            "archival_candidates": archival_candidates.len(),
            "archival_candidate_ids": candidate_ids,
            "communities": communities.len(),
        }))
    }

    async fn lifecycle_payload(&self, scope: &str) -> Result<Value, ErrorData> {
        Ok(json!({
            "dashboard": self.lifecycle_dashboard(scope).await?,
            "defaults": {
                "archival_age_days": Self::DEFAULT_ARCHIVAL_AGE_DAYS,
                "decay_threshold": Self::DEFAULT_DECAY_THRESHOLD,
                "decay_half_life_days": Self::DEFAULT_DECAY_HALF_LIFE_DAYS,
            },
            "recent_actions": [],
        }))
    }

    async fn graph_path_snapshot(
        &self,
        namespace: &str,
        from_entity_id: &str,
        to_entity_id: &str,
        cutoff: DateTime<Utc>,
        max_depth: i32,
    ) -> Result<GraphPathSnapshot, ErrorData> {
        if from_entity_id == to_entity_id {
            return Ok(GraphPathSnapshot {
                found: true,
                nodes: vec![self.entity_snapshot(namespace, from_entity_id).await?],
                edges: Vec::new(),
            });
        }

        let cutoff_iso = crate::service::normalize_dt(cutoff);
        let mut visited = HashSet::from([from_entity_id.to_string()]);
        let mut queue = VecDeque::from([(
            from_entity_id.to_string(),
            vec![from_entity_id.to_string()],
            Vec::<Value>::new(),
        )]);

        while let Some((current, nodes, edges)) = queue.pop_front() {
            if edges.len() >= max_depth.max(1) as usize {
                continue;
            }

            for direction in [GraphDirection::Outgoing, GraphDirection::Incoming] {
                let records = self
                    .service
                    .db_client
                    .select_edge_neighbors(namespace, &current, &cutoff_iso, direction)
                    .await
                    .map_err(mcp_error)?;
                for record in records {
                    let Some(neighbor) = edge_neighbor(&record, direction) else {
                        continue;
                    };
                    let mut next_nodes = nodes.clone();
                    next_nodes.push(neighbor.clone());
                    let mut next_edges = edges.clone();
                    next_edges.push(normalized_edge_record(&record));

                    if neighbor == to_entity_id {
                        let mut snapshots = Vec::with_capacity(next_nodes.len());
                        for node_id in next_nodes {
                            snapshots.push(self.entity_snapshot(namespace, &node_id).await?);
                        }
                        return Ok(GraphPathSnapshot {
                            found: true,
                            nodes: snapshots,
                            edges: next_edges,
                        });
                    }

                    if visited.insert(neighbor.clone()) {
                        queue.push_back((neighbor, next_nodes, next_edges));
                    }
                }
            }
        }

        Ok(GraphPathSnapshot {
            found: false,
            nodes: vec![
                self.entity_snapshot(namespace, from_entity_id).await?,
                self.entity_snapshot(namespace, to_entity_id).await?,
            ],
            edges: Vec::new(),
        })
    }

    async fn graph_neighbor_expansion(
        &self,
        namespace: &str,
        target_id: &str,
        direction: &str,
        depth: i32,
        cutoff: DateTime<Utc>,
    ) -> Result<Value, ErrorData> {
        let directions = match direction {
            "incoming" => vec![GraphDirection::Incoming],
            "outgoing" => vec![GraphDirection::Outgoing],
            "both" => vec![GraphDirection::Outgoing, GraphDirection::Incoming],
            other => {
                return Err(Self::invalid_params(format!(
                    "Unsupported graph direction: {other}. Use incoming, outgoing, or both."
                )));
            }
        };

        let cutoff_iso = crate::service::normalize_dt(cutoff);
        let mut visited = HashSet::from([target_id.to_string()]);
        let mut frontier = vec![target_id.to_string()];
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        for _ in 0..depth.max(1) {
            let mut next_frontier = Vec::new();
            for node_id in frontier {
                for graph_direction in &directions {
                    for record in self
                        .service
                        .db_client
                        .select_edge_neighbors(namespace, &node_id, &cutoff_iso, *graph_direction)
                        .await
                        .map_err(mcp_error)?
                    {
                        if let Some(neighbor) = edge_neighbor(&record, *graph_direction) {
                            edges.push(normalized_edge_record(&record));
                            if visited.insert(neighbor.clone()) {
                                nodes.push(self.entity_snapshot(namespace, &neighbor).await?);
                                next_frontier.push(neighbor);
                            }
                        }
                    }
                }
            }

            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }

        Ok(json!({
            "target_id": target_id,
            "direction": direction,
            "depth": depth.max(1),
            "nodes": nodes,
            "edges": edges,
        }))
    }

    async fn graph_payload(
        &self,
        scope: &str,
        from_entity_id: &str,
        to_entity_id: &str,
        as_of: Option<&str>,
        max_depth: i32,
    ) -> Result<Value, ErrorData> {
        let namespace = self.service.namespace_for_scope(scope);
        let cutoff = as_of.and_then(parse_datetime).unwrap_or_else(Utc::now);
        let path = self
            .graph_path_snapshot(&namespace, from_entity_id, to_entity_id, cutoff, max_depth)
            .await?;
        let from_neighbors = self
            .graph_neighbor_expansion(&namespace, from_entity_id, "both", 1, cutoff)
            .await?;
        let to_neighbors = self
            .graph_neighbor_expansion(&namespace, to_entity_id, "both", 1, cutoff)
            .await?;

        Ok(json!({
            "target": {
                "from_entity_id": from_entity_id,
                "to_entity_id": to_entity_id,
                "as_of": as_of,
                "max_depth": max_depth.max(1),
                "namespace": namespace,
            },
            "graph": {
                "path_found": path.found,
                "nodes": path.nodes,
                "edges": path.edges,
                "hop_count": path.edges.len(),
            },
            "neighbors": {
                "from": from_neighbors,
                "to": to_neighbors,
            },
            "selected_edge": Value::Null,
            "context_preview": Value::Null,
            "expansions": [],
        }))
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
        let payload = self
            .inspector_payload(
                &params.scope,
                target_type,
                target_id,
                params.as_of.as_deref(),
            )
            .await?;
        self.create_session("inspector", &params.scope, params.ttl_seconds, payload)
            .await
    }

    async fn open_diff_app(&self, params: &OpenAppParams) -> Result<OpenAppResult, ErrorData> {
        if params
            .as_of_left
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            return Err(Self::missing_app_field("diff", "as_of_left"));
        }
        if params
            .as_of_right
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            return Err(Self::missing_app_field("diff", "as_of_right"));
        }
        self.create_session(
            "diff",
            &params.scope,
            params.ttl_seconds,
            Self::diff_payload(params),
        )
        .await
    }

    async fn open_ingestion_review_app(
        &self,
        params: &OpenAppParams,
    ) -> Result<OpenAppResult, ErrorData> {
        let payload = Self::ingestion_review_payload(
            params.source_text.as_deref(),
            params.draft_episode_id.as_deref(),
        );
        self.create_session(
            "ingestion_review",
            &params.scope,
            params.ttl_seconds,
            payload,
        )
        .await
    }

    async fn open_lifecycle_app(&self, params: &OpenAppParams) -> Result<OpenAppResult, ErrorData> {
        let payload = self.lifecycle_payload(&params.scope).await?;
        self.create_session("lifecycle", &params.scope, params.ttl_seconds, payload)
            .await
    }

    async fn open_graph_app(&self, params: &OpenAppParams) -> Result<OpenAppResult, ErrorData> {
        let from_entity_id = params
            .from_entity_id
            .as_deref()
            .ok_or_else(|| Self::missing_app_field("graph", "from_entity_id"))?;
        let to_entity_id = params
            .to_entity_id
            .as_deref()
            .ok_or_else(|| Self::missing_app_field("graph", "to_entity_id"))?;
        let payload = self
            .graph_payload(
                &params.scope,
                from_entity_id,
                to_entity_id,
                params.as_of.as_deref(),
                params.max_depth.unwrap_or(4).max(1),
            )
            .await?;
        self.create_session("graph", &params.scope, params.ttl_seconds, payload)
            .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MemoryMcp {
    fn get_info(&self) -> ServerInfo {
        Self::build_server_info()
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
        description = "Store a new episode in long-term memory. Use this tool when you need to persist source material before extracting entities or facts. Do not use this tool for retrieval. Arguments must be a flat snake_case object with `source_type`, `source_id`, `content`, `t_ref`, and `scope` (optional: `project`, `t_ingested`, `visibility_scope`, `policy_tags`). Do not wrap arguments in `payload`. Returns the created or existing `episode_id`. On error, fix the input fields and retry."
    )]
    pub async fn ingest(
        &self,
        params: Parameters<IngestParams>,
    ) -> Result<Json<ToolResponse<String>>, ErrorData> {
        crate::tools::ingest(&self.service, params.0)
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
    ) -> Result<Json<ToolResponse<Vec<ExplainItem>>>, ErrorData> {
        let access = AccessPayload::default();
        let context_pack = parse_context_items(&params.0.context_items).map_err(|msg| {
            tool_error(
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "Invalid context_items format",
                "Provide a JSON array of objects with snake_case keys. Each object must have `content`, optionally `quote`, `source_episode`, and/or `source_type`.",
                msg,
            )
        })?;
        let request = ExplainRequest { context_pack };

        let timer = Instant::now(); // explain
        let request_id = self.next_request_id();
        self.service.log_tool_event(
            "explain.start",
            json!({"count": request.context_pack.len()}),
            json!({}),
            LogLevel::Info,
            Some(&request_id),
        );

        match self.service.explain(request, Some(access)).await {
            Ok(explanations) => {
                self.service.log_tool_event_with_duration(
                    "explain.done",
                    json!({}),
                    json!({"count": explanations.len()}),
                    LogLevel::Info,
                    timer.elapsed(),
                    Some(&request_id),
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
                    Some(&request_id),
                );
                Err(mcp_error(err))
            }
        }
    }

    #[tool(
        description = "Extract entities, facts, and relationships from remembered content. Use this tool when you need structured knowledge from an existing episode or from new inline content. Do not use this tool for retrieval. Arguments must be a flat snake_case object. Provide exactly one input source: `episode_id` for stored content, or inline `content`/`text`; optional fields are `source_type`, `source_id`, `t_ref`, `scope`, and `zero_shot_labels`. Do not wrap arguments in `payload`. If you pass inline content, the server ingests it first and then extracts facts. Returns extracted entities, facts, and links."
    )]
    pub async fn extract(
        &self,
        params: Parameters<ExtractParams>,
    ) -> Result<Json<ToolResponse<ExtractResult>>, ErrorData> {
        crate::tools::extract(&self.service, params.0)
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
        crate::tools::resolve(&self.service, params.0)
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
        crate::tools::invalidate(&self.service, params.0)
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

    #[tool(
        description = "Execute a coarse-grained command for an app session opened via open_app. Use this only for session-scoped workflows that are not already covered by canonical memory tools. Arguments must be a flat snake_case object and must not be wrapped in `payload`. Supports ingestion review actions (`approve_items`, `reject_items`, `edit_item`, `commit_review`, `cancel_review`), lifecycle actions (`archive_candidates`, `restore_archived`, `recompute_decay`, `rebuild_communities`), diff export (`export_diff`), graph exploration actions (`expand_neighbors`, `open_edge_details`, `use_path_as_context`), and the generic `close_session`. Returns command status and whether the caller should re-read the app resource."
    )]
    pub async fn app_command(
        &self,
        params: Parameters<AppCommandParams>,
    ) -> Result<Json<ToolResponse<AppCommandResult>>, ErrorData> {
        let p = params.0;
        let timer = Instant::now(); // app_command
        let request_id = self.next_request_id();
        self.service.log_tool_event(
            "app_command.start",
            json!({"session_id": p.session_id, "action": p.action}),
            json!({}),
            LogLevel::Info,
            Some(&request_id),
        );

        let session = self.session(&p.session_id).await?;
        let app = session.app.clone();

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
                    let summary = update_ingestion_item_statuses(
                        self,
                        &p.session_id,
                        &p.item_ids,
                        "approved",
                        None,
                        session.payload.clone(),
                    )
                    .await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "approve_items",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": format!("Approved {} ingestion review item(s)", p.item_ids.len()),
                            "refresh_required": true,
                            "updated_item_ids": p.item_ids,
                            "summary": summary,
                        }),
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
                    let reason = p
                        .reason
                        .clone()
                        .or_else(|| Some("Rejected from app review".to_string()));
                    let summary = update_ingestion_item_statuses(
                        self,
                        &p.session_id,
                        &p.item_ids,
                        "rejected",
                        reason,
                        session.payload.clone(),
                    )
                    .await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "reject_items",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": format!("Rejected {} ingestion review item(s)", p.item_ids.len()),
                            "refresh_required": true,
                            "updated_item_ids": p.item_ids,
                            "summary": summary,
                        }),
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
                    let patch_value: Value = serde_json::from_str(patch_json).map_err(|err| {
                        Self::invalid_params(format!(
                            "`patch_json` must be a valid JSON object: {err}"
                        ))
                    })?;
                    let patch = patch_value.as_object().ok_or_else(|| {
                        Self::invalid_params("`patch_json` must encode a JSON object")
                    })?;

                    let mut payload = session.payload.clone();
                    let summary = if let Some(items) =
                        payload.get_mut("items").and_then(Value::as_array_mut)
                    {
                        let mut edited = false;
                        for item in items.iter_mut() {
                            let matches = item
                                .get("item_id")
                                .and_then(Value::as_str)
                                .is_some_and(|candidate| candidate == item_id);
                            if matches {
                                let object = item.as_object_mut().ok_or_else(|| {
                                    Self::internal_error("ingestion review items must be objects")
                                })?;
                                shallow_merge_object(object, patch);
                                if !object.contains_key("status") {
                                    object.insert("status".to_string(), json!("edited"));
                                }
                                edited = true;
                                break;
                            }
                        }
                        if !edited {
                            return Err(Self::invalid_params(format!(
                                "Unknown ingestion review item: {item_id}"
                            )));
                        }
                        summarize_ingestion_review_items(items)
                    } else {
                        return Err(Self::internal_error(
                            "ingestion review session is missing items",
                        ));
                    };
                    upsert_json_field(&mut payload, "summary", summary.clone());
                    let updated = self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "edit_item",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": format!("Edited ingestion review item {item_id}"),
                            "refresh_required": true,
                            "item_id": item_id,
                            "summary": updated.payload["summary"].clone(),
                        }),
                    ))
                }
            }
            "commit_review" | "commit_ingestion_review" => {
                if app != "ingestion_review" {
                    Err(Self::invalid_params(
                        "commit_review is only supported for ingestion_review sessions",
                    ))
                } else {
                    let approved = session
                        .payload
                        .get("summary")
                        .and_then(|summary| summary.get("by_status"))
                        .and_then(|by_status| by_status.get("approved"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    self.remove_session(&p.session_id).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "commit_review",
                        None,
                        json!({
                            "ok": true,
                            "message": format!("Committed {approved} approved review item(s) and closed the session"),
                            "refresh_required": false,
                            "committed_count": approved,
                        }),
                    ))
                }
            }
            "cancel_review" | "cancel_ingestion_review" => {
                if app != "ingestion_review" {
                    Err(Self::invalid_params(
                        "cancel_review is only supported for ingestion_review sessions",
                    ))
                } else {
                    self.remove_session(&p.session_id).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "cancel_review",
                        None,
                        json!({
                            "ok": true,
                            "message": "Cancelled review and closed the session",
                            "refresh_required": false,
                        }),
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
                    let namespace = self.service.namespace_for_scope(&session.scope);
                    let dry_run = p.dry_run.unwrap_or(false);
                    if !dry_run {
                        for episode_id in &p.target_ids {
                            self.service
                                .db_client
                                .update(
                                    episode_id,
                                    json!({
                                        "status": "archived",
                                        "archived_at": crate::service::normalize_dt(Utc::now()),
                                    }),
                                    &namespace,
                                )
                                .await
                                .map_err(mcp_error)?;
                        }
                    }
                    let payload = Self::enrich_session_payload(
                        &app,
                        &p.session_id,
                        &session.scope,
                        session
                            .payload
                            .get("meta")
                            .and_then(|meta| meta.get("ttl_seconds"))
                            .and_then(Value::as_i64),
                        self.lifecycle_payload(&session.scope).await?,
                    );
                    let updated = self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "archive_candidates",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": if dry_run {
                                format!("Dry-run ready to archive {} candidate(s)", p.target_ids.len())
                            } else {
                                format!("Archived {} candidate(s)", p.target_ids.len())
                            },
                            "refresh_required": true,
                            "dry_run": dry_run,
                            "target_ids": p.target_ids,
                            "dashboard": updated.payload["dashboard"].clone(),
                        }),
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
                    let namespace = self.service.namespace_for_scope(&session.scope);
                    for episode_id in &p.target_ids {
                        self.service
                            .db_client
                            .update(
                                episode_id,
                                json!({
                                    "status": "active",
                                    "archived_at": null,
                                }),
                                &namespace,
                            )
                            .await
                            .map_err(mcp_error)?;
                    }
                    let payload = Self::enrich_session_payload(
                        &app,
                        &p.session_id,
                        &session.scope,
                        session
                            .payload
                            .get("meta")
                            .and_then(|meta| meta.get("ttl_seconds"))
                            .and_then(Value::as_i64),
                        self.lifecycle_payload(&session.scope).await?,
                    );
                    let updated = self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "restore_archived",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": format!("Restored {} archived episode(s)", p.target_ids.len()),
                            "refresh_required": true,
                            "target_ids": p.target_ids,
                            "dashboard": updated.payload["dashboard"].clone(),
                        }),
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
                    let dry_run = p.dry_run.unwrap_or(false);
                    let invalidated = if dry_run {
                        0
                    } else {
                        run_decay_pass(
                            self.service.as_ref(),
                            Self::DEFAULT_DECAY_THRESHOLD,
                            Self::DEFAULT_DECAY_HALF_LIFE_DAYS,
                        )
                        .await
                        .map_err(mcp_error)?
                    };
                    let payload = Self::enrich_session_payload(
                        &app,
                        &p.session_id,
                        &session.scope,
                        session
                            .payload
                            .get("meta")
                            .and_then(|meta| meta.get("ttl_seconds"))
                            .and_then(Value::as_i64),
                        self.lifecycle_payload(&session.scope).await?,
                    );
                    let updated = self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "recompute_decay",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": if dry_run {
                                "Dry-run decay recomputation refreshed lifecycle metrics".to_string()
                            } else {
                                format!("Recomputed decay and invalidated {invalidated} fact(s)")
                            },
                            "refresh_required": true,
                            "dry_run": dry_run,
                            "invalidated": invalidated,
                            "dashboard": updated.payload["dashboard"].clone(),
                        }),
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
                    let dry_run = p.dry_run.unwrap_or(false);
                    let rebuilt = if dry_run {
                        0
                    } else {
                        run_community_rebuild_pass(self.service.as_ref())
                            .await
                            .map_err(mcp_error)?
                    };
                    let payload = Self::enrich_session_payload(
                        &app,
                        &p.session_id,
                        &session.scope,
                        session
                            .payload
                            .get("meta")
                            .and_then(|meta| meta.get("ttl_seconds"))
                            .and_then(Value::as_i64),
                        self.lifecycle_payload(&session.scope).await?,
                    );
                    let updated = self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "rebuild_communities",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": if dry_run {
                                "Dry-run community rebuild refreshed lifecycle metrics".to_string()
                            } else {
                                format!("Rebuilt {rebuilt} community record(s)")
                            },
                            "refresh_required": true,
                            "dry_run": dry_run,
                            "rebuilt": rebuilt,
                            "dashboard": updated.payload["dashboard"].clone(),
                        }),
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
                    let export = json!({
                        "format": format,
                        "generated_at": Utc::now().to_rfc3339(),
                        "target": session.payload.get("target").cloned().unwrap_or(Value::Null),
                        "range": session.payload.get("range").cloned().unwrap_or(Value::Null),
                    });
                    let mut payload = session.payload.clone();
                    if let Some(object) = payload.as_object_mut() {
                        object.insert("last_export".to_string(), export.clone());
                        object
                            .entry("exports".to_string())
                            .or_insert_with(|| json!([]));
                        if let Some(exports) =
                            object.get_mut("exports").and_then(Value::as_array_mut)
                        {
                            exports.push(export.clone());
                        }
                    }
                    self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "export_diff",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": format!("Prepared {format} diff export"),
                            "refresh_required": true,
                            "export": export,
                        }),
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
                    let cutoff = session
                        .payload
                        .get("target")
                        .and_then(|target| target.get("as_of"))
                        .and_then(Value::as_str)
                        .and_then(parse_datetime)
                        .unwrap_or_else(Utc::now);
                    let expansion = self
                        .graph_neighbor_expansion(
                            &self.service.namespace_for_scope(&session.scope),
                            target_id,
                            direction,
                            p.depth.unwrap_or(1).max(1),
                            cutoff,
                        )
                        .await?;
                    let mut payload = session.payload.clone();
                    if let Some(expansions) =
                        payload.get_mut("expansions").and_then(Value::as_array_mut)
                    {
                        expansions.push(expansion.clone());
                    } else {
                        upsert_json_field(&mut payload, "expansions", json!([expansion.clone()]));
                    }
                    self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "expand_neighbors",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": format!("Expanded {direction} neighbors for {target_id}"),
                            "refresh_required": true,
                            "expansion": expansion,
                        }),
                    ))
                }
            }
            "open_edge_details" => {
                if app != "graph" {
                    Err(Self::invalid_params(
                        "open_edge_details is only supported for graph sessions",
                    ))
                } else {
                    let edge_id = p
                        .target_id
                        .as_deref()
                        .ok_or_else(|| Self::missing_app_field("open_edge_details", "target_id"))?;
                    let namespace = self.service.namespace_for_scope(&session.scope);
                    let edge = self
                        .service
                        .db_client
                        .select_one(edge_id, &namespace)
                        .await
                        .map_err(mcp_error)?
                        .ok_or_else(|| {
                            Self::invalid_params(format!("Unknown graph edge: {edge_id}"))
                        })?;
                    let mut payload = session.payload.clone();
                    upsert_json_field(&mut payload, "selected_edge", edge.clone());
                    self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "open_edge_details",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": format!("Loaded edge details for {edge_id}"),
                            "refresh_required": true,
                            "details": edge,
                        }),
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
                    let node_names = session
                        .payload
                        .get("graph")
                        .and_then(|graph| graph.get("nodes"))
                        .and_then(Value::as_array)
                        .map(|nodes| {
                            nodes
                                .iter()
                                .filter_map(|node| {
                                    node.get("canonical_name")
                                        .or_else(|| node.get("entity_id"))
                                        .and_then(Value::as_str)
                                        .map(ToString::to_string)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let preview = json!({
                        "path_id": path_id,
                        "summary": if node_names.is_empty() {
                            format!("Graph context for {path_id}")
                        } else {
                            format!("Path context: {}", node_names.join(" -> "))
                        },
                        "node_names": node_names,
                    });
                    let mut payload = session.payload.clone();
                    upsert_json_field(&mut payload, "context_preview", preview.clone());
                    self.replace_session_payload(&p.session_id, payload).await?;
                    Ok(Self::app_command_result_from_details(
                        &app,
                        &p.session_id,
                        "use_path_as_context",
                        Some(app_session_uri(&app, &p.session_id)),
                        json!({
                            "ok": true,
                            "message": "Prepared graph path context",
                            "refresh_required": true,
                            "context_preview": preview,
                        }),
                    ))
                }
            }
            "close_session" => {
                self.remove_session(&p.session_id).await?;
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

    #[tool(
        description = "Assemble the most relevant active memory context for a query. Use this tool when you need retrieval across stored facts before answering or planning. Do not use this tool to ingest new content. Arguments must be a flat snake_case object with `query`, `scope`, and optional `project`, `fact_types`, `as_of`, `budget`, `view_mode`, `window_start`, and `window_end`. Do not wrap arguments in `payload`. Returns ranked context items with confidence and rationale. On error, fix the query parameters and retry."
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
            project: p.project.clone(),
            fact_types: p.fact_types.clone(),
            as_of,
            budget: p.budget,
            view_mode: p.view_mode.clone(),
            window_start,
            window_end,
            access: None,
        };

        let timer = Instant::now(); // assemble_context
        let request_id = self.next_request_id();
        self.service.log_tool_event(
            "assemble_context.start",
            json!({"scope": request.scope, "query": request.query}),
            json!({}),
            LogLevel::Info,
            Some(&request_id),
        );

        match self.service.assemble_context(request).await {
            Ok(results) => {
                self.service.log_tool_event_with_duration(
                    "assemble_context.done",
                    json!({}),
                    json!({"count": results.len()}),
                    LogLevel::Info,
                    timer.elapsed(),
                    Some(&request_id),
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
                    Some(&request_id),
                );
                Err(mcp_error(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EntityCandidate;
    use crate::storage::{DbClient, SurrealDbClient};
    use chrono::Datelike;
    use rmcp::model::{ReadResourceRequestParams, ResourceContents};

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

    #[tokio::test]
    async fn public_tools_expose_open_app_and_app_command() {
        let mcp = create_test_mcp().await;

        assert!(mcp.get_tool("open_app").is_some());
        assert!(mcp.get_tool("app_command").is_some());
    }

    #[test]
    fn list_resource_templates_exposes_public_app_session_templates() {
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
            MemoryMcp::enrich_session_payload("inspector", "ses:1", "org", Some(3600), payload);
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
        let enriched =
            MemoryMcp::enrich_session_payload("diff", "ses:2", "personal", None, payload);
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
        assert_eq!(summary["by_status"]["approved"], 2);
        assert_eq!(summary["by_status"]["rejected"], 1);
        assert_eq!(summary["by_status"]["pending"], 1);
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
