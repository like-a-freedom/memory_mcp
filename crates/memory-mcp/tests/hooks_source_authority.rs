#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn fake_stdio_server(temp: &TempDir) -> PathBuf {
    let script = temp.path().join("fake-memory-server.py");
    std::fs::write(
        &script,
        r##"#!/usr/bin/env python3
import json
import os
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")
    if method == "initialize":
        result = {"serverInfo": {"name": "source-authority-test", "version": "1"}}
    elif method == "tools/call":
        with open(os.environ["MEMORY_SOURCE_AUTHORITY_CAPTURE"], "w", encoding="utf-8") as handle:
            json.dump(request["params"]["arguments"], handle)
        result = {"structuredContent": {"status": "success", "result": "episode:test"}}
    else:
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)
"##,
    )
    .expect("write fake stdio server");
    let mut permissions = std::fs::metadata(&script)
        .expect("server metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("make fake server executable");
    script
}

fn run_stop_hook(content: &str) -> Value {
    let temp = TempDir::new().expect("create hook test directory");
    let capture = temp.path().join("ingest-arguments.json");
    let fake_server = fake_stdio_server(&temp);
    let server_command = format!("python3 '{}'", fake_server.display());
    let hook = repository_root().join("hooks/memory_stop_hook.sh");

    let output = Command::new("bash")
        .arg(hook)
        .current_dir(repository_root())
        .env("MEMORY_MCP_SERVER_CMD", server_command)
        .env("MEMORY_MCP_SERVER_CWD", repository_root())
        .env("MEMORY_SOURCE_AUTHORITY_CAPTURE", &capture)
        .env("MEMORY_HOOK_CONTENT", content)
        .output()
        .expect("run actual stop hook script");
    assert!(
        output.status.success(),
        "stop hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&std::fs::read(capture).expect("captured ingest arguments"))
        .expect("captured arguments are JSON")
}

#[test]
fn stop_hook_sends_an_existing_path_as_literal_ingest_content() {
    let source = TempDir::new().expect("create source directory");
    let path = source.path().join("private-source-canary.txt");
    let secret_file_contents = "the hook must not read this local file";
    std::fs::write(&path, secret_file_contents).expect("write local source file");
    let literal_path = path.to_string_lossy().into_owned();

    let arguments = run_stop_hook(&literal_path);

    let captured_content = arguments["content"].as_str().expect("content field");
    assert!(captured_content.ends_with(&literal_path));
    assert!(!captured_content.contains(secret_file_contents));
    assert_eq!(arguments["source_type"], "session_summary");
}

#[test]
fn stop_hook_sends_a_loopback_url_as_literal_ingest_content() {
    let literal_url = "http://127.0.0.1:9/agent-hook-reference";

    let arguments = run_stop_hook(literal_url);

    let captured_content = arguments["content"].as_str().expect("content field");
    assert!(captured_content.ends_with(literal_url));
    assert_eq!(arguments["source_type"], "session_summary");
}
