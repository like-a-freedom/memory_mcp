//! Graph traversal for the `graph` app's sessions.
//!
//! This was the reachable half of `service/apps/graph.rs`. It lived in the
//! transport adapter because that is where the store adapter was when the
//! app-session BFS was written; it never moved, because nothing required a
//! traversal to sit next to its caller. ADR-0060 moved the reads out to
//! `memory/retrieval/graph_reads.rs` and left the traversal behind.
//!
//! The traversal is parameterised on `&KnowledgeGraphStore` rather than on
//! `&impl GraphContext`: the port existed so the container could supply the
//! store, and once the traversal no longer needs the container there is
//! nothing to abstract over. The `GraphContext` impls stay in
//! `service/apps/graph.rs` for the two readers that still use them.

use std::collections::{HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::knowledge::graph_store::KnowledgeGraphStore;
use crate::shared::temporal::normalize_dt;
use crate::storage::GraphDirection;

// ---------------------------------------------------------------------------
// Graph traversal for app sessions
// ---------------------------------------------------------------------------

/// Result of a BFS path search between two entities.
#[derive(Debug, Clone)]
pub struct GraphPathSnapshot {
    pub found: bool,
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
}

/// Typed state persisted for a graph app session.
///
/// The nested values intentionally remain JSON values because graph records
/// are storage-shaped and may gain fields independently of the session
/// protocol. The session envelope and its mutation points are typed here,
/// preserving the exact payload shape consumed by the app HTML resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct GraphSessionState {
    pub(crate) target: GraphSessionTarget,
    pub(crate) graph: GraphSessionGraph,
    pub(crate) neighbors: GraphSessionNeighbors,
    pub(crate) selected_edge: Value,
    pub(crate) context_preview: Value,
    pub(crate) expansions: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct GraphSessionTarget {
    pub(crate) from_entity_id: String,
    pub(crate) to_entity_id: String,
    pub(crate) max_depth: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) as_of: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct GraphSessionGraph {
    pub(crate) path_found: bool,
    pub(crate) nodes: Vec<Value>,
    pub(crate) edges: Vec<Value>,
    pub(crate) hop_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct GraphSessionNeighbors {
    pub(crate) from: Value,
    pub(crate) to: Value,
}

impl GraphSessionState {
    #[cfg(any(test, feature = "mcp-apps"))]
    pub(crate) fn from_payload(payload: &Value) -> Result<Self, MemoryError> {
        serde_json::from_value(payload.clone()).map_err(|error| {
            MemoryError::Validation(format!("invalid graph session payload: {error}"))
        })
    }

    pub(crate) fn to_payload(&self) -> Result<Value, MemoryError> {
        serde_json::to_value(self).map_err(|error| {
            MemoryError::Storage(format!(
                "failed to serialize graph session payload: {error}"
            ))
        })
    }
}

/// Extracts the neighbor entity ID from an edge record based on traversal direction.
pub fn edge_neighbor(record: &Value, direction: GraphDirection) -> Option<String> {
    let map = record.as_object()?;
    match direction {
        GraphDirection::Incoming => map.get("in").and_then(|v| v.as_str()).map(String::from),
        GraphDirection::Outgoing => map.get("out").and_then(|v| v.as_str()).map(String::from),
    }
}

/// Returns a JSON snapshot of an entity (entity_id + canonical_name).
pub async fn entity_snapshot(
    store: &KnowledgeGraphStore,
    entity_id: &str,
) -> Result<Value, MemoryError> {
    let record = store.select_entity(entity_id).await?;
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

/// BFS path finding between two entities in the knowledge graph.
pub async fn graph_path_snapshot(
    store: &KnowledgeGraphStore,
    from_entity_id: &str,
    to_entity_id: &str,
    cutoff: DateTime<Utc>,
    max_depth: i32,
) -> Result<GraphPathSnapshot, MemoryError> {
    if from_entity_id == to_entity_id {
        return Ok(GraphPathSnapshot {
            found: true,
            nodes: vec![entity_snapshot(store, from_entity_id).await?],
            edges: Vec::new(),
        });
    }

    let cutoff_iso = normalize_dt(cutoff);
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
            let records = store
                .select_edge_neighbors(&current, &cutoff_iso, direction)
                .await?;
            for record in records {
                let Some(neighbor) = edge_neighbor(&record, direction) else {
                    continue;
                };
                let mut next_nodes = nodes.clone();
                next_nodes.push(neighbor.clone());
                let mut next_edges = edges.clone();
                next_edges.push(crate::storage::value_helpers::normalized_edge_record(
                    &record,
                ));

                if neighbor == to_entity_id {
                    let mut snapshots = Vec::with_capacity(next_nodes.len());
                    for node_id in next_nodes {
                        snapshots.push(entity_snapshot(store, &node_id).await?);
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
            entity_snapshot(store, from_entity_id).await?,
            entity_snapshot(store, to_entity_id).await?,
        ],
        edges: Vec::new(),
    })
}

/// BFS neighbor expansion from a target entity.
pub async fn graph_neighbor_expansion(
    store: &KnowledgeGraphStore,
    target_id: &str,
    direction: &str,
    depth: i32,
    cutoff: DateTime<Utc>,
) -> Result<Value, MemoryError> {
    let directions = match direction {
        "incoming" => vec![GraphDirection::Incoming],
        "outgoing" => vec![GraphDirection::Outgoing],
        "both" => vec![GraphDirection::Outgoing, GraphDirection::Incoming],
        other => {
            return Err(MemoryError::Validation(format!(
                "Unsupported graph direction: {other}. Use incoming, outgoing, or both."
            )));
        }
    };

    let cutoff_iso = normalize_dt(cutoff);
    let mut visited = HashSet::from([target_id.to_string()]);
    let mut frontier = vec![target_id.to_string()];
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for _ in 0..depth.max(1) {
        let mut next_frontier = Vec::new();
        for node_id in &frontier {
            for graph_direction in &directions {
                for record in store
                    .select_edge_neighbors(node_id, &cutoff_iso, *graph_direction)
                    .await?
                {
                    if let Some(neighbor) = edge_neighbor(&record, *graph_direction) {
                        edges.push(crate::storage::value_helpers::normalized_edge_record(
                            &record,
                        ));
                        if visited.insert(neighbor.clone()) {
                            nodes.push(entity_snapshot(store, &neighbor).await?);
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

/// Builds the full graph payload: path + neighbor expansion for both endpoints.
pub async fn graph_payload(
    store: &KnowledgeGraphStore,
    from_entity_id: &str,
    to_entity_id: &str,
    cutoff: DateTime<Utc>,
    max_depth: i32,
) -> Result<Value, MemoryError> {
    let path = graph_path_snapshot(store, from_entity_id, to_entity_id, cutoff, max_depth).await?;
    let from_neighbors = graph_neighbor_expansion(store, from_entity_id, "both", 1, cutoff).await?;
    let to_neighbors = graph_neighbor_expansion(store, to_entity_id, "both", 1, cutoff).await?;
    let hop_count = path.edges.len();

    GraphSessionState {
        target: GraphSessionTarget {
            from_entity_id: from_entity_id.to_string(),
            to_entity_id: to_entity_id.to_string(),
            max_depth: max_depth.max(1),
            as_of: None,
        },
        graph: GraphSessionGraph {
            path_found: path.found,
            nodes: path.nodes,
            edges: path.edges,
            hop_count,
        },
        neighbors: GraphSessionNeighbors {
            from: from_neighbors,
            to: to_neighbors,
        },
        selected_edge: Value::Null,
        context_preview: Value::Null,
        expansions: Vec::new(),
    }
    .to_payload()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::service::MemoryService;

    /// `MemoryService::resolve_entity` used to provide this inside the crate.
    /// No production caller reached it, so the container's copy went and the
    /// tests call the capability directly.
    async fn resolve_entity_for_test(
        service: &MemoryService,
        entity_type: &str,
        name: &str,
    ) -> Result<String, crate::MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_resolve::ResolveCapability::resolve_from_service(
            service,
            crate::models::EntityCandidate {
                entity_type: entity_type.to_string(),
                canonical_name: name.to_string(),
                aliases: Vec::new(),
            },
            None,
        )
        .await
    }
    use crate::storage::{DbClient, SurrealDbClient};

    #[test]
    fn graph_session_state_round_trips_the_app_payload_shape() {
        let payload = json!({
            "target": {
                "from_entity_id": "entity:alice",
                "to_entity_id": "entity:acme",
                "max_depth": 4
            },
            "graph": {
                "path_found": true,
                "nodes": [{"entity_id": "entity:alice", "canonical_name": "Alice"}],
                "edges": [],
                "hop_count": 0
            },
            "neighbors": {
                "from": {
                    "target_id": "entity:alice",
                    "direction": "both",
                    "depth": 1,
                    "nodes": [],
                    "edges": []
                },
                "to": {
                    "target_id": "entity:acme",
                    "direction": "both",
                    "depth": 1,
                    "nodes": [],
                    "edges": []
                }
            },
            "selected_edge": null,
            "context_preview": null,
            "expansions": []
        });

        let state = GraphSessionState::from_payload(&payload).expect("valid graph payload");
        let encoded = state.to_payload().expect("graph payload should serialize");

        assert_eq!(encoded, payload);
    }

    #[tokio::test]
    async fn resolve_entity_by_type_delegates_to_resolve() {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "resolve_entity_test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in-memory test db"),
        );
        for ns in &namespaces {
            db_client
                .apply_migrations(ns)
                .await
                .expect("apply migrations");
        }
        let service = MemoryService::new(db_client, "org".to_string(), "warn".to_string(), 50, 100)
            .expect("create test service");

        // Resolve the same entity via different typed methods
        let id1 = resolve_entity_for_test(&service, "person", "Alice Smith")
            .await
            .expect("resolve person");
        let id2 = resolve_entity_for_test(&service, "person", "Alice Smith")
            .await
            .expect("resolve person again");
        assert_eq!(id1, id2);

        let id3 = resolve_entity_for_test(&service, "company", "Acme Corp")
            .await
            .expect("resolve company");
        assert_ne!(id1, id3);
    }

    #[tokio::test]
    async fn relate_creates_edge_between_entities() {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces("relate_test", &namespaces, "warn")
                .await
                .expect("connect in-memory test db"),
        );
        for ns in &namespaces {
            db_client
                .apply_migrations(ns)
                .await
                .expect("apply migrations");
        }
        let service = MemoryService::new(db_client, "org".to_string(), "warn".to_string(), 50, 100)
            .expect("create test service");

        let from_id = resolve_entity_for_test(&service, "person", "Alice Relate")
            .await
            .expect("resolve alice");
        let to_id = resolve_entity_for_test(&service, "company", "Acme Relate")
            .await
            .expect("resolve acme");

        service
            .relate(
                &from_id,
                "works_at",
                &to_id,
                crate::models::EdgeAttributes::inferred(),
            )
            .await
            .expect("relate entities");
    }
}
