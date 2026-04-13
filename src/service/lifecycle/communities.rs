//! Periodic community recomputation background worker.
//!
//! Rebuilds the `community` table from the currently active edge graph using a
//! union-find pass over active edges.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::Utc;
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};

use crate::service::{MemoryError, MemoryService};

/// Spawns the community recomputation background task.
pub fn spawn_community_worker(
    service: MemoryService,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(TokioDuration::from_secs(interval_secs));

        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::Value::String("lifecycle.communities.start".to_string()),
        );
        event.insert(
            "interval_secs".to_string(),
            serde_json::Value::Number(serde_json::Number::from(interval_secs)),
        );
        service.logger.log(event, crate::logging::LogLevel::Info);

        loop {
            interval.tick().await;
            match run_community_rebuild_pass(&service).await {
                Ok(count) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::Value::String("lifecycle.communities.complete".to_string()),
                    );
                    event.insert(
                        "communities_rebuilt".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(count)),
                    );
                    service.logger.log(event, crate::logging::LogLevel::Info);
                }
                Err(err) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::Value::String("lifecycle.communities.error".to_string()),
                    );
                    event.insert(
                        "error".to_string(),
                        serde_json::Value::String(err.to_string()),
                    );
                    service.logger.log(event, crate::logging::LogLevel::Warn);
                }
            }
        }
    })
}

/// Rebuilds the community table from all currently active edges.
///
/// Important: `select_edges_filtered` applies a hard 10K edge limit. If a
/// namespace exceeds that limit, community detection will be incomplete for
/// that pass; the storage layer emits a warning when the cap is hit.
pub async fn run_community_rebuild_pass(service: &MemoryService) -> Result<usize, MemoryError> {
    let cutoff = crate::service::normalize_dt(Utc::now());
    let updated_at = crate::service::normalize_dt(Utc::now());
    let mut rebuilt_total = 0;

    for namespace in &service.namespaces {
        service.logger.log(
            crate::service::log_event(
                "lifecycle.communities.namespace_start",
                json!({"namespace": namespace, "cutoff": cutoff}),
                json!({}),
                None,
                None,
                None,
            ),
            crate::logging::LogLevel::Debug,
        );
        rebuilt_total +=
            rebuild_namespace_communities(service, namespace, &cutoff, &updated_at).await?;
    }

    Ok(rebuilt_total)
}

async fn rebuild_namespace_communities(
    service: &MemoryService,
    namespace: &str,
    cutoff: &str,
    updated_at: &str,
) -> Result<usize, MemoryError> {
    let edge_records = service
        .db_client
        .select_edges_filtered(namespace, cutoff)
        .await?;
    let rebuilt = build_communities_from_active_edges(service, namespace, &edge_records).await?;
    let active_ids = rebuilt
        .iter()
        .map(|community| community.community_id.clone())
        .collect::<BTreeSet<_>>();

    for community in &rebuilt {
        let payload = json!({
            "community_id": community.community_id,
            "member_entities": community.member_entities,
            "summary": community.summary,
            "updated_at": updated_at,
        });

        if service
            .db_client
            .select_one(&community.community_id, namespace)
            .await?
            .is_some()
        {
            service
                .db_client
                .update(&community.community_id, payload, namespace)
                .await?;
        } else {
            service
                .db_client
                .create(&community.community_id, payload, namespace)
                .await?;
        }
    }

    let mut stale_deleted = 0;
    for stale in service
        .db_client
        .select_table("community", namespace)
        .await?
    {
        let Some(community_id) = stale
            .get("community_id")
            .and_then(super::super::episode::unwrap_record_string)
            .or_else(|| {
                stale
                    .get("id")
                    .and_then(super::super::episode::unwrap_record_string)
            })
        else {
            continue;
        };

        if !active_ids.contains(&community_id) {
            service
                .db_client
                .query(
                    "DELETE type::record($community_id);",
                    Some(json!({"community_id": community_id})),
                    namespace,
                )
                .await?;
            stale_deleted += 1;
        }
    }

    service.logger.log(
        crate::service::log_event(
            "lifecycle.communities.namespace_complete",
            json!({"namespace": namespace}),
            json!({
                "edge_count": edge_records.len(),
                "communities_rebuilt": rebuilt.len(),
                "stale_deleted": stale_deleted,
            }),
            None,
            None,
            None,
        ),
        crate::logging::LogLevel::Trace,
    );

    Ok(rebuilt.len())
}

