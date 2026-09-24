//! Admin CLI configuration, separate from HttpConfig.
//!
//! Reads only the env vars the admin commands share: the privileged control
//! registry, the browser keys, the public base URL and the enabled method set.
//! What a *command* additionally requires is enforced by the command —
//! `create`/`recover` need the local method, while removing it needs the
//! opposite — so the loader stays a snapshot of the deployment's environment
//! rather than a per-command policy.

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
    /// The operator allowlist, read for the one guard that depends on it: a
    /// deployment must not lose its local method while no operator identity is
    /// configured, because that is the lockout the guard exists to prevent.
    /// Empty when the variable is unset, which is the state the guard refuses.
    pub operator_identities: Vec<String>,
}

impl AdminCliConfig {
    /// Load configuration from environment variables.
    ///
    /// The requirement that the local method be enabled belongs to
    /// `create`/`recover` rather than to the loader: creating or recovering a
    /// local administrator in a deployment that authenticates browsers through
    /// an identity provider alone would write records nothing can use, and
    /// removing that method is the opposite case. A set that also enables `oidc`
    /// is fine for every command.
    pub fn from_env() -> Result<Self, MemoryError> {
        let auth_methods = resolve_auth_methods()?;
        let session_key = resolve_secret_slot("MEMORY_MCP_HTTP_SESSION_KEY")?;
        let csrf_key = resolve_secret_slot("MEMORY_MCP_HTTP_CSRF_KEY")?;
        let public_base_url = std::env::var("MEMORY_MCP_HTTP_PUBLIC_BASE_URL")
            .unwrap_or_else(|_| "https://localhost".into());
        let operator_identities = parse_csv_env("MEMORY_MCP_HTTP_OPERATOR_IDENTITIES");

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
            operator_identities,
        })
    }
}

/// One HMAC secret slot for the CLI: the same resolution the server uses
/// (explicit value, then `MEMORY_MCP_HTTP_SECRET_KEY`, then the missing-value
/// error), so a root-only deployment the server accepts can also run
/// `memory_mcp admin`. Root expansion exists where `hmac` is compiled
/// (the `streamable-http` profile); other profiles keep the strict
/// explicit-only contract.
fn resolve_secret_slot(key: &str) -> Result<[u8; 32], MemoryError> {
    let root_secret = crate::config::secrets::read_root_secret()?;
    let supplied = std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty());
    #[cfg(feature = "streamable-http")]
    {
        crate::config::secrets::resolve_key_slot(supplied, key, root_secret.as_deref(), || {
            parse_hex_32_env(key)
        })
    }
    #[cfg(not(feature = "streamable-http"))]
    {
        let _ = (root_secret, supplied);
        parse_hex_32_env(key)
    }
}

fn require_env(key: &str) -> Result<String, MemoryError> {
    std::env::var(key).map_err(|_| MemoryError::ConfigInvalid(format!("{key} is required")))
}

/// A comma-separated list, empty when unset. Mirrors the server's own reader so
/// the CLI and the server agree on what "configured" means.
fn parse_csv_env(key: &str) -> Vec<String> {
    std::env::var(key)
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_hex_32_env(key: &str) -> Result<[u8; 32], MemoryError> {
    let hex_str = require_env(key)?;
    let bytes = hex::decode(&hex_str)
        .map_err(|_| MemoryError::ConfigInvalid(format!("{key} must be 64-char hex")))?;
    bytes.try_into().map_err(|_| {
        MemoryError::ConfigInvalid(format!("{key} must be exactly 32 bytes (64 hex chars)"))
    })
}
