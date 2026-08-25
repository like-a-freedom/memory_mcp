//! Release-binary smoke test for the filesystem ingestion feature contract.
//!
//! Spawns the freshly built release artifact (path via the test-only
//! `MEMORY_RELEASE_BINARY` environment variable), writes a valid MCP
//! `initialize` request to stdin, reads the matching success response from
//! stdout, closes stdin, and requires bounded process shutdown. Uses an empty
//! inbox so no extraction model call occurs.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn release_binary() -> Option<std::path::PathBuf> {
    let raw = std::env::var_os("MEMORY_RELEASE_BINARY").map(std::path::PathBuf::from)?;
    if raw.is_absolute() {
        Some(raw)
    } else {
        // Cargo runs integration tests with the crate directory as cwd, so
        // relative artifact paths are resolved from the workspace root (two
        // levels up from the crate).
        Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join(raw),
        )
    }
}

#[test]
#[allow(clippy::zombie_processes)]
fn release_binary_serves_mcp_and_accepts_configured_inbox() {
    let Some(binary) = release_binary() else {
        eprintln!("skipping: MEMORY_RELEASE_BINARY not set");
        return;
    };
    let temp = tempfile::tempdir().expect("temp dir");
    let data_dir = temp.path().join("db");
    let inbox = temp.path().join("inbox");
    std::fs::create_dir_all(&inbox).expect("create inbox");

    let mut child = Command::new(binary)
        .arg("serve")
        .env_clear()
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", &data_dir)
        .env("SURREALDB_DB_NAME", "memory_fs_release")
        .env("SURREALDB_NAMESPACE", "org")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("EMBEDDINGS_ENABLED", "false")
        .env("NER_EXTRACTOR", "anno")
        .env("MEMORY_INGESTION_INBOX", &inbox)
        .env("RUST_LOG", "warn")
        .env_remove("SURREALDB_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start release binary");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");

    let (sender, responses) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = sender.send(line);
        }
    });
    let stderr_lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_capture = stderr_lines.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            stderr_capture.lock().expect("stderr lock").push(line);
        }
    });

    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "release-smoke", "version": "1.0.0"}
        }
    });
    writeln!(stdin, "{message}").expect("write initialize");
    stdin.flush().expect("flush initialize");

    let response = responses
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|error| panic!("timed out waiting for initialize response: {error}"));
    let value: serde_json::Value = serde_json::from_str(&response).expect("valid JSON-RPC");
    assert_eq!(
        value.get("id").and_then(serde_json::Value::as_i64),
        Some(1),
        "initialize must reply with matching id: {value}"
    );
    assert!(
        value.get("error").is_none(),
        "initialize must succeed: {value}"
    );

    // Close stdin and require bounded shutdown.
    drop(stdin);
    let deadline = std::time::Instant::now() + Duration::from_secs(35);
    let mut exited = false;
    while std::time::Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(err) => panic!("failed to poll release binary exit: {err}"),
        }
    }
    assert!(
        exited,
        "release binary must shut down within the bounded window"
    );

    let stderr = stderr_lines
        .lock()
        .expect("stderr lock")
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !stderr.contains("built without the fs-watch feature"),
        "release binary must include fs-watch: {stderr}"
    );
}
