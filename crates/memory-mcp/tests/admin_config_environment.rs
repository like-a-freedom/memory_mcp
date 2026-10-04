#![cfg(feature = "streamable-http")]

//! Environment-adapter integration coverage for the public admin CLI loader.

use std::sync::Mutex;

use memory_mcp::cli::admin_config::AdminCliConfig;
use memory_mcp::error::MemoryError;

const KEY_HEX: &str = "00000000000000000000000000000000000000000000000000000000000000ff";
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

static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

struct RestoreEnvironment(Vec<(String, Option<String>)>);

impl Drop for RestoreEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn with_environment(variables: &[(&str, &str)]) -> Result<AdminCliConfig, MemoryError> {
    let _lock = ENVIRONMENT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let saved = ENV_KEYS
        .iter()
        .map(|key| ((*key).to_string(), std::env::var(key).ok()))
        .collect();
    let _restore = RestoreEnvironment(saved);

    for key in ENV_KEYS {
        unsafe { std::env::remove_var(key) };
    }
    for (key, value) in variables {
        unsafe { std::env::set_var(key, value) };
    }

    AdminCliConfig::from_env()
}

fn valid_environment() -> Vec<(&'static str, &'static str)> {
    vec![
        ("SURREALDB_CONTROL_URL", "mem://"),
        ("SURREALDB_CONTROL_USERNAME", "root"),
        ("SURREALDB_CONTROL_PASSWORD", "root"),
        ("SURREALDB_CONTROL_DB", "control"),
        ("SURREALDB_CONTROL_NAMESPACE", "control"),
        ("MEMORY_MCP_HTTP_AUTH_METHODS", "local"),
        ("MEMORY_MCP_HTTP_SESSION_KEY", KEY_HEX),
        ("MEMORY_MCP_HTTP_CSRF_KEY", KEY_HEX),
    ]
}

#[test]
fn loads_and_normalizes_the_admin_configuration_from_environment() {
    let mut variables = valid_environment();
    variables.push(("MEMORY_MCP_HTTP_AUTH_METHODS", "oidc,local"));
    variables.push((
        "MEMORY_MCP_HTTP_PUBLIC_BASE_URL",
        "https://memory.example.test",
    ));
    variables.push((
        "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES",
        "ops@example.test,,  ,sre@example.test",
    ));

    let loaded = with_environment(&variables).expect("complete configuration loads");

    assert_eq!(loaded.control_db.url, "mem://");
    assert_eq!(loaded.control_db.namespace, "control");
    assert_eq!(loaded.public_base_url, "https://memory.example.test");
    assert_eq!(
        loaded.operator_identities,
        vec!["ops@example.test", "sre@example.test"]
    );
    assert_eq!(loaded.auth_methods.len(), 2);
    let mut expected_key = [0u8; 32];
    expected_key[31] = 0xff;
    assert_eq!(loaded.session_key, expected_key);
    assert_eq!(loaded.csrf_key, expected_key);
}

#[test]
fn defaults_optional_admin_environment_values() {
    let loaded = with_environment(&valid_environment()).expect("configuration loads");

    assert_eq!(loaded.public_base_url, "https://localhost");
    assert!(loaded.operator_identities.is_empty());
}

#[test]
fn refuses_a_missing_control_database_url() {
    let variables = valid_environment()
        .into_iter()
        .filter(|(key, _)| *key != "SURREALDB_CONTROL_URL")
        .collect::<Vec<_>>();

    let Err(error) = with_environment(&variables) else {
        panic!("control URL is mandatory");
    };

    assert_eq!(
        error.to_string(),
        "config invalid: SURREALDB_CONTROL_URL is required"
    );
}

#[test]
fn refuses_a_short_session_key() {
    let mut variables = valid_environment();
    variables.retain(|(key, _)| *key != "MEMORY_MCP_HTTP_SESSION_KEY");
    variables.push(("MEMORY_MCP_HTTP_SESSION_KEY", "abcd"));

    let Err(error) = with_environment(&variables) else {
        panic!("short key is refused");
    };

    assert!(error.to_string().contains("MEMORY_MCP_HTTP_SESSION_KEY"));
}

#[test]
fn refuses_non_hex_session_key_material() {
    let mut variables = valid_environment();
    variables.retain(|(key, _)| *key != "MEMORY_MCP_HTTP_SESSION_KEY");
    variables.push(("MEMORY_MCP_HTTP_SESSION_KEY", "zz"));

    let Err(error) = with_environment(&variables) else {
        panic!("non-hex material is refused");
    };

    assert!(error.to_string().contains("MEMORY_MCP_HTTP_SESSION_KEY"));
}

#[test]
fn accepts_a_root_secret_when_an_explicit_session_key_is_absent() {
    let mut variables = valid_environment();
    variables.retain(|(key, _)| *key != "MEMORY_MCP_HTTP_SESSION_KEY");
    variables.push((
        "MEMORY_MCP_HTTP_SECRET_KEY",
        "0123456789abcdef0123456789abcdef",
    ));

    let loaded = with_environment(&variables).expect("root secret can supply the session key");

    assert_eq!(loaded.session_key.len(), 32);
}

#[test]
fn refuses_a_root_secret_below_the_strength_floor() {
    let mut variables = valid_environment();
    variables.retain(|(key, _)| *key != "MEMORY_MCP_HTTP_SESSION_KEY");
    variables.push(("MEMORY_MCP_HTTP_SECRET_KEY", "too-short"));

    let Err(error) = with_environment(&variables) else {
        panic!("weak root secret is refused");
    };

    assert!(error.to_string().contains("secret"));
}
