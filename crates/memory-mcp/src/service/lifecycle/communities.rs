//! Periodic community recomputation background worker.
//!
//! Rebuilds the `community` table from the currently active edge graph using a
//! union-find pass over active edges gathered in paginated batches.

use std::collections::BTreeSet;

use chrono::Utc;
use serde_json::Value;
use serde_json::json;
use tokio::time::{self, Duration as TokioDuration};
use tokio_util::sync::CancellationToken;

use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::service::community::converge_communities_from_active_edges;
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

    let namespace = &service.active_namespace;
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
        &crate::storage::AppStoreClient::new(
            service.db_client.clone(),
            service.active_namespace.clone(),
        ),
        cutoff,
        batch_size,
    )
    .await?;
    let rebuilt = build_communities_from_active_edges(service, &edge_records).await?;
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

        service
            .app_store()
            .upsert_community(&community.community_id, payload)
            .await?;
    }

    let mut stale_deleted = 0;
    for stale in service.app_store().select_communities().await? {
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
            service.app_store().delete_record(&community_id).await?;
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
    cutoff: &str,
    batch_size: usize,
) -> Result<(Vec<Value>, usize), MemoryError> {
    let batch_size = batch_size.max(1);
    let mut edge_records = Vec::new();
    let mut batch_count = 0;
    let mut start = 0;

    loop {
        let batch = app_store
            .select_edges_filtered_page(cutoff, start, batch_size)
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
    edge_records: &[serde_json::Value],
) -> Result<Vec<RebuiltCommunity>, MemoryError> {
    let memberships = converge_communities_from_active_edges(edge_records);

    let mut rebuilt = Vec::new();
    for membership in memberships {
        let summary =
            super::super::episode::build_community_summary(service, &membership.member_entities)
                .await?;

        rebuilt.push(RebuiltCommunity {
            community_id: membership.community_id,
            member_entities: membership.member_entities,
            summary,
        });
    }

    rebuilt.sort_by(|left, right| left.community_id.cmp(&right.community_id));
    Ok(rebuilt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
        crate::storage::EpisodeStoreClient::new(db.clone(), "org")
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
                    "provenance": {},
                    "t_valid": "2026-01-01T00:00:00Z",
                    "t_ingested": "2026-01-01T00:00:00Z",
                }),
            )
            .await
            .expect("seed edge");
    }

    #[tokio::test]
    async fn collect_active_edge_records_pages_until_partial_batch() {
        let db = make_in_memory_db().await;
        seed_edge(&db, "edge:1", "entity:alice", "entity:bob").await;
        seed_edge(&db, "edge:2", "entity:bob", "entity:carol").await;
        seed_edge(&db, "edge:3", "entity:carol", "entity:dana").await;
        let app_store = crate::storage::AppStoreClient::new(db, "org");

        let (edges, batches) = collect_active_edge_records(&app_store, "2026-05-13T00:00:00Z", 2)
            .await
            .expect("paged edge scan should succeed");

        assert_eq!(edges.len(), 3);
        assert_eq!(batches, 2);
    }

    #[tokio::test]
    async fn collect_active_edge_records_returns_empty_when_first_page_is_empty() {
        let db = make_in_memory_db().await;
        let app_store = crate::storage::AppStoreClient::new(db, "org");

        let (edges, batches) = collect_active_edge_records(&app_store, "2026-05-13T00:00:00Z", 2)
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
        let app_store = crate::storage::AppStoreClient::new(db, "org");

        let (edges, batches) = collect_active_edge_records(&app_store, "2026-05-13T00:00:00Z", 2)
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
        let app_store = crate::storage::AppStoreClient::new(db, "org");

        let (edges, batches) = collect_active_edge_records(&app_store, "2026-05-13T00:00:00Z", 2)
            .await
            .expect("paged edge scan should succeed");
        let grouped = converge_communities_from_active_edges(&edges);

        assert_eq!(batches, 2);
        assert_eq!(grouped.len(), 1);
        assert_eq!(
            grouped[0].member_entities,
            vec![
                "entity:alice".to_string(),
                "entity:bob".to_string(),
                "entity:carol".to_string(),
            ]
        );
    }
}

#[derive(Debug, Clone)]
struct RebuiltCommunity {
    community_id: String,
    member_entities: Vec<String>,
    summary: String,
}
