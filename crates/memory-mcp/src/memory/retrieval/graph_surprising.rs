//! The surprising-connection scan.
use crate::error::MemoryError;
use crate::knowledge::community::is_entity_id;
use crate::logging::LogLevel;
use crate::memory::retrieval::graph_reads::{
    GraphCommunity, GraphContext, graph_community_from_value,
};
use crate::models::SurprisingConnection;
use crate::platform::log_event::log_event;
use crate::platform::traversal_budget::GraphTraversalBudget;
use crate::shared::temporal::normalize_dt;
use crate::storage::GraphDirection;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
pub(crate) async fn find_surprising_connections(
    ctx: &impl GraphContext,
    source_entity: &str,
    max_depth: i32,
    budget: GraphTraversalBudget,
) -> Result<Vec<SurprisingConnection>, MemoryError> {
    if !is_entity_id(source_entity) || max_depth < 2 {
        ctx.logger().log(
            log_event(
                "graph.surprising_connections.skipped",
                json!({"source_entity": source_entity, "max_depth": max_depth}),
                json!({"reason": "invalid_source_or_depth"}),
                None,
                None,
                None,
            ),
            LogLevel::Trace,
        );
        return Ok(Vec::new());
    }

    ctx.logger().log(
        log_event(
            "graph.surprising_connections.start",
            json!({"source_entity": source_entity, "max_depth": max_depth}),
            json!({}),
            None,
            None,
            None,
        ),
        LogLevel::Debug,
    );

    let cutoff_iso = normalize_dt(crate::shared::temporal::now());
    let communities = ctx
        .knowledge_graph_store()
        .select_communities()
        .await?
        .into_iter()
        .filter_map(|record| graph_community_from_value(&record))
        .collect::<Vec<_>>();
    let source_community_ids = community_ids_for_member(&communities, source_entity);
    let mut name_cache = HashMap::new();
    let source_entity_name = cached_entity_name(ctx, source_entity, &mut name_cache).await?;

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
                .knowledge_graph_store()
                .select_edge_neighbors(&current, &cutoff_iso, direction)
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
                        cached_entity_name(ctx, &neighbor, &mut name_cache).await?;
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
        log_event(
            "graph.surprising_connections.done",
            json!({"source_entity": source_entity}),
            json!({"count": surprising_connections.len()}),
            None,
            None,
            None,
        ),
        LogLevel::Trace,
    );
    Ok(surprising_connections)
}

fn neighbor_node(record: &Value, direction: GraphDirection, current: &str) -> Option<String> {
    let map = record.as_object()?;
    let in_id = map
        .get("in")
        .and_then(crate::storage::value_helpers::unwrap_record_string)?;
    let out_id = map
        .get("out")
        .and_then(crate::storage::value_helpers::unwrap_record_string)?;

    match direction {
        GraphDirection::Incoming if out_id == current => Some(in_id),
        GraphDirection::Outgoing if in_id == current => Some(out_id),
        _ => None,
    }
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
    entity_id: &str,
    cache: &mut HashMap<String, String>,
) -> Result<String, MemoryError> {
    if let Some(name) = cache.get(entity_id) {
        return Ok(name.clone());
    }

    let name = ctx
        .knowledge_graph_store()
        .select_entity(entity_id)
        .await?
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|map| {
            map.get("canonical_name")
                .and_then(crate::storage::value_helpers::unwrap_record_string)
                .filter(|candidate| !candidate.trim().is_empty())
                .or_else(|| {
                    map.get("entity_id")
                        .and_then(crate::storage::value_helpers::unwrap_record_string)
                        .or_else(|| {
                            map.get("id")
                                .and_then(crate::storage::value_helpers::unwrap_record_string)
                        })
                })
        })
        .unwrap_or_else(|| entity_id.to_string());

    cache.insert(entity_id.to_string(), name.clone());
    Ok(name)
}

pub(crate) fn is_traversable_graph_node(record_id: &str) -> bool {
    is_entity_id(record_id) || record_id.starts_with("episode:") || record_id.starts_with("fact:")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::client::DbClient;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
                table: crate::storage::table_scope::OwnedTable,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                if table.as_str() == "community" {
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

            #[allow(clippy::too_many_arguments)]
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
                sql: &str,
                vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                // The app store now runs graph-neighbor lookups through the core
                // `query` op; serve the deterministic chain of edges here.
                if sql.contains("FROM edge") {
                    self.neighbor_queries.fetch_add(1, Ordering::Relaxed);
                    if sql.contains("WHERE out =") {
                        return Ok(Value::Array(Vec::new()));
                    }
                    let node_id = vars
                        .and_then(|vars| vars["node_id"].as_str().map(str::to_string))
                        .unwrap_or_default();
                    let next_edge = if let Some(idx) = node_id.strip_prefix("entity:") {
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
                        return Ok(Value::Array(Vec::new()));
                    };
                    return Ok(Value::Array(vec![next_edge]));
                }
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let db = Arc::new(BudgetedGraphDbClient::default());
        let service = crate::service::MemoryService::new(
            db.clone(),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let connections =
            find_surprising_connections(&service, "entity:0", 32, GraphTraversalBudget::FULL)
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
