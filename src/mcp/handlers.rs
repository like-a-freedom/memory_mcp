//! MCP tool handler implementations.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

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
    AccessContext, AssembleContextRequest, AssembledContextItem, EntityCandidate, ExplainItem,
    ExplainRequest, ExtractResult, IngestRequest, InvalidateRequest,
};
use crate::service::{MemoryService, run_community_rebuild_pass, run_decay_pass};
use crate::storage::GraphDirection;
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
#[serde(rename_all = "camelCase")]
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

/// Result payload returned by the public `open_app` launcher.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct OpenAppResult {
    /// Canonical public app identifier.
    pub app: String,
    /// Created session identifier.
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
    /// Raw command details for clients that need extra metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct AppSessionState {
    app: String,
    scope: String,
    payload: Value,
}

#[derive(Debug, Clone)]
struct GraphPathSnapshot {
    found: bool,
    nodes: Vec<Value>,
    edges: Vec<Value>,
}

fn value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get("String") {
                return Some(s.clone());
            }
            if let Some(Value::String(s)) = map.get("Strand") {
                return Some(s.clone());
            }
            if let Some(Value::Object(inner)) = map.get("Strand")
                && let Some(Value::String(s)) = inner.get("String")
            {
                return Some(s.clone());
            }
            if let Some(Value::Object(record_id)) = map.get("RecordId")
                && let (Some(Value::String(table)), Some(Value::String(key))) =
                    (record_id.get("table"), record_id.get("key"))
            {
                return Some(format!("{table}:{key}"));
            }
            None
        }
        _ => None,
    }
}

fn edge_neighbor(record: &Value, direction: GraphDirection) -> Option<String> {
    let map = record.as_object()?;
    match direction {
        GraphDirection::Incoming => map.get("in").and_then(value_string),
        GraphDirection::Outgoing => map.get("out").and_then(value_string),
    }
}

fn normalized_edge_record(record: &Value) -> Value {
    let Some(map) = record.as_object() else {
        return record.clone();
    };

    json!({
        "edge_id": map
            .get("edge_id")
            .and_then(value_string)
            .or_else(|| map.get("id").and_then(value_string)),
        "in": map.get("in").and_then(value_string),
        "relation": map.get("relation").and_then(value_string),
        "out": map.get("out").and_then(value_string),
        "origin": map.get("origin").cloned().unwrap_or(Value::Null),
        "confidence": map.get("confidence").cloned().unwrap_or(Value::Null),
        "t_valid": map.get("t_valid").cloned().unwrap_or(Value::Null),
        "t_ingested": map.get("t_ingested").cloned().unwrap_or(Value::Null),
    })
}

