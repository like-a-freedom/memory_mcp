use std::sync::Arc;

use chrono::{DateTime, Utc};
use memory_mcp::models::IngestRequest;
use memory_mcp::service::MemoryService;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

#[allow(dead_code)]
pub async fn make_service() -> MemoryService {
    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];
    let db_client =
        SurrealDbClient::connect_in_memory_with_namespaces("memory_test", &namespaces, "warn")
            .await
            .expect("connect in memory service");
    for namespace in &namespaces {
        db_client
            .apply_migrations(namespace)
            .await
            .expect("apply in-memory migrations");
    }

    MemoryService::new(Arc::new(db_client), namespaces, "warn".to_string(), 50, 100)
        .expect("service init")
}

#[allow(dead_code)]
pub async fn make_service_with_client() -> (MemoryService, Arc<SurrealDbClient>) {
    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];
    let db_client = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces("memory_test", &namespaces, "warn")
            .await
            .expect("connect in memory service"),
    );
    for namespace in &namespaces {
        db_client
            .apply_migrations(namespace)
            .await
            .expect("apply in-memory migrations");
    }

    let service = MemoryService::new(db_client.clone(), namespaces, "warn".to_string(), 50, 100)
        .expect("service init");

    (service, db_client)
}

#[allow(dead_code)]
pub async fn ingest_episode(service: &MemoryService, source_id: &str, content: &str) -> String {
    let request = IngestRequest {
        source_type: "chat".to_string(),
        source_id: source_id.to_string(),
        content: content.to_string(),
        t_ref: "2026-03-01T10:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("static timestamp should parse"),
        scope: "personal".to_string(),
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };
    let episode_id = service
        .ingest(request, None)
        .await
        .expect("ingest should succeed");
    service
        .extract(&episode_id, None)
        .await
        .expect("extract should succeed");
    episode_id
}

#[allow(dead_code)]
pub async fn seed_fact_at(
    service: &MemoryService,
    scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
) -> String {
    service
        .add_fact(
            "note",
            content,
            content,
            "episode:seed",
            t_valid,
            scope,
            0.9,
            vec![],
            vec![],
            json!({"source_episode": "episode:seed"}),
        )
        .await
        .expect("seed fact should succeed")
}
