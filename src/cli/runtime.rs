use std::collections::HashMap;
use std::path::PathBuf;

use rmcp::ServiceExt;
use rmcp::transport::io::stdio;
use serde_json::json;

use crate::cli::args::ReembedArgs;
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::EmbeddingActivationMode;
use crate::service::ReembedSummary;
use crate::service::reembed_options::{ReembedOptions, ReembedOutcome};
use crate::service::reembed_progress::{
    IndicatifProgressReporter, LogProgressReporter, ReembedProgressReporter,
};
use crate::{MemoryMcp, MemoryService};
use tokio_util::sync::CancellationToken;

/// Builds a structured log event map from key-value pairs.
macro_rules! event {
    ($($key:expr => $value:expr),+ $(,)?) => {{
        let mut m = HashMap::new();
        $(m.insert($key.to_string(), $value);)+
        m
    }};
}

/// Logs an error event and returns the error wrapped for propagation.
fn log_and_return_error(
    logger: &StdoutLogger,
    op: &str,
    err: impl std::error::Error + 'static,
) -> Box<dyn std::error::Error> {
    let err_msg = err.to_string();
    logger.log(
        event!("op" => json!(op), "error" => json!(err_msg)),
        LogLevel::Error,
    );
    Box::new(err) as Box<dyn std::error::Error>
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchCommand {
    pub dir: PathBuf,
    pub project: Option<String>,
    pub scope: String,
    pub interval_secs: u64,
}

/// Helper: log startup event with pid and mode label.
pub fn log_startup(logger: &StdoutLogger, mode_label: &str) {
    let mut m = HashMap::new();
    m.insert("op".to_string(), json!("main.startup"));
    m.insert("pid".to_string(), json!(std::process::id()));
    m.insert("mode".to_string(), json!(mode_label));
    logger.log(m, LogLevel::Info);
}

/// Helper: log session duration event.
pub fn log_session_duration(logger: &StdoutLogger, duration_secs: i64) {
    let mut m = HashMap::new();
    m.insert("op".to_string(), json!("main.session_duration"));
    m.insert("duration_secs".to_string(), json!(duration_secs));
    logger.log(m, LogLevel::Info);
}

pub async fn build_memory_service(
    logger: &StdoutLogger,
    mode: EmbeddingActivationMode,
) -> Result<MemoryService, Box<dyn std::error::Error>> {
    crate::observability::install()
        .map_err(|err| log_and_return_error(logger, "main.observability_failed", err))?;
    MemoryService::new_from_env_with_mode(mode)
        .await
        .map_err(|err| log_and_return_error(logger, "main.startup_failed", err))
}

pub async fn run_stdio_server(logger: &StdoutLogger) -> Result<(), Box<dyn std::error::Error>> {
    let memory_service = build_memory_service(logger, EmbeddingActivationMode::Standard).await?;
    let claim_worker = memory_service.start_claim_workers().await;
    let lifecycle_worker = memory_service.start_lifecycle_worker().await;
    // Keep a handle for background-worker shutdown. The runtime is shared via
    // `Arc` internally, so the clone observes the same cancellation token.
    let shutdown_service = memory_service.clone();
    let server = MemoryMcp::new(memory_service);

    logger.log(event!("op" => json!("main.serve_starting")), LogLevel::Info);

    let (stdin, stdout) = stdio();
    let service = server
        .serve((stdin, stdout))
        .await
        .map_err(|err| log_and_return_error(logger, "main.serve_failed", err))?;

    logger.log(event!("op" => json!("main.running")), LogLevel::Info);

    let result = service
        .waiting()
        .await
        .map(|_quit_reason| {
            logger.log(event!("op" => json!("main.shutdown")), LogLevel::Info);
        })
        .map_err(|err| log_and_return_error(logger, "main.error", err));

    claim_worker.shutdown().await;
    lifecycle_worker.shutdown().await;
    shutdown_service
        .shutdown_lifecycle_background_workers()
        .await;
    result
}

pub async fn run_watch_mode(
    logger: &StdoutLogger,
    watch: WatchCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    logger.log(
        event!(
            "op" => json!("main.watch_starting"),
            "dir" => json!(watch.dir.display().to_string()),
            "scope" => json!(watch.scope),
            "interval_secs" => json!(watch.interval_secs),
            "project" => json!(watch.project),
        ),
        LogLevel::Info,
    );

    let memory_service = build_memory_service(logger, EmbeddingActivationMode::Standard).await?;
    let claim_worker = memory_service.start_claim_workers().await;
    let lifecycle_worker = memory_service.start_lifecycle_worker().await;
    // Keep a handle for background-worker shutdown.
    let shutdown_service = memory_service.clone();

    #[cfg(feature = "cli-watch")]
    {
        let result = crate::service::FsWatcher::run_with_interval(
            watch.dir,
            watch.project,
            watch.scope,
            watch.interval_secs,
            memory_service,
        )
        .await
        .map_err(|err| log_and_return_error(logger, "main.watch_failed", err));
        claim_worker.shutdown().await;
        lifecycle_worker.shutdown().await;
        shutdown_service
            .shutdown_lifecycle_background_workers()
            .await;
        result
    }

    #[cfg(not(feature = "cli-watch"))]
    {
        let _ = (watch, memory_service);
        claim_worker.shutdown().await;
        lifecycle_worker.shutdown().await;
        shutdown_service
            .shutdown_lifecycle_background_workers()
            .await;
        Err(Box::new(std::io::Error::other(
            "watch subcommand requires the cli-watch feature",
        )) as Box<dyn std::error::Error>)
    }
}

pub async fn run_reembed_mode(
    logger: &StdoutLogger,
    args: ReembedArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    logger.log(
        event!("op" => json!("main.reembed_starting")),
        LogLevel::Info,
    );

    let options = ReembedOptions {
        max_failures: args.max_failures,
        retry_failed: args.retry_failed,
    };

    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let indicatif_reporter = if is_tty {
        Some(IndicatifProgressReporter::new())
    } else {
        None
    };
    if let Some(ref indicatif) = indicatif_reporter {
        indicatif.start_init_spinner("Initializing reembed service...");
    }
    let reporter: Box<dyn ReembedProgressReporter> = match indicatif_reporter {
        Some(indicatif) => Box::new(indicatif),
        None => Box::new(LogProgressReporter::new(logger.clone())),
    };

    let memory_service =
        build_memory_service(logger, EmbeddingActivationMode::ForceEnabledForReembed).await?;

    let cancel_token = CancellationToken::new();
    let cancel_for_handler = cancel_token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_for_handler.cancel();
        }
    });

    let started_at = std::time::Instant::now();
    let (summary, outcome) = memory_service
        .reembed_all_facts(&options, reporter.as_ref(), &cancel_token)
        .await
        .map_err(|err| log_and_return_error(logger, "main.reembed_failed", err))?;
    let elapsed = started_at.elapsed();

    print_reembed_summary(&outcome, &summary, elapsed);

    logger.log(
        event!(
            "op" => json!("main.reembed_completed"),
            "total_facts" => json!(summary.total_facts),
            "processed_facts" => json!(summary.processed_facts),
            "succeeded_facts" => json!(summary.succeeded_facts),
            "failed_facts" => json!(summary.failed_facts),
            "outcome" => json!(format!("{outcome:?}")),
            "duration_ms" => json!(elapsed.as_millis() as u64),
        ),
        LogLevel::Info,
    );

    memory_service.shutdown_lifecycle_background_workers().await;

    if outcome.exit_code() != 0 {
        return Err(Box::new(std::io::Error::other(format!(
            "reembed exited with status: {outcome:?}"
        ))));
    }
    Ok(())
}

