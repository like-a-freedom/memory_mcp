use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::SurprisingConnection;
use crate::service::value_helpers::string_from_value;
use crate::service::{MemoryError, MemoryService, normalize_dt, parse_iso};
use crate::storage::{AppStore, GraphDirection};

/// Minimal context required by graph traversal functions.
/// Allows `ExplanationService` (and future services) to call graph
/// operations without depending on `MemoryService` directly.
pub(crate) trait GraphContext: Send + Sync {
    fn app_store(&self) -> &dyn AppStore;
    fn logger(&self) -> &StdoutLogger;
}

impl GraphContext for MemoryService {
    fn app_store(&self) -> &dyn AppStore {
        MemoryService::app_store(self)
    }
    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}

const HUB_CANDIDATE_SCAN_MULTIPLIER: usize = 12;
const MAX_HUB_CANDIDATE_SCAN: usize = 64;
const MAX_SURPRISING_CONNECTION_NODE_EXPANSIONS: usize = 64;
const MAX_SURPRISING_CONNECTION_NEIGHBOR_QUERIES: usize = 128;
const MAX_SURPRISING_CONNECTION_RESULTS: usize = 12;

/// Budget controls for graph traversal to prevent query explosion in different contexts.
#[derive(Debug, Clone, Copy)]
pub struct GraphTraversalBudget {
    pub max_hub_scan: usize,
    pub max_node_expansions: usize,
    pub max_neighbor_queries: usize,
    pub max_results: usize,
}

impl GraphTraversalBudget {
    /// Full budget — used by dedicated graph exploration (open_app, context views).
    pub const FULL: Self = Self {
        max_hub_scan: MAX_HUB_CANDIDATE_SCAN,
        max_node_expansions: MAX_SURPRISING_CONNECTION_NODE_EXPANSIONS,
        max_neighbor_queries: MAX_SURPRISING_CONNECTION_NEIGHBOR_QUERIES,
        max_results: MAX_SURPRISING_CONNECTION_RESULTS,
    };

    /// Reduced budget — used by inline `explain` calls to avoid per-item query explosion.
    pub const EXPLAIN: Self = Self {
        max_hub_scan: 24,
        max_node_expansions: 16,
        max_neighbor_queries: 32,
        max_results: 5,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HubEntity {
    pub entity_id: String,
    pub canonical_name: String,
    pub degree: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GraphCommunity {
    pub community_id: String,
    pub summary: String,
    pub member_entities: Vec<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub(crate) async fn find_hub_entities(
    ctx: &impl GraphContext,
    namespace: &str,
    cutoff: DateTime<Utc>,
    limit: i32,
    budget: GraphTraversalBudget,
) -> Result<Vec<HubEntity>, MemoryError> {
    let cutoff_iso = normalize_dt(cutoff);
    ctx.logger().log(
        crate::service::log_event(
            "graph.hubs.start",
            json!({"namespace": namespace, "cutoff": cutoff_iso, "limit": limit}),
            json!({}),
            None,
            None,
            None,
        ),
        LogLevel::Debug,
    );
    let entity_records = ctx.app_store().select_entities(namespace).await?;
    let mut hubs = Vec::new();
    let candidate_scan_limit =
        (limit.max(1) as usize * HUB_CANDIDATE_SCAN_MULTIPLIER).min(budget.max_hub_scan);

    for record in entity_records.into_iter().take(candidate_scan_limit) {
        let Some(map) = record.as_object() else {
            continue;
        };
        let Some(entity_id) = map
            .get("entity_id")
            .and_then(super::super::episode::unwrap_record_string)
            .or_else(|| {
                map.get("id")
                    .and_then(super::super::episode::unwrap_record_string)
            })
        else {
            continue;
        };

        let canonical_name = map
            .get("canonical_name")
            .and_then(super::super::episode::unwrap_record_string)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| entity_id.clone());

        let mut unique_edges = HashSet::new();
        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            for edge in ctx
                .app_store()
                .select_graph_neighbors(namespace, &entity_id, &cutoff_iso, direction)
                .await?
            {
                if let Some(edge_key) = edge_identity(&edge) {
                    unique_edges.insert(edge_key);
                }
            }
        }

        if unique_edges.is_empty() {
            continue;
        }

        hubs.push(HubEntity {
            entity_id,
            canonical_name,
            degree: unique_edges.len(),
        });
    }

    hubs.sort_by(|left, right| {
        right
            .degree
            .cmp(&left.degree)
            .then_with(|| left.canonical_name.cmp(&right.canonical_name))
            .then_with(|| left.entity_id.cmp(&right.entity_id))
    });
    hubs.truncate(limit.max(1) as usize);
    ctx.logger().log(
        crate::service::log_event(
            "graph.hubs.done",
            json!({"namespace": namespace, "limit": limit}),
            json!({"count": hubs.len()}),
            None,
            None,
            None,
        ),
        LogLevel::Trace,
    );
    Ok(hubs)
}

pub(crate) async fn list_communities(
    ctx: &impl GraphContext,
    namespace: &str,
    cutoff: DateTime<Utc>,
    limit: i32,
) -> Result<Vec<GraphCommunity>, MemoryError> {
    ctx.logger().log(
        crate::service::log_event(
            "graph.communities.start",
            json!({"namespace": namespace, "limit": limit}),
            json!({}),
            None,
            None,
            None,
        ),
        LogLevel::Debug,
    );
    let mut communities = ctx
        .app_store()
        .select_communities(namespace)
        .await?
        .into_iter()
        .filter_map(|record| graph_community_from_value(&record))
        .filter(|community| {
            community
                .updated_at
                .is_none_or(|updated_at| updated_at <= cutoff)
        })
        .collect::<Vec<_>>();

    communities.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.community_id.cmp(&right.community_id))
    });
    communities.truncate(limit.max(1) as usize);
    ctx.logger().log(
        crate::service::log_event(
            "graph.communities.done",
            json!({"namespace": namespace, "limit": limit}),
            json!({"count": communities.len()}),
            None,
            None,
            None,
        ),
        LogLevel::Trace,
    );
    Ok(communities)
}

