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
    if cfg.enable_control_plane
        && (cfg.oidc_issuer.is_empty()
            || cfg.oidc_client_id.is_empty()
            || cfg.oidc_audience.is_empty()
            || cfg.oidc_redirect_uri.is_empty())
    {
        return Err(MemoryError::ConfigInvalid(
            "control plane requires OIDC issuer, client id, audience, and redirect URI".into(),
        ));
    }
    if cfg.enable_control_plane
        && !matches!(cfg.oidc_allowed_alg.as_str(), "RS256" | "ES256" | "EdDSA")
    {
        return Err(MemoryError::ConfigInvalid(
            "OIDC allowed algorithm must be RS256, ES256, or EdDSA".into(),
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
