#![cfg_attr(not(feature = "mcp-apps"), allow(dead_code, unused_imports))]
use std::collections::{HashSet, VecDeque};

use chrono::{DateTime, Utc};
use rmcp::ErrorData;
use rmcp::model::ResourceContents;
use rmcp::model::{
    ListResourceTemplatesResult, ListResourcesResult, ReadResourceRequestParams, ReadResourceResult,
};
use serde_json::{Value, json};

#[cfg(feature = "mcp-apps")]
use crate::service::apply_ingestion_review_status;
use crate::service::value_helpers::{json_string, normalized_edge_record};
use crate::service::{IngestionReviewItem, IngestionReviewSummary};
use crate::storage::GraphDirection;

use super::super::error::mcp_error;
use super::super::params::OpenAppParams;
use super::super::parsers::parse_datetime;
use super::super::resources::{
    APPS_INDEX_URI, app_root_payload, app_session_html_document, apps_index_payload,
    parse_app_root_uri, parse_app_session_uri,
};
use super::super::resources::{app_catalog_resources, app_resource_templates};
use super::super::response::{AppCommandResult, OpenAppResult};
use super::super::session;
use super::MemoryMcp;

#[derive(Debug, Clone)]
struct GraphPathSnapshot {
    found: bool,
    nodes: Vec<Value>,
    edges: Vec<Value>,
}

pub(super) fn edge_neighbor(record: &Value, direction: GraphDirection) -> Option<String> {
    let map = record.as_object()?;
    match direction {
        GraphDirection::Incoming => map.get("in").and_then(json_string).map(String::from),
        GraphDirection::Outgoing => map.get("out").and_then(json_string).map(String::from),
    }
}

