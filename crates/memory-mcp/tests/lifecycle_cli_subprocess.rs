//! Subprocess coverage for the hidden lifecycle CLI transport.
//!
//! These tests exercise the real exit-code/error-envelope path, not only the
//! command functions. Legacy event partition fields must fail before any
//! lifecycle capability is invoked.

use std::process::Command;

use tempfile::TempDir;

fn run_lifecycle_recall(temp_dir: &TempDir, event: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_memory_mcp"))
        .env_clear()
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", temp_dir.path())
        .env("SURREALDB_NAMESPACE", "main")
        .env("SURREALDB_DB_NAME", "memory")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("LIFECYCLE_ENABLED", "false")
        .env("RUST_LOG", "error")
        .args([
            "lifecycle-recall",
            "--event",
            event,
            "--context",
            r#"{"origin":{"kind":"lifecycle_adapter","adapter_id":"test","adapter_version":"1","host_event":"session_start"}}"#,
        ])
        .output()
        .expect("run hidden lifecycle CLI")
}

#[test]
fn hidden_lifecycle_cli_accepts_scope_free_event() {
    let temp_dir = tempfile::tempdir().expect("temporary lifecycle data directory");
    let output = run_lifecycle_recall(
        &temp_dir,
        r#"{"event_kind":"session_start","task_fingerprint":"task:1","normalized_task":"do work"}"#,
    );

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("disabled"));
}

#[test]
fn hidden_lifecycle_cli_rejects_legacy_event_partition_fields() {
    for legacy_field in ["scope", "project"] {
        let temp_dir = tempfile::tempdir().expect("temporary lifecycle data directory");
        let event = if legacy_field == "scope" {
            r#"{"event_kind":"session_start","task_fingerprint":"task:1","normalized_task":"do work","scope":"org"}"#
        } else {
            r#"{"event_kind":"session_start","task_fingerprint":"task:1","normalized_task":"do work","project":"legacy"}"#
        };
        let output = run_lifecycle_recall(&temp_dir, event);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(output.status.code(), Some(2), "{legacy_field}: {stderr}");
        assert!(stderr.contains("Validation"), "{legacy_field}: {stderr}");
        assert!(stderr.contains(legacy_field), "{legacy_field}: {stderr}");
    }
}
