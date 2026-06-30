#[tokio::main]
async fn main() -> std::process::ExitCode {
    match memory_mcp::runner::run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(code) => code,
    }
}
