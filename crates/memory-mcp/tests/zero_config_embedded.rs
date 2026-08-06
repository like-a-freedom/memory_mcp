use std::fs;
use std::process::Command;

use memory_mcp::config::SurrealConfigBuilder;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn embedded_rocksdb_root_root_round_trip() {
    let temp_dir = TempDir::new().expect("temporary RocksDB directory");
    let config = SurrealConfigBuilder::new()
        .db_name("memory")
        .namespace("org")
        .credentials("root", "root")
        .embedded(true)
        .data_dir(temp_dir.path().display().to_string())
        .build()
        .expect("valid embedded config");

    let client = SurrealDbClient::connect(&config, "org")
        .await
        .expect("embedded RocksDB connection with root/root");
    client
        .create("zero_config_smoke", json!({"value": "ok"}), "org")
        .await
        .expect("create record");
    let record = client
        .select_one("zero_config_smoke", "org")
        .await
        .expect("select record")
        .expect("record exists");

    assert_eq!(record["value"], "ok");
}

#[test]
fn legacy_data_dir_subprocess_emits_startup_event() {
    let test_dir = TempDir::new().expect("temporary subprocess directory");
    let executable_dir = test_dir.path().join("bin");
    fs::create_dir_all(&executable_dir).expect("create executable directory");
    let copied_executable = executable_dir.join("memory_mcp");
    let source_executable = std::path::Path::new(env!("CARGO_BIN_EXE_memory_mcp"));
    fs::copy(source_executable, &copied_executable).expect("copy test executable");
    fs::set_permissions(
        &copied_executable,
        fs::metadata(source_executable)
            .expect("read source executable metadata")
            .permissions(),
    )
    .expect("copy executable permissions");

    let legacy_data_dir = executable_dir.join("data").join("surrealdb");
    fs::create_dir_all(&legacy_data_dir).expect("create legacy data directory");
    let xdg_data_home = test_dir.path().join("xdg");
    fs::create_dir_all(&xdg_data_home).expect("create isolated XDG directory");
    let new_data_dir = xdg_data_home.join("memory_mcp");

    let output = Command::new(&copied_executable)
        .env_clear()
        .env("XDG_DATA_HOME", &xdg_data_home)
        .env("RUST_LOG", "info")
        .args([
            "ingest",
            "--source-type",
            "note",
            "--source-id",
            "legacy-subprocess",
            "--content",
            "legacy data directory compatibility",
            "--t-ref",
            "2026-08-06T00:00:00Z",
        ])
        .output()
        .expect("run copied executable");

    assert!(
        output.status.success(),
        "ingest failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("config.legacy_data_dir_detected"),
        "missing legacy event in stderr: {stderr}"
    );
    let legacy_path = legacy_data_dir.to_string_lossy().into_owned();
    assert!(
        stderr.contains(&legacy_path),
        "legacy event did not identify selected path: {stderr}"
    );
    assert!(
        !new_data_dir.exists(),
        "compatibility startup must not create the new path while legacy path is selected"
    );
}