pub(crate) async fn find_surprising_connections(
    ctx: &impl GraphContext,
    namespace: &str,
    source_entity: &str,
    max_depth: i32,
    budget: GraphTraversalBudget,
) -> Result<Vec<SurprisingConnection>, MemoryError> {
    if !is_entity_id(source_entity) || max_depth < 2 {
        ctx.logger().log(
            crate::service::log_event(
                "graph.surprising_connections.skipped",
                json!({"namespace": namespace, "source_entity": source_entity, "max_depth": max_depth}),
                json!({"reason": "invalid_source_or_depth"}),
                None, None, None,
            ),
            LogLevel::Trace,
        );
        return Ok(Vec::new());
    }

    ctx.logger().log(
        crate::service::log_event(
            "graph.surprising_connections.start",
            json!({"namespace": namespace, "source_entity": source_entity, "max_depth": max_depth}),
            json!({}),
            None,
            None,
            None,
        ),
        LogLevel::Debug,
    );

    let cutoff_iso = normalize_dt(crate::service::now());
    let communities = ctx
        .app_store()
        .select_communities(namespace)
        .await?
        .into_iter()
        .filter_map(|record| graph_community_from_value(&record))
        .collect::<Vec<_>>();
    let source_community_ids = community_ids_for_member(&communities, source_entity);
    let mut name_cache = HashMap::new();
    let source_entity_name =
        cached_entity_name(ctx, namespace, source_entity, &mut name_cache).await?;

    let mut visited = HashSet::from([source_entity.to_string()]);
    let mut frontier = VecDeque::from([(
        source_entity.to_string(),
        vec![source_entity.to_string()],
        0_usize,
    )]);
    let mut connections = BTreeMap::new();
    let mut expanded_nodes = 0usize;
    let mut neighbor_queries = 0usize;

    while let Some((current, path, depth)) = frontier.pop_front() {
        if expanded_nodes >= budget.max_node_expansions
            || neighbor_queries >= budget.max_neighbor_queries
            || connections.len() >= budget.max_results
        {
            break;
        }

        expanded_nodes += 1;
        if depth >= max_depth as usize {
            continue;
        }

        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            if neighbor_queries >= budget.max_neighbor_queries
                || connections.len() >= budget.max_results
            {
                break;
            }

            neighbor_queries += 1;
            for edge in ctx
                .app_store()
                .select_graph_neighbors(namespace, &current, &cutoff_iso, direction)
                .await?
            {
                let Some(neighbor) = neighbor_node(&edge, direction, &current) else {
                    continue;
                };
                if !is_traversable_graph_node(&neighbor) {
                    continue;
                }

                let next_depth = depth + 1;
                let mut next_path = path.clone();
                next_path.push(neighbor.clone());

                if is_entity_id(&neighbor)
                    && neighbor != source_entity
                    && next_depth >= 2
                    && is_surprising_target(
                        &source_community_ids,
                        &community_ids_for_member(&communities, &neighbor),
                    )
                {
                    let target_entity_name =
                        cached_entity_name(ctx, namespace, &neighbor, &mut name_cache).await?;
                    connections
                        .entry(neighbor.clone())
                        .or_insert_with(|| SurprisingConnection {
                            source_entity_id: source_entity.to_string(),
                            source_entity_name: source_entity_name.clone(),
                            target_entity_id: neighbor.clone(),
                            target_entity_name,
                            hop_count: next_depth,
                            path: next_path.clone(),
                        });
                    if connections.len() >= budget.max_results {
                        break;
                    }
                }
                if !is_traversable_graph_node(&neighbor) {
                    continue;
                }

                if visited.insert(neighbor.clone()) && next_depth < max_depth as usize {
                    frontier.push_back((neighbor, next_path, next_depth));
                }
            }
        }
    }

    let mut surprising_connections = connections.into_values().collect::<Vec<_>>();
    surprising_connections.sort_by(|left, right| {
        left.hop_count
            .cmp(&right.hop_count)
            .then_with(|| left.target_entity_name.cmp(&right.target_entity_name))
            .then_with(|| left.target_entity_id.cmp(&right.target_entity_id))
    });
    ctx.logger().log(
        crate::service::log_event(
            "graph.surprising_connections.done",
            json!({"namespace": namespace, "source_entity": source_entity}),
            json!({"count": surprising_connections.len()}),
            None,
            None,
            None,
        ),
        LogLevel::Trace,
    );
    Ok(surprising_connections)
}

