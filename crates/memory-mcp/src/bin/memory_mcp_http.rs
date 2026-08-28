//! HTTP SaaS profile composition root (ADR-0052). Loads `HttpConfig`
//! from environment, builds `HttpState`, installs the Prometheus
//! recorder (when the `prometheus` feature is enabled), runs the
//! signal handler, and serves until shutdown.

use std::process::ExitCode;

use memory_mcp::http::config::HttpConfig;
use memory_mcp::http::router;
use memory_mcp::http::server;
use memory_mcp::http::HttpState;
use memory_mcp::logging::{LogLevel, StdoutLogger};

#[tokio::main]
async fn main() -> ExitCode {
    let logger = StdoutLogger::new("info");
    let cfg = match HttpConfig::from_env() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("config error: {err}");
            return ExitCode::from(2);
        }
    };
    if let Err(err) = cfg.validate() {
        eprintln!("config invalid: {err}");
        return ExitCode::from(2);
    }

    #[cfg(feature = "prometheus")]
    let state = {
        if let Err(err) = memory_mcp::http::metrics::validate_no_listener_env() {
            eprintln!("config invalid: {err}");
            return ExitCode::from(2);
        }
        let handle = match memory_mcp::http::metrics::install_recorder() {
            Ok(h) => h,
            Err(err) => {
                eprintln!("metrics init error: {err}");
                return ExitCode::from(2);
            }
        };
        match HttpState::new_tenantless(cfg.clone(), handle).await {
            Ok(s) => s,
            Err(err) => {
                eprintln!("tenant runtime init error: {err}");
                return ExitCode::from(2);
            }
        }
    };
    #[cfg(not(feature = "prometheus"))]
    let state = match HttpState::new_tenantless(cfg.clone()).await {
        Ok(s) => s,
        Err(err) => {
            eprintln!("tenant runtime init error: {err}");
            return ExitCode::from(2);
        }
    };

    let shutdown = state.shutdown.clone();
    let signal_shutdown = shutdown.clone();
    let admission = state.admission.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut terminate = match signal(SignalKind::terminate()) {
                Ok(s) => Some(s),
                Err(_) => None,
            };
            match terminate.as_mut() {
                Some(t) => {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = t.recv() => {}
                    }
                }
                None => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        admission.close();
        signal_shutdown.begin();
    });

    let router = router::build_router(state);
    logger.log(
        std::collections::HashMap::from([
            ("event".to_string(), serde_json::Value::from("http_start")),
            (
                "profile".to_string(),
                serde_json::Value::from("streamable_http_saas"),
            ),
            (
                "bind".to_string(),
                serde_json::Value::from(cfg.bind.to_string()),
            ),
            (
                "control_plane".to_string(),
                serde_json::Value::from(cfg.enable_control_plane),
            ),
            (
                "embedded_tenant_db".to_string(),
                serde_json::Value::from(cfg.tenant_db.url == "mem://"),
            ),
        ]),
        LogLevel::Info,
    );
    if let Err(err) = server::serve(cfg, router, shutdown).await {
        eprintln!("server error: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
