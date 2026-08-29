//! HTTP SaaS profile composition root (ADR-0052). Loads `HttpConfig`
//! from environment, builds `HttpState`, runs the signal watcher,
//! and serves until shutdown.

use std::process::ExitCode;

use memory_mcp::http::config::HttpConfig;
use memory_mcp::http::router;
use memory_mcp::http::runtime::{bootstrap, signal as signal_watcher};
use memory_mcp::http::server;
use memory_mcp::logging::StdoutLogger;

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
    if let Err(msg) = bootstrap::validate_no_listener_env() {
        eprintln!("{msg}");
        return ExitCode::from(2);
    }
    let state = match bootstrap::build_state(&cfg).await {
        Ok(s) => s,
        Err((code, msg)) => {
            eprintln!("{msg}");
            return code;
        }
    };

    signal_watcher::spawn(state.shutdown.clone(), state.admission.clone());

    bootstrap::emit_startup_log(&logger, &cfg);
    if let Err(err) = server::serve(
        cfg,
        router::build_router(state.clone()),
        state.shutdown.clone(),
    )
    .await
    {
        eprintln!("server error: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
