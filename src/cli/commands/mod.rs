//! CLI command handlers — thin adapters that build `*Params` from clap `*Args`,
//! delegate to `crate::tools::*`, and print `ToolResponse<T>` as JSON.

use std::io::Write;

/// Write a tool response as pretty JSON to stdout, followed by a trailing newline.
pub(crate) fn write_response<T: serde::Serialize>(response: &T) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, response)?;
    writeln!(handle)?;
    Ok(())
}