/// Prints a compact human-readable summary to stdout after reembed completes.
///
/// This remains visible after the progress bar clears.
fn print_reembed_summary(
    outcome: &ReembedOutcome,
    summary: &ReembedSummary,
    elapsed: std::time::Duration,
) {
    println!();
    match outcome {
        ReembedOutcome::Completed => println!("✓ Reembed completed"),
        ReembedOutcome::CompletedWithErrors => println!("✓ Reembed completed (with errors)"),
        ReembedOutcome::Failed => println!("✗ Reembed failed"),
        ReembedOutcome::Interrupted => println!("⏹ Reembed interrupted"),
        ReembedOutcome::NothingToDo => {
            println!("✓ Nothing to do — all embeddings already match target signature");
        }
    }
    println!();
    println!("  Total:       {} facts", summary.total_facts);
    println!(
        "  Processed:   {} ({} succeeded, {} failed)",
        summary.processed_facts, summary.succeeded_facts, summary.failed_facts
    );
    println!("  Duration:    {:.1}s", elapsed.as_secs_f64());
    if summary.processed_facts > 0 && elapsed.as_secs_f64() > 0.0 {
        println!(
            "  Speed:       {:.0} facts/sec",
            summary.processed_facts as f64 / elapsed.as_secs_f64()
        );
    }
    println!();
    if summary.failed_facts > 0 {
        match outcome {
            ReembedOutcome::CompletedWithErrors => {
                println!(
                    "  {} facts failed. Re-run with --retry-failed to retry only failures.",
                    summary.failed_facts
                );
            }
            ReembedOutcome::Failed => {
                println!(
                    "  {} facts failed (quota exceeded). Fix the provider and re-run with --retry-failed.",
                    summary.failed_facts
                );
            }
            ReembedOutcome::Interrupted => {
                println!("  Resume with 'memory_mcp reembed' to continue from where it stopped.");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cli::Command;
    use crate::cli::{Cli, args::WatchArgs};
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_defaults_to_stdio_serve_mode() {
        let cli = Cli::parse_from(["memory_mcp"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_serve_subcommand() {
        let cli = Cli::parse_from(["memory_mcp", "serve"]);
        assert!(matches!(cli.command, Some(Command::Serve)));
    }

    #[test]
    fn cli_watch_with_optional_flags() {
        let cli = Cli::parse_from([
            "memory_mcp",
            "watch",
            "/tmp/inbox",
            "--project",
            "atlas",
            "--scope",
            "team",
            "--interval-secs",
            "7",
        ]);
        let watch: WatchArgs = match cli.command {
            Some(Command::Watch(w)) => w,
            _ => panic!("expected Watch command"),
        };
        assert_eq!(watch.dir.to_str().unwrap(), "/tmp/inbox");
        assert_eq!(watch.project.as_deref(), Some("atlas"));
        assert_eq!(watch.scope, "team");
        assert_eq!(watch.interval_secs, 7);
    }

    #[test]
    fn cli_watch_defaults() {
        let cli = Cli::parse_from(["memory_mcp", "watch", "/tmp/inbox"]);
        let watch: WatchArgs = match cli.command {
            Some(Command::Watch(w)) => w,
            _ => panic!("expected Watch command"),
        };
        assert_eq!(watch.scope, "team");
        assert_eq!(watch.interval_secs, 2);
    }

    #[test]
    fn cli_watch_rejects_missing_directory() {
        let result = Cli::try_parse_from(["memory_mcp", "watch"]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_reembed_subcommand() {
        let cli = Cli::parse_from(["memory_mcp", "reembed"]);
        assert!(matches!(cli.command, Some(Command::Reembed(_))));
    }

    #[test]
    fn cli_reembed_with_flags() {
        let cli = Cli::parse_from([
            "memory_mcp",
            "reembed",
            "--max-failures",
            "5",
            "--retry-failed",
        ]);
        match cli.command {
            Some(Command::Reembed(args)) => {
                assert_eq!(args.max_failures, Some(5));
                assert!(args.retry_failed);
            }
            _ => panic!("expected Reembed command"),
        }
    }

    #[test]
    fn cli_ingest_subcommand() {
        let cli = Cli::parse_from([
            "memory_mcp",
            "ingest",
            "--source-type",
            "email",
            "--source-id",
            "m-1",
            "--content",
            "hello",
            "--t-ref",
            "2026-06-30T10:00:00Z",
        ]);
        assert!(matches!(cli.command, Some(Command::Ingest(_))));
    }

    #[test]
    fn cli_extract_subcommand() {
        let cli = Cli::parse_from(["memory_mcp", "extract", "--episode-id", "ep:1"]);
        assert!(matches!(cli.command, Some(Command::Extract(_))));
    }

    #[test]
    fn cli_resolve_subcommand() {
        let cli = Cli::parse_from([
            "memory_mcp",
            "resolve",
            "--entity-type",
            "person",
            "--canonical-name",
            "Alice",
            "--aliases",
            "Ali",
        ]);
        assert!(matches!(cli.command, Some(Command::Resolve(_))));
    }

    #[test]
    fn cli_invalidate_subcommand() {
        let cli = Cli::parse_from([
            "memory_mcp",
            "invalidate",
            "--fact-id",
            "f:1",
            "--reason",
            "test",
            "--t-invalid",
            "2026-06-30T00:00:00Z",
        ]);
        assert!(matches!(cli.command, Some(Command::Invalidate(_))));
    }

    #[test]
    fn cli_explain_subcommand() {
        let cli = Cli::parse_from([
            "memory_mcp",
            "explain",
            "--context-items",
            r#"[{"fact_id":"f:1"}]"#,
        ]);
        assert!(matches!(cli.command, Some(Command::Explain(_))));
    }

    #[test]
    fn cli_assemble_context_subcommand() {
        let cli = Cli::parse_from(["memory_mcp", "assemble-context", "--query", "test"]);
        assert!(matches!(cli.command, Some(Command::AssembleContext(_))));
    }

    #[test]
    fn cli_lifecycle_capture_subcommand_parses() {
        let cli = Cli::parse_from([
            "memory_mcp",
            "lifecycle-capture",
            "--event",
            "{\"event_kind\":\"post_tool_result\",\"task_fingerprint\":\"task:1\",\"normalized_task\":\"do work\",\"scope\":\"org\"}",
            "--context",
            "{\"origin\":{\"kind\":\"lifecycle_adapter\",\"adapter_id\":\"claude_code\",\"adapter_version\":\"1\",\"host_event\":\"post_tool_result\"}}",
        ]);
        assert!(matches!(cli.command, Some(Command::LifecycleCapture(_))));
    }

    #[test]
    fn cli_lifecycle_recall_subcommand_parses() {
        let cli = Cli::parse_from([
            "memory_mcp",
            "lifecycle-recall",
            "--event",
            "{\"event_kind\":\"session_start\",\"task_fingerprint\":\"task:1\",\"normalized_task\":\"do work\",\"scope\":\"org\"}",
            "--context",
            "{\"origin\":{\"kind\":\"lifecycle_adapter\",\"adapter_id\":\"claude_code\",\"adapter_version\":\"1\",\"host_event\":\"session_start\"}}",
        ]);
        assert!(matches!(cli.command, Some(Command::LifecycleRecall(_))));
    }

    #[test]
    fn cli_lifecycle_subcommands_are_hidden_from_help() {
        // The hidden subcommands should not appear in --help output.
        let help = Cli::command().render_help();
        let help_str = help.to_string();
        assert!(
            !help_str.contains("lifecycle-capture"),
            "hidden subcommand leaked into --help"
        );
        assert!(
            !help_str.contains("lifecycle-recall"),
            "hidden subcommand leaked into --help"
        );
    }
}
