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
use super::super::leases::migration::ApplyMigrations;
use crate::platform::fault_injection::FaultInjector;

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
        crate::platform::fault_injection::FailOnceAt::from_env()
    }
    #[cfg(not(any(test, feature = "test-fixtures")))]
    {
        Arc::new(crate::platform::fault_injection::NoFaults)
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

/// The profile this build serves, as it appears in the startup line.
///
/// It is the Cargo feature name, so a log query and a build command use the
/// same word. It was `streamable_http_saas`, which described the business
/// model rather than the shape: the profile is a Streamable HTTP server, and
/// whether it is sold as a SaaS is a deployment decision this binary cannot
/// see.
///
/// The `op` is the event's own name, so `RUST_LOG=http.start=debug` reaches
/// this line and a startup event is filterable like any other. It was `event`,
/// which the subsystem filter does not read.
const PROFILE: &str = "streamable-http";

/// Single startup line. Other structured events go through
/// `logging::request_log` middleware.
pub fn emit_startup_log(logger: &StdoutLogger, cfg: &HttpConfig) {
    let mut fields = std::collections::HashMap::new();
    fields.insert("op".into(), serde_json::Value::from("http.start"));
    fields.insert("profile".into(), serde_json::Value::from(PROFILE));
    fields.insert("bind".into(), serde_json::Value::from(cfg.bind.to_string()));
    fields.insert(
        "embedded_tenant_db".into(),
        serde_json::Value::from(cfg.tenant_db.url == "mem://"),
    );
    logger.log(fields, crate::logging::LogLevel::Info);
}

#[cfg(test)]
mod tests {
    use super::PROFILE;

    /// The startup line is how an operator tells which build a deployment is
    /// running, and the first thing they compare is the profile against the
    /// feature they think they enabled. Those two words have to be the same
    /// word, and the feature is `streamable-http` — a Cargo feature name, not
    /// a Rust identifier, so the dash is kept rather than folded into an
    /// underscore.
    #[test]
    fn the_startup_profile_names_the_feature_this_build_compiled() {
        assert_eq!(PROFILE, "streamable-http");
    }

    /// The old name described the business model rather than the shape, and it
    /// was the only place in the repository still using it.
    #[test]
    fn the_profile_no_longer_names_a_business_model() {
        assert!(
            !PROFILE.contains("saas"),
            "the profile is a transport, not a commercial model: {PROFILE}"
        );
    }
}
