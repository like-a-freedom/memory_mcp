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
