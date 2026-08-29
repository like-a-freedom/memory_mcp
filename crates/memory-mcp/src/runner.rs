//! Top-level dispatch: clap parse, build MemoryService once per arm, route to
//! serve / watch / reembed / one-shot CLI tool subcommand.
//!
//! Returns `Result<(), ExitCode>`. The only `std::process::exit` call lives in
//! `main.rs`. See Risk R7.

use std::collections::HashMap;
use std::process::ExitCode;

use clap::Parser;
use serde_json::json;

use crate::cli::commands;
use crate::cli::{
    Cli, Command, build_memory_service, log_session_duration, log_startup, run_reembed_mode,
    run_stdio_server,
};
use crate::logging::{LogLevel, StdoutLogger, install_log_file};
// `EmbeddingActivationMode` is `pub(crate)` re-exported from `service` (the
// underlying `startup` module is private). The `error` submodule is also
// private — reach `MemoryError` via the `pub use error::MemoryError;` at
// `src/service.rs:15`, not via `service::error::`. See Risk R12.
use crate::service::EmbeddingActivationMode;
use crate::service::MemoryError;

/// Normalizes a raw `MEMORY_LOG_FILE` value. Returns `Some(trimmed_path)`
/// if the value is non-empty after trimming; `None` otherwise.
fn resolve_log_file_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Application entry point. Called from `main.rs`.
///
/// `Ok(())` ⇒ exit 0. `Err(code)` ⇒ `main.rs` exits with that code.
/// Internal panics and boxable startup errors are mapped to `ExitCode::FAILURE`
/// after a structured error object is written to stderr.
pub async fn run() -> Result<(), ExitCode> {
    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
    let logger = StdoutLogger::new(&log_level);

    // Install file log sink if configured. Must happen before log_startup
    // so the very first event goes to the file.
    if let Ok(raw_path) = std::env::var("MEMORY_LOG_FILE")
        && let Some(path) = resolve_log_file_path(&raw_path)
        && let Err(err) = install_log_file(&path)
    {
        // Fallback: warn to stderr unconditionally. The sink is not installed,
        // so stderr works; going through `logger.log` would let `RUST_LOG=error`
        // silently swallow the one diagnostic that must not be lost.
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("main.log_file_open_failed"));
        event.insert("path".to_string(), json!(&path));
        event.insert("error".to_string(), json!(err.to_string()));
        eprintln!(
            "{}",
            StdoutLogger::format_event_line(&event, LogLevel::Warn)
        );
    }

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

        Some(Command::Reembed(args)) => run_reembed_mode(logger, args)
            .await
            .map_err(boxed_to_failure),

        Some(Command::Init(args)) => commands::init::run(args).map_err(report_cli_error),

        // One-shot CLI tool subcommands: build the service once, then run the
        // command's erased runner (label + error policy live in `cli.rs`). No
        // closures, no `Pin<Box>` in this file, no `std::process::exit` inside
        // async — see Risk R7.
        Some(command) => run_one_shot(logger, command).await,
    }
}

/// Builds the service once and runs a one-shot CLI tool subcommand, mapping
/// its `MemoryError` through the shared JSON envelope + exit-code policy.
/// Defined once so the policy cannot drift between subcommands.
/// See Risk R6 and Risk R7.
async fn run_one_shot(logger: &StdoutLogger, command: Command) -> Result<(), ExitCode> {
    let service = build_memory_service(logger, EmbeddingActivationMode::Standard)
        .await
        .map_err(boxed_to_failure)?;

    let Some(runner) = command.into_one_shot() else {
        // Defensive guard: `dispatch` matches the four service-mode variants
        // above, so every command reaching here is a one-shot subcommand.
        eprintln!("memory_mcp: internal dispatch error: not a one-shot subcommand");
        return Err(ExitCode::FAILURE);
    };

    runner(&service).await.map_err(report_cli_error)
}

/// Shared error mapper for one-shot CLI tools. Prints the JSON envelope on
/// stderr and returns the matching `ExitCode`. Defined once so the policy
/// cannot drift between arms. See Risk R6 and Risk R7.
fn cli_error_json(err: &MemoryError) -> serde_json::Value {
    let code = error_exit_code(err);
    let mut envelope = serde_json::json!({
        "error": err.to_string(),
        "kind": error_kind(err),
        "exit_code": code,
    });

    match err {
        MemoryError::ConfigMissing(_) => {
            envelope["hint"] = serde_json::json!(
                "Run `memory_mcp init` for host configuration, or unset remote database variables to use embedded mode."
            );
        }
        MemoryError::ConfigInvalid(_) => {
            envelope["hint"] = serde_json::json!(
                "Check the environment values or run `memory_mcp init` to print a known-good configuration."
            );
        }
        _ => {}
    }

    envelope
}

fn report_cli_error(err: MemoryError) -> ExitCode {
    eprintln!("{}", cli_error_json(&err));
    ExitCode::from(error_exit_code(&err))
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
        | MemoryError::BudgetExhausted(_)
        | MemoryError::ModelNotReady(_)
        | MemoryError::Auth(_)
        | MemoryError::Unavailable(_) => 1,
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
        MemoryError::ModelNotReady(_) => "ModelNotReady",
        MemoryError::Auth(_) => "Auth",
        MemoryError::Unavailable(_) => "Unavailable",
    }
}

fn mode_label(cli: &Cli) -> &'static str {
    cli.command.as_ref().map_or("serve", Command::mode_label)
}

fn boxed_to_failure(err: Box<dyn std::error::Error>) -> ExitCode {
    if let Some(memory_error) = err.downcast_ref::<MemoryError>() {
        eprintln!("{}", cli_error_json(memory_error));
    } else {
        eprintln!(
            "{}",
            serde_json::json!({"error": err.to_string(), "exit_code": 1u8})
        );
    }
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_error_config_missing_contains_zero_config_hint() {
        let value = cli_error_json(&MemoryError::ConfigMissing("SURREALDB_URL".into()));

        assert_eq!(value["kind"], "ConfigMissing");
        assert_eq!(
            value["hint"],
            "Run `memory_mcp init` for host configuration, or unset remote database variables to use embedded mode."
        );
    }

    #[test]
    fn cli_error_config_invalid_contains_repair_hint() {
        let value = cli_error_json(&MemoryError::ConfigInvalid("bad namespace".into()));

        assert_eq!(value["kind"], "ConfigInvalid");
        assert_eq!(
            value["hint"],
            "Check the environment values or run `memory_mcp init` to print a known-good configuration."
        );
    }

    #[test]
    fn cli_error_non_config_error_has_no_hint_field() {
        let value = cli_error_json(&MemoryError::Validation("bad input".into()));

        assert!(value.get("hint").is_none());
    }

    #[test]
    fn resolve_log_file_path_trims_and_rejects_empty() {
        assert_eq!(
            super::resolve_log_file_path("  /tmp/test.log  "),
            Some("/tmp/test.log".to_string())
        );
        assert_eq!(super::resolve_log_file_path(""), None);
        assert_eq!(super::resolve_log_file_path("   "), None);
        assert_eq!(
            super::resolve_log_file_path("/var/log/memory.log"),
            Some("/var/log/memory.log".to_string())
        );
    }
}
