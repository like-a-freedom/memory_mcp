use std::sync::Arc;
use tempfile::TempDir;

use memory_mcp::config::SurrealConfig;
use memory_mcp::service::{MemoryError, MemoryService};
use memory_mcp::storage::SurrealDbClient;

pub async fn setup_embedded_service() -> Result<(TempDir, MemoryService), MemoryError> {
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let data_dir = tmp.path().to_str().unwrap().to_string();
    let config = SurrealConfig {
        db_name: "example".to_string(),
        url: None,
        namespaces: vec!["example".to_string()],
        username: "root".to_string(),
        password: "root".to_string(),
        log_level: "trace".to_string(),
        embedded: true,
        data_dir: Some(data_dir),
    };

    let default = config.namespaces[0].clone();
    let db_client = SurrealDbClient::connect(&config, &default).await?;
    db_client.apply_migrations(&default).await?;

    let service = MemoryService::new(
        Arc::new(db_client),
        config.namespaces.clone(),
        config.log_level.clone(),
        50,
        100,
    )?;
    Ok((tmp, service))
}
