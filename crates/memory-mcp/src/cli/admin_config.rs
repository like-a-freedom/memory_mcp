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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variable the loader reads, so each test starts from a known
    /// environment and restores it afterwards.
    const ENV_KEYS: &[&str] = &[
        "MEMORY_MCP_HTTP_AUTH_METHODS",
        "MEMORY_MCP_HTTP_SESSION_KEY",
        "MEMORY_MCP_HTTP_CSRF_KEY",
        "MEMORY_MCP_HTTP_PUBLIC_BASE_URL",
        "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES",
        "MEMORY_MCP_HTTP_SECRET_KEY",
        "SURREALDB_CONTROL_URL",
        "SURREALDB_CONTROL_USERNAME",
        "SURREALDB_CONTROL_PASSWORD",
        "SURREALDB_CONTROL_DB",
        "SURREALDB_CONTROL_NAMESPACE",
    ];

    /// A 32-byte key rendered as hex, the only form `parse_hex_32_env` accepts.
    const KEY_HEX: &str = "00000000000000000000000000000000000000000000000000000000000000ff";

    /// Run `body` with exactly `vars` set (and every other loader variable
    /// removed), restoring the ambient environment afterwards. The process
    /// environment is shared, so `config::env_lock` serializes these against
    /// every other env-reading test in the crate.
    fn with_env<R>(vars: &[(&str, &str)], body: impl FnOnce() -> R) -> R {
        let _guard = crate::config::env_lock()
            .lock()
            .expect("admin config env lock");
        let saved: Vec<(String, Option<String>)> = ENV_KEYS
            .iter()
            .map(|key| ((*key).to_string(), std::env::var(key).ok()))
            .collect();
        for key in ENV_KEYS {
            unsafe { std::env::remove_var(key) };
        }
        for (key, value) in vars {
            unsafe { std::env::set_var(key, value) };
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        for (key, value) in saved {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(&key, value),
                    None => std::env::remove_var(&key),
                }
            }
        }
        outcome.expect("admin config test body")
    }

    /// The five control-registry variables, which every successful load needs.
    fn control_db_vars() -> Vec<(&'static str, &'static str)> {
        vec![
            ("SURREALDB_CONTROL_URL", "mem://"),
            ("SURREALDB_CONTROL_USERNAME", "root"),
            ("SURREALDB_CONTROL_PASSWORD", "root"),
            ("SURREALDB_CONTROL_DB", "control"),
            ("SURREALDB_CONTROL_NAMESPACE", "control"),
        ]
    }

    /// A fully-valid environment, so each test can drop exactly one variable.
    fn valid_vars() -> Vec<(&'static str, &'static str)> {
        let mut vars = control_db_vars();
        vars.push(("MEMORY_MCP_HTTP_AUTH_METHODS", "local"));
        vars.push(("MEMORY_MCP_HTTP_SESSION_KEY", KEY_HEX));
        vars.push(("MEMORY_MCP_HTTP_CSRF_KEY", KEY_HEX));
        vars
    }

    /// `valid_vars()` with the variable named `omit` removed.
    fn vars_without(omit: &str) -> Vec<(&'static str, &'static str)> {
        valid_vars()
            .into_iter()
            .filter(|(key, _)| *key != omit)
            .collect()
    }

    /// `valid_vars()` with `key` set to `value`.
    fn vars_with(key: &'static str, value: &'static str) -> Vec<(&'static str, &'static str)> {
        let mut vars = valid_vars();
        vars.push((key, value));
        vars
    }

    #[test]
    fn loads_a_complete_configuration() {
        let observed = with_env(&valid_vars(), AdminCliConfig::from_env);

        assert!(observed.is_ok());
    }

    #[test]
    fn reads_the_control_database_url() {
        let observed = with_env(&valid_vars(), AdminCliConfig::from_env)
            .expect("config loads")
            .control_db
            .url;

        assert_eq!(observed, "mem://");
    }

    #[test]
    fn reads_the_control_database_namespace() {
        let observed = with_env(&valid_vars(), AdminCliConfig::from_env)
            .expect("config loads")
            .control_db
            .namespace;

        assert_eq!(observed, "control");
    }

    #[test]
    fn requires_the_control_database_url() {
        let observed = with_env(
            &vars_without("SURREALDB_CONTROL_URL"),
            AdminCliConfig::from_env,
        );

        assert!(observed.is_err(), "the control URL is mandatory");
    }

    #[test]
    fn requires_the_control_database_namespace() {
        let observed = with_env(
            &vars_without("SURREALDB_CONTROL_NAMESPACE"),
            AdminCliConfig::from_env,
        );

        assert!(observed.is_err(), "the control namespace is mandatory");
    }

    #[test]
    fn names_the_missing_variable_in_the_error() {
        let observed = with_env(
            &vars_without("SURREALDB_CONTROL_DB"),
            AdminCliConfig::from_env,
        );

        assert_eq!(
            observed
                .err()
                .expect("missing variable is an error")
                .to_string(),
            "config invalid: SURREALDB_CONTROL_DB is required"
        );
    }

    #[test]
    fn reads_the_session_key() {
        let observed = with_env(&valid_vars(), AdminCliConfig::from_env)
            .expect("config loads")
            .session_key;

        let mut expected = [0u8; 32];
        expected[31] = 0xff;
        assert_eq!(observed, expected);
    }

    #[test]
    fn rejects_a_session_key_that_is_not_32_bytes() {
        let observed = with_env(
            &vars_with("MEMORY_MCP_HTTP_SESSION_KEY", "abcd"),
            AdminCliConfig::from_env,
        );

        assert!(observed.is_err(), "a short key must be refused, not padded");
    }

    #[test]
    fn rejects_a_session_key_that_is_not_hex() {
        let observed = with_env(
            &vars_with("MEMORY_MCP_HTTP_SESSION_KEY", "zz"),
            AdminCliConfig::from_env,
        );

        assert!(observed.is_err(), "non-hex material must be refused");
    }

    #[test]
    fn defaults_the_public_base_url_to_localhost() {
        let observed = with_env(&valid_vars(), AdminCliConfig::from_env)
            .expect("config loads")
            .public_base_url;

        assert_eq!(observed, "https://localhost");
    }

    #[test]
    fn reads_an_explicit_public_base_url() {
        let observed = with_env(
            &vars_with("MEMORY_MCP_HTTP_PUBLIC_BASE_URL", "https://memory.example"),
            AdminCliConfig::from_env,
        )
        .expect("config loads")
        .public_base_url;

        assert_eq!(observed, "https://memory.example");
    }

    #[test]
    fn reads_the_enabled_auth_methods() {
        let observed = with_env(
            &vars_with("MEMORY_MCP_HTTP_AUTH_METHODS", "oidc,local"),
            AdminCliConfig::from_env,
        )
        .expect("config loads")
        .auth_methods;

        assert_eq!(observed.len(), 2, "both enabled methods must be loaded");
    }

    #[test]
    fn has_no_operator_identities_when_the_variable_is_unset() {
        let observed = with_env(&valid_vars(), AdminCliConfig::from_env)
            .expect("config loads")
            .operator_identities;

        assert!(
            observed.is_empty(),
            "an unset allowlist is the lockout case"
        );
    }

    #[test]
    fn reads_a_comma_separated_operator_allowlist() {
        let observed = with_env(
            &vars_with(
                "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES",
                "ops@example.com, sre@example.com",
            ),
            AdminCliConfig::from_env,
        )
        .expect("config loads")
        .operator_identities;

        assert_eq!(observed, vec!["ops@example.com", "sre@example.com"]);
    }

    #[test]
    fn drops_empty_entries_from_the_operator_allowlist() {
        let observed = with_env(
            &vars_with(
                "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES",
                "ops@example.com,,  ,sre@example.com",
            ),
            AdminCliConfig::from_env,
        )
        .expect("config loads")
        .operator_identities;

        assert_eq!(observed, vec!["ops@example.com", "sre@example.com"]);
    }

    #[test]
    fn derives_the_session_key_from_the_root_secret() {
        let mut vars = vars_without("MEMORY_MCP_HTTP_SESSION_KEY");
        vars.push((
            "MEMORY_MCP_HTTP_SECRET_KEY",
            "0123456789abcdef0123456789abcdef",
        ));

        let observed = with_env(&vars, AdminCliConfig::from_env);

        assert!(
            observed.is_ok(),
            "a root-only deployment the server accepts must also run the CLI"
        );
    }

    #[test]
    fn rejects_a_root_secret_below_the_strength_floor() {
        let observed = with_env(
            &vars_with("MEMORY_MCP_HTTP_SECRET_KEY", "too-short"),
            AdminCliConfig::from_env,
        );

        assert!(observed.is_err(), "a weak root must not be expanded");
    }
}
