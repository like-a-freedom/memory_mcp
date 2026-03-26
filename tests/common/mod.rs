use std::sync::Arc;

use memory_mcp::service::MemoryService;
use memory_mcp::storage::{DbClient, SurrealDbClient};

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