pub(super) fn upsert_json_field(payload: &mut Value, key: &str, value: Value) {
    if let Some(object) = payload.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

#[cfg(feature = "mcp-apps")]
pub(super) async fn update_ingestion_item_statuses(
    service: &MemoryMcp,
    session_id: &str,
    item_ids: &[String],
    status: &str,
    reason: Option<String>,
    session_payload: Value,
) -> Result<serde_json::Value, ErrorData> {
    let mut payload = session_payload;
    let summary = if let Some(items) = payload.get_mut("items").and_then(Value::as_array_mut) {
        let mut typed_items: Vec<IngestionReviewItem> =
            serde_json::from_value(Value::Array(items.clone())).map_err(|error| {
                session::internal_error(format!("invalid ingestion review items: {error}"))
            })?;
        let summary =
            apply_ingestion_review_status(&mut typed_items, item_ids, status, reason.as_deref());
        *items = serde_json::to_value(&typed_items)
            .map_err(|error| {
                session::internal_error(format!("failed to encode ingestion review items: {error}"))
            })?
            .as_array()
            .cloned()
            .unwrap_or_default();
        serde_json::to_value(summary).map_err(|error| {
            session::internal_error(format!(
                "failed to encode ingestion review summary: {error}"
            ))
        })?
    } else {
        serde_json::to_value(IngestionReviewSummary::from_items(&[])).map_err(|error| {
            session::internal_error(format!(
                "failed to encode ingestion review summary: {error}"
            ))
        })?
    };
    upsert_json_field(&mut payload, "summary", summary.clone());
    let updated = service.replace_session_payload(session_id, payload).await?;
    Ok(updated.payload["summary"].clone())
}

#[cfg(test)]
pub(super) fn shallow_merge_object(
    target: &mut serde_json::Map<String, Value>,
    patch: &serde_json::Map<String, Value>,
) {
    for (key, value) in patch {
        target.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
pub(super) fn summarize_ingestion_review_items(items: &[Value]) -> Value {
    let mut pending = 0;
    let mut approved = 0;
    let mut rejected = 0;
    let mut edited = 0;

    for item in items {
        match item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
        {
            "approved" => approved += 1,
            "rejected" => rejected += 1,
            "edited" => edited += 1,
            _ => pending += 1,
        }
    }

    json!({
        "total": items.len(),
        "pending": pending,
        "approved": approved,
        "rejected": rejected,
        "edited": edited,
        "committable": approved + edited,
    })
}

impl MemoryMcp {
    #[cfg(feature = "mcp-apps")]
    pub(super) fn list_resources_result() -> ListResourcesResult {
        ListResourcesResult {
            resources: app_catalog_resources(),
            meta: None,
            next_cursor: None,
        }
    }

    #[cfg(feature = "mcp-apps")]
    pub(super) fn list_resource_templates_result() -> ListResourceTemplatesResult {
        ListResourceTemplatesResult {
            resource_templates: app_resource_templates(),
            meta: None,
            next_cursor: None,
        }
    }

    pub(super) fn normalize_public_app_name(app: &str) -> Option<&'static str> {
        match app {
            "inspector" | "memory_inspector" => Some("inspector"),
            "diff" | "temporal_diff" => Some("diff"),
            "ingestion_review" | "ingestion" => Some("ingestion_review"),
            "lifecycle" | "lifecycle_console" => Some("lifecycle"),
            "graph" | "graph_path" => Some("graph"),
            _ => None,
        }
    }

    pub(super) fn enrich_session_payload(
        app: &str,
        session_id: &str,
        scope: &str,
        ttl_seconds: Option<i64>,
        payload: Value,
    ) -> Value {
        session::enrich_session_payload(app, session_id, scope, ttl_seconds, payload)
    }

    pub(super) async fn session(
        &self,
        session_id: &str,
    ) -> Result<session::AppSessionState, ErrorData> {
        self.session_manager.purge_expired().await;
        self.session_manager.get_valid(session_id).await
    }

    pub(super) async fn replace_session_payload(
        &self,
        session_id: &str,
        payload: Value,
    ) -> Result<session::AppSessionState, ErrorData> {
        self.session_manager
            .replace_payload(session_id, payload)
            .await
    }

    pub(super) async fn remove_session(
        &self,
        session_id: &str,
    ) -> Result<session::AppSessionState, ErrorData> {
        self.session_manager.remove(session_id).await
    }

    pub(super) async fn create_session(
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

    pub(super) fn app_command_result_from_details(
        app: &str,
        session_id: &str,
        action: &str,
        resource_uri: Option<String>,
        details: Value,
    ) -> AppCommandResult {
        session::app_command_result_from_details(app, session_id, action, resource_uri, details)
    }

    #[cfg(feature = "mcp-apps")]
    pub(super) async fn read_app_resource_payload(
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
            .app_store()
            .select_entity(entity_id, namespace)
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
        let namespace = self.service.namespace_for_scope(scope).map_err(mcp_error)?;
        let (record, record_namespace) = match target_type {
            "entity" => {
                let record = self
                    .service
                    .app_store()
                    .select_entity(target_id, &namespace)
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

    pub(super) async fn lifecycle_payload(&self, scope: &str) -> Result<Value, ErrorData> {
        serde_json::to_value(
            &self
                .service
                .build_lifecycle_view(scope)
                .await
                .map_err(mcp_error)?,
        )
        .map_err(|error| Self::internal_error(error.to_string()))
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
                    .app_store()
                    .select_graph_neighbors(namespace, &current, &cutoff_iso, direction)
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

    pub(super) async fn graph_neighbor_expansion(
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
                        .app_store()
                        .select_graph_neighbors(namespace, &node_id, &cutoff_iso, *graph_direction)
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
        let namespace = self.service.namespace_for_scope(scope).map_err(mcp_error)?;
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

    pub(super) async fn open_inspector_app(
        &self,
        params: &OpenAppParams,
    ) -> Result<OpenAppResult, ErrorData> {
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

    pub(super) async fn open_diff_app(
        &self,
        params: &OpenAppParams,
    ) -> Result<OpenAppResult, ErrorData> {
        let as_of_left = params
            .as_of_left
            .as_deref()
            .ok_or_else(|| Self::missing_app_field("diff", "as_of_left"))?;
        let as_of_right = params
            .as_of_right
            .as_deref()
            .ok_or_else(|| Self::missing_app_field("diff", "as_of_right"))?;
        let diff = self
            .service
            .build_diff(crate::service::DiffRequest {
                scope: params.scope.clone(),
                target_type: params.target_type.clone().unwrap_or_else(|| {
                    if params.target_id.is_some() {
                        "entity".to_string()
                    } else {
                        "scope".to_string()
                    }
                }),
                target_id: params.target_id.clone(),
                as_of_left: parse_datetime(as_of_left).ok_or_else(|| {
                    Self::invalid_params("`as_of_left` must be a valid ISO 8601 timestamp")
                })?,
                as_of_right: parse_datetime(as_of_right).ok_or_else(|| {
                    Self::invalid_params("`as_of_right` must be a valid ISO 8601 timestamp")
                })?,
                time_axis: params
                    .time_axis
                    .clone()
                    .unwrap_or_else(|| "valid".to_string()),
            })
            .await
            .map_err(mcp_error)?;
        let mut payload =
            serde_json::to_value(&diff).map_err(|error| Self::internal_error(error.to_string()))?;
        upsert_json_field(&mut payload, "exports", json!([]));
        self.create_session("diff", &params.scope, params.ttl_seconds, payload)
            .await
    }

    pub(super) async fn open_ingestion_review_app(
        &self,
        params: &OpenAppParams,
    ) -> Result<OpenAppResult, ErrorData> {
        let bundle = self
            .service
            .prepare_ingestion_review(crate::service::PrepareIngestionReviewRequest {
                scope: params.scope.clone(),
                source_text: params.source_text.clone(),
                draft_episode_id: params.draft_episode_id.clone(),
            })
            .await
            .map_err(mcp_error)?;
        let payload = serde_json::to_value(&bundle)
            .map_err(|error| Self::internal_error(error.to_string()))?;
        self.create_session(
            "ingestion_review",
            &params.scope,
            params.ttl_seconds,
            payload,
        )
        .await
    }

    pub(super) async fn open_lifecycle_app(
        &self,
        params: &OpenAppParams,
    ) -> Result<OpenAppResult, ErrorData> {
        let payload = self.lifecycle_payload(&params.scope).await?;
        self.create_session("lifecycle", &params.scope, params.ttl_seconds, payload)
            .await
    }

    pub(super) async fn open_graph_app(
        &self,
        params: &OpenAppParams,
    ) -> Result<OpenAppResult, ErrorData> {
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

    pub(super) async fn read_resource_result(
        &self,
        request: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, ErrorData> {
        #[cfg(not(feature = "mcp-apps"))]
        {
            let _ = request;
            Err(Self::invalid_params(
                "MCP app resources are disabled; enable the `mcp-apps` feature",
            ))
        }

        #[cfg(feature = "mcp-apps")]
        {
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
    }
}
