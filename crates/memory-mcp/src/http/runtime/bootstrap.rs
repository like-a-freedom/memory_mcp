//! HTTP composition-root helpers (ADR-0052).
//!
//! Pure startup functions, no CLI parsing. The binary's `main` only
//! reads env, dispatches to these helpers, and serves.

use std::process::ExitCode;

use crate::logging::StdoutLogger;

use super::super::HttpState;
use super::super::config::HttpConfig;

/// Build `HttpState` from the config, optionally installing a
/// Prometheus recorder. Maps every error to a `startup_error()`
/// log line + `ExitCode::from(2)`.
pub async fn build_state(
    cfg: &HttpConfig,
) -> Result<std::sync::Arc<HttpState>, (ExitCode, String)> {
    #[cfg(feature = "prometheus")]
    {
        let handle = match crate::http::metrics::install_recorder() {
            Ok(h) => Some(h),
            Err(err) => return Err((ExitCode::from(2), format!("metrics init error: {err}"))),
        };
        match HttpState::new_tenantless(cfg.clone(), handle).await {
            Ok(s) => Ok(s),
            Err(err) => Err((
                ExitCode::from(2),
                format!("tenant runtime init error: {err}"),
            )),
        }
    }
    #[cfg(not(feature = "prometheus"))]
    match HttpState::new_tenantless(cfg.clone()).await {
        Ok(s) => Ok(s),
        Err(err) => Err((
            ExitCode::from(2),
            format!("tenant runtime init error: {err}"),
        )),
    }
}

/// Validate that the stdio profile's `MEMORY_PROMETHEUS_LISTEN_ADDR`
/// is not set in HTTP mode. Only meaningful under the `prometheus`
/// feature.
#[cfg(feature = "prometheus")]
pub fn validate_no_listener_env() -> Result<(), String> {
    crate::http::metrics::validate_no_listener_env().map_err(|err| format!("config invalid: {err}"))
}

#[cfg(not(feature = "prometheus"))]
pub fn validate_no_listener_env() -> Result<(), String> {
    Ok(())
}

/// Single startup line. Other structured events go through
/// `logging::request_log` middleware.
pub fn emit_startup_log(logger: &StdoutLogger, cfg: &HttpConfig) {
    let mut fields = std::collections::HashMap::new();
    fields.insert("event".into(), serde_json::Value::from("http_start"));
    fields.insert(
        "profile".into(),
        serde_json::Value::from("streamable_http_saas"),
    );
    fields.insert("bind".into(), serde_json::Value::from(cfg.bind.to_string()));
    fields.insert(
        "control_plane".into(),
        serde_json::Value::from(cfg.enable_control_plane),
    );
    fields.insert(
        "embedded_tenant_db".into(),
        serde_json::Value::from(cfg.tenant_db.url == "mem://"),
    );
    logger.log(fields, crate::logging::LogLevel::Info);
}