fn edge_identity(record: &Value) -> Option<String> {
    let map = record.as_object()?;

    map.get("edge_id")
        .and_then(super::super::episode::unwrap_record_string)
        .or_else(|| {
            let in_id = map
                .get("in")
                .and_then(super::super::episode::unwrap_record_string)?;
            let relation = map
                .get("relation")
                .and_then(super::super::episode::unwrap_record_string)?;
            let out_id = map
                .get("out")
                .and_then(super::super::episode::unwrap_record_string)?;
            Some(format!("{in_id}:{relation}:{out_id}"))
        })
}

fn neighbor_node(record: &Value, direction: GraphDirection, current: &str) -> Option<String> {
    let map = record.as_object()?;
    let in_id = map
        .get("in")
        .and_then(super::super::episode::unwrap_record_string)?;
    let out_id = map
        .get("out")
        .and_then(super::super::episode::unwrap_record_string)?;

    match direction {
        GraphDirection::Incoming if out_id == current => Some(in_id),
        GraphDirection::Outgoing if in_id == current => Some(out_id),
        _ => None,
    }
}

fn graph_community_from_value(value: &Value) -> Option<GraphCommunity> {
    let map = value.as_object()?;
    let community_id = map
        .get("community_id")
        .and_then(super::super::episode::unwrap_record_string)
        .or_else(|| {
            map.get("id")
                .and_then(super::super::episode::unwrap_record_string)
        })?;
    let summary = map
        .get("summary")
        .and_then(super::super::episode::unwrap_record_string)
        .unwrap_or_default();
    let member_entities = map
        .get("member_entities")
        .and_then(unwrap_array)
        .map(|values| {
            values
                .iter()
                .filter_map(super::super::episode::unwrap_record_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let updated_at = map
        .get("updated_at")
        .and_then(super::super::episode::unwrap_record_string)
        .as_deref()
        .and_then(parse_iso);

    if summary.is_empty() || member_entities.is_empty() {
        return None;
    }

    Some(GraphCommunity {
        community_id,
        summary,
        member_entities,
        updated_at,
    })
}

fn community_ids_for_member(communities: &[GraphCommunity], entity_id: &str) -> HashSet<String> {
    communities
        .iter()
        .filter(|community| {
            community
                .member_entities
                .iter()
                .any(|member| member == entity_id)
        })
        .map(|community| community.community_id.clone())
        .collect()
}

fn is_surprising_target(
    source_communities: &HashSet<String>,
    target_communities: &HashSet<String>,
) -> bool {
    !target_communities.is_empty()
        && (source_communities.is_empty() || source_communities.is_disjoint(target_communities))
}

async fn cached_entity_name(
    ctx: &impl GraphContext,
    namespace: &str,
    entity_id: &str,
    cache: &mut HashMap<String, String>,
) -> Result<String, MemoryError> {
    if let Some(name) = cache.get(entity_id) {
        return Ok(name.clone());
    }

    let name = ctx
        .app_store()
        .select_entity(entity_id, namespace)
        .await?
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|map| {
            map.get("canonical_name")
                .and_then(super::super::episode::unwrap_record_string)
                .filter(|candidate| !candidate.trim().is_empty())
                .or_else(|| {
                    map.get("entity_id")
                        .and_then(super::super::episode::unwrap_record_string)
                        .or_else(|| {
                            map.get("id")
                                .and_then(super::super::episode::unwrap_record_string)
                        })
                })
        })
        .unwrap_or_else(|| entity_id.to_string());

    cache.insert(entity_id.to_string(), name.clone());
    Ok(name)
}

fn is_entity_id(record_id: &str) -> bool {
    record_id.starts_with("entity:")
}

fn is_traversable_graph_node(record_id: &str) -> bool {
    is_entity_id(record_id) || record_id.starts_with("episode:") || record_id.starts_with("fact:")
}

// ---------------------------------------------------------------------------
// Introduction-chain traversal (moved from service/core.rs, ADR-0024 step 1)
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
/// Free function (not a method on `MemoryService`) so later stages of ADR-0024
/// can supply any context that exposes an `AppStore`, without dragging a
/// `MemoryService` construction into the call.
pub(crate) async fn find_intro_chain(
    ctx: &impl GraphContext,
    namespaces: &[String],
    default_namespace: &str,
    target_name: &str,
    max_hops: i32,
    as_of: Option<DateTime<Utc>>,
) -> Result<Vec<String>, MemoryError> {
    let target_id = find_entity_id_by_name(ctx, default_namespace, namespaces, target_name).await?;
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
            for namespace in namespaces {
                for record in ctx
                    .app_store()
                    .select_graph_neighbors(
                        namespace,
                        node_id,
                        &cutoff_iso,
                        GraphDirection::Incoming,
                    )
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

/// Resolves an entity name to its `entity_id`. Tries the indexed lookup via
/// `select_entity_lookup` in the default namespace first, then falls back to
/// scanning entities across all namespaces. Returns `None` when the name is
/// unknown in every namespace.
async fn find_entity_id_by_name(
    ctx: &impl GraphContext,
    default_namespace: &str,
    namespaces: &[String],
    target_name: &str,
) -> Result<Option<String>, MemoryError> {
    let normalized_name = crate::service::normalize_text(target_name);

    // Prefer the indexed lookup in the default namespace.
    if let Some(record) = ctx
        .app_store()
        .select_entity_lookup(default_namespace, &normalized_name)
        .await?
        .and_then(|value| value.as_object().cloned())
    {
        return Ok(record
            .get("entity_id")
            .and_then(string_from_value)
            .or_else(|| record.get("id").and_then(string_from_value)));
    }

    for namespace in namespaces {
        for record in ctx.app_store().select_entities(namespace).await? {
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
            if crate::service::normalize_text(&name) != normalized_name {
                continue;
            }
            return Ok(map
                .get("entity_id")
                .and_then(string_from_value)
                .or_else(|| map.get("id").and_then(string_from_value)));
        }
    }

    Ok(None)
}

fn unwrap_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        Some(array)
    } else if let Some(object) = value.as_object() {
        object.get("Array").and_then(Value::as_array)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Graph traversal for app sessions (extracted from MCP handlers)
// ---------------------------------------------------------------------------

/// Result of a BFS path search between two entities.
#[derive(Debug, Clone)]
pub struct GraphPathSnapshot {
    pub found: bool,
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
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
    store: &dyn AppStore,
    namespace: &str,
    entity_id: &str,
) -> Result<Value, MemoryError> {
    let record = store.select_entity(entity_id, namespace).await?;
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
    store: &dyn AppStore,
    namespace: &str,
    from_entity_id: &str,
    to_entity_id: &str,
    cutoff: DateTime<Utc>,
    max_depth: i32,
) -> Result<GraphPathSnapshot, MemoryError> {
    if from_entity_id == to_entity_id {
        return Ok(GraphPathSnapshot {
            found: true,
            nodes: vec![entity_snapshot(store, namespace, from_entity_id).await?],
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
                .select_graph_neighbors(namespace, &current, &cutoff_iso, direction)
                .await?;
            for record in records {
                let Some(neighbor) = edge_neighbor(&record, direction) else {
                    continue;
                };
                let mut next_nodes = nodes.clone();
                next_nodes.push(neighbor.clone());
                let mut next_edges = edges.clone();
                next_edges.push(crate::service::value_helpers::normalized_edge_record(
                    &record,
                ));

                if neighbor == to_entity_id {
                    let mut snapshots = Vec::with_capacity(next_nodes.len());
                    for node_id in next_nodes {
                        snapshots.push(entity_snapshot(store, namespace, &node_id).await?);
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
            entity_snapshot(store, namespace, from_entity_id).await?,
            entity_snapshot(store, namespace, to_entity_id).await?,
        ],
        edges: Vec::new(),
    })
}

/// BFS neighbor expansion from a target entity.
pub async fn graph_neighbor_expansion(
    store: &dyn AppStore,
    namespace: &str,
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
                    .select_graph_neighbors(namespace, node_id, &cutoff_iso, *graph_direction)
                    .await?
                {
                    if let Some(neighbor) = edge_neighbor(&record, *graph_direction) {
                        edges.push(crate::service::value_helpers::normalized_edge_record(
                            &record,
                        ));
                        if visited.insert(neighbor.clone()) {
                            nodes.push(entity_snapshot(store, namespace, &neighbor).await?);
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
    store: &dyn AppStore,
    namespace: &str,
    from_entity_id: &str,
    to_entity_id: &str,
    cutoff: DateTime<Utc>,
    max_depth: i32,
) -> Result<Value, MemoryError> {
    let path = graph_path_snapshot(
        store,
        namespace,
        from_entity_id,
        to_entity_id,
        cutoff,
        max_depth,
    )
    .await?;
    let from_neighbors =
        graph_neighbor_expansion(store, namespace, from_entity_id, "both", 1, cutoff).await?;
    let to_neighbors =
        graph_neighbor_expansion(store, namespace, to_entity_id, "both", 1, cutoff).await?;

    Ok(json!({
        "target": {
            "from_entity_id": from_entity_id,
            "to_entity_id": to_entity_id,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use serde_json::Value;

    use crate::storage::DbClient;

    #[test]
    fn is_traversable_graph_node_accepts_valid_types() {
        assert!(is_traversable_graph_node("entity:abc"));
        assert!(is_traversable_graph_node("episode:123"));
        assert!(is_traversable_graph_node("fact:456"));
    }

    #[test]
    fn is_traversable_graph_node_rejects_other_types() {
        assert!(!is_traversable_graph_node("community:abc"));
        assert!(!is_traversable_graph_node("user:123"));
        assert!(!is_traversable_graph_node("random"));
    }

    #[test]
    fn unwrap_array_handles_plain_array() {
        let v = json!([1, 2, 3]);
        assert!(unwrap_array(&v).is_some());
    }

    #[test]
    fn unwrap_array_handles_wrapped_array() {
        let v = json!({"Array": [1, 2, 3]});
        assert!(unwrap_array(&v).is_some());
    }

    #[test]
    fn unwrap_array_returns_none_for_object() {
        let v = json!({"key": "value"});
        assert!(unwrap_array(&v).is_none());
    }

    #[test]
    fn unwrap_array_returns_none_for_scalar() {
        assert!(unwrap_array(&json!("string")).is_none());
        assert!(unwrap_array(&json!(42)).is_none());
    }

    #[test]
    fn graph_community_from_value_returns_none_for_empty() {
        let value = json!({});
        assert!(graph_community_from_value(&value).is_none());
    }

    // ------------------------------------------------------------------
    // GraphTraversalBudget
    // ------------------------------------------------------------------

    #[test]
    fn explain_budget_is_stricter_than_full() {
        const {
            assert!(
                GraphTraversalBudget::EXPLAIN.max_hub_scan
                    < GraphTraversalBudget::FULL.max_hub_scan
            );
            assert!(
                GraphTraversalBudget::EXPLAIN.max_node_expansions
                    < GraphTraversalBudget::FULL.max_node_expansions
            );
            assert!(
                GraphTraversalBudget::EXPLAIN.max_neighbor_queries
                    < GraphTraversalBudget::FULL.max_neighbor_queries
            );
            assert!(
                GraphTraversalBudget::EXPLAIN.max_results < GraphTraversalBudget::FULL.max_results
            );
        }
    }

    #[test]
    fn graph_traversal_budget_is_copy() {
        let a = GraphTraversalBudget::FULL;
        let b = a; // Copy, not move
        assert_eq!(a.max_hub_scan, b.max_hub_scan);
    }

    #[test]
    fn graph_traversal_budget_constants_are_nonzero() {
        for budget in [GraphTraversalBudget::FULL, GraphTraversalBudget::EXPLAIN] {
            assert!(budget.max_hub_scan > 0);
            assert!(budget.max_node_expansions > 0);
            assert!(budget.max_neighbor_queries > 0);
            assert!(budget.max_results > 0);
        }
    }

    #[tokio::test]
    async fn find_surprising_connections_honors_neighbor_query_budget() {
        #[derive(Default)]
        struct BudgetedGraphDbClient {
            neighbor_queries: AtomicUsize,
        }

        #[async_trait]
        impl DbClient for BudgetedGraphDbClient {
            async fn select_one(
                &self,
                record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(Some(json!({
                    "entity_id": record_id,
                    "canonical_name": record_id,
                })))
            }

            async fn select_table(
                &self,
                table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                if table == "community" {
                    return Ok((0..256)
                        .map(|idx| {
                            json!({
                                "community_id": format!("community:{idx}"),
                                "summary": format!("Community {idx}"),
                                "member_entities": [format!("entity:{idx}")],
                                "updated_at": "2026-04-15T00:00:00Z",
                            })
                        })
                        .collect());
                }

                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                node_id: &str,
                _cutoff: &str,
                direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                self.neighbor_queries.fetch_add(1, Ordering::Relaxed);

                if direction == GraphDirection::Incoming {
                    return Ok(vec![]);
                }

                let next_edge = if let Some(idx) = node_id.strip_prefix("entity:") {
                    let idx = idx.parse::<usize>().unwrap_or(0);
                    json!({
                        "in": format!("entity:{idx}"),
                        "out": format!("episode:{idx}"),
                        "relation": "linked",
                    })
                } else if let Some(idx) = node_id.strip_prefix("episode:") {
                    let idx = idx.parse::<usize>().unwrap_or(0);
                    json!({
                        "in": format!("episode:{idx}"),
                        "out": format!("entity:{}", idx + 1),
                        "relation": "linked",
                    })
                } else {
                    return Ok(vec![]);
                };

                Ok(vec![next_edge])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let db = Arc::new(BudgetedGraphDbClient::default());
        let service = crate::service::MemoryService::new(
            db.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let connections = find_surprising_connections(
            &service,
            "org",
            "entity:0",
            32,
            GraphTraversalBudget::FULL,
        )
        .await
        .expect("connections");

        assert!(
            db.neighbor_queries.load(Ordering::Relaxed)
                <= GraphTraversalBudget::FULL.max_neighbor_queries,
            "neighbor queries should stop at the configured traversal budget"
        );
        assert!(connections.len() <= GraphTraversalBudget::FULL.max_results);
    }
}
