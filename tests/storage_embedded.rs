use tempfile::tempdir;

#[tokio::test]
async fn embedded_rocksdb_persistence() {
    use memory_mcp::config::SurrealConfig;
    use memory_mcp::storage::DbClient;
    use memory_mcp::storage::SurrealDbClient;

    let dir = tempdir().unwrap();
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
        data_dir: Some(db_path.clone()),
    };

    let client: SurrealDbClient = SurrealDbClient::connect(&cfg, "org")
        .await
        .expect("connect");
    client
        .query("CREATE person SET name = 'Alice';", None, "org")
        .await
        .expect("create");
    let names: Vec<String> = client
        .query_get("SELECT name FROM person WHERE name = 'Alice';", "org")
        .await
        .expect("select name");
    assert!(
        !names.is_empty(),
        "expected at least one record after create"
    );

    drop(client);

    // Allow some time for the embedded engine to release file locks on drop
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Reconnect to same path to verify persistence
    let cfg2 = SurrealConfig {
        data_dir: Some(db_path),
        ..cfg
    };
    let client2: SurrealDbClient = SurrealDbClient::connect(&cfg2, "org")
        .await
        .expect("reconnect");
    let names2: Vec<String> = client2
        .query_get("SELECT name FROM person WHERE name = 'Alice';", "org")
        .await
        .expect("select2");
    assert!(
        !names2.is_empty(),
        "expected persisted record after reconnect"
    );
}
