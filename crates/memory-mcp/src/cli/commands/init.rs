//! Output-only host setup command.

use crate::cli::args::InitArgs;
use crate::cli::commands::write_response;
use crate::service::MemoryError;

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
    }))
}

/// Runs the host setup command without building a service or touching storage.
pub fn run(args: InitArgs) -> Result<(), MemoryError> {
    let target = parse_target(&args.target)?;
    let value = render(target)?;
    write_response(&value).map_err(|err| MemoryError::Transient(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_targets() {
        assert_eq!(
            parse_target("vscode").expect("vscode target"),
            InitTarget::Vscode
        );
        assert_eq!(
            parse_target("claude-desktop").expect("Claude target"),
            InitTarget::ClaudeDesktop
        );
        assert_eq!(
            parse_target("codex").expect("Codex target"),
            InitTarget::Codex
        );
        assert_eq!(parse_target("zed").expect("Zed target"), InitTarget::Zed);
        assert_eq!(parse_target("env").expect("env target"), InitTarget::Env);
    }

    #[test]
    fn rejects_unknown_target() {
        assert!(matches!(
            parse_target("cursor"),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn vscode_renderer_uses_current_mcp_json_schema() {
        let value = render(InitTarget::Vscode).expect("vscode snippet");
        let snippet: serde_json::Value =
            serde_json::from_str(value["snippet"].as_str().expect("snippet string"))
                .expect("VS Code snippet is JSON");

        assert_eq!(value["format"], "json");
        assert_eq!(value["path"], ".vscode/mcp.json");
        assert_eq!(snippet["servers"]["memory_mcp"]["type"], "stdio");
        assert_eq!(snippet["servers"]["memory_mcp"]["command"], "memory_mcp");
        assert_eq!(
            snippet["servers"]["memory_mcp"]["args"],
            serde_json::json!([])
        );
    }

    #[test]
    fn claude_renderer_uses_mcp_servers_schema() {
        let value = render(InitTarget::ClaudeDesktop).expect("Claude snippet");
        let snippet: serde_json::Value =
            serde_json::from_str(value["snippet"].as_str().expect("snippet string"))
                .expect("Claude snippet is JSON");

        assert_eq!(snippet["mcpServers"]["memory_mcp"]["command"], "memory_mcp");
    }

    #[test]
    fn codex_renderer_uses_toml_mcp_servers_table() {
        let value = render(InitTarget::Codex).expect("Codex snippet");
        let snippet = value["snippet"].as_str().expect("snippet string");

        assert!(snippet.contains("[mcp_servers.memory_mcp]"));
        assert!(snippet.contains("command = \"memory_mcp\""));
        assert!(snippet.contains("args = []"));
    }

    #[test]
    fn zed_renderer_uses_context_servers_schema() {
        let value = render(InitTarget::Zed).expect("Zed snippet");
        let snippet: serde_json::Value =
            serde_json::from_str(value["snippet"].as_str().expect("snippet string"))
                .expect("Zed snippet is JSON");

        assert_eq!(
            snippet["context_servers"]["memory_mcp"]["command"],
            "memory_mcp"
        );
        assert_eq!(
            snippet["context_servers"]["memory_mcp"]["args"],
            serde_json::json!([])
        );
    }

    #[test]
    fn env_renderer_is_shell_and_contains_no_secret() {
        let value = render(InitTarget::Env).expect("environment snippet");
        let shell = value["snippet"].as_str().expect("snippet string");

        assert_eq!(value["format"], "shell");
        assert!(shell.contains("embedded zero-config"));
        assert!(shell.contains("SURREALDB_USERNAME"));
        assert!(shell.contains("SURREALDB_NAMESPACE=work"));
        assert!(!shell.contains("SURREALDB_NAMESPACE=org"));
        assert!(!shell.contains("root"));
        assert!(!shell.contains("secret"));
    }

    #[test]
    fn every_renderer_is_non_mutating() {
        let targets = [
            InitTarget::Vscode,
            InitTarget::ClaudeDesktop,
            InitTarget::Codex,
            InitTarget::Zed,
            InitTarget::Env,
        ];

        for target in targets {
            let value = render(target).expect("renderer output");
            assert_eq!(value["mutates_files"], false);
        }
    }
}
