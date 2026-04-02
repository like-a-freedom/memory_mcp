use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

use memory_mcp::logging::{LogLevel, StdoutLogger};
use memory_mcp::{MemoryMcp, MemoryService, log_error, log_event};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let logger = StdoutLogger::new("info");

    let startup_ts = chrono::Utc::now();
    logger.log(
        log_event!("main.startup", "success", "pid" => std::process::id()),
        LogLevel::Info,
    );

    let memory_service = match MemoryService::new_from_env().await {
        Ok(service) => service,
        Err(err) => {
            logger.log(log_error!("main.startup_failed", &err), LogLevel::Error);
            return Err(Box::new(err) as Box<dyn std::error::Error>);
        }
    };

    let server = MemoryMcp::new(memory_service);

    logger.log(log_event!("main.serve_starting", "success"), LogLevel::Info);

    let (stdin, stdout) = stdio();
    let service = match server.serve((stdin, stdout)).await {
        Ok(s) => s,
        Err(err) => {
            logger.log(log_error!("main.serve_failed", &err), LogLevel::Error);
            return Err(Box::new(err) as Box<dyn std::error::Error>);
        }
    };

    logger.log(log_event!("main.running", "success"), LogLevel::Info);

    match service.waiting().await {
        Ok(_quit_reason) => {
            logger.log(log_event!("main.shutdown", "success"), LogLevel::Info);
        }
        Err(err) => {
            logger.log(log_error!("main.error", &err), LogLevel::Error);
            return Err(Box::new(err) as Box<dyn std::error::Error>);
        }
    }

    let duration = chrono::Utc::now().signed_duration_since(startup_ts);
    logger.log(
        log_event!("main.session_duration", "success", "duration_secs" => duration.num_seconds()),
        LogLevel::Info,
    );

    Ok(())
}
