use std::env;
use std::fs;
use std::process::Command;

use memory_mcp::config::SurrealConfigBuilder;
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;
use tempfile::TempDir;

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    /// Applies a clean zero-config environment: embedded SurrealDB in `data_dir`
    /// with every NER-related variable cleared.
    fn zero_config_embedded(data_dir: &std::path::Path) -> Self {
        const NER_KEYS: &[&str] = &[
            "NER_EXTRACTOR",
            "NER_PROVIDER",
            "NER_MODEL",
            "NER_CACHE_DIR",
            "NER_LABELS",
            "NER_THRESHOLD",
            "NER_MAX_CONCURRENCY",
            "NER_IDLE_UNLOAD_SECS",
            "GLINER_BATCH_SIZE",
            "GLINER_MAX_BATCH_TOKENS",
            "GLINER_DEVICE",
            "EMBEDDINGS_ENABLED",
            "EMBEDDINGS_PROVIDER",
            "EMBEDDINGS_MODEL",
            "EMBEDDINGS_MODEL_DIR",
            "EMBEDDINGS_BASE_URL",
            "EMBEDDINGS_API_KEY",
            "EMBEDDINGS_SIMILARITY_THRESHOLD",
            "SURREALDB_URL",
        ];
        let mut saved = Vec::new();
        for (key, value) in [
            ("SURREALDB_DB_NAME", "memory_zero_config"),
            ("SURREALDB_EMBEDDED", "true"),
            (
                "SURREALDB_DATA_DIR",
                data_dir.to_str().expect("utf8 data dir"),
            ),
            ("SURREALDB_NAMESPACES", "org"),
            ("SURREALDB_USERNAME", "root"),
            ("SURREALDB_PASSWORD", "root"),
            ("RUST_LOG", "warn"),
        ] {
            let saved_value = env::var(key).ok();
            unsafe { env::set_var(key, value) };
            saved.push((key.to_string(), saved_value));
        }
        for key in NER_KEYS {
            let saved_value = env::var(key).ok();
            unsafe { env::remove_var(key) };
            saved.push((key.to_string(), saved_value));
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.iter().rev() {
            unsafe {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
    }
}

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

/// Zero-configuration contract: with no NER environment variables set, the
/// service selects lightweight Anno, creates no model cache, and performs a
/// full ingest/extract/recall round trip offline.
#[tokio::test]
async fn zero_config_anno_creates_no_model_cache_and_round_trips() {
    let temp_dir = TempDir::new().expect("temporary data directory");
    let data_dir = temp_dir.path().join("data");
    let _env = EnvGuard::zero_config_embedded(&data_dir);

    let service = memory_mcp::MemoryService::new_from_env()
        .await
        .expect("service bootstraps with zero-config defaults");

    // Lightweight Anno is selected and no model artifacts are created.
    assert!(
        !data_dir.join("models").join("ner").exists(),
        "zero-config startup must not create a model cache under <data>/models/ner"
    );

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        memory_mcp::models::IngestRequest {
            source_type: "test".to_string(),
            source_id: "zero-config-1".to_string(),
            content: "Alice Smith presented Project Atlas at OpenAI".to_string(),
            t_ref: chrono::Utc::now(),
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: Vec::new(),
        },
        None,
    )
    .await
    .expect("ingest succeeds");

    let extracted = ExtractCapability::extract(&service.build_context(), &episode_id, None, None)
        .await
        .expect("extract succeeds");
    assert!(
        extracted.entities.iter().any(|entity| {
            entity.canonical_name.contains("Alice")
                || entity.canonical_name.contains("OpenAI")
                || entity.canonical_name.contains("Atlas")
        }),
        "expected extracted entities, got {:?}",
        extracted.entities
    );

    let recalled = AssembleContextCapability::assemble_context(
        &service.build_context(),
        memory_mcp::models::AssembleContextRequest {
            query: "Who presented Project Atlas?".to_string(),
            scope: "org".to_string(),
            project: None,
            fact_types: Vec::new(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: true,
        },
    )
    .await
    .expect("recall succeeds");
    assert!(
        !recalled.is_empty(),
        "zero-config recall must return stored context"
    );
    assert!(
        !data_dir.join("models").join("ner").exists(),
        "extraction must not create a model cache with the lightweight extractor"
    );
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