async fn build_communities_from_active_edges(
    service: &MemoryService,
    namespace: &str,
    edge_records: &[serde_json::Value],
) -> Result<Vec<RebuiltCommunity>, MemoryError> {
    let mut union_find = UnionFind::default();
    let mut entity_nodes = BTreeSet::new();

    for (left, right) in edge_records.iter().filter_map(edge_endpoints_from_record) {
        union_find.union(&left, &right);
        if is_entity_id(&left) {
            entity_nodes.insert(left);
        }
        if is_entity_id(&right) {
            entity_nodes.insert(right);
        }
    }

    let mut grouped_entities = BTreeMap::<String, BTreeSet<String>>::new();
    for entity_id in entity_nodes {
        let root = union_find.find(&entity_id);
        grouped_entities.entry(root).or_default().insert(entity_id);
    }

    let mut rebuilt = Vec::new();
    for members in grouped_entities.into_values() {
        if members.len() < 2 {
            continue;
        }

        let member_entities = members.into_iter().collect::<Vec<_>>();
        let summary =
            super::super::episode::build_community_summary(service, namespace, &member_entities)
                .await?;
        let community_id = super::super::ids::deterministic_community_id(&member_entities);

        rebuilt.push(RebuiltCommunity {
            community_id,
            member_entities,
            summary,
        });
    }

    rebuilt.sort_by(|left, right| left.community_id.cmp(&right.community_id));
    Ok(rebuilt)
}

fn edge_endpoints_from_record(record: &serde_json::Value) -> Option<(String, String)> {
    let map = record.as_object()?;
    let left = map
        .get("in")
        .and_then(super::super::episode::unwrap_record_string)?;
    let right = map
        .get("out")
        .and_then(super::super::episode::unwrap_record_string)?;
    Some((left, right))
}

