use memory_mcp::service::{MemoryError, MemoryService};
use memory_mcp::storage::{DbClient, SurrealDbClient};

pub async fn setup_embedded_service() -> Result<MemoryService, MemoryError> {
    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];
    let db_client =
        SurrealDbClient::connect_in_memory_with_namespaces_and_dimension(
            "embedded_test",
            &namespaces,
            "warn",
            4,
        )
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
