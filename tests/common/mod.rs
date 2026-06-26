pub mod mock_db;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use memory_mcp::models::IngestRequest;
use memory_mcp::service::{MemoryService, normalize_dt, normalize_text};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

fn namespace_for_scope(scope: &str) -> &str {
    match scope {
        "personal" => "personal",
        "private" => "private",
        _ => "org",
    }
}

#[allow(dead_code)]
pub async fn make_service() -> MemoryService {
    make_service_with_query_logging(false).await
}

#[allow(dead_code)]
pub async fn make_service_with_client() -> (MemoryService, Arc<SurrealDbClient>) {
    make_service_with_client_and_query_logging(false).await
}

#[allow(dead_code)]
pub async fn make_service_with_query_logging(query_logging_enabled: bool) -> MemoryService {
    make_service_with_client_and_query_logging(query_logging_enabled)
        .await
        .0
}

#[allow(dead_code)]
pub async fn make_service_with_client_and_query_logging(
    query_logging_enabled: bool,
) -> (MemoryService, Arc<SurrealDbClient>) {
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
        .expect("service init")
        .with_query_logging_enabled(query_logging_enabled);

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
        project: None,
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };
    let episode_id = service
        .ingest(request, None)
        .await
        .expect("ingest should succeed");
    service
        .extract(&episode_id, None, None)
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
    seed_fact_with_links(service, scope, content, t_valid, Vec::new()).await
}

#[allow(dead_code)]
pub async fn seed_fact_with_links(
    service: &MemoryService,
    scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
    entity_links: Vec<String>,
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
            entity_links,
            vec![],
            json!({"source_episode": "episode:seed"}),
        )
        .await
        .expect("seed fact should succeed")
}

#[allow(dead_code)]
pub async fn seed_episode_backed_fact_with_source_id(
    service: &MemoryService,
    scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
    source_id: &str,
) -> String {
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "seed".to_string(),
                source_id: source_id.to_string(),
                content: content.to_string(),
                t_ref: t_valid,
                scope: scope.to_string(),
                project: None,
                t_ingested: Some(t_valid),
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("seed episode should succeed");

    let extracted = service
        .extract(&episode_id, None, None)
        .await
        .expect("seed extraction should succeed");
    let entity_links = extracted
        .entities
        .into_iter()
        .map(|entity| entity.entity_id)
        .collect::<Vec<_>>();

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
            json!({
                "source_episode": episode_id,
                "source_type": "seed",
                "source_id": source_id,
            }),
        )
        .await
        .expect("seed note fact should succeed")
}

#[allow(dead_code)]
pub async fn seed_fact_with_links_and_project(
    service: &MemoryService,
    scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
    entity_links: Vec<String>,
    project: Option<&str>,
    source_id: Option<&str>,
) -> String {
    let normalized_project = project.filter(|project| !project.trim().is_empty());
    let normalized_source_id = source_id.filter(|source_id| !source_id.trim().is_empty());

    if normalized_project.is_none() && normalized_source_id.is_none() {
        return seed_fact_with_links(service, scope, content, t_valid, entity_links).await;
    }

    let source_id = normalized_source_id.map(str::to_string).unwrap_or_else(|| {
        format!(
            "seed:{}:{}:{}",
            scope,
            normalized_project.unwrap_or("default"),
            normalize_text(content)
        )
    });

    let episode_id = service
        .ingest(
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
            json!({"source_episode": episode_id}),
        )
        .await
        .expect("seed project fact should succeed")
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
