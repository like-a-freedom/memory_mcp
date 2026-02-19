use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

use memory_mcp::{MemoryMcp, MemoryService};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let memory_service = MemoryService::new_from_env().await?;
    let server = MemoryMcp::new(memory_service);

    // Dump assemble_context schema to /tmp/assemble_context_schema.json for debugging
    let schema = schemars::schema_for!(memory_mcp::mcp::AssembleContextParams);
    if let Ok(json) = serde_json::to_string_pretty(&schema) {
        let _ = std::fs::write("/tmp/assemble_context_schema.json", json);
    }

    let (stdin, stdout) = stdio();
    let service = server.serve((stdin, stdout)).await?;
    service.waiting().await?;
    Ok(())
}
