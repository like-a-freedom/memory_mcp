//! HTTP composition-root helpers.
//!
//! Pure startup functions, no CLI parsing. The binary's `main` only
//! reads env, dispatches to these helpers, and serves.

use std::process::ExitCode;
use std::sync::Arc;

use crate::logging::StdoutLogger;

use super::super::HttpState;
use super::super::composition::HttpProductionComposition;
use super::super::config::HttpConfig;
use super::super::fault_injection::FaultInjector;
use super::super::leases::migration::ApplyMigrations;

/// The startup-composed HTTP runtime. The binary keeps
/// `tenant_migrations` alive for the scheduler hooks; it never
/// re-selects either adapter after this point (ADR-0053). The fault
/// injector is threaded into the scheduler and the deletion worker
/// the same way.
pub struct HttpRuntime {
    pub state: std::sync::Arc<HttpState>,
    pub tenant_migrations: std::sync::Arc<dyn ApplyMigrations>,
    pub fault_injector: Arc<dyn FaultInjector>,
}

/// Compose the production adapters, build `HttpState`, and optionally
/// install a Prometheus recorder. Maps every error to a
/// `startup_error()` log line + `ExitCode::from(2)`.
///
/// The fault injector is read from the test-fixtures env var when the
/// `test-fixtures` feature is enabled; production builds install
/// [`NoFaults`].
pub async fn build_state(cfg: &HttpConfig) -> Result<HttpRuntime, (ExitCode, String)> {
    #[cfg(feature = "prometheus")]
    let metrics_handle = match crate::http::metrics::install_recorder() {
        Ok(h) => Some(h),
        Err(err) => return Err((ExitCode::from(2), format!("metrics init error: {err}"))),
    };
    #[cfg(not(feature = "prometheus"))]
    let metrics_handle = None;

    let fault_injector: Arc<dyn FaultInjector> = load_fault_injector();

    let composition =
        match HttpProductionComposition::connect_with_injector(cfg, fault_injector.clone()).await {
            Ok(c) => c,
            Err(err) => {
                return Err((
                    ExitCode::from(2),
                    format!("tenant runtime init error: {err}"),
                ));
            }
        };
    let state = match HttpState::assemble(cfg.clone(), composition.registry, metrics_handle).await {
        Ok(s) => s,
        Err(err) => {
            return Err((
                ExitCode::from(2),
                format!("tenant runtime init error: {err}"),
            ));
        }
    };
    Ok(HttpRuntime {
        state,
        tenant_migrations: composition.tenant_migrations,
        fault_injector: composition.fault_injector,
    })
}

/// Resolve the fault injector the binary runs with. When the
/// `test-fixtures` feature is enabled the test-only env var
/// `MEMORY_MCP_HTTP_TEST_FAULT_POINT` selects a `FailOnceAt`; in every
/// other build the binary uses [`NoFaults`].
fn load_fault_injector() -> Arc<dyn FaultInjector> {
    #[cfg(any(test, feature = "test-fixtures"))]
    {
        crate::http::fault_injection::FailOnceAt::from_env()
    }
    #[cfg(not(any(test, feature = "test-fixtures")))]
    {
        Arc::new(crate::http::fault_injection::NoFaults)
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
