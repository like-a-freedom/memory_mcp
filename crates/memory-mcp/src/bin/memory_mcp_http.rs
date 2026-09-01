//! HTTP SaaS profile composition root. Loads `HttpConfig`
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

    #[cfg(feature = "test-fixtures")]
    if let Err(err) = memory_mcp::http::test_bootstrap::apply_test_bootstrap(&state).await {
        eprintln!("test bootstrap error: {err}");
        return ExitCode::from(2);
    }

    signal_watcher::spawn(state.shutdown.clone(), state.admission.clone());

    let scheduler_hooks =
        match memory_mcp::http::leases::scheduler::SchedulerHooks::with_provisioning_only().map(
            |hooks| {
                hooks
                    .with_additional_job(memory_mcp::http::app_sessions::scheduler::scheduler_job())
                    .with_additional_job(memory_mcp::http::tasks::scheduler::scheduler_job())
                    .with_additional_job(memory_mcp::http::subscriptions::scheduler::scheduler_job())
                    .with_additional_job(memory_mcp::http::registry::plan::scheduler_job())
            },
        ) {
            Ok(hooks) => hooks,
            Err(err) => {
                eprintln!("scheduler config error: {err}");
                return ExitCode::from(2);
            }
        };
    let scheduler = memory_mcp::http::leases::scheduler::start(
        state.registry.clone(),
        scheduler_hooks,
        state.shutdown.token(),
    );

    bootstrap::emit_startup_log(&logger, &cfg);
    let server_result = server::serve(
        cfg,
        router::build_router(state.clone()),
        state.shutdown.clone(),
    )
    .await;
    state.admission.close();
    state.shutdown.begin();
    scheduler.join().await;
    match server_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("server error: {err}");
            ExitCode::FAILURE
        }
    }
}
