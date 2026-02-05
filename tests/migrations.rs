use serde_json::Value;
use tempfile::tempdir;

use memory_mcp::config::SurrealConfig;
use memory_mcp::storage::SurrealDbClient;

fn index_names(info: &Value) -> Vec<String> {
    match info.get("indexes") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn apply_migrations_creates_episode_indexes() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir
        .path()
        .join("surreal-root")
        .to_str()
        .unwrap()
        .to_string();

    let cfg = SurrealConfig {
        db_name: "testdb".to_string(),
        url: None,
        namespaces: vec!["org".to_string()],
        username: "root".to_string(),
        password: "root".to_string(),
        log_level: "debug".to_string(),
        embedded: true,
        data_dir: Some(db_path),
    };

    let client = SurrealDbClient::connect(&cfg, "org")
        .await
        .expect("connect");

    client
        .apply_migrations("org")
        .await
        .expect("apply migrations");

    // Idempotency check
    client
        .apply_migrations("org")
        .await
        .expect("apply migrations again");

    let info: Value = client
        .query_get("INFO FOR TABLE episode", "org")
        .await
        .expect("info for table episode");

    let indexes = index_names(&info);
    assert!(
        indexes.iter().any(|name| name == "episode_source_id"),
        "expected episode_source_id index after migrations"
    );
}
