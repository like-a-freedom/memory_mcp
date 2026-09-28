//! The knowledge-graph write paths and the app-session state machine.
//!
//! Reads live in `memory::retrieval::graph_reads`, reached through the
//! `GraphContext` contract implemented here for the container and for
//! context assembly. What remains is what writes: relating entities,
//! recording edges, and the traversal state the map app serialises.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::knowledge::graph_store::KnowledgeGraphStore;
use crate::logging::StdoutLogger;
use crate::memory::retrieval::graph_reads::GraphContext;
use crate::service::MemoryService;
use crate::shared::temporal::normalize_dt;
use crate::storage::GraphDirection;
use crate::storage::value_helpers::string_from_value;

impl GraphContext for MemoryService {
    fn knowledge_graph_store(&self) -> KnowledgeGraphStore {
        KnowledgeGraphStore::new(self.db_client.clone(), self.active_namespace.clone())
    }
    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}

impl MemoryService {
    /// Finds an introduction chain.
    ///
    /// Graph traversal lives in this module; the method is exposed on
    /// `MemoryService` so callers can use it without reaching into the internal graph API.
    pub async fn find_intro_chain(
        &self,
        target_name: &str,
        max_hops: i32,
        as_of: Option<DateTime<Utc>>,
    ) -> Result<Vec<String>, MemoryError> {
        find_intro_chain(self, target_name, max_hops, as_of).await
    }

    /// Resolves an entity by its type and canonical name.
    ///
    /// Graph/entity convenience built on [`ResolveCapability`]; lives here with
    /// the other graph conveniences.
    pub async fn resolve_entity(
        &self,
        entity_type: &str,
        name: &str,
    ) -> Result<String, MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_resolve::ResolveCapability::resolve_from_service(
            self,
            crate::models::EntityCandidate {
                entity_type: entity_type.to_string(),
                canonical_name: name.to_string(),
                aliases: Vec::new(),
            },
            None,
        )
        .await
    }

    /// Creates a relationship edge between two entities.
    pub async fn relate(
        &self,
        from_id: &str,
        relation: &str,
        to_id: &str,
    ) -> Result<(), MemoryError> {
        use crate::models::{Edge, EdgeOrigin};
        let edge = Edge {
            in_id: from_id.to_string(),
            relation: relation.to_string(),
            out_id: to_id.to_string(),
            origin: EdgeOrigin::Inferred,
            strength: 1.0,
            confidence: 0.8,
            provenance: crate::models::Provenance::manual(),
            t_valid: crate::shared::temporal::now(),
            t_ingested: crate::shared::temporal::now(),
            t_invalid: None,
            t_invalid_ingested: None,
        };
        crate::memory::episode::store_edge(
            &crate::memory::capabilities::deps::ExtractDeps::from(self),
            &edge,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Introduction-chain traversal
// ---------------------------------------------------------------------------

/// Builds an introduction-chain path from a BFS next-hop map.
///
/// Returns the entity-id path from `start_id` back toward `target_id`,
/// following the discovered predecessor links. Returns `None` when the chain
/// breaks before reaching the target.
pub(crate) fn intro_chain_from_start(
    start_id: &str,
    target_id: &str,
    next_hop: &HashMap<String, String>,
) -> Option<Vec<String>> {
    let mut path = vec![start_id.to_string()];
    let mut current = start_id;

    while let Some(next) = next_hop.get(current) {
        path.push(next.clone());
        if next == target_id {
            return Some(path);
        }
        current = next;
    }

    None
}

/// Finds the best introduction chain to an entity named `target_name` by
/// walking `GraphDirection::Incoming` edges inward from the target and
/// returning the smallest discovered chain to any starting entity.
///
/// Free function (not a method on `MemoryService`) so later stages
/// can supply any context that exposes an `KnowledgeGraphStore`, without dragging a
/// `MemoryService` construction into the call.
pub(crate) async fn find_intro_chain(
    ctx: &impl GraphContext,
    target_name: &str,
    max_hops: i32,
    as_of: Option<DateTime<Utc>>,
) -> Result<Vec<String>, MemoryError> {
    let target_id = find_entity_id_by_name(ctx, target_name).await?;
    let Some(target_id) = target_id else {
        return Ok(vec![]);
    };

    let cutoff = as_of.unwrap_or_else(crate::service::now);
    let cutoff_iso = normalize_dt(cutoff);

    let mut frontier = vec![target_id.clone()];
    let mut visited = HashSet::from([target_id.clone()]);
    let mut next_hop: HashMap<String, String> = HashMap::new();
    let mut discovered_nodes = HashSet::new();
    let mut nodes_with_predecessors = HashSet::new();

    for _ in 0..max_hops {
        let mut next_frontier = Vec::new();

        for node_id in &frontier {
            for record in ctx
                .knowledge_graph_store()
                .select_edge_neighbors(node_id, &cutoff_iso, GraphDirection::Incoming)
                .await?
            {
                if let Value::Object(map) = record
                    && let (Some(in_id), Some(out_id)) = (
                        map.get("in").and_then(string_from_value),
                        map.get("out").and_then(string_from_value),
                    )
                    && visited.insert(in_id.clone())
                {
                    next_hop.insert(in_id.clone(), out_id);
                    discovered_nodes.insert(in_id.clone());
                    nodes_with_predecessors.insert(node_id.clone());
                    next_frontier.push(in_id);
                }
            }
        }

        if next_frontier.is_empty() {
            break;
        }

        next_frontier.sort();
        next_frontier.dedup();
        frontier = next_frontier;
    }

    let mut candidate_paths = discovered_nodes
        .into_iter()
        .filter(|node_id| !nodes_with_predecessors.contains(node_id))
        .filter_map(|start_id| intro_chain_from_start(&start_id, &target_id, &next_hop))
        .collect::<Vec<_>>();

    candidate_paths
        .sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));

    let Some(best_path) = candidate_paths.into_iter().next() else {
        return Ok(vec![]);
    };

    Ok(best_path)
}

