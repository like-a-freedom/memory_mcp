use memory_mcp::service::{MemoryError, MemoryService};
use memory_mcp::storage::{DbClient, SurrealDbClient};

pub async fn setup_embedded_service() -> Result<MemoryService, MemoryError> {
    let db_client = SurrealDbClient::connect_in_memory("embedded_test", "org", "warn").await?;
    db_client.apply_migrations("org").await?;

    let service = MemoryService::new(
        std::sync::Arc::new(db_client),
        vec![
            "org".to_string(),
            "personal".to_string(),
            "private".to_string(),
        ],
        "warn".to_string(),
        50,
        100,
    )?;
    Ok(service)
}
