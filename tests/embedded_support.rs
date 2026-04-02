use chrono::{DateTime, Utc};
use memory_mcp::service::{AddFactRequest, MemoryError, MemoryService};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::Value;

#[allow(dead_code, clippy::too_many_arguments)]
pub async fn add_fact(
    service: &MemoryService,
    fact_type: &str,
    content: &str,
    quote: &str,
    source_episode: &str,
    t_valid: DateTime<Utc>,
    scope: &str,
    confidence: f64,
    entity_links: Vec<String>,
    policy_tags: Vec<String>,
    provenance: Value,
) -> Result<String, MemoryError> {
    MemoryService::add_fact(
        service,
        AddFactRequest {
            fact_type,
            content,
            quote,
            source_episode,
            t_valid,
            scope,
            confidence,
            entity_links,
            policy_tags,
            provenance,
        },
    )
    .await
}

pub async fn setup_embedded_service() -> Result<MemoryService, MemoryError> {
    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];
    let db_client =
        SurrealDbClient::connect_in_memory_with_namespaces("embedded_test", &namespaces, "warn")
            .await?;
    for namespace in &namespaces {
        db_client.apply_migrations(namespace).await?;
    }

    let service = MemoryService::new(
        std::sync::Arc::new(db_client),
        namespaces,
        "warn".to_string(),
        50,
        100,
    )?;
    Ok(service)
}
