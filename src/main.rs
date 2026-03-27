use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

use memory_mcp::{MemoryMcp, MemoryService};
use memory_mcp::logging::{LogLevel, StdoutLogger};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let logger = StdoutLogger::new("info");
    
    let startup_ts = chrono::Utc::now();
    let mut startup_event = std::collections::HashMap::new();
    startup_event.insert("op".to_string(), serde_json::json!("main.startup"));
    startup_event.insert("pid".to_string(), serde_json::json!(std::process::id()));
    logger.log(startup_event, LogLevel::Info);

    let memory_service = match MemoryService::new_from_env().await {
        Ok(service) => service,
        Err(err) => {
            let mut error_event = std::collections::HashMap::new();
            error_event.insert("op".to_string(), serde_json::json!("main.startup_failed"));
            error_event.insert("error".to_string(), serde_json::json!(err.to_string()));
            logger.log(error_event, LogLevel::Error);
            return Err(Box::new(err) as Box<dyn std::error::Error>);
        }
    };
    
    let server = MemoryMcp::new(memory_service);

    let schema = schemars::schema_for!(memory_mcp::mcp::AssembleContextParams);
    if let Ok(json) = serde_json::to_string_pretty(&schema) {
        let _ = std::fs::write("/tmp/assemble_context_schema.json", json);
    }

    let mut serve_event = std::collections::HashMap::new();
    serve_event.insert("op".to_string(), serde_json::json!("main.serve_starting"));
    logger.log(serve_event, LogLevel::Info);

    let (stdin, stdout) = stdio();
    let service = match server.serve((stdin, stdout)).await {
        Ok(s) => s,
        Err(err) => {
            let mut error_event = std::collections::HashMap::new();
            error_event.insert("op".to_string(), serde_json::json!("main.serve_failed"));
            error_event.insert("error".to_string(), serde_json::json!(err.to_string()));
            logger.log(error_event, LogLevel::Error);
            return Err(Box::new(err) as Box<dyn std::error::Error>);
        }
    };
    
    let mut running_event = std::collections::HashMap::new();
    running_event.insert("op".to_string(), serde_json::json!("main.running"));
    logger.log(running_event, LogLevel::Info);

    match service.waiting().await {
        Ok(_quit_reason) => {
            let mut shutdown_event = std::collections::HashMap::new();
            shutdown_event.insert("op".to_string(), serde_json::json!("main.shutdown"));
            logger.log(shutdown_event, LogLevel::Info);
        }
        Err(err) => {
            let mut error_event = std::collections::HashMap::new();
            error_event.insert("op".to_string(), serde_json::json!("main.error"));
            error_event.insert("error".to_string(), serde_json::json!(err.to_string()));
            logger.log(error_event, LogLevel::Error);
            return Err(Box::new(err) as Box<dyn std::error::Error>);
        }
    }

    let duration = chrono::Utc::now().signed_duration_since(startup_ts);
    let mut duration_event = std::collections::HashMap::new();
    duration_event.insert("op".to_string(), serde_json::json!("main.session_duration"));
    duration_event.insert("duration_secs".to_string(), serde_json::json!(duration.num_seconds()));
    logger.log(duration_event, LogLevel::Info);

    Ok(())
}
