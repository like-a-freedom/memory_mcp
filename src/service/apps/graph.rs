use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::SurprisingConnection;
use crate::service::{MemoryError, MemoryService, normalize_dt, parse_iso};
use crate::storage::GraphDirection;

const HUB_CANDIDATE_SCAN_MULTIPLIER: usize = 12;
const MAX_HUB_CANDIDATE_SCAN: usize = 64;
const MAX_SURPRISING_CONNECTION_NODE_EXPANSIONS: usize = 64;
const MAX_SURPRISING_CONNECTION_NEIGHBOR_QUERIES: usize = 128;
const MAX_SURPRISING_CONNECTION_RESULTS: usize = 12;

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
            None,
            None,
        ),
        LogLevel::Debug,
    );
    let entity_records = service.db_client.select_table("entity", namespace).await?;
    let mut hubs = Vec::new();
    let candidate_scan_limit = hub_candidate_scan_limit(limit);

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
            None,
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
            None,
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
            None,
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
                None, None, None,
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
            None,
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
    let mut expanded_nodes = 0usize;
    let mut neighbor_queries = 0usize;

    while let Some((current, path, depth)) = frontier.pop_front() {
        if expanded_nodes >= MAX_SURPRISING_CONNECTION_NODE_EXPANSIONS
            || neighbor_queries >= MAX_SURPRISING_CONNECTION_NEIGHBOR_QUERIES
            || connections.len() >= MAX_SURPRISING_CONNECTION_RESULTS
        {
            break;
        }

        expanded_nodes += 1;
        if depth >= max_depth as usize {
            continue;
        }

        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            if neighbor_queries >= MAX_SURPRISING_CONNECTION_NEIGHBOR_QUERIES
                || connections.len() >= MAX_SURPRISING_CONNECTION_RESULTS
            {
                break;
            }

            neighbor_queries += 1;
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
                    if connections.len() >= MAX_SURPRISING_CONNECTION_RESULTS {
                        break;
                    }
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
            None,
            None,
        ),
        LogLevel::Trace,
    );
    Ok(surprising_connections)
}

fn hub_candidate_scan_limit(limit: i32) -> usize {
    (limit.max(1) as usize * HUB_CANDIDATE_SCAN_MULTIPLIER).min(MAX_HUB_CANDIDATE_SCAN)
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

    #[test]
    fn hub_candidate_scan_limit_caps_large_requests() {
        assert_eq!(hub_candidate_scan_limit(1), 12);
        assert_eq!(hub_candidate_scan_limit(5), 60);
        assert_eq!(hub_candidate_scan_limit(50), MAX_HUB_CANDIDATE_SCAN);
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

        let connections = find_surprising_connections(&service, "org", "entity:0", 32)
            .await
            .expect("connections");

        assert!(
            db.neighbor_queries.load(Ordering::Relaxed)
                <= MAX_SURPRISING_CONNECTION_NEIGHBOR_QUERIES,
            "neighbor queries should stop at the configured traversal budget"
        );
        assert!(connections.len() <= MAX_SURPRISING_CONNECTION_RESULTS);
    }
}
