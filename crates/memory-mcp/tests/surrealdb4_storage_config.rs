#![cfg(feature = "streamable-http")]

use memory_mcp::config::SurrealTargetConfig;
use memory_mcp::error::MemoryError;
use memory_mcp::http::registry::{PrivilegedEngine, StoreHealth, SurrealRegistryStore};

fn target(url: String) -> SurrealTargetConfig {
    SurrealTargetConfig {
        url,
        username: "root".into(),
        password: "root".into(),
        database: "registry".into(),
        namespace: "registry".into(),
    }
}

#[tokio::test]
async fn registry_connects_to_a_surreal_kv_path() {
    let directory = tempfile::tempdir().expect("temporary SurrealKV directory");
    let target = target(format!("surrealkv://{}", directory.path().display()));

    SurrealRegistryStore::connect(&target)
        .await
        .expect("registry should open the SurrealKV store");
}

#[tokio::test]
async fn legacy_rocksdb_registry_alias_connects_to_a_local_store() {
    let directory = tempfile::tempdir().expect("temporary SurrealKV directory");
    let target = target(format!("rocksdb://{}", directory.path().display()));

    let store = SurrealRegistryStore::connect(&target)
        .await
        .expect("legacy alias should open a SurrealKV registry store");

    assert!(matches!(
        store.privileged_engine(),
        PrivilegedEngine::Local(_)
    ));
    assert!(store.ping().await);
}

#[tokio::test]
async fn registry_rejects_an_empty_surrealkv_path_before_connecting() {
    let error = match SurrealRegistryStore::connect(&target("surrealkv://".into())).await {
        Ok(_) => panic!("empty SurrealKV path must fail before opening an engine"),
        Err(error) => error,
    };

    assert!(matches!(error, MemoryError::Validation(_)));
    assert!(error.to_string().contains("path is empty"));
}

#[tokio::test]
async fn registry_rejects_an_empty_legacy_rocksdb_path_before_connecting() {
    let error = match SurrealRegistryStore::connect(&target("rocksdb://".into())).await {
        Ok(_) => panic!("empty legacy path must fail before opening an engine"),
        Err(error) => error,
    };

    assert!(matches!(error, MemoryError::Validation(_)));
    assert!(error.to_string().contains("path is empty"));
}
