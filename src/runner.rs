//! Top-level dispatch: clap parse, build MemoryService once per arm, route to
//! serve / watch / reembed / one-shot CLI tool subcommand.
//!
//! Returns `Result<(), ExitCode>`. The only `std::process::exit` call lives in
//! `main.rs`. See Risk R7.

use std::process::ExitCode;

use clap::Parser;

use crate::cli::args::WatchArgs;
use crate::cli::commands;
use crate::cli::runtime::WatchCommand;
use crate::cli::{
    Cli, Command, build_memory_service, log_session_duration, log_startup, run_reembed_mode,
    run_stdio_server, run_watch_mode,
};
use crate::logging::StdoutLogger;
// `EmbeddingActivationMode` is `pub(crate)` re-exported from `service` (the
// underlying `startup` module is private). The `error` submodule is also
// private — reach `MemoryError` via the `pub use error::MemoryError;` at
// `src/service.rs:15`, not via `service::error::`. See Risk R12.
use crate::service::EmbeddingActivationMode;
use crate::service::MemoryError;

/// Application entry point. Called from `main.rs`.
///
/// `Ok(())` ⇒ exit 0. `Err(code)` ⇒ `main.rs` exits with that code.
/// Internal panics and boxable startup errors are mapped to `ExitCode::FAILURE`
/// after a structured error object is written to stderr.
pub async fn run() -> Result<(), ExitCode> {
    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
    let logger = StdoutLogger::new(&log_level);
    let cli = Cli::parse();

    let startup_ts = chrono::Utc::now();
    log_startup(&logger, mode_label(&cli));

    let outcome = dispatch(&logger, cli).await;

    let duration = chrono::Utc::now().signed_duration_since(startup_ts);
    log_session_duration(&logger, duration.num_seconds());

    outcome
}

async fn dispatch(logger: &StdoutLogger, cli: Cli) -> Result<(), ExitCode> {
    match cli.command {
        // Back-compat: no subcommand, OR explicit `serve`, both run the stdio
        // MCP server. See Risk R8.
        None | Some(Command::Serve) => run_stdio_server(logger).await.map_err(boxed_to_failure),

        Some(Command::Reembed) => run_reembed_mode(logger).await.map_err(boxed_to_failure),

        Some(Command::Watch(args)) => run_watch_mode(logger, watch_command_from_args(args))
            .await
            .map_err(boxed_to_failure),

        // One-shot CLI tool arms. Each builds the service once at the top of
        // the arm and calls the corresponding command handler directly. No
        // closures, no `Pin<Box>`, no `std::process::exit` inside async — see
        // Risk R7.
        Some(Command::Ingest(args)) => {
            let service = build_memory_service(logger, EmbeddingActivationMode::Standard)
                .await
                .map_err(boxed_to_failure)?;
            commands::ingest::run(&service, args)
                .await
                .map_err(report_cli_error)
        }
        Some(Command::Extract(args)) => {
            let service = build_memory_service(logger, EmbeddingActivationMode::Standard)
                .await
                .map_err(boxed_to_failure)?;
            commands::extract::run(&service, args)
                .await
                .map_err(report_cli_error)
        }
        Some(Command::Resolve(args)) => {
            let service = build_memory_service(logger, EmbeddingActivationMode::Standard)
                .await
                .map_err(boxed_to_failure)?;
            commands::resolve::run(&service, args)
                .await
                .map_err(report_cli_error)
        }
        Some(Command::Invalidate(args)) => {
            let service = build_memory_service(logger, EmbeddingActivationMode::Standard)
                .await
                .map_err(boxed_to_failure)?;
            commands::invalidate::run(&service, args)
                .await
                .map_err(report_cli_error)
        }
        Some(Command::Explain(args)) => {
            let service = build_memory_service(logger, EmbeddingActivationMode::Standard)
                .await
                .map_err(boxed_to_failure)?;
            commands::explain::run(&service, args)
                .await
                .map_err(report_cli_error)
        }
        Some(Command::AssembleContext(args)) => {
            let service = build_memory_service(logger, EmbeddingActivationMode::Standard)
                .await
                .map_err(boxed_to_failure)?;
            commands::assemble_context::run(&service, args)
                .await
                .map_err(report_cli_error)
        }
    }
}

/// Shared error mapper for one-shot CLI tools. Prints the JSON envelope on
/// stderr and returns the matching `ExitCode`. Defined once so the policy
/// cannot drift between arms. See Risk R6 and Risk R7.
fn report_cli_error(err: MemoryError) -> ExitCode {
    let code = error_exit_code(&err);
    eprintln!(
        "{}",
        serde_json::json!({
            "error": err.to_string(),
            "kind": error_kind(&err),
            "exit_code": code,
        })
    );
    ExitCode::from(code)
}

/// Exit-code policy for `MemoryError`.
fn error_exit_code(err: &MemoryError) -> u8 {
    match err {
        MemoryError::Validation(_) | MemoryError::NotFound(_) => 2,
        MemoryError::Storage(_)
        | MemoryError::Transient(_)
        | MemoryError::ConfigMissing(_)
        | MemoryError::ConfigInvalid(_)
        | MemoryError::Conflict(_)
        | MemoryError::BudgetExhausted(_) => 1,
    }
}

/// Stable string name for the `kind` field of the CLI error envelope.
fn error_kind(err: &MemoryError) -> &'static str {
    match err {
        MemoryError::ConfigMissing(_) => "ConfigMissing",
        MemoryError::ConfigInvalid(_) => "ConfigInvalid",
        MemoryError::Storage(_) => "Storage",
        MemoryError::Transient(_) => "Transient",
        MemoryError::NotFound(_) => "NotFound",
        MemoryError::Validation(_) => "Validation",
        MemoryError::Conflict(_) => "Conflict",
        MemoryError::BudgetExhausted(_) => "BudgetExhausted",
    }
}

fn mode_label(cli: &Cli) -> &'static str {
    match &cli.command {
        None | Some(Command::Serve) => "serve",
        Some(Command::Watch(_)) => "watch",
        Some(Command::Reembed) => "reembed",
        Some(Command::Ingest(_)) => "cli.ingest",
        Some(Command::Extract(_)) => "cli.extract",
        Some(Command::Resolve(_)) => "cli.resolve",
        Some(Command::Invalidate(_)) => "cli.invalidate",
        Some(Command::Explain(_)) => "cli.explain",
        Some(Command::AssembleContext(_)) => "cli.assemble_context",
    }
}

fn watch_command_from_args(args: WatchArgs) -> WatchCommand {
    WatchCommand {
        dir: args.dir,
        project: args.project,
        scope: args.scope,
        interval_secs: args.interval_secs,
    }
}

fn boxed_to_failure(err: Box<dyn std::error::Error>) -> ExitCode {
    eprintln!(
        "{}",
        serde_json::json!({
            "error": err.to_string(),
            "exit_code": 1u8,
        })
    );
    ExitCode::FAILURE
}
