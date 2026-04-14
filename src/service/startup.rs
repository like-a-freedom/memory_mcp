use crate::service::error::MemoryError;
use crate::storage::DbClient;
use std::sync::Arc;

/// Build a startup versions event payload used for diagnostic logging.
pub(crate) fn build_startup_versions_event(
    client_version: &str,
    server_version: Option<&str>,
) -> std::collections::HashMap<String, serde_json::Value> {
    let mut m = std::collections::HashMap::new();
    m.insert("op".to_string(), serde_json::json!("startup.versions"));
    m.insert(
        "client_version".to_string(),
        serde_json::json!(client_version),
    );
    if let Some(sv) = server_version {
        m.insert(
            "surrealdb_server_version".to_string(),
            serde_json::json!(sv),
        );
    }
    m
}

/// Apply startup migrations to all configured namespaces.
pub(crate) async fn apply_startup_migrations(
    db_client: &Arc<dyn DbClient>,
    namespaces: &[String],
) -> Result<(), MemoryError> {
    for namespace in namespaces {
        db_client.apply_migrations(namespace).await?;
    }
    Ok(())
}
