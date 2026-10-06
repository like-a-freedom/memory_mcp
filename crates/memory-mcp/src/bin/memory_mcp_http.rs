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
    let logger = StdoutLogger::from_env();
    // Install the file sink before anything logs or reads configuration, so
    // `MEMORY_LOG_FILE` means the same here as in the stdio profile. It used
    // to be installed only by the stdio runner, and this binary silently kept
    // writing to stderr no matter what the variable said.
    memory_mcp::logging::install_log_file_from_env();
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
    let runtime = match bootstrap::build_state(&cfg, &logger).await {
        Ok(r) => r,
        Err((code, msg)) => {
            eprintln!("{msg}");
            return code;
        }
    };
    let state = runtime.state;

    #[cfg(feature = "test-fixtures")]
    if let Err(err) = memory_mcp::http::test_bootstrap::apply_test_bootstrap(&state).await {
        eprintln!("test bootstrap error: {err}");
        return ExitCode::from(2);
    }

    #[cfg(feature = "test-fixtures")]
    if let Err(err) = memory_mcp::http::test_bootstrap::apply_test_seed_reserved(&state).await {
        eprintln!("test seed reserved error: {err}");
        return ExitCode::from(2);
    }

    #[cfg(all(feature = "test-fixtures", feature = "control-plane"))]
    if let Err(err) = memory_mcp::http::test_bootstrap::apply_test_seed_session(&state).await {
        eprintln!("test seed session error: {err}");
        return ExitCode::from(2);
    }

    signal_watcher::spawn(state.shutdown.clone(), state.admission.clone());

    // The backfill job walks tenants, so a deployment with no embedding
    // policy has nothing for it to do. It is not registered at all in that
    // case rather than registered-and-gated: a job that cannot act is a line in
    // the job list an operator has to reason about, and a lexical-only
    // deployment should not have one.
    let backfill_policy = runtime.deployment_policy.clone();

    let scheduler_hooks = match memory_mcp::bootstrap::provisioning_scheduler_hooks(
        runtime.tenant_migrations.clone(),
        runtime.fault_injector.clone(),
    )
    .map(|hooks| {
        let task_options =
            memory_mcp::http::runtime::storage::RuntimeOptions::from_http_config(&cfg)
                .with_fault_injector(runtime.fault_injector.clone());
        let hooks = hooks
            .with_additional_job(memory_mcp::http::app_sessions::scheduler::scheduler_job())
            // The task job carries the deployment policy, because a `reembed`
            // row can only force-enable a provider if the provider is
            // reachable from here. `None` for a lexical-only deployment, whose
            // reembed rows then fail loudly rather than running degraded.
            .with_additional_job(
                memory_mcp::http::tasks::scheduler::scheduler_job_with_policy(
                    task_options,
                    Some(backfill_policy.clone()),
                ),
            )
            .with_additional_job(memory_mcp::http::subscriptions::scheduler::scheduler_job())
            .with_additional_job(memory_mcp::http::registry::plan::scheduler_job())
            .with_additional_job(
                memory_mcp::http::runtime::pool::Pool::eviction_scheduler_job(state.pool.clone()),
            )
            .with_additional_job(
                memory_mcp::http::registry::provisioning::reconciliation_scheduler_job(),
            );
        // The throttle table only exists under `control-plane`; the
        // maintenance pass is registered with the same feature so a
        // data-plane-only HTTP build does not carry it.
        #[cfg(feature = "control-plane")]
        let hooks = hooks.with_additional_job(
            memory_mcp::http::registry::surreal_store::rate_bucket_cleanup_scheduler_job(),
        );
        match backfill_policy.embedding.is_some() {
            true => hooks.with_additional_job(
                memory_mcp::http::embedding::backfill_scheduler::backfill_scheduler_job(
                    backfill_policy,
                ),
            ),
            false => hooks,
        }
    })
    .and_then(|hooks| hooks.with_maintenance_parallelism(cfg.maintenance_parallelism))
    {
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

    let router = match router::build_router(state.clone(), Some(runtime.fault_injector.clone())) {
        Ok(router) => router,
        Err(err) => {
            eprintln!("router config error: {err}");
            return ExitCode::from(2);
        }
    };
    bootstrap::emit_startup_log(&logger, &cfg);
    let server_result = server::serve(cfg, router, state.shutdown.clone()).await;
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
