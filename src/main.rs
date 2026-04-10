use std::collections::HashMap;
use std::path::PathBuf;

use rmcp::ServiceExt;
use rmcp::transport::io::stdio;
use serde_json::json;

use memory_mcp::logging::{LogLevel, StdoutLogger};
use memory_mcp::{MemoryMcp, MemoryService};

const DEFAULT_WATCH_INTERVAL_SECS: u64 = 2;

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
enum RunMode {
    Serve,
    Watch(WatchCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchCommand {
    dir: PathBuf,
    project: Option<String>,
    scope: String,
    interval_secs: u64,
}

fn parse_cli_args<I, S>(args: I) -> Result<RunMode, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();

    let Some(subcommand) = args.next() else {
        return Ok(RunMode::Serve);
    };

    if subcommand != "watch" {
        return Err(format!("unknown subcommand {subcommand}"));
    }

    let Some(dir) = args.next() else {
        return Err("watch requires <dir>".to_string());
    };

    let mut watch = WatchCommand {
        dir: PathBuf::from(dir),
        project: None,
        scope: "org".to_string(),
        interval_secs: DEFAULT_WATCH_INTERVAL_SECS,
    };

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--project" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--project requires a value".to_string())?;
                watch.project = Some(value);
            }
            "--scope" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--scope requires a value".to_string())?;
                watch.scope = value;
            }
            "--interval" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--interval requires a value".to_string())?;
                let interval_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --interval value {value}"))?;
                if interval_secs == 0 {
                    return Err("--interval must be greater than 0".to_string());
                }
                watch.interval_secs = interval_secs;
            }
            _ => return Err(format!("unknown watch flag {flag}")),
        }
    }

    Ok(RunMode::Watch(watch))
}

async fn build_memory_service(
    logger: &StdoutLogger,
) -> Result<MemoryService, Box<dyn std::error::Error>> {
    MemoryService::new_from_env()
        .await
        .map_err(|err| log_and_return_error(logger, "main.startup_failed", err))
}

async fn run_stdio_server(logger: &StdoutLogger) -> Result<(), Box<dyn std::error::Error>> {
    let memory_service = build_memory_service(logger).await?;
    let server = MemoryMcp::new(memory_service);

    logger.log(event!("op" => json!("main.serve_starting")), LogLevel::Info);

    let (stdin, stdout) = stdio();
    let service = server
        .serve((stdin, stdout))
        .await
        .map_err(|err| log_and_return_error(logger, "main.serve_failed", err))?;

    logger.log(event!("op" => json!("main.running")), LogLevel::Info);

    service
        .waiting()
        .await
        .map(|_quit_reason| {
            logger.log(event!("op" => json!("main.shutdown")), LogLevel::Info);
        })
        .map_err(|err| log_and_return_error(logger, "main.error", err))
}

async fn run_watch_mode(
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

    let memory_service = build_memory_service(logger).await?;

    #[cfg(feature = "cli-watch")]
    {
        memory_mcp::service::FsWatcher::run_with_interval(
            watch.dir,
            watch.project,
            watch.scope,
            watch.interval_secs,
            memory_service,
        )
        .await
        .map_err(|err| log_and_return_error(logger, "main.watch_failed", err))
    }

    #[cfg(not(feature = "cli-watch"))]
    {
        let _ = (watch, memory_service);
        Err(Box::new(std::io::Error::other(
            "watch subcommand requires the cli-watch feature",
        )) as Box<dyn std::error::Error>)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let logger = StdoutLogger::new("info");
    let run_mode = parse_cli_args(std::env::args())
        .map_err(|err| Box::new(std::io::Error::other(err)) as Box<dyn std::error::Error>)?;

    let startup_ts = chrono::Utc::now();
    let mode_label = match &run_mode {
        RunMode::Serve => "serve",
        RunMode::Watch(_) => "watch",
    };
    logger.log(
        event!(
            "op" => json!("main.startup"),
            "pid" => json!(std::process::id()),
            "mode" => json!(mode_label),
        ),
        LogLevel::Info,
    );

    match run_mode {
        RunMode::Serve => run_stdio_server(&logger).await?,
        RunMode::Watch(watch) => run_watch_mode(&logger, watch).await?,
    }

    let duration = chrono::Utc::now().signed_duration_since(startup_ts);
    logger.log(
        event!(
            "op" => json!("main.session_duration"),
            "duration_secs" => json!(duration.num_seconds()),
        ),
        LogLevel::Info,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_args_defaults_to_stdio_serve_mode() {
        let mode = parse_cli_args(["memory_mcp".to_string()]).expect("serve mode should parse");

        assert_eq!(mode, RunMode::Serve);
    }

    #[test]
    fn parse_cli_args_builds_watch_command_with_optional_flags() {
        let mode = parse_cli_args([
            "memory_mcp".to_string(),
            "watch".to_string(),
            "/tmp/inbox".to_string(),
            "--project".to_string(),
            "atlas".to_string(),
            "--scope".to_string(),
            "team".to_string(),
            "--interval".to_string(),
            "7".to_string(),
        ])
        .expect("watch mode should parse");

        assert_eq!(
            mode,
            RunMode::Watch(WatchCommand {
                dir: std::path::PathBuf::from("/tmp/inbox"),
                project: Some("atlas".to_string()),
                scope: "team".to_string(),
                interval_secs: 7,
            })
        );
    }

    #[test]
    fn parse_cli_args_rejects_missing_watch_directory() {
        let error = parse_cli_args(["memory_mcp".to_string(), "watch".to_string()])
            .expect_err("watch without directory should fail");

        assert!(error.contains("watch requires <dir>"));
    }

    #[test]
    fn parse_cli_args_rejects_unknown_watch_flag() {
        let error = parse_cli_args([
            "memory_mcp".to_string(),
            "watch".to_string(),
            "/tmp/inbox".to_string(),
            "--mystery".to_string(),
        ])
        .expect_err("unknown flag should fail");

        assert!(error.contains("unknown watch flag --mystery"));
    }
}