/// Resolves an entity name to its `entity_id` within the store's bound
/// Active Namespace. The indexed lookup is preferred, with a table scan as a
/// compatibility fallback when the lookup record is unavailable.
async fn find_entity_id_by_name(
    ctx: &impl GraphContext,
    target_name: &str,
) -> Result<Option<String>, MemoryError> {
    let normalized_name = crate::shared::search::normalize_text(target_name);

    // Prefer the indexed lookup in the store's bound Active Namespace.
    if let Some(record) = ctx
        .knowledge_graph_store()
        .select_entity_lookup(&normalized_name)
        .await?
        .and_then(|value| value.as_object().cloned())
    {
        return Ok(record
            .get("entity_id")
            .and_then(string_from_value)
            .or_else(|| record.get("id").and_then(string_from_value)));
    }

    for record in ctx.knowledge_graph_store().select_entities().await? {
        let Some(map) = record.as_object() else {
            continue;
        };
        let entity_name = map
            .get("canonical_name")
            .and_then(string_from_value)
            .or_else(|| map.get("name").and_then(string_from_value));
        let Some(name) = entity_name else {
            continue;
        };
        if crate::shared::search::normalize_text(&name) != normalized_name {
            continue;
        }
        return Ok(map
            .get("entity_id")
            .and_then(string_from_value)
            .or_else(|| map.get("id").and_then(string_from_value)));
    }

    Ok(None)
}

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

/// The graph reads are also reached during context assembly, which holds a
/// narrow dependency set rather than the container. This is the second
/// implementor of the same read contract.
impl GraphContext for crate::memory::retrieval_deps::AssembleContextDeps {
    fn knowledge_graph_store(&self) -> KnowledgeGraphStore {
        self.graph_store.clone()
    }

    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
        let id1 = service
            .resolve_entity("person", "Alice Smith")
            .await
            .expect("resolve person");
        let id2 = service
            .resolve_entity("person", "Alice Smith")
            .await
            .expect("resolve person again");
        assert_eq!(id1, id2);

        let id3 = service
            .resolve_entity("company", "Acme Corp")
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

        let from_id = service
            .resolve_entity("person", "Alice Relate")
            .await
            .expect("resolve alice");
        let to_id = service
            .resolve_entity("company", "Acme Relate")
            .await
            .expect("resolve acme");

        service
            .relate(&from_id, "works_at", &to_id)
            .await
            .expect("relate entities");
    }
}
