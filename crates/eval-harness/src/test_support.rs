use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use memory_mcp::models::{IngestRequest, Provenance};
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::service::{MemoryService, normalize_dt, normalize_text};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

static TEST_DB_COUNTER: AtomicUsize = AtomicUsize::new(1);

fn namespace_for_scope(scope: &str) -> &str {
    match scope {
        "personal" => "personal",
        "team" => "team",
        "private" | "private-domain" | "private_domain" => "private-domain",
        _ => "org",
    }
}

fn next_test_db_name() -> String {
    let seq = TEST_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("eval_test_{seq}")
}

pub struct TestMemory {
    pub service: MemoryService,
    #[allow(dead_code)]
    pub db_client: Arc<SurrealDbClient>,
}

impl TestMemory {
    pub async fn new() -> Self {
        let namespaces = vec![
            "org".to_string(),
            "personal".to_string(),
            "team".to_string(),
            "private-domain".to_string(),
        ];
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

        let service =
            MemoryService::new(db_client.clone(), namespaces, "warn".to_string(), 50, 100)
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

pub async fn seed_entity(
    db_client: &Arc<SurrealDbClient>,
    scope: &str,
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
            namespace_for_scope(scope),
        )
        .await
        .expect("seed entity should succeed");
}

pub async fn seed_community(
    db_client: &Arc<SurrealDbClient>,
    scope: &str,
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
            namespace_for_scope(scope),
        )
        .await
        .expect("seed community should succeed");
}

pub async fn seed_fact_with_links_and_project(
    service: &MemoryService,
    scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
    entity_links: Vec<String>,
    project: Option<&str>,
    source_id: Option<&str>,
) -> String {
    let normalized_project = project.filter(|p| !p.trim().is_empty());
    let normalized_source_id = source_id.filter(|s| !s.trim().is_empty());

    if normalized_project.is_none() && normalized_source_id.is_none() {
        return service
            .add_fact(
                "note",
                content,
                content,
                "episode:seed",
                t_valid,
                scope,
                0.9,
                entity_links,
                vec![],
                Provenance::agent_observation("episode:seed"),
            )
            .await
            .expect("seed fact should succeed");
    }

    let source_id = normalized_source_id.map(str::to_string).unwrap_or_else(|| {
        format!(
            "seed:{}:{}:{}",
            scope,
            normalized_project.unwrap_or("default"),
            normalize_text(content)
        )
    });

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "seed".to_string(),
            source_id,
            content: format!("seed source for {content}"),
            t_ref: t_valid,
            scope: scope.to_string(),
            project: normalized_project.map(str::to_string),
            t_ingested: None,
            visibility_scope: None,
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
            scope,
            0.9,
            entity_links,
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .expect("seed project fact should succeed")
}
