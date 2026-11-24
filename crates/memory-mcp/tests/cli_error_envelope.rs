use std::io::{Seek, Write};
use std::process::Command;

#[test]
fn startup_config_failure_uses_the_same_hint_envelope() {
    let output = Command::new(env!("CARGO_BIN_EXE_memory_mcp"))
        .env_clear()
        .env("SURREALDB_EMBEDDED", "false")
        .args([
            "ingest",
            "--source-type",
            "note",
            "--source-id",
            "test",
            "--content",
            "x",
            "--t-ref",
            "2026-08-04T00:00:00Z",
        ])
        .output()
        .expect("run binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ConfigMissing"), "stderr: {stderr}");
    assert!(stderr.contains("memory_mcp init"), "stderr: {stderr}");
}

fn ingest_stdin(content: &[u8]) -> std::process::Output {
    let store = tempfile::tempdir().expect("temporary store");
    let mut input = tempfile::tempfile().expect("temporary stdin");
    input.write_all(content).expect("write test content");
    input.rewind().expect("rewind test content");

    Command::new(env!("CARGO_BIN_EXE_memory_mcp"))
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", store.path())
        .env_remove("SURREALDB_URL")
        .env("EMBEDDINGS_ENABLED", "false")
        .args([
            "ingest",
            "--source-type",
            "other",
            "--source-id",
            "stdin-regression",
            "--t-ref",
            "2026-01-01T00:00:00Z",
            "--content-stdin",
        ])
        .stdin(input)
        .output()
        .expect("run ingest CLI")
}

#[test]
fn ingest_stdin_accepts_a_body_the_size_of_the_longest_longmemeval_trajectory() {
    let output = ingest_stdin(&vec![b'x'; 7_600_069]);

    assert!(
        output.status.success(),
        "{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("tool envelope");
    assert!(
        response["result"]
            .as_str()
            .is_some_and(|id| id.starts_with("episode:"))
    );
}

#[test]
fn ingest_stdin_rejects_content_over_eight_mib() {
    let output = ingest_stdin(&vec![b'x'; 8 * 1024 * 1024 + 1]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("stdin episode content exceeds 8 MiB")
    );
}

#[test]
fn ingest_stdin_rejects_invalid_utf8() {
    let output = ingest_stdin(&[0xff]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("stdin episode content must be UTF-8")
    );
}
