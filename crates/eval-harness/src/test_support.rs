use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use memory_mcp::models::{IngestRequest, Provenance};
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::service::{MemoryService, normalize_dt, normalize_text};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

static TEST_DB_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// Every eval service is bound to one explicit Active Namespace.
pub const ACTIVE_NAMESPACE: &str = "main";

fn next_test_db_name() -> String {
    let seq = TEST_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("eval_test_{seq}")
}

pub struct TestMemory {
    pub service: MemoryService,
    pub db_client: Arc<SurrealDbClient>,
}

impl TestMemory {
    pub async fn new() -> Self {
        let namespaces = vec![ACTIVE_NAMESPACE.to_string()];
        let db_name = next_test_db_name();
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
                .await
                .expect("connect in memory service"),
        );
        for namespace in &namespaces {
            db_client
                .apply_migrations(namespace)
                .await
                .expect("apply in-memory migrations");
        }

        let service = MemoryService::new(
            db_client.clone(),
            ACTIVE_NAMESPACE.to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service init");

        Self { service, db_client }
    }
}

pub async fn make_service() -> MemoryService {
    TestMemory::new().await.service
}

pub async fn make_service_with_client() -> (MemoryService, Arc<SurrealDbClient>) {
    let memory = TestMemory::new().await;
    (memory.service, memory.db_client)
}

/// Ingests one episode through the production path for benchmark probes.
/// The request shape every bench used to hand-roll lives here.
pub async fn ingest_probe(service: &MemoryService, source_id: &str, content: &str) -> String {
    IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "bench".into(),
            source_id: source_id.to_string(),
            content: content.to_string(),
            t_ref: Utc::now(),
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("probe ingest should succeed")
}

pub async fn seed_entity(
    db_client: &Arc<SurrealDbClient>,
    entity_id: &str,
    entity_type: &str,
    canonical_name: &str,
    aliases: &[String],
) {
    db_client
        .create(
            entity_id,
            json!({
                "entity_id": entity_id,
                "entity_type": entity_type,
                "canonical_name": canonical_name,
                "canonical_name_normalized": normalize_text(canonical_name),
                "aliases": aliases,
            }),
            ACTIVE_NAMESPACE,
        )
        .await
        .expect("seed entity should succeed");
}

pub async fn seed_community(
    db_client: &Arc<SurrealDbClient>,
    community_id: &str,
    member_entities: &[String],
    summary: &str,
    updated_at: DateTime<Utc>,
) {
    db_client
        .create(
            community_id,
            json!({
                "community_id": community_id,
                "member_entities": member_entities,
                "summary": summary,
                "updated_at": normalize_dt(updated_at),
            }),
            ACTIVE_NAMESPACE,
        )
        .await
        .expect("seed community should succeed");
}

pub async fn seed_fact_with_links(
    service: &MemoryService,
    content: &str,
    t_valid: DateTime<Utc>,
    entity_links: Vec<String>,
    source_id: Option<&str>,
) -> String {
    let normalized_source_id = source_id.filter(|s| !s.trim().is_empty());

    if normalized_source_id.is_none() {
        return service
            .add_fact(
                "note",
                content,
                content,
                "episode:seed",
                t_valid,
                0.9,
                entity_links,
                vec![],
                Provenance::agent_observation("episode:seed"),
            )
            .await
            .expect("seed fact should succeed");
    }

    let source_id = normalized_source_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("seed:{}", normalize_text(content)));

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "seed".to_string(),
            source_id,
            content: format!("seed source for {content}"),
            t_ref: t_valid,
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("seed project episode should succeed");

    service
        .add_fact(
            "note",
            content,
            content,
            &episode_id,
            t_valid,
            0.9,
            entity_links,
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .expect("seed project fact should succeed")
}
