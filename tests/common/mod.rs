use std::sync::Arc;

use memory_mcp::service::MemoryService;
use memory_mcp::storage::{DbClient, SurrealDbClient};

pub async fn make_service() -> MemoryService {
    let db_client = SurrealDbClient::connect_in_memory("memory_test", "org", "warn")
        .await
        .expect("connect in memory service");
    db_client
        .apply_migrations("org")
        .await
        .expect("apply in-memory migrations");

    MemoryService::new(
        Arc::new(db_client),
        vec![
            "org".to_string(),
            "personal".to_string(),
            "private".to_string(),
        ],
        "warn".to_string(),
        50,
        100,
    )
    .expect("service init")
}
