//! Admin CLI configuration, separate from HttpConfig.
//!
//! Reads only the env vars needed for admin create/recover commands.

use crate::config::SurrealTargetConfig;
use crate::error::MemoryError;
use crate::http::config::{BrowserAuthMethod, resolve_auth_methods};

/// Configuration for the admin CLI commands.
pub struct AdminCliConfig {
    pub control_db: SurrealTargetConfig,
    pub session_key: [u8; 32],
    pub csrf_key: [u8; 32],
    pub public_base_url: String,
    /// The browser authentication methods this deployment enables, resolved
    /// through the same contract the server reads (ADR-0057).
    pub auth_methods: Vec<BrowserAuthMethod>,
}

impl AdminCliConfig {
    /// Load configuration from environment variables.
    ///
    /// Spec §6: the admin commands **require the local method**. Creating or
    /// recovering a local administrator in a deployment that authenticates
    /// browsers through an identity provider alone would write records nothing
    /// can use, so a method set without `local` fails before any connection is
    /// opened. A set that also enables `oidc` is fine — the command writes to
    /// the same durable policy the server reconciles, and refuses to narrow it.
    pub fn from_env() -> Result<Self, MemoryError> {
        let auth_methods = resolve_auth_methods()?;
        if !auth_methods.contains(&BrowserAuthMethod::Local) {
            return Err(MemoryError::ConfigInvalid(
                "admin commands require the 'local' browser authentication method \
                 (MEMORY_MCP_HTTP_AUTH_METHODS=local)"
                    .into(),
            ));
        }
        let session_key = parse_hex_32_env("MEMORY_MCP_HTTP_SESSION_KEY")?;
        let csrf_key = parse_hex_32_env("MEMORY_MCP_HTTP_CSRF_KEY")?;
        let public_base_url = std::env::var("MEMORY_MCP_HTTP_PUBLIC_BASE_URL")
            .unwrap_or_else(|_| "https://localhost".into());

        let control_db = SurrealTargetConfig {
            url: require_env("SURREALDB_CONTROL_URL")?,
            username: require_env("SURREALDB_CONTROL_USERNAME")?,
            password: require_env("SURREALDB_CONTROL_PASSWORD")?,
            database: require_env("SURREALDB_CONTROL_DB")?,
            namespace: require_env("SURREALDB_CONTROL_NAMESPACE")?,
        };

        Ok(Self {
            control_db,
            session_key,
            csrf_key,
            public_base_url,
            auth_methods,
        })
    }
}

fn require_env(key: &str) -> Result<String, MemoryError> {
    std::env::var(key).map_err(|_| MemoryError::ConfigInvalid(format!("{key} is required")))
}

fn parse_hex_32_env(key: &str) -> Result<[u8; 32], MemoryError> {
    let hex_str = require_env(key)?;
    let bytes = hex::decode(&hex_str)
        .map_err(|_| MemoryError::ConfigInvalid(format!("{key} must be 64-char hex")))?;
    bytes.try_into().map_err(|_| {
        MemoryError::ConfigInvalid(format!("{key} must be exactly 32 bytes (64 hex chars)"))
    })
}
