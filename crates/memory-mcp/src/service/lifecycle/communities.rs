//! Periodic community recomputation background worker.
//!
//! Rebuilds the `community` table from the currently active edge graph using a
//! union-find pass over active edges gathered in paginated batches.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::Utc;
use serde_json::Value;
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};
use tokio_util::sync::CancellationToken;

use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::service::service_context::ServiceContext;

/// Spawns the community recomputation background task.
///
/// The task runs until `shutdown` is cancelled, at which point it exits
/// cleanly after completing any in-flight pass.
pub fn spawn_community_worker(
    service: MemoryService,
    interval_secs: u64,
    shutdown: CancellationToken,
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
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {}
            }
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
pub async fn run_community_rebuild_pass(service: &MemoryService) -> Result<usize, MemoryError> {
    let ctx = service.build_context();
    run_community_rebuild_pass_inner(&ctx).await
}

async fn run_community_rebuild_pass_inner(service: &ServiceContext) -> Result<usize, MemoryError> {
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
    service: &ServiceContext,
    namespace: &str,
    cutoff: &str,
    updated_at: &str,
) -> Result<usize, MemoryError> {
    let batch_size = crate::storage::active_edge_scan_batch_size().max(1) as usize;
    rebuild_namespace_communities_with_batch_size(
        service, namespace, cutoff, updated_at, batch_size,
    )
    .await
}