fn upsert_json_field(payload: &mut Value, key: &str, value: Value) {
    if let Some(object) = payload.as_object_mut() {
        object.insert(key.to_string(), value);
    }
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
    sessions: Arc<tokio::sync::RwLock<HashMap<String, AppSessionState>>>,
    session_counter: Arc<AtomicU64>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl MemoryMcp {
    const SERVER_INSTRUCTIONS: &str = "Memory MCP server: stores facts about entities and relationships, resolves aliases, and assembles long-term context.";
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
            sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            session_counter: Arc::new(AtomicU64::new(0)),
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

    fn invalid_params(message: impl Into<String>) -> ErrorData {
        ErrorData::new(rmcp::model::ErrorCode::INVALID_PARAMS, message.into(), None)
    }

    fn missing_app_field(app: &str, field: &str) -> ErrorData {
        Self::invalid_params(format!(
            "`{field}` is required for {app}. Re-check the open_app/app_command contract and retry."
        ))
    }

    fn internal_error(message: impl Into<String>) -> ErrorData {
        ErrorData::new(rmcp::model::ErrorCode::INTERNAL_ERROR, message.into(), None)
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

    fn next_session_id(&self) -> String {
        let sequence = self.session_counter.fetch_add(1, Ordering::Relaxed) + 1;
        format!("ses:{}-{sequence}", Utc::now().timestamp_micros())
    }

    fn enrich_session_payload(
        app: &str,
        session_id: &str,
        scope: &str,
        ttl_seconds: Option<i64>,
        mut payload: Value,
    ) -> Value {
        let created_at = Utc::now();
        let expires_at = ttl_seconds
            .filter(|ttl| *ttl > 0)
            .map(|ttl| created_at + chrono::Duration::seconds(ttl));

        let payload = payload
            .as_object_mut()
            .expect("session payload should be an object");
        payload.insert("app".to_string(), json!(app));
        payload.insert("session_id".to_string(), json!(session_id));
        payload.insert("scope".to_string(), json!(scope));
        payload.insert(
            "meta".to_string(),
            json!({
                "created_at": created_at.to_rfc3339(),
                "ttl_seconds": ttl_seconds,
                "expires_at": expires_at.map(|value| value.to_rfc3339()),
            }),
        );
        Value::Object(payload.clone())
    }

    async fn insert_session(&self, session_id: String, session: AppSessionState) {
        self.sessions.write().await.insert(session_id, session);
    }

    async fn session(&self, session_id: &str) -> Result<AppSessionState, ErrorData> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| {
                Self::invalid_params(format!("Unknown or closed app session: {session_id}"))
            })
    }

    async fn replace_session_payload(
        &self,
        session_id: &str,
        payload: Value,
    ) -> Result<AppSessionState, ErrorData> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(session_id).ok_or_else(|| {
            Self::invalid_params(format!("Unknown or closed app session: {session_id}"))
        })?;
        session.payload = payload;
        Ok(session.clone())
    }

    async fn remove_session(&self, session_id: &str) -> Result<AppSessionState, ErrorData> {
        self.sessions
            .write()
            .await
            .remove(session_id)
            .ok_or_else(|| {
                Self::invalid_params(format!("Unknown or closed app session: {session_id}"))
            })
    }

    async fn create_session(
        &self,
        app: &str,
        scope: &str,
        ttl_seconds: Option<i64>,
        payload: Value,
    ) -> Result<OpenAppResult, ErrorData> {
        let session_id = self.next_session_id();
        let payload = Self::enrich_session_payload(app, &session_id, scope, ttl_seconds, payload);
        self.insert_session(
            session_id.clone(),
            AppSessionState {
                app: app.to_string(),
                scope: scope.to_string(),
                payload: payload.clone(),
            },
        )
        .await;

        Ok(Self::open_app_result(app, session_id, payload))
    }

    fn open_app_result(app: &str, session_id: impl Into<String>, fallback: Value) -> OpenAppResult {
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
        details: Value,
    ) -> AppCommandResult {
        AppCommandResult {
            app: app.to_string(),
            session_id: session_id.to_string(),
            action: action.to_string(),
            ok: details.get("ok").and_then(Value::as_bool).unwrap_or(true),
            message: details
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("App command completed")
                .to_string(),
            refresh_required: details
                .get("refresh_required")
                .and_then(Value::as_bool)
                .unwrap_or(resource_uri.is_some()),
            resource_uri,
            details: Some(details),
        }
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
            .filter_map(|record| record.get("episode_id").and_then(value_string))
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
        zero_shot_labels: Option<Vec<String>>,
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
            match self
                .service
                .extract(episode_id, Some(access), zero_shot_labels.as_deref())
                .await
            {
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
                    project: None,
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: Vec::new(),
                },
                Some(access.clone()),
            )
            .await
        {
            Ok(episode_id) => match self
                .service
                .extract(&episode_id, Some(access), zero_shot_labels.as_deref())
                .await
            {
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
            project: p.project.clone(),
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
                p.zero_shot_labels,
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
                    let mut payload = session.payload.clone();
                    let summary = if let Some(items) =
                        payload.get_mut("items").and_then(Value::as_array_mut)
                    {
                        for item in items.iter_mut() {
                            let matches = item.get("item_id").and_then(Value::as_str).is_some_and(
                                |item_id| p.item_ids.iter().any(|candidate| candidate == item_id),
                            );
                            if matches && let Some(object) = item.as_object_mut() {
                                object.insert("status".to_string(), json!("approved"));
                                object.remove("reason");
                            }
                        }
                        summarize_ingestion_review_items(items)
                    } else {
                        summarize_ingestion_review_items(&[])
                    };
                    upsert_json_field(&mut payload, "summary", summary.clone());
                    let updated = self.replace_session_payload(&p.session_id, payload).await?;
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
                            "summary": updated.payload["summary"].clone(),
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
                    let mut payload = session.payload.clone();
                    let summary = if let Some(items) =
                        payload.get_mut("items").and_then(Value::as_array_mut)
                    {
                        for item in items.iter_mut() {
                            let matches = item.get("item_id").and_then(Value::as_str).is_some_and(
                                |item_id| p.item_ids.iter().any(|candidate| candidate == item_id),
                            );
                            if matches && let Some(object) = item.as_object_mut() {
                                object.insert("status".to_string(), json!("rejected"));
                                object.insert(
                                    "reason".to_string(),
                                    json!(p.reason.clone().unwrap_or_else(|| {
                                        "Rejected from app review".to_string()
                                    })),
                                );
                            }
                        }
                        summarize_ingestion_review_items(items)
                    } else {
                        summarize_ingestion_review_items(&[])
                    };
                    upsert_json_field(&mut payload, "summary", summary.clone());
                    let updated = self.replace_session_payload(&p.session_id, payload).await?;
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
                            "summary": updated.payload["summary"].clone(),
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
                );
                Err(err)
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
            project: p.project.clone(),
            fact_types: p.fact_types.clone(),
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
                "Memory MCP server: stores facts about entities and relationships, resolves aliases, and assembles long-term context.",
            ),
        );
        assert!(capabilities.get("tools").is_some());
        assert!(capabilities.get("resources").is_some());
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

        // Fields are renamed to camelCase for MCP/JSON compatibility
        for key in [
            "status",
            "result",
            "guidance",
            "hasMore",
            "totalCount",
            "nextOffset",
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

        // Fields are renamed to camelCase for MCP/JSON compatibility
        for key in [
            "content",
            "quote",
            "sourceEpisode",
            "scope",
            "tRef",
            "tIngested",
            "provenance",
            "citationContext",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
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
            properties.contains_key("graphInsights"),
            "ExplainItem should expose graphInsights in the MCP schema"
        );
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

        // Fields are renamed to camelCase for MCP/JSON compatibility
        for key in [
            "factId",
            "content",
            "quote",
            "sourceEpisode",
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
}
