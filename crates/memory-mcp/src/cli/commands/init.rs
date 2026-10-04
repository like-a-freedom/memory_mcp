//! Output-only host setup command.

use crate::cli::args::InitArgs;
use crate::cli::commands::write_response;
use crate::error::MemoryError;

const NEXT_STEP: &str = "Copy the snippet into the indicated host configuration, start the host, then ingest and extract one source before assembling context.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitTarget {
    Vscode,
    ClaudeDesktop,
    Codex,
    Zed,
    Env,
}

fn parse_target(raw: &str) -> Result<InitTarget, MemoryError> {
    match raw {
        "vscode" => Ok(InitTarget::Vscode),
        "claude-desktop" => Ok(InitTarget::ClaudeDesktop),
        "codex" => Ok(InitTarget::Codex),
        "zed" => Ok(InitTarget::Zed),
        "env" => Ok(InitTarget::Env),
        _ => Err(MemoryError::Validation(format!(
            "unsupported init target `{raw}`; choose vscode, claude-desktop, codex, zed, or env"
        ))),
    }
}

fn render(target: InitTarget) -> Result<serde_json::Value, MemoryError> {
    let (target_name, format, path, snippet) = match target {
        InitTarget::Vscode => (
            "vscode",
            "json",
            ".vscode/mcp.json",
            serde_json::to_string(&serde_json::json!({
                "servers": {
                    "memory_mcp": {
                        "type": "stdio",
                        "command": "memory_mcp",
                        "args": [],
                    }
                }
            }))
            .map_err(|err| MemoryError::Transient(err.to_string()))?,
        ),
        InitTarget::ClaudeDesktop => (
            "claude-desktop",
            "json",
            "Claude Desktop config file (platform-specific)",
            serde_json::to_string(&serde_json::json!({
                "mcpServers": {
                    "memory_mcp": {
                        "command": "memory_mcp",
                        "args": [],
                    }
                }
            }))
            .map_err(|err| MemoryError::Transient(err.to_string()))?,
        ),
        InitTarget::Codex => (
            "codex",
            "toml",
            "~/.codex/config.toml",
            "[mcp_servers.memory_mcp]\ncommand = \"memory_mcp\"\nargs = []\n".to_string(),
        ),
        InitTarget::Zed => (
            "zed",
            "json",
            "Zed settings.json",
            serde_json::to_string(&serde_json::json!({
                "context_servers": {
                    "memory_mcp": {
                        "command": "memory_mcp",
                        "args": [],
                    }
                }
            }))
            .map_err(|err| MemoryError::Transient(err.to_string()))?,
        ),
        InitTarget::Env => (
            "env",
            "shell",
            "shell profile or .env",
            "# Embedded zero-config mode requires no environment variables.\n# Optional remote configuration; omit these for embedded zero-config mode.\n# export SURREALDB_URL=ws://localhost:8000\n# export SURREALDB_DB_NAME=memory\n# export SURREALDB_NAMESPACE=work\n# export SURREALDB_USERNAME=<your-remote-username>\n# export SURREALDB_PASSWORD=<your-remote-password>\n"
                .to_string(),
        ),
    };

    Ok(serde_json::json!({
        "target": target_name,
        "format": format,
        "path": path,
        "mutates_files": false,
        "snippet": snippet,
        "next": NEXT_STEP,
        "guidance": [
            "Optional filesystem ingestion: set MEMORY_INGESTION_INBOX to an existing absolute directory; omit it to keep filesystem ingestion disabled.",
            "Each embedded stdio Memory MCP process must use a unique SURREALDB_DATA_DIR; changing only the database name or namespace does not avoid the directory lock."
        ],
    }))
}

/// Runs the host setup command without building a service or touching storage.
pub fn run(args: InitArgs) -> Result<(), MemoryError> {
    let target = parse_target(&args.target)?;
    let value = render(target)?;
    write_response(&value).map_err(|err| MemoryError::Transient(err.to_string()))
}
