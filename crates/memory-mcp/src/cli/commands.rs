//! CLI command handlers — thin adapters that build `*Params` from clap `*Args`,
//! delegate to `crate::tools::*`, and print `ToolResponse<T>` as JSON.

use std::io::Write;

pub mod assemble_context;
pub mod explain;
pub mod extract;
pub mod ingest;
pub mod init;
pub mod invalidate;
pub mod lifecycle_capture;
pub mod lifecycle_recall;
pub mod resolve;

pub use assemble_context::run as run_assemble_context;
pub use explain::run as run_explain;
pub use extract::run as run_extract;
pub use ingest::run as run_ingest;
pub use invalidate::run as run_invalidate;
pub use lifecycle_capture::run as run_lifecycle_capture;
pub use lifecycle_recall::run as run_lifecycle_recall;
pub use resolve::run as run_resolve;

/// Write a tool response as pretty JSON to stdout, followed by a trailing newline.
pub(crate) fn write_response<T: serde::Serialize>(response: &T) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, response)?;
    writeln!(handle)?;
    Ok(())
}
