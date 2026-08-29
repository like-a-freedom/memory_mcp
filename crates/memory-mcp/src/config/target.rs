//! Connection target shape shared by storage and HTTP config.

use serde::Deserialize;

/// Connection shape used by both the control and tenant databases.
/// Lives in `config` (not `http::config`) so the storage layer can
/// depend on it without depending on the HTTP profile.
#[derive(Debug, Clone, Deserialize)]
pub struct SurrealTargetConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    pub database: String,
    pub namespace: String,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl SurrealTargetConfig {
    pub fn default_for_test() -> Self {
        Self {
            url: "mem://".into(),
            username: "root".into(),
            password: "root".into(),
            database: "memory_test".into(),
            namespace: "test".into(),
        }
    }
}
