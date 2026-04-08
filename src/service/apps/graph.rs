use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::SurprisingConnection;
use crate::service::{MemoryError, MemoryService, normalize_dt, parse_iso};
use crate::storage::GraphDirection;

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
    service: &MemoryService,
    namespace: &str,
    cutoff: DateTime<Utc>,
    limit: i32,
) -> Result<Vec<HubEntity>, MemoryError> {
    let cutoff_iso = normalize_dt(cutoff);
    service.logger.log(
        crate::service::log_event(
            "graph.hubs.start",
            json!({"namespace": namespace, "cutoff": cutoff_iso, "limit": limit}),
            json!({}),
            None,
        ),
        LogLevel::Debug,
    );
    let entity_records = service.db_client.select_table("entity", namespace).await?;
    let mut hubs = Vec::new();

    for record in entity_records {
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
            for edge in service
                .db_client
                .select_edge_neighbors(namespace, &entity_id, &cutoff_iso, direction)
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
    service.logger.log(
        crate::service::log_event(
            "graph.hubs.done",
            json!({"namespace": namespace, "limit": limit}),
            json!({"count": hubs.len()}),
            None,
        ),
        LogLevel::Trace,
    );
    Ok(hubs)
}

pub(crate) async fn list_communities(
    service: &MemoryService,
    namespace: &str,
    cutoff: DateTime<Utc>,
    limit: i32,
) -> Result<Vec<GraphCommunity>, MemoryError> {
    service.logger.log(
        crate::service::log_event(
            "graph.communities.start",
            json!({"namespace": namespace, "limit": limit}),
            json!({}),
            None,
        ),
        LogLevel::Debug,
    );
    let mut communities = service
        .db_client
        .select_table("community", namespace)
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
    service.logger.log(
        crate::service::log_event(
            "graph.communities.done",
            json!({"namespace": namespace, "limit": limit}),
            json!({"count": communities.len()}),
            None,
        ),
        LogLevel::Trace,
    );
    Ok(communities)
}

pub(crate) async fn find_surprising_connections(
    service: &MemoryService,
    namespace: &str,
    source_entity: &str,
    max_depth: i32,
) -> Result<Vec<SurprisingConnection>, MemoryError> {
    if !is_entity_id(source_entity) || max_depth < 2 {
        service.logger.log(
            crate::service::log_event(
                "graph.surprising_connections.skipped",
                json!({"namespace": namespace, "source_entity": source_entity, "max_depth": max_depth}),
                json!({"reason": "invalid_source_or_depth"}),
                None,
            ),
            LogLevel::Trace,
        );
        return Ok(Vec::new());
    }

    service.logger.log(
        crate::service::log_event(
            "graph.surprising_connections.start",
            json!({"namespace": namespace, "source_entity": source_entity, "max_depth": max_depth}),
            json!({}),
            None,
        ),
        LogLevel::Debug,
    );

    let cutoff_iso = normalize_dt(crate::service::now());
    let communities = service
        .db_client
        .select_table("community", namespace)
        .await?
        .into_iter()
        .filter_map(|record| graph_community_from_value(&record))
        .collect::<Vec<_>>();
    let source_community_ids = community_ids_for_member(&communities, source_entity);
    let mut name_cache = HashMap::new();
    let source_entity_name =
        cached_entity_name(service, namespace, source_entity, &mut name_cache).await?;

    let mut visited = HashSet::from([source_entity.to_string()]);
    let mut frontier = VecDeque::from([(
        source_entity.to_string(),
        vec![source_entity.to_string()],
        0_usize,
    )]);
    let mut connections = BTreeMap::new();

    while let Some((current, path, depth)) = frontier.pop_front() {
        if depth >= max_depth as usize {
            continue;
        }

        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            for edge in service
                .db_client
                .select_edge_neighbors(namespace, &current, &cutoff_iso, direction)
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
                        cached_entity_name(service, namespace, &neighbor, &mut name_cache).await?;
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
    service.logger.log(
        crate::service::log_event(
            "graph.surprising_connections.done",
            json!({"namespace": namespace, "source_entity": source_entity}),
            json!({"count": surprising_connections.len()}),
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
    service: &MemoryService,
    namespace: &str,
    entity_id: &str,
    cache: &mut HashMap<String, String>,
) -> Result<String, MemoryError> {
    if let Some(name) = cache.get(entity_id) {
        return Ok(name.clone());
    }

    let name = service
        .db_client
        .select_one(entity_id, namespace)
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

fn unwrap_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        Some(array)
    } else if let Some(object) = value.as_object() {
        object.get("Array").and_then(Value::as_array)
    } else {
        None
    }
}
