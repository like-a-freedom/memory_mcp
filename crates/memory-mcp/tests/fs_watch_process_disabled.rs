//! Process-level test: a binary compiled without the `fs-watch` feature must
//! reject a configured `MEMORY_INGESTION_INBOX` with an actionable startup
//! error, and must start `serve` exactly as before when the variable is absent.

#![cfg(not(feature = "fs-watch"))]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long any spawned `serve` process may live before it is killed. A test
/// binary that waits forever on a child hangs the whole CI job, so every wait
/// here is bounded and the child is reaped on expiry.
const SERVE_DEADLINE: Duration = Duration::from_secs(30);

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_memory_mcp")
}

fn serve_command(data_dir: &std::path::Path, db_name: &str) -> Command {
    let mut command = Command::new(binary());
    command
        .env_clear()
        .arg("serve")
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", data_dir)
        .env("SURREALDB_DB_NAME", db_name)
        .env("SURREALDB_NAMESPACE", "org")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("EMBEDDINGS_ENABLED", "false")
        .env("NER_EXTRACTOR", "anno")
        .env("RUST_LOG", "warn")
        .env_remove("SURREALDB_URL");
    command
}

/// Wait for `child` for at most `deadline`, killing and reaping it on expiry.
///
/// Returns the observed exit status, or `None` when the deadline expired. The
/// kill happens here rather than in the caller so no assertion message can
/// leave a live child behind holding the database directory.
fn wait_bounded(
    child: &mut std::process::Child,
    deadline: Duration,
) -> Option<std::process::ExitStatus> {
    let limit = Instant::now() + deadline;
    loop {
        if let Some(status) = poll_exit(child) {
            return Some(status);
        }
        if Instant::now() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether the child has already exited. Never blocks and never kills, so a
/// caller can sample liveness inside a longer startup window.
fn poll_exit(child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
    match child.try_wait() {
        Ok(status) => status,
        Err(err) => panic!("failed to poll serve process: {err}"),
    }
}

/// Configures one stdio serve process with the given inbox env, waits for its
/// exit within `SERVE_DEADLINE`, and returns its status and stderr.
fn run_serve_with_inbox(inbox: Option<&std::path::Path>) -> (std::process::ExitStatus, String) {
    let data_dir = tempfile::tempdir().expect("data dir");
    let mut command = serve_command(data_dir.path(), "memory_fs_disabled");
    if let Some(inbox) = inbox {
        command.env("MEMORY_INGESTION_INBOX", inbox);
    } else {
        command.env_remove("MEMORY_INGESTION_INBOX");
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve process");

    let status = wait_bounded(&mut child, SERVE_DEADLINE)
        .unwrap_or_else(|| panic!("serve did not exit within {SERVE_DEADLINE:?}"));
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stderr);
    }
    (status, stderr)
}

#[test]
fn configured_inbox_fails_with_actionable_error_without_feature() {
    // This test file is compiled without `fs-watch`; a configured inbox must be
    // rejected before MCP readiness.
    let inbox = tempfile::tempdir().expect("temp inbox");
    let (status, stderr) = run_serve_with_inbox(Some(inbox.path()));

    assert!(
        !status.success(),
        "a configured inbox without the fs-watch feature must fail startup; stderr: {stderr}"
    );
    assert!(
        stderr.contains("without the fs-watch feature"),
        "startup error must be actionable, got: {stderr}"
    );
}

#[test]
fn absent_inbox_starts_serve_normally_without_feature() {
    // Absent env must not reference `notify` or alter startup: the process
    // blocks waiting on stdio, so we only verify it starts and stays alive for
    // a bounded interval, then exits without reporting an fs-watch problem.
    let data_dir = tempfile::tempdir().expect("data dir");
    let mut child = serve_command(data_dir.path(), "memory_fs_absent")
        .env_remove("MEMORY_INGESTION_INBOX")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start serve");

    // Alive past the startup window (and not crashed) is the observation; the
    // deadline also bounds a cold CI runner rather than a fixed sleep alone.
    let startup_window = Duration::from_secs(10);
    let limit = Instant::now() + startup_window;
    loop {
        if let Some(status) = poll_exit(&mut child) {
            panic!("serve exited unexpectedly with {status}");
        }
        if Instant::now() >= limit {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Still running: close stdin to trigger a clean shutdown.
    drop(child.stdin.take());

    let status = wait_bounded(&mut child, SERVE_DEADLINE).unwrap_or_else(|| {
        panic!("serve did not exit within {SERVE_DEADLINE:?} after stdin closed")
    });
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        let _ = pipe.read_to_string(&mut stderr);
    }
    // A clean shutdown after stdin close is success (exit 0), or the process
    // may already be shutting down; either way it must not crash.
    assert!(
        !stderr.contains("fs-watch"),
        "absent inbox must not mention fs-watch (exit {status}): {stderr}"
    );
}
