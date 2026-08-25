//! Process-level tests for feature-enabled filesystem ingestion inside
//! `serve`: MCP readiness with a valid inbox, watcher-driven ingestion,
//! responsiveness through corrupt files, and bounded shutdown on stdio close.

#![cfg(feature = "fs-watch")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_memory_mcp")
}

/// Spawns `memory_mcp serve` with an isolated data dir and inbox, returning a
/// driver with request/response helpers.
struct ServeDriver {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    responses: mpsc::Receiver<String>,
    stderr_lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    _temp: tempfile::TempDir,
}

impl ServeDriver {
    fn spawn(inbox: Option<&std::path::Path>) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let data_dir = temp.path().join("db");
        let mut command = Command::new(binary());
        command
            .env_clear()
            .arg("serve")
            .env("SURREALDB_EMBEDDED", "true")
            .env("SURREALDB_DATA_DIR", &data_dir)
            .env("SURREALDB_DB_NAME", "memory_fs_enabled")
            .env("SURREALDB_NAMESPACE", "org")
            .env("SURREALDB_USERNAME", "root")
            .env("SURREALDB_PASSWORD", "root")
            .env("EMBEDDINGS_ENABLED", "false")
            .env("NER_EXTRACTOR", "anno")
            .env("RUST_LOG", "warn")
            .env_remove("SURREALDB_URL");
        if let Some(inbox) = inbox {
            command.env("MEMORY_INGESTION_INBOX", inbox);
        } else {
            command.env_remove("MEMORY_INGESTION_INBOX");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start serve");
        let stdin = child.stdin.take().expect("stdin");
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
        Self {
            child,
            stdin,
            responses,
            stderr_lines,
            _temp: temp,
        }
    }

    fn request(&mut self, id: i64, method: &str, params: serde_json::Value) -> serde_json::Value {
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{message}").expect("write request");
        self.stdin.flush().expect("flush request");
        loop {
            let line = self
                .responses
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|error| panic!("timed out waiting for MCP response {id}: {error}"));
            let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON-RPC");
            if value.get("id").and_then(serde_json::Value::as_i64) == Some(id) {
                return value;
            }
        }
    }

    fn initialize(&mut self) {
        let response = self.request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "fs-watch-process-test", "version": "1.0.0"}
            }),
        );
        assert!(
            response.get("error").is_none(),
            "initialize must succeed: {response}"
        );
    }

    fn shutdown(mut self) {
        drop(self.stdin);
        let deadline = std::time::Instant::now() + Duration::from_secs(35);
        let mut exited = false;
        while std::time::Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(err) => panic!("failed to poll serve exit: {err}"),
            }
        }
        assert!(
            exited,
            "serve must exit within the bounded shutdown window"
        );
    }
}

#[test]
fn serve_reaches_readiness_and_ingests_dropped_file() {
    let inbox = tempfile::tempdir().expect("temp inbox");
    let mut driver = ServeDriver::spawn(Some(inbox.path()));
    driver.initialize();

    // Drop a supported file into the inbox; the watcher should eventually
    // produce an episode and facts.
    std::fs::write(
        inbox.path().join("note.md"),
        "Alice Smith reports ARR is $5M.",
    )
    .expect("write markdown");

    // Poll through the assemble-context tool until a fact from the dropped
    // file is queryable.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut found = false;
    let mut last_response = serde_json::Value::Null;
    while std::time::Instant::now() < deadline {
        last_response = driver.request(
            2,
            "tools/call",
            serde_json::json!({
                "name": "assemble_context",
                "arguments": {"query": "Alice Smith ARR", "budget": 10}
            }),
        );
        let result = last_response
            .pointer("/result/structuredContent/result")
            .and_then(|value| value.as_array())
            .map(Vec::len)
            .unwrap_or(0);
        if result > 0 {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let stderr_snapshot = self_stderr(&driver.stderr_lines);
    assert!(
        found,
        "expected the dropped file to be ingested and queryable; last response: {last_response}\nstderr:\n{stderr_snapshot}",
    );

    driver.shutdown();
}

fn self_stderr(lines: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> String {
    lines
        .lock()
        .expect("stderr lock")
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn corrupt_file_leaves_mcp_responsive() {
    let inbox = tempfile::tempdir().expect("temp inbox");
    let mut driver = ServeDriver::spawn(Some(inbox.path()));
    driver.initialize();

    // A corrupt supported-extension file must not terminate MCP.
    std::fs::write(inbox.path().join("corrupt.pdf"), b"not a real pdf").expect("write corrupt");

    // MCP remains responsive.
    let response = driver.request(
        3,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "source_type": "test",
                "source_id": "responsive-1",
                "content": "Alice Smith from OpenAI presented Project Atlas.",
                "t_ref": "2026-02-05T00:00:00Z"
            }
        }),
    );
    assert!(
        response.get("error").is_none(),
        "MCP must stay responsive after a corrupt file: {response}"
    );

    driver.shutdown();
}

#[test]
fn stdio_close_triggers_bounded_shutdown() {
    let inbox = tempfile::tempdir().expect("temp inbox");
    let mut driver = ServeDriver::spawn(Some(inbox.path()));
    driver.initialize();
    // Closing stdin triggers a bounded watcher shutdown.
    driver.shutdown();
}
