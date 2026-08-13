use memory_mcp::service::{MemoryError, MemoryService};
use memory_mcp::storage::{DbClient, SurrealDbClient};

pub async fn setup_embedded_service() -> Result<MemoryService, MemoryError> {
    let namespaces = vec!["org".to_string()];
    let db_client =
        SurrealDbClient::connect_in_memory_with_namespaces("embedded_test", &namespaces, "warn")
            .await?;
    for namespace in &namespaces {
        db_client.apply_migrations(namespace).await?;
    }

    let service = MemoryService::new(
        std::sync::Arc::new(db_client),
        "org".to_string(),
        "warn".to_string(),
        50,
        100,
    )?;
    Ok(service)
}
