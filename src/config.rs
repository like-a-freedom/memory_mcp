use std::env;

use crate::service::MemoryError;

#[derive(Debug, Clone)]
pub struct SurrealConfig {
    pub db_name: String,
    /// URL is optional when running embedded RocksDB
    pub url: Option<String>,
    pub namespaces: Vec<String>,
    pub username: String,
    pub password: String,
    pub log_level: String,
    /// If true, use embedded local RocksDB engine (persistent)
    pub embedded: bool,
    /// Optional path to RocksDB data directory (defaults to ./data/surrealdb)
    pub data_dir: Option<String>,
}

impl SurrealConfig {
    pub fn from_env() -> Result<Self, MemoryError> {
        let db_name = env::var("SURREALDB_DB_NAME")
            .map_err(|_| MemoryError::ConfigMissing("SURREALDB_DB_NAME".to_string()))?;
        let embedded = env::var("SURREALDB_EMBEDDED")
            .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        let url = if embedded {
            env::var("SURREALDB_URL").ok()
        } else {
            Some(
                env::var("SURREALDB_URL")
                    .map_err(|_| MemoryError::ConfigMissing("SURREALDB_URL".to_string()))?,
            )
        };
        let namespaces_raw = env::var("SURREALDB_NAMESPACES")
            .map_err(|_| MemoryError::ConfigMissing("SURREALDB_NAMESPACES".to_string()))?;
        let namespaces = namespaces_raw
            .split(',')
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(String::from)
            .collect::<Vec<_>>();
        if namespaces.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "SURREALDB_NAMESPACES is empty".to_string(),
            ));
        }
        let username = env::var("SURREALDB_USERNAME")
            .map_err(|_| MemoryError::ConfigMissing("SURREALDB_USERNAME".to_string()))?;
        let password = env::var("SURREALDB_PASSWORD")
            .map_err(|_| MemoryError::ConfigMissing("SURREALDB_PASSWORD".to_string()))?;
        let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "warn".to_string());
        let data_dir = env::var("SURREALDB_DATA_DIR").ok();

        Ok(Self {
            db_name,
            url,
            namespaces,
            username,
            password,
            log_level,
            embedded,
            data_dir,
        })
    }
}