fn is_entity_id(record_id: &str) -> bool {
    record_id.starts_with("entity:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::episode;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // UnionFind tests (pure data structure, no DB needed)
    // -----------------------------------------------------------------------

    #[test]
    fn union_find_singleton_node_returns_itself() {
        let mut uf = UnionFind::default();
        let root = uf.find("entity:alice");
        assert_eq!(root, "entity:alice");
    }

    #[test]
    fn union_find_two_nodes_share_root_after_union() {
        let mut uf = UnionFind::default();
        uf.union("entity:alice", "entity:bob");
        let root_alice = uf.find("entity:alice");
        let root_bob = uf.find("entity:bob");
        assert_eq!(root_alice, root_bob);
    }

    #[test]
    fn union_find_transitive_connectivity() {
        let mut uf = UnionFind::default();
        uf.union("entity:alice", "entity:bob");
        uf.union("entity:bob", "entity:charlie");
        let root_alice = uf.find("entity:alice");
        let root_charlie = uf.find("entity:charlie");
        assert_eq!(root_alice, root_charlie);
    }

    #[test]
    fn union_find_disjoint_sets_remain_separate() {
        let mut uf = UnionFind::default();
        uf.union("entity:alice", "entity:bob");
        uf.union("entity:charlie", "entity:dave");
        let root_ab = uf.find("entity:alice");
        let root_cd = uf.find("entity:charlie");
        assert_ne!(root_ab, root_cd);
    }

    #[test]
    fn union_find_idempotent_union() {
        let mut uf = UnionFind::default();
        uf.union("entity:alice", "entity:bob");
        uf.union("entity:alice", "entity:bob"); // duplicate union
        let root_alice = uf.find("entity:alice");
        let root_bob = uf.find("entity:bob");
        assert_eq!(root_alice, root_bob);
    }

    #[test]
    fn union_find_merges_two_existing_communities() {
        let mut uf = UnionFind::default();
        uf.union("entity:alice", "entity:bob");
        uf.union("entity:charlie", "entity:dave");
        // Now merge the two communities
        uf.union("entity:bob", "entity:charlie");
        let root = uf.find("entity:alice");
        assert_eq!(root, uf.find("entity:bob"));
        assert_eq!(root, uf.find("entity:charlie"));
        assert_eq!(root, uf.find("entity:dave"));
    }

    // -----------------------------------------------------------------------
    // edge_endpoints_from_record tests
    // -----------------------------------------------------------------------

    #[test]
    fn edge_endpoints_extracts_in_and_out() {
        let record = json!({
            "in": "entity:alice",
            "out": "entity:bob",
            "relation": "knows",
        });
        let result = edge_endpoints_from_record(&record);
        assert_eq!(
            result,
            Some(("entity:alice".to_string(), "entity:bob".to_string()))
        );
    }

    #[test]
    fn edge_endpoints_returns_none_for_non_object() {
        let record = json!("not an object");
        assert!(edge_endpoints_from_record(&record).is_none());
    }

    #[test]
    fn edge_endpoints_returns_none_when_in_missing() {
        let record = json!({
            "out": "entity:bob",
        });
        assert!(edge_endpoints_from_record(&record).is_none());
    }

    #[test]
    fn edge_endpoints_returns_none_when_out_missing() {
        let record = json!({
            "in": "entity:alice",
        });
        assert!(edge_endpoints_from_record(&record).is_none());
    }

    #[test]
    fn edge_endpoints_handles_wrapped_record_strings() {
        let record = json!({
            "in": {"String": "entity:alice"},
            "out": {"String": "entity:bob"},
        });
        // unwrap_record_string handles {"String": ...} wrappers
        let left = record.get("in").and_then(episode::unwrap_record_string);
        let right = record.get("out").and_then(episode::unwrap_record_string);
        assert_eq!(left, Some("entity:alice".to_string()));
        assert_eq!(right, Some("entity:bob".to_string()));
    }

    // -----------------------------------------------------------------------
    // is_entity_id tests
    // -----------------------------------------------------------------------

    #[test]
    fn is_entity_id_true_for_entity_prefix() {
        assert!(is_entity_id("entity:alice"));
        assert!(is_entity_id("entity:project-123"));
    }

    #[test]
    fn is_entity_id_false_for_non_entity() {
        assert!(!is_entity_id("episode:abc"));
        assert!(!is_entity_id("fact:123"));
        assert!(!is_entity_id("community:xyz"));
        assert!(!is_entity_id(""));
        assert!(!is_entity_id("entity")); // no colon
    }
}

#[derive(Debug, Clone)]
struct RebuiltCommunity {
    community_id: String,
    member_entities: Vec<String>,
    summary: String,
}

#[derive(Debug, Default)]
struct UnionFind {
    parent: HashMap<String, String>,
    rank: HashMap<String, usize>,
}

impl UnionFind {
    fn find(&mut self, node: &str) -> String {
        let parent = self
            .parent
            .entry(node.to_string())
            .or_insert_with(|| node.to_string())
            .clone();
        self.rank.entry(node.to_string()).or_insert(0);

        if parent == node {
            return parent;
        }

        let root = self.find(&parent);
        self.parent.insert(node.to_string(), root.clone());
        root
    }

    fn union(&mut self, left: &str, right: &str) {
        let left_root = self.find(left);
        let right_root = self.find(right);

        if left_root == right_root {
            return;
        }

        let left_rank = *self.rank.get(&left_root).unwrap_or(&0);
        let right_rank = *self.rank.get(&right_root).unwrap_or(&0);

        if left_rank < right_rank {
            self.parent.insert(left_root, right_root);
        } else if left_rank > right_rank {
            self.parent.insert(right_root, left_root);
        } else {
            self.parent.insert(right_root.clone(), left_root.clone());
            self.rank.insert(left_root, left_rank + 1);
        }
    }
}
