use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::service::error::MemoryError;
use crate::storage::{DbClient, GraphDirection};

/// Budget limit for BFS node exploration (NFR-GRAPH-01).
const BUDGET_LIMIT: usize = 500;
/// Default maximum search depth for `find_path`.
const DEFAULT_MAX_DEPTH: usize = 4;
/// Maximum neighbor expansion depth for `expand_neighbors`.
const EXPAND_MAX_DEPTH: usize = 2;
/// Maximum number of neighbor results (NFR-GRAPH-02).
const EXPAND_NEIGHBOR_LIMIT: usize = 50;

/// Returns JSON fallback per §2.5 for APP-05.
#[must_use]
pub fn graph_fallback(result: &PathResult) -> serde_json::Value {
    serde_json::json!({
        "path": result.path.iter().map(|n| serde_json::json!({
            "node_id": n.node_id,
            "type": n.node_type,
            "edges": n.edges.iter().map(|e| serde_json::json!({
                "relation": e.relation,
                "target": e.target_id,
                "confidence": e.confidence,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "path_found": result.path_found,
        "reason_if_empty": result.reason_if_empty,
    })
}

/// APP-05 Graph Path Explorer.
///
/// Provides BFS-based shortest-path search and neighbor expansion over the
/// temporal knowledge graph.
pub struct GraphPathExplorer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathResult {
    pub path: Vec<PathNode>,
    pub path_found: bool,
    pub reason_if_empty: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathNode {
    pub node_id: String,
    pub node_type: String,
    pub label: String,
    pub edges: Vec<PathEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathEdge {
    pub edge_id: String,
    pub relation: String,
    pub target_id: String,
    pub confidence: f64,
    pub strength: f64,
    pub provenance: serde_json::Value,
    pub t_valid: DateTime<Utc>,
    pub t_invalid: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborResult {
    pub neighbors: Vec<NeighborNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborNode {
    pub node_id: String,
    pub node_type: String,
    pub label: String,
    pub relation: String,
    pub direction: String,
    pub confidence: f64,
}

/// BFS state for a single node in the search frontier.
struct BfsEntry {
    node_id: String,
    depth: usize,
    path: Vec<PathEdge>,
}

/// Extract a string field from a JSON map, handling SurrealDB record literal wrappers.
fn get_json_str(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    fn value_to_string(value: &Value) -> Option<String> {
        if let Some(value) = crate::storage::json_string(value) {
            return Some(value.to_string());
        }

        let object = value.as_object()?;
        let record_id = object.get("RecordId")?.as_object()?;
        let table = record_id
            .get("table")
            .and_then(crate::storage::json_string)
            .unwrap_or_default();
        let key = record_id
            .get("key")
            .and_then(crate::storage::json_string)
            .unwrap_or_default();

        if table.is_empty() || key.is_empty() {
            return None;
        }

        Some(format!("{table}:{key}"))
    }

    map.get(key).and_then(value_to_string)
}

/// Extract an f64 field from a JSON map.
fn get_json_f64(map: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key).and_then(crate::storage::json_f64)
}

/// Build a [`PathEdge`] from a raw edge record map.
fn edge_from_record(map: &serde_json::Map<String, Value>, target_id: String) -> Option<PathEdge> {
    let edge_id = get_json_str(map, "edge_id")
        .or_else(|| get_json_str(map, "id"))
        .unwrap_or_default();
    let relation = get_json_str(map, "relation").unwrap_or_default();
    let confidence = get_json_f64(map, "confidence").unwrap_or(0.0);
    let strength = get_json_f64(map, "strength").unwrap_or(0.0);
    let provenance = map.get("provenance").cloned().unwrap_or(Value::Null);
    let t_valid = get_json_str(map, "t_valid")
        .as_deref()
        .and_then(crate::service::parse_iso)
        .unwrap_or_else(crate::service::now);
    let t_invalid = get_json_str(map, "t_invalid")
        .as_deref()
        .and_then(crate::service::parse_iso);

    Some(PathEdge {
        edge_id,
        relation,
        target_id,
        confidence,
        strength,
        provenance,
        t_valid,
        t_invalid,
    })
}

/// Look up a node's type and label from the database.
async fn fetch_node_metadata(
    db: &dyn DbClient,
    namespace: &str,
    node_id: &str,
) -> (String, String) {
    #[allow(clippy::collapsible_if)]
    if let Ok(Some(record)) = db.select_one(node_id, namespace).await {
        if let Some(map) = record.as_object() {
            let node_type = get_json_str(map, "entity_type")
                .or_else(|| get_json_str(map, "fact_type"))
                .or_else(|| get_json_str(map, "node_type"))
                .unwrap_or_else(|| node_id.split(':').next().unwrap_or("unknown").to_string());
            let label = get_json_str(map, "canonical_name")
                .or_else(|| get_json_str(map, "name"))
                .or_else(|| get_json_str(map, "content"))
                .unwrap_or_else(|| node_id.to_string());
            return (node_type, label);
        }
    }
    let node_type = node_id.split(':').next().unwrap_or("unknown").to_string();
    (node_type, node_id.to_string())
}

impl GraphPathExplorer {
    /// Find the shortest path between two entities using BFS.
    ///
    /// Explores up to `BUDGET_LIMIT` nodes (NFR-GRAPH-01) up to `max_depth`
    /// hops. Returns the path on success, or a reason code on failure:
    /// `"no_path"` or `"depth_limit_exceeded"` (FR-GRAPH-05).
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError`] on database failures.
    pub async fn find_path(
        from_entity_id: &str,
        to_entity_id: &str,
        scope: &str,
        as_of: DateTime<Utc>,
        max_depth: Option<usize>,
        db: &Arc<dyn DbClient>,
    ) -> Result<PathResult, MemoryError> {
        let depth_limit = max_depth.unwrap_or(DEFAULT_MAX_DEPTH);
        let cutoff = crate::service::normalize_dt(as_of);

        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<BfsEntry> = VecDeque::new();
        let mut nodes_explored: usize = 0;

        visited.insert(from_entity_id.to_string());
        queue.push_back(BfsEntry {
            node_id: from_entity_id.to_string(),
            depth: 0,
            path: Vec::new(),
        });

        while let Some(entry) = queue.pop_front() {
            nodes_explored += 1;
            if nodes_explored > BUDGET_LIMIT {
                return Ok(PathResult {
                    path: Vec::new(),
                    path_found: false,
                    reason_if_empty: Some("depth_limit_exceeded".to_string()),
                });
            }

            if entry.depth >= depth_limit {
                continue;
            }

            for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
                let edge_records = db
                    .select_edge_neighbors(scope, &entry.node_id, &cutoff, direction)
                    .await?;

                for record in &edge_records {
                    let Some(map) = record.as_object() else {
                        continue;
                    };

                    let neighbor_id = match direction {
                        GraphDirection::Incoming => get_json_str(map, "in").unwrap_or_default(),
                        GraphDirection::Outgoing => get_json_str(map, "out").unwrap_or_default(),
                    };

                    if neighbor_id.is_empty() || !visited.insert(neighbor_id.clone()) {
                        continue;
                    }

                    if neighbor_id == to_entity_id {
                        let mut final_path = entry.path.clone();
                        if let Some(edge) = edge_from_record(map, neighbor_id.clone()) {
                            final_path.push(edge);
                        }
                        let (node_type, label) =
                            fetch_node_metadata(db.as_ref(), scope, to_entity_id).await;
                        final_path.push(PathEdge {
                            edge_id: String::new(),
                            relation: String::new(),
                            target_id: String::new(),
                            confidence: 0.0,
                            strength: 0.0,
                            provenance: Value::Null,
                            t_valid: as_of,
                            t_invalid: None,
                        });
                        let mut path_nodes = build_path_nodes(
                            final_path,
                            from_entity_id,
                            to_entity_id,
                            node_type,
                            label,
                            db,
                            scope,
                        )
                        .await;
                        // Remove the sentinel edge appended as path terminator.
                        if let Some(last) = path_nodes.last_mut() {
                            last.edges.pop();
                        }
                        return Ok(PathResult {
                            path: path_nodes,
                            path_found: true,
                            reason_if_empty: None,
                        });
                    }

                    let mut next_path = entry.path.clone();
                    if let Some(edge) = edge_from_record(map, neighbor_id.clone()) {
                        next_path.push(edge);
                    }

                    queue.push_back(BfsEntry {
                        node_id: neighbor_id,
                        depth: entry.depth + 1,
                        path: next_path,
                    });
                }
            }
        }

        Ok(PathResult {
            path: Vec::new(),
            path_found: false,
            reason_if_empty: Some("no_path".to_string()),
        })
    }

    /// Expand neighbors of an entity up to `depth` hops.
    ///
    /// `depth` is clamped to a maximum of 2. Results are capped at 50
    /// neighbors (NFR-GRAPH-02).
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError`] on database failures.
    pub async fn expand_neighbors(
        entity_id: &str,
        scope: &str,
        direction: GraphDirection,
        depth: usize,
        as_of: DateTime<Utc>,
        db: &Arc<dyn DbClient>,
    ) -> Result<NeighborResult, MemoryError> {
        let depth_limit = depth.min(EXPAND_MAX_DEPTH);
        let cutoff = crate::service::normalize_dt(as_of);
        let mut neighbors = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        visited.insert(entity_id.to_string());
        queue.push_back((entity_id.to_string(), 0));

        while let Some((current, current_depth)) = queue.pop_front() {
            if neighbors.len() >= EXPAND_NEIGHBOR_LIMIT {
                break;
            }

            if current_depth >= depth_limit {
                continue;
            }

            let edge_records = db
                .select_edge_neighbors(scope, &current, &cutoff, direction)
                .await?;

            for record in &edge_records {
                if neighbors.len() >= EXPAND_NEIGHBOR_LIMIT {
                    break;
                }

                let Some(map) = record.as_object() else {
                    continue;
                };

                let neighbor_id = match direction {
                    GraphDirection::Incoming => get_json_str(map, "in").unwrap_or_default(),
                    GraphDirection::Outgoing => get_json_str(map, "out").unwrap_or_default(),
                };

                if neighbor_id.is_empty() || !visited.insert(neighbor_id.clone()) {
                    continue;
                }

                let (node_type, label) =
                    fetch_node_metadata(db.as_ref(), scope, &neighbor_id).await;
                let relation = get_json_str(map, "relation").unwrap_or_default();
                let confidence = get_json_f64(map, "confidence").unwrap_or(0.0);
                let dir_str = match direction {
                    GraphDirection::Incoming => "incoming",
                    GraphDirection::Outgoing => "outgoing",
                };

                neighbors.push(NeighborNode {
                    node_id: neighbor_id.clone(),
                    node_type,
                    label,
                    relation,
                    direction: dir_str.to_string(),
                    confidence,
                });

                if current_depth + 1 < depth_limit {
                    queue.push_back((neighbor_id, current_depth + 1));
                }
            }
        }

        Ok(NeighborResult { neighbors })
    }
}

/// Convert a sequence of edges into [`PathNode`]s.
///
/// Each edge in the sequence connects one node to the next. The `terminator_*`
/// parameters provide metadata for the final node (the path destination).
/// Also requires `source_id` to include the starting node of the path.
/// Fetches metadata from DB for source and intermediate nodes.
async fn build_path_nodes(
    edges: Vec<PathEdge>,
    source_id: &str,
    terminator_id: &str,
    terminator_type: String,
    terminator_label: String,
    db: &Arc<dyn DbClient>,
    scope: &str,
) -> Vec<PathNode> {
    let mut nodes = Vec::new();

    let (source_type, source_label) = fetch_node_metadata(db.as_ref(), scope, source_id).await;
    nodes.push(PathNode {
        node_id: source_id.to_string(),
        node_type: source_type,
        label: source_label,
        edges: if !edges.is_empty() {
            vec![edges[0].clone()]
        } else {
            Vec::new()
        },
    });

    for (i, edge) in edges.iter().enumerate() {
        let node_id = edge.target_id.clone();

        let (node_type, node_label) = fetch_node_metadata(db.as_ref(), scope, &node_id).await;

        let has_next_edge = i + 1 < edges.len();
        let node_edges = if has_next_edge {
            vec![edges[i + 1].clone()]
        } else {
            Vec::new()
        };

        nodes.push(PathNode {
            node_id,
            node_type,
            label: node_label,
            edges: node_edges,
        });
    }

    if terminator_id != edges.last().map(|e| e.target_id.as_str()).unwrap_or("") {
        nodes.push(PathNode {
            node_id: terminator_id.to_string(),
            node_type: terminator_type,
            label: terminator_label,
            edges: Vec::new(),
        });
    }

    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn graph_fallback_with_path_returns_valid_json() {
        let result = PathResult {
            path: vec![PathNode {
                node_id: "entity:a".to_string(),
                node_type: "person".to_string(),
                label: "Alice".to_string(),
                edges: vec![PathEdge {
                    edge_id: "edge:1".to_string(),
                    relation: "knows".to_string(),
                    target_id: "entity:b".to_string(),
                    confidence: 0.9,
                    strength: 0.8,
                    provenance: json!({"source": "test"}),
                    t_valid: Utc::now(),
                    t_invalid: None,
                }],
            }],
            path_found: true,
            reason_if_empty: None,
        };
        let val = graph_fallback(&result);
        assert!(val["path_found"].as_bool().unwrap());
        assert_eq!(val["path"].as_array().unwrap().len(), 1);
        assert_eq!(val["path"][0]["edges"][0]["relation"], "knows");
    }

    #[test]
    fn graph_fallback_empty_path_has_reason() {
        let result = PathResult {
            path: vec![],
            path_found: false,
            reason_if_empty: Some("no_path".to_string()),
        };
        let val = graph_fallback(&result);
        assert!(!val["path_found"].as_bool().unwrap());
        assert_eq!(val["reason_if_empty"], "no_path");
    }

    #[test]
    fn path_edge_serializes() {
        let edge = PathEdge {
            edge_id: "e1".to_string(),
            relation: "related_to".to_string(),
            target_id: "entity:t".to_string(),
            confidence: 0.7,
            strength: 0.6,
            provenance: json!({}),
            t_valid: Utc::now(),
            t_invalid: None,
        };
        let val = serde_json::to_value(&edge).unwrap();
        assert_eq!(val["confidence"], 0.7);
    }

    #[test]
    fn neighbor_result_serializes() {
        let result = NeighborResult {
            neighbors: vec![NeighborNode {
                node_id: "entity:x".to_string(),
                node_type: "org".to_string(),
                label: "X Corp".to_string(),
                relation: "works_at".to_string(),
                direction: "outgoing".to_string(),
                confidence: 0.95,
            }],
        };
        let val = serde_json::to_value(&result).unwrap();
        assert_eq!(val["neighbors"].as_array().unwrap().len(), 1);
        assert_eq!(val["neighbors"][0]["direction"], "outgoing");
    }

    #[test]
    fn get_json_str_unwraps_record_ids() {
        let map = serde_json::Map::from_iter([(
            "out".to_string(),
            json!({
                "RecordId": {
                    "table": "entity",
                    "key": "bob"
                }
            }),
        )]);

        assert_eq!(get_json_str(&map, "out").as_deref(), Some("entity:bob"));
    }
}
