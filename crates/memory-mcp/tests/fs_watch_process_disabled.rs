//! Process-level test: a binary compiled without the `fs-watch` feature must
//! reject a configured `MEMORY_INGESTION_INBOX` with an actionable startup
//! error, and must start `serve` exactly as before when the variable is absent.

#![cfg(not(feature = "fs-watch"))]

use std::process::{Command, Stdio};
use std::time::Duration;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_memory_mcp")
}

/// Configures one stdio serve process with the given inbox env and waits for
/// its exit, returning stdout + stderr.
fn run_serve_with_inbox(inbox: Option<&std::path::Path>) -> std::process::Output {
    let data_dir = tempfile::tempdir().expect("data dir");
    let mut command = Command::new(binary());
    command
        .env_clear()
        .arg("serve")
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", data_dir.path())
        .env("SURREALDB_DB_NAME", "memory_fs_disabled")
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
    command
        .stdin(Stdio::null())
        .output()
        .expect("run serve process")
}

#[test]
fn configured_inbox_fails_with_actionable_error_without_feature() {
    // This test file is compiled without `fs-watch`; a configured inbox must be
    // rejected before MCP readiness.
    let inbox = tempfile::tempdir().expect("temp inbox");
    let output = run_serve_with_inbox(Some(inbox.path()));

    assert!(
        !output.status.success(),
        "a configured inbox without the fs-watch feature must fail startup"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("without the fs-watch feature"),
        "startup error must be actionable, got: {stderr}"
    );
}

#[test]
fn absent_inbox_starts_serve_normally_without_feature() {
    // Absent env must not reference `notify` or alter startup: the process
    // blocks waiting on stdio, so we only verify it starts and stays alive.
    let data_dir = tempfile::tempdir().expect("data dir");
    let mut child = Command::new(binary())
        .env_clear()
        .arg("serve")
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", data_dir.path())
        .env("SURREALDB_DB_NAME", "memory_fs_absent")
        .env("SURREALDB_NAMESPACE", "org")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("EMBEDDINGS_ENABLED", "false")
        .env("NER_EXTRACTOR", "anno")
        .env("RUST_LOG", "warn")
        .env_remove("SURREALDB_URL")
        .env_remove("MEMORY_INGESTION_INBOX")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start serve");

    // Give the process time to either start serving or fail fast.
    std::thread::sleep(Duration::from_millis(1500));
    match child.try_wait() {
        Ok(Some(status)) => panic!("serve exited unexpectedly with {status}"),
        Ok(None) => {
            // Still running: close stdin to trigger a clean shutdown.
            drop(child.stdin.take());
        }
        Err(err) => panic!("failed to poll serve process: {err}"),
    }

    let output = child.wait_with_output().expect("wait for serve exit");
    // A clean shutdown after stdin close is success (exit 0), or the process
    // may already be shutting down; either way it must not crash.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("fs-watch"),
        "absent inbox must not mention fs-watch: {stderr}"
    );
}
