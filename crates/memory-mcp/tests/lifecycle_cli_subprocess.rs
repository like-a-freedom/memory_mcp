//! Subprocess coverage for public lifecycle CLI behavior.
//!
//! These scenarios exercise the real exit-code, structured-output and
//! environment-adapter paths. Each child has an owned data/home directory and
//! a hard execution deadline.

use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const LIFECYCLE_CONTEXT: &str = r#"{"origin":{"kind":"lifecycle_adapter","adapter_id":"test","adapter_version":"1","host_event":"session_start"}}"#;
const SCOPE_FREE_EVENT: &str =
    r#"{"event_kind":"session_start","task_fingerprint":"task:1","normalized_task":"do work"}"#;

fn run_cli(temp_dir: &TempDir, args: &[&str]) -> Output {
    let home = temp_dir.path().join("home");
    let data_dir = temp_dir.path().join("data");
    let current_dir = temp_dir.path().join("cwd");
    std::fs::create_dir_all(&home).expect("create isolated HOME");
    std::fs::create_dir_all(&data_dir).expect("create isolated data directory");
    std::fs::create_dir_all(&current_dir).expect("create isolated current directory");
    let profile_dir = TempDir::new().expect("owned profile output directory");

    let mut command = Command::new(env!("CARGO_BIN_EXE_memory_mcp"));
    command
        .env_clear()
        .env("HOME", &home)
        .env("XDG_DATA_HOME", temp_dir.path().join("xdg"))
        .env(
            "LLVM_PROFILE_FILE",
            profile_dir.path().join("cli-%p-%m.profraw"),
        )
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", &data_dir)
        .env("SURREALDB_NAMESPACE", "main")
        .env("SURREALDB_DB_NAME", "memory")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("LIFECYCLE_ENABLED", "false")
        .env("RUST_LOG", "error")
        .current_dir(current_dir)
        .args(args);
    bounded_output(&mut command)
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

#[test]
fn hidden_lifecycle_cli_accepts_a_scope_free_event() {
    let temp_dir = tempfile::tempdir().expect("owned lifecycle fixture");
    let output = run_cli(
        &temp_dir,
        &[
            "lifecycle-recall",
            "--event",
            SCOPE_FREE_EVENT,
            "--context",
            LIFECYCLE_CONTEXT,
        ],
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON response");
    assert_eq!(result["status"], "disabled");
}

#[test]
fn hidden_lifecycle_cli_rejects_a_legacy_scope_field() {
    let temp_dir = tempfile::tempdir().expect("owned lifecycle fixture");
    let event = r#"{"event_kind":"session_start","task_fingerprint":"task:1","normalized_task":"do work","scope":"org"}"#;
    let output = run_cli(
        &temp_dir,
        &[
            "lifecycle-recall",
            "--event",
            event,
            "--context",
            LIFECYCLE_CONTEXT,
        ],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("Validation"));
    assert!(stderr.contains("scope"));
}

#[test]
fn hidden_lifecycle_cli_rejects_a_legacy_project_field() {
    let temp_dir = tempfile::tempdir().expect("owned lifecycle fixture");
    let event = r#"{"event_kind":"session_start","task_fingerprint":"task:1","normalized_task":"do work","project":"legacy"}"#;
    let output = run_cli(
        &temp_dir,
        &[
            "lifecycle-recall",
            "--event",
            event,
            "--context",
            LIFECYCLE_CONTEXT,
        ],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("Validation"));
    assert!(stderr.contains("project"));
}

#[test]
fn lifecycle_dashboard_returns_the_public_operation_envelope() {
    let temp_dir = tempfile::tempdir().expect("owned lifecycle fixture");
    let output = run_cli(&temp_dir, &["lifecycle", "dashboard"]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON response");
    assert_eq!(result["operation"], "dashboard");
    assert!(result["result"].is_object());
    assert_eq!(result.as_object().map(serde_json::Map::len), Some(2));
}

#[test]
fn lifecycle_mutation_without_confirmation_fails_at_the_public_command() {
    let temp_dir = tempfile::tempdir().expect("owned lifecycle fixture");
    let output = run_cli(&temp_dir, &["lifecycle", "recompute-decay"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("recompute_decay requires `confirmed=true`"));
}
