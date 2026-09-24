//! `HttpConfig` validator.
//!
//! The body is large because every startup-time invariant the
//! HTTP SaaS profile enforces lives here: env-gated feature flags,
//! positive numeric limits, allowlist shape, OIDC completeness,
//! storage target separation, and the stdio-only env rejection
//! rules documented in the operations runbooks.

use std::time::Duration;

use crate::error::MemoryError;

use super::types::{HttpConfig, SignupMode};

/// Whether a public base URL points at a development loopback host.
///
/// Local mode allows plain HTTP as a development convenience, but the
/// exception has to key off the URL's **host**: a substring test would accept
/// `http://evil.example/?localhost` and let a deployment run these Secure
/// cookies over public plain HTTP.
fn is_loopback_public_url(url: &str) -> bool {
    let authority = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    let Some(authority) = authority else {
        return false;
    };
    // Drop any path, query or fragment, then any port.
    let host_port = authority.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host_port
        .rsplit_once(':')
        .map_or(host_port, |(host, _port)| host);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

pub(super) fn validate(cfg: &HttpConfig) -> Result<(), MemoryError> {
    // Reject the test-only bootstrap env var unless the
    // `test-fixtures` feature is enabled (Task 5.8).
    // The literal is intentional: a production build
    // cannot accidentally gain the bootstrap impl by
    // name resolution.
    #[cfg(not(feature = "test-fixtures"))]
    if std::env::var("MEMORY_MCP_HTTP_TEST_BOOTSTRAP")
        .ok()
        .is_some()
    {
        return Err(MemoryError::ConfigInvalid(
            "MEMORY_MCP_HTTP_TEST_BOOTSTRAP is only valid with the test-fixtures feature".into(),
        ));
    }
    // A test-fixtures binary must identify itself explicitly.
    // This prevents an accidentally released build compiled
    // with the fixture feature from silently selecting the
    // in-memory registry instead of a durable backend.
    #[cfg(all(feature = "test-fixtures", not(test)))]
    if std::env::var("MEMORY_MCP_HTTP_TEST_BOOTSTRAP")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .is_none()
    {
        return Err(MemoryError::ConfigInvalid(
            "test-fixtures HTTP builds require MEMORY_MCP_HTTP_TEST_BOOTSTRAP".into(),
        ));
    }
    // The fault-injection test env var is gated on the
    // test-fixtures feature too, but it is independent
    // of MEMORY_MCP_HTTP_TEST_BOOTSTRAP — a recovery test
    // may set MEMORY_MCP_HTTP_TEST_SEED_RESERVED (or the
    // bootstrap var) and the fault var separately.
    #[cfg(not(feature = "test-fixtures"))]
    if std::env::var("MEMORY_MCP_HTTP_TEST_FAULT_POINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .is_some()
    {
        return Err(MemoryError::ConfigInvalid(
            "MEMORY_MCP_HTTP_TEST_FAULT_POINT is only valid with the test-fixtures feature".into(),
        ));
    }
    if cfg.bind.ip().is_unspecified() && !cfg.public_base_url.contains("localhost") {
        eprintln!(
            "memory_mcp::http::config: binding to unspecified address; production must run behind a reverse proxy"
        );
    }
    if cfg.body_limit_bytes == 0 {
        return Err(MemoryError::ConfigInvalid(
            "MEMORY_MCP_HTTP_BODY_LIMIT must be positive".into(),
        ));
    }
    if cfg.request_deadline.is_zero()
        || cfg.shutdown_grace.is_zero()
        || cfg.pool_cap == 0
        || cfg.runtime_idle_ttl.is_zero()
        || cfg.runtime_capacity_wait.is_zero()
        || cfg.runtime_activation_timeout.is_zero()
        || cfg.global_request_limit == 0
        || cfg.subscription_limit == 0
        || cfg.maintenance_parallelism == 0
        || cfg.subscription_queue_capacity == 0
        || cfg.subscription_auth_recheck.is_zero()
        || cfg.task_retention_secs == 0
        || cfg.task_queue_capacity == 0
        || cfg.task_sync_max_bytes == 0
    {
        return Err(MemoryError::ConfigInvalid(
            "HTTP request deadline, shutdown grace, runtime, and admission limits must be positive"
                .into(),
        ));
    }
    if cfg.subscription_auth_recheck > Duration::from_secs(60) {
        return Err(MemoryError::ConfigInvalid(
            "subscription authorization recheck must be no more than 60 seconds".into(),
        ));
    }
    if cfg.allowed_hosts.is_empty() {
        return Err(MemoryError::ConfigInvalid(
            "ALLOWED_HOSTS must be explicit in HTTP SaaS profile".into(),
        ));
    }
    if cfg.allowed_origins.is_empty() {
        return Err(MemoryError::ConfigInvalid(
            "ALLOWED_ORIGINS must be explicit in HTTP SaaS profile".into(),
        ));
    }
    if cfg.allowed_origins.iter().any(|o| o == "*") {
        return Err(MemoryError::ConfigInvalid(
            "wildcard ALLOWED_ORIGINS is rejected (spec §3.3)".into(),
        ));
    }
    if cfg.api_key_pepper.len() < 32 {
        return Err(MemoryError::ConfigInvalid(
            "MEMORY_MCP_API_KEY_PEPPER must be ≥32 bytes".into(),
        ));
    }
    if cfg.signup_mode == SignupMode::Open && !open_signup_quotas_set(cfg) {
        return Err(MemoryError::ConfigInvalid(
            "open signup requires explicit quota values (spec §12)".into(),
        ));
    }
    if cfg.task_retention_secs > i64::MAX as u64 {
        return Err(MemoryError::ConfigInvalid(
            "HTTP task retention must fit a signed duration".into(),
        ));
    }
    if let Some(limits) = &cfg.signup_plan_limits {
        if limits.max_ingested_bytes > i64::MAX as u64 || limits.max_episode_count > i64::MAX as u64
        {
            return Err(MemoryError::ConfigInvalid(
                "HTTP quota counters must fit SurrealDB signed integers".into(),
            ));
        }
        if limits.per_tenant_request_concurrency == 0 || limits.extraction_concurrency == 0 {
            return Err(MemoryError::ConfigInvalid(
                "HTTP request and extraction concurrency limits must be positive".into(),
            ));
        }
    }
    // OIDC completeness and algorithm checks apply only when the
    // deployment actually authenticates browsers through OIDC. A set without
    // `oidc` has no identity provider and must not be forced to configure
    // one; a disabled control plane mounts neither surface. Material for a
    // method the deployment does not enable is rejected in `from_env`, which
    // alone can tell a supplied key from a derived one.
    let uses_oidc = cfg.has_method(super::types::BrowserAuthMethod::Oidc);
    if uses_oidc
        && (cfg.oidc_issuer.is_empty()
            || cfg.oidc_client_id.is_empty()
            || cfg.oidc_audience.is_empty()
            || cfg.oidc_redirect_uri.is_empty())
    {
        return Err(MemoryError::ConfigInvalid(
            "control plane requires OIDC issuer and client id (audience and redirect URI are \
             derived from the client id and the public base URL when unset)"
                .into(),
        ));
    }
    // Keep in lockstep with the algorithm mapping in `control::oidc::client`.
    if uses_oidc
        && !matches!(
            cfg.oidc_allowed_alg.as_str(),
            "auto" | "RS256" | "RS384" | "RS512" | "ES256" | "EdDSA"
        )
    {
        return Err(MemoryError::ConfigInvalid(
            "OIDC allowed algorithm must be 'auto', RS256, RS384, RS512, ES256, or EdDSA".into(),
        ));
    }
    if cfg.enable_control_plane_ui && !cfg.enable_control_plane {
        return Err(MemoryError::ConfigInvalid(
            "control-plane UI requires control plane to be enabled".into(),
        ));
    }
    if cfg.control_db.url == cfg.tenant_db.url
        && cfg.control_db.namespace == cfg.tenant_db.namespace
        && cfg.control_db.database == cfg.tenant_db.database
    {
        return Err(MemoryError::ConfigInvalid(
            "control and tenant storage must use different namespace/database bindings".into(),
        ));
    }

    // ─── Browser auth method validation ────────────────────
    if !cfg.has_method(super::types::BrowserAuthMethod::Oidc) {
        // Material for a disabled method is an ambiguous deployment rather
        // than a harmless extra: a provider that is configured but not enabled
        // is a deployment that believes it has SSO when it does not. The three
        // HMAC keys are absent from this list because only the env loader can
        // tell a supplied key from the one local mode derives.
        for (variable, value) in [
            ("MEMORY_MCP_HTTP_OIDC_ISSUER", &cfg.oidc_issuer),
            ("MEMORY_MCP_HTTP_OIDC_CLIENT_ID", &cfg.oidc_client_id),
            ("MEMORY_MCP_HTTP_OIDC_AUDIENCE", &cfg.oidc_audience),
            ("MEMORY_MCP_HTTP_OIDC_REDIRECT_URI", &cfg.oidc_redirect_uri),
        ] {
            if !value.is_empty() {
                return Err(MemoryError::ConfigInvalid(format!(
                    "{variable} is set but the 'oidc' browser authentication method is not \
                     enabled; add 'oidc' to MEMORY_MCP_HTTP_AUTH_METHODS"
                )));
            }
        }
        if cfg.oidc_allowed_alg != super::parse::DEFAULT_OIDC_ALG {
            return Err(MemoryError::ConfigInvalid(
                "MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG is set but the 'oidc' browser authentication \
                 method is not enabled; add 'oidc' to MEMORY_MCP_HTTP_AUTH_METHODS"
                    .into(),
            ));
        }
        // Open signup is meaningless without an identity provider and would
        // advertise accounts nobody can create. Invite-only is the only
        // coherent setting for a deployment with no OIDC method.
        if cfg.signup_mode != SignupMode::InviteOnly {
            return Err(MemoryError::ConfigInvalid(
                "signup mode 'open' requires the 'oidc' authentication method".into(),
            ));
        }
        if !cfg.operator_identity_allowlist.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "the operator identity allowlist requires the 'oidc' authentication method".into(),
            ));
        }
    }
    if cfg.has_method(super::types::BrowserAuthMethod::Local) {
        // The local method serves a single-tenant administrator console over a
        // password, so it needs an HTTPS origin (or loopback) and the plan it
        // publishes for the clients it provisions.
        if !cfg.public_base_url.starts_with("https://")
            && !is_loopback_public_url(&cfg.public_base_url)
        {
            return Err(MemoryError::ConfigInvalid(
                "the 'local' authentication method requires HTTPS public_base_url (or localhost for development)"
                    .into(),
            ));
        }
        if cfg.signup_plan_limits.is_none() {
            return Err(MemoryError::ConfigInvalid(
                "the 'local' authentication method requires explicit signup plan limits".into(),
            ));
        }
    }
    #[cfg(not(any(test, feature = "test-fixtures")))]
    if cfg.control_db.url.starts_with("mem://") || cfg.tenant_db.url.starts_with("mem://") {
        return Err(MemoryError::ConfigInvalid(
            "mem:// is test-only; production HTTP SaaS requires remote SurrealDB or documented embedded RocksDB"
                .into(),
        ));
    }
    #[cfg(not(feature = "control-plane"))]
    if cfg.enable_control_plane || cfg.enable_control_plane_ui {
        return Err(MemoryError::ConfigInvalid(
            "control-plane settings require the control-plane feature".into(),
        ));
    }
    #[cfg(not(feature = "control-plane-ui"))]
    if cfg.enable_control_plane_ui {
        return Err(MemoryError::ConfigInvalid(
            "control-plane UI requires the control-plane-ui feature".into(),
        ));
    }
    // fs-watch is the stdio-only ingestion path. The
    // HTTP SaaS profile must not enable it; a
    // deployment that sets the env var while running
    // the HTTP binary has misconfigured itself.
    if std::env::var("SURREALDB_FS_WATCH_INBOX").is_ok() {
        return Err(MemoryError::ConfigInvalid(
            "SURREALDB_FS_WATCH_INBOX must not be set in the HTTP SaaS profile".into(),
        ));
    }
    Ok(())
}

fn open_signup_quotas_set(cfg: &HttpConfig) -> bool {
    cfg.signup_plan_limits.is_some()
}
