use std::collections::HashMap;
use std::path::PathBuf;

use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

use memory_mcp::logging::{LogLevel, StdoutLogger};
use memory_mcp::{MemoryMcp, MemoryService};

const DEFAULT_WATCH_INTERVAL_SECS: u64 = 2;

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
    match MemoryService::new_from_env().await {
        Ok(service) => Ok(service),
        Err(err) => {
            let mut error_event = HashMap::new();
            error_event.insert("op".to_string(), serde_json::json!("main.startup_failed"));
            error_event.insert("error".to_string(), serde_json::json!(err.to_string()));
            logger.log(error_event, LogLevel::Error);
            Err(Box::new(err) as Box<dyn std::error::Error>)
        }
    }
}

async fn run_stdio_server(logger: &StdoutLogger) -> Result<(), Box<dyn std::error::Error>> {
    let memory_service = build_memory_service(logger).await?;
    let server = MemoryMcp::new(memory_service);

    let mut serve_event = HashMap::new();
    serve_event.insert("op".to_string(), serde_json::json!("main.serve_starting"));
    logger.log(serve_event, LogLevel::Info);

    let (stdin, stdout) = stdio();
    let service = match server.serve((stdin, stdout)).await {
        Ok(s) => s,
        Err(err) => {
            let mut error_event = HashMap::new();
            error_event.insert("op".to_string(), serde_json::json!("main.serve_failed"));
            error_event.insert("error".to_string(), serde_json::json!(err.to_string()));
            logger.log(error_event, LogLevel::Error);
            return Err(Box::new(err) as Box<dyn std::error::Error>);
        }
    };

    let mut running_event = HashMap::new();
    running_event.insert("op".to_string(), serde_json::json!("main.running"));
    logger.log(running_event, LogLevel::Info);

    match service.waiting().await {
        Ok(_quit_reason) => {
            let mut shutdown_event = HashMap::new();
            shutdown_event.insert("op".to_string(), serde_json::json!("main.shutdown"));
            logger.log(shutdown_event, LogLevel::Info);
            Ok(())
        }
        Err(err) => {
            let mut error_event = HashMap::new();
            error_event.insert("op".to_string(), serde_json::json!("main.error"));
            error_event.insert("error".to_string(), serde_json::json!(err.to_string()));
            logger.log(error_event, LogLevel::Error);
            Err(Box::new(err) as Box<dyn std::error::Error>)
        }
    }
}

async fn run_watch_mode(
    logger: &StdoutLogger,
    watch: WatchCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut watch_event = HashMap::new();
    watch_event.insert("op".to_string(), serde_json::json!("main.watch_starting"));
    watch_event.insert(
        "dir".to_string(),
        serde_json::json!(watch.dir.display().to_string()),
    );
    watch_event.insert("scope".to_string(), serde_json::json!(watch.scope.clone()));
    watch_event.insert(
        "interval_secs".to_string(),
        serde_json::json!(watch.interval_secs),
    );
    if let Some(project) = watch.project.clone() {
        watch_event.insert("project".to_string(), serde_json::json!(project));
    }
    logger.log(watch_event, LogLevel::Info);

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
        .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
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
    let mut startup_event = HashMap::new();
    startup_event.insert("op".to_string(), serde_json::json!("main.startup"));
    startup_event.insert("pid".to_string(), serde_json::json!(std::process::id()));
    startup_event.insert(
        "mode".to_string(),
        serde_json::json!(match &run_mode {
            RunMode::Serve => "serve",
            RunMode::Watch(_) => "watch",
        }),
    );
    logger.log(startup_event, LogLevel::Info);

    match run_mode {
        RunMode::Serve => run_stdio_server(&logger).await?,
        RunMode::Watch(watch) => run_watch_mode(&logger, watch).await?,
    }

    let duration = chrono::Utc::now().signed_duration_since(startup_ts);
    let mut duration_event = HashMap::new();
    duration_event.insert("op".to_string(), serde_json::json!("main.session_duration"));
    duration_event.insert(
        "duration_secs".to_string(),
        serde_json::json!(duration.num_seconds()),
    );
    logger.log(duration_event, LogLevel::Info);

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
