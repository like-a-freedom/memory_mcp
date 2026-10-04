use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

type TreeSnapshot = BTreeMap<String, Vec<u8>>;

fn snapshot_tree(root: &Path) -> TreeSnapshot {
    fn visit(root: &Path, path: &Path, snapshot: &mut TreeSnapshot) {
        let metadata = fs::symlink_metadata(path).expect("read snapshot metadata");
        let relative = path
            .strip_prefix(root)
            .expect("snapshot path must be under root");
        let key = relative.to_string_lossy().into_owned();

        if metadata.is_dir() {
            if !key.is_empty() {
                snapshot.insert(format!("{key}/"), Vec::new());
            }
            for entry in fs::read_dir(path).expect("read snapshot directory") {
                visit(root, &entry.expect("read snapshot entry").path(), snapshot);
            }
        } else {
            snapshot.insert(key, fs::read(path).expect("read snapshot file"));
        }
    }

    let mut snapshot = TreeSnapshot::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn run_cli(args: &[&str]) -> (TempDir, Output) {
    let temp_dir = TempDir::new().expect("owned CLI fixture directory");
    let home = temp_dir.path().join("home");
    let xdg_data_home = temp_dir.path().join("xdg");
    let current_dir = temp_dir.path().join("cwd");
    fs::create_dir_all(&home).expect("create isolated HOME");
    fs::create_dir_all(&xdg_data_home).expect("create isolated XDG data directory");
    fs::create_dir_all(&current_dir).expect("create isolated current directory");
    fs::write(current_dir.join("sentinel.txt"), b"do not change").expect("create sentinel");
    let before = snapshot_tree(temp_dir.path());
    let profile_dir = TempDir::new().expect("owned profile output directory");

    let mut command = Command::new(env!("CARGO_BIN_EXE_memory_mcp"));
    command
        .env_clear()
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &xdg_data_home)
        .env(
            "LLVM_PROFILE_FILE",
            profile_dir.path().join("init-%p-%m.profraw"),
        )
        .current_dir(&current_dir)
        .args(args);
    let output = bounded_output(&mut command);

    assert_eq!(snapshot_tree(temp_dir.path()), before);
    assert!(
        !xdg_data_home.join("memory_mcp").exists(),
        "init must not initialize the embedded data directory"
    );
    (temp_dir, output)
}

fn bounded_output(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn memory_mcp CLI");
    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().expect("collect CLI output"),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                child.kill().expect("kill timed-out CLI child");
                let output = child
                    .wait_with_output()
                    .expect("collect timed-out CLI output");
                panic!(
                    "CLI exceeded its 30-second limit: stdout={} stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("wait for CLI child: {error}");
            }
        }
    }
}

fn run_target(target: &str) -> (TempDir, Value) {
    let (temp_dir, output) = run_cli(&["init", "--target", target]);
    assert!(
        output.status.success(),
        "init target {target} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("one JSON result");
    assert_eq!(value["target"], target);
    assert_eq!(value["mutates_files"], false);
    assert!(value["snippet"].as_str().is_some());
    let guidance = value["guidance"]
        .as_array()
        .expect("guidance is an array")
        .iter()
        .map(|item| item.as_str().expect("guidance item is a string"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(guidance.contains("Optional filesystem ingestion"));
    assert!(guidance.contains("MEMORY_INGESTION_INBOX"));
    assert!(guidance.contains("existing absolute directory"));
    assert!(guidance.contains("unique SURREALDB_DATA_DIR"));
    assert!(guidance.contains("directory lock"));
    assert!(
        !value["snippet"]
            .as_str()
            .expect("snippet is a string")
            .contains("MEMORY_INGESTION_INBOX")
    );
    (temp_dir, value)
}

#[test]
fn init_prints_vscode_configuration_through_the_public_command() {
    let (_temp_dir, value) = run_target("vscode");
    let snippet: Value = serde_json::from_str(value["snippet"].as_str().expect("snippet string"))
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
fn init_prints_claude_desktop_configuration_through_the_public_command() {
    let (_temp_dir, value) = run_target("claude-desktop");
    let snippet: Value = serde_json::from_str(value["snippet"].as_str().expect("snippet string"))
        .expect("Claude snippet is JSON");

    assert_eq!(value["format"], "json");
    assert_eq!(snippet["mcpServers"]["memory_mcp"]["command"], "memory_mcp");
}

#[test]
fn init_prints_codex_configuration_through_the_public_command() {
    let (_temp_dir, value) = run_target("codex");
    let snippet = value["snippet"].as_str().expect("snippet string");

    assert_eq!(value["format"], "toml");
    assert!(snippet.contains("[mcp_servers.memory_mcp]"));
    assert!(snippet.contains("command = \"memory_mcp\""));
    assert!(snippet.contains("args = []"));
}

#[test]
fn init_prints_zed_configuration_through_the_public_command() {
    let (_temp_dir, value) = run_target("zed");
    let snippet: Value = serde_json::from_str(value["snippet"].as_str().expect("snippet string"))
        .expect("Zed snippet is JSON");

    assert_eq!(value["format"], "json");
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
fn init_prints_a_secret_free_environment_snippet_through_the_public_command() {
    let (_temp_dir, value) = run_target("env");
    let snippet = value["snippet"].as_str().expect("snippet string");

    assert_eq!(value["format"], "shell");
    assert!(snippet.contains("embedded zero-config"));
    assert!(snippet.contains("SURREALDB_USERNAME"));
    assert!(snippet.contains("SURREALDB_NAMESPACE=work"));
    assert!(!snippet.contains("SURREALDB_NAMESPACE=org"));
    assert!(!snippet.contains("root"));
    assert!(!snippet.contains("secret"));
}

#[test]
fn init_rejects_an_unsupported_target_through_the_public_command() {
    let (_temp_dir, output) = run_cli(&["init", "--target", "cursor"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported init target `cursor`"));
    assert!(stderr.contains("vscode, claude-desktop, codex, zed, or env"));
}
