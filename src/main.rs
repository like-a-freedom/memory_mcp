use memory_mcp::cli::{
    RunMode, log_session_duration, log_startup, parse_cli_args, run_stdio_server, run_watch_mode,
};
use memory_mcp::logging::StdoutLogger;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
    let logger = StdoutLogger::new(&log_level);
    let run_mode = parse_cli_args(std::env::args())
        .map_err(|err| Box::new(std::io::Error::other(err)) as Box<dyn std::error::Error>)?;

    let startup_ts = chrono::Utc::now();
    let mode_label = match &run_mode {
        RunMode::Serve => "serve",
        RunMode::Watch(_) => "watch",
    };
    log_startup(&logger, mode_label);

    match run_mode {
        RunMode::Serve => run_stdio_server(&logger).await?,
        RunMode::Watch(watch) => run_watch_mode(&logger, watch).await?,
    }

    let duration = chrono::Utc::now().signed_duration_since(startup_ts);
    log_session_duration(&logger, duration.num_seconds());

    Ok(())
}