async fn rebuild_namespace_communities_with_batch_size(
    service: &ServiceContext,
    namespace: &str,
    cutoff: &str,
    updated_at: &str,
    batch_size: usize,
) -> Result<usize, MemoryError> {
    let (edge_records, edge_scan_batches) = collect_active_edge_records(
        &crate::storage::AppStoreClient::new(service.db_client.clone()),
        namespace,
        cutoff,
        batch_size,
    )
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
                "edge_scan_batches": edge_scan_batches,
                "edge_scan_batch_size": batch_size,
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

async fn collect_active_edge_records(
    app_store: &crate::storage::AppStoreClient,
    namespace: &str,
    cutoff: &str,
    batch_size: usize,
) -> Result<(Vec<Value>, usize), MemoryError> {
    let batch_size = batch_size.max(1);
    let mut edge_records = Vec::new();
    let mut batch_count = 0;
    let mut start = 0;

    loop {
        let batch = app_store
            .select_edges_filtered_page(namespace, cutoff, start, batch_size)
            .await?;
        if batch.is_empty() {
            break;
        }

        batch_count += 1;
        let fetched = batch.len();
        start += fetched;
        edge_records.extend(batch);

        if fetched < batch_size {
            break;
        }
    }

    Ok((edge_records, batch_count))
}

async fn build_communities_from_active_edges(
    service: &ServiceContext,
    namespace: &str,
    edge_records: &[serde_json::Value],
) -> Result<Vec<RebuiltCommunity>, MemoryError> {
    let grouped_entities = group_entity_components_from_active_edges(edge_records);

    let mut rebuilt = Vec::new();
    for member_entities in grouped_entities {
        let summary =
            super::super::episode::build_community_summary(service, namespace, &member_entities)
                .await?;
        let community_id = crate::service::deterministic_community_id(&member_entities);

        rebuilt.push(RebuiltCommunity {
            community_id,
            member_entities,
            summary,
        });
    }

    rebuilt.sort_by(|left, right| left.community_id.cmp(&right.community_id));
    Ok(rebuilt)
}

fn group_entity_components_from_active_edges(
    edge_records: &[serde_json::Value],
) -> Vec<Vec<String>> {
    let mut union_find = UnionFind::default();
    let mut entity_nodes = BTreeSet::<String>::new();

    for record in edge_records {
        let Some((left, right)) = edge_endpoints_from_record(record) else {
            continue;
        };

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
        let root = union_find.find(entity_id.as_str());
        grouped_entities.entry(root).or_default().insert(entity_id);
    }

    grouped_entities
        .into_values()
        .filter(|members| members.len() >= 2)
        .map(|members| members.into_iter().collect::<Vec<_>>())
        .collect()
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
    use std::sync::Arc;

    use crate::service::episode;
    use crate::storage::{DbClient, SurrealDbClient};
    use serde_json::json;

    /// Connects an in-memory SurrealDB with migrations applied for `org`.
    async fn make_in_memory_db() -> Arc<SurrealDbClient> {
        let db = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                &format!(
                    "lifecycle_communities_test_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ),
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory db"),
        );
        db.apply_migrations("org").await.expect("apply migrations");
        db
    }

    /// Seeds an active edge record (visible at the test cutoff) via RELATE.
    async fn seed_edge(db: &Arc<SurrealDbClient>, edge_id: &str, from_id: &str, to_id: &str) {
        crate::storage::EpisodeStoreClient::new(db.clone())
            .relate_edge(
                edge_id,
                from_id,
                to_id,
                json!({
                    "edge_id": edge_id,
                    "in": from_id,
                    "relation": "linked",
                    "out": to_id,
                    "origin": "inferred",
                    "strength": 1.0,
                    "confidence": 0.8,
                    "t_valid": "2026-01-01T00:00:00Z",
                    "t_ingested": "2026-01-01T00:00:00Z",
                }),
                "org",
            )
            .await
            .expect("seed edge");
    }

    fn edge(from_id: &str, to_id: &str) -> Value {
        json!({"in": from_id, "out": to_id})
    }

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

    #[tokio::test]
    async fn collect_active_edge_records_pages_until_partial_batch() {
        let db = make_in_memory_db().await;
        seed_edge(&db, "edge:1", "entity:alice", "entity:bob").await;
        seed_edge(&db, "edge:2", "entity:bob", "entity:carol").await;
        seed_edge(&db, "edge:3", "entity:carol", "entity:dana").await;
        let app_store = crate::storage::AppStoreClient::new(db);

        let (edges, batches) =
            collect_active_edge_records(&app_store, "org", "2026-05-13T00:00:00Z", 2)
                .await
                .expect("paged edge scan should succeed");

        assert_eq!(edges.len(), 3);
        assert_eq!(batches, 2);
    }

    #[tokio::test]
    async fn collect_active_edge_records_returns_empty_when_first_page_is_empty() {
        let db = make_in_memory_db().await;
        let app_store = crate::storage::AppStoreClient::new(db);

        let (edges, batches) =
            collect_active_edge_records(&app_store, "org", "2026-05-13T00:00:00Z", 2)
                .await
                .expect("empty paged edge scan should succeed");

        assert!(edges.is_empty());
        assert_eq!(batches, 0);
    }

    #[tokio::test]
    async fn collect_active_edge_records_checks_trailing_empty_page_for_exact_multiple() {
        let db = make_in_memory_db().await;
        seed_edge(&db, "edge:1", "entity:alice", "entity:bob").await;
        seed_edge(&db, "edge:2", "entity:bob", "entity:carol").await;
        seed_edge(&db, "edge:3", "entity:carol", "entity:dana").await;
        seed_edge(&db, "edge:4", "entity:dana", "entity:erin").await;
        let app_store = crate::storage::AppStoreClient::new(db);

        let (edges, batches) =
            collect_active_edge_records(&app_store, "org", "2026-05-13T00:00:00Z", 2)
                .await
                .expect("exact-multiple paged edge scan should succeed");

        assert_eq!(edges.len(), 4);
        assert_eq!(batches, 2);
    }

    #[tokio::test]
    async fn batched_edge_scan_can_build_entity_communities_across_context_nodes() {
        let db = make_in_memory_db().await;
        seed_edge(&db, "edge:1", "entity:alice", "episode:shared").await;
        seed_edge(&db, "edge:2", "entity:bob", "episode:shared").await;
        seed_edge(&db, "edge:3", "entity:bob", "fact:joint").await;
        seed_edge(&db, "edge:4", "entity:carol", "fact:joint").await;
        let app_store = crate::storage::AppStoreClient::new(db);

        let (edges, batches) =
            collect_active_edge_records(&app_store, "org", "2026-05-13T00:00:00Z", 2)
                .await
                .expect("paged edge scan should succeed");
        let grouped = group_entity_components_from_active_edges(&edges);

        assert_eq!(batches, 2);
        assert_eq!(
            grouped,
            vec![vec![
                "entity:alice".to_string(),
                "entity:bob".to_string(),
                "entity:carol".to_string(),
            ]]
        );
    }

    #[test]
    fn group_entity_components_from_active_edges_ignores_single_entity_components() {
        let grouped = group_entity_components_from_active_edges(&[
            edge("entity:solo", "episode:orphan"),
            edge("fact:orphan", "episode:orphan"),
        ]);

        assert!(grouped.is_empty());
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
