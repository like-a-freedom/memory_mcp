//! Offline migration, exact rollback, and recovery point on native SurrealKV.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use memory_mcp::{
    MemoryError,
    config::SurrealConfigBuilder,
    memory::fact_access_store::FactAccessStore,
    storage::{DbClient, SurrealDbClient},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const NAMESPACE: &str = "native_upgrade";
const DATABASE: &str = "memory";
const MIGRATION: &str = "script_migration:057_fact_access_isolation_surql";
const STAMP: &str = "2026-10-03T12:00:00Z";
const ROLLBACK_BATCH: &str = include_str!("../operations/fact_access_rollback_batch.surql");

async fn open_store(path: &Path) -> std::sync::Arc<SurrealDbClient> {
    let config = SurrealConfigBuilder::new()
        .embedded(true)
        .credentials("root", "root")
        .db_name(DATABASE)
        .namespace(NAMESPACE)
        .data_dir(path.to_str().expect("UTF-8 fixture path"))
        .build()
        .expect("disposable config");
    std::sync::Arc::new(
        SurrealDbClient::connect(&config)
            .await
            .expect("open SurrealKV"),
    )
}

async fn seed_fact(db: &SurrealDbClient, key: &str, count: i64, stamp: Option<&str>) {
    let id = format!("fact:{key}");
    db.create(
        &id,
        json!({
            "fact_id": id, "fact_type": "note", "content": "Original source payload",
            "quote": "Original source payload", "source_episode": format!("episode:{key}"),
            "t_valid": STAMP, "t_ingested": STAMP, "confidence": 0.9, "index_keys": [key],
            "entity_links": [], "scope": NAMESPACE, "policy_tags": [],
            "access_count": count, "last_accessed": stamp, "provenance": {"source": "manual"},
        }),
        NAMESPACE,
        &["t_valid", "t_ingested", "last_accessed"],
    )
    .await
    .expect("seed fact in existing namespace");
}

async fn raw_record(db: &SurrealDbClient, id: &str) -> Value {
    db.select_one(id, NAMESPACE)
        .await
        .expect("read record")
        .expect("record exists")
}

fn cold_copy(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let copy = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            cold_copy(&entry.path(), &copy)?;
        } else {
            fs::copy(entry.path(), copy)?;
        }
    }
    Ok(())
}

fn file_hashes(root: &Path) -> io::Result<BTreeMap<PathBuf, String>> {
    fn visit(root: &Path, path: &Path, hashes: &mut BTreeMap<PathBuf, String>) -> io::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                visit(root, &path, hashes)?;
            } else {
                let relative = path.strip_prefix(root).expect("relative fixture path");
                let hash = hex::encode(Sha256::digest(fs::read(&path)?));
                hashes.insert(relative.to_owned(), hash);
            }
        }
        Ok(())
    }
    let mut hashes = BTreeMap::new();
    visit(root, root, &mut hashes)?;
    Ok(hashes)
}

async fn rollback_keys(db: &SurrealDbClient, after: Option<&str>) -> Vec<String> {
    let range = after.map_or("", |_| "WHERE id > type::record('fact_access', $after)");
    let query = format!(
        "SELECT id, record::id(id) AS fact_key FROM fact_access {range} ORDER BY id LIMIT 128"
    );
    let rows = db
        .query(&query, Some(json!({"after": after})), NAMESPACE)
        .await
        .expect("read bounded rollback key page");
    rows.as_array()
        .expect("rollback rows")
        .iter()
        .map(|row| {
            row["fact_key"]
                .as_str()
                .expect("string fact key")
                .to_owned()
        })
        .collect()
}

async fn rollback_batch(db: &SurrealDbClient, keys: &[String]) -> Result<(), MemoryError> {
    db.query(
        ROLLBACK_BATCH,
        Some(json!({"fact_access_rollback_keys": keys})),
        NAMESPACE,
    )
    .await
    .map(|_| ())
}

#[tokio::test]
async fn native_upgrade_reconciles_exact_heat_and_restores_verified_recovery_point() {
    let dir = tempfile::tempdir().expect("disposable store directory");
    let working = dir.path().join("working");
    let backup = dir.path().join("backup");
    let restored = dir.path().join("restored");

    let db = open_store(&working).await;
    db.apply_migrations(NAMESPACE)
        .await
        .expect("install fixture schema");
    seed_fact(&db, "hot", -9, Some(STAMP)).await;
    seed_fact(&db, "timestamp_only", 0, Some(STAMP)).await;
    seed_fact(&db, "new_heat", 0, None).await;
    seed_fact(&db, "untouched", 0, None).await;
    for n in 0..127 {
        seed_fact(&db, &format!("row_{n:03}"), 1, None).await;
    }
    let before = raw_record(&db, "fact:hot").await;
    let timestamp_only = raw_record(&db, "fact:timestamp_only").await;
    let new_heat_before = raw_record(&db, "fact:new_heat").await;
    let untouched = raw_record(&db, "fact:untouched").await;
    // Remove only the 057 bookkeeping to simulate a pre-057 fixture. Its
    // additive empty schema is left present to exercise idempotent DDL.
    db.query(&format!("DELETE {MIGRATION}"), None, NAMESPACE)
        .await
        .expect("mark legacy fixture");
    assert!(rollback_keys(&db, None).await.is_empty());
    db.shutdown().await.expect("close legacy store for backup");
    drop(db);

    cold_copy(&working, &backup).expect("copy closed store");
    let baseline = file_hashes(&backup).expect("hash backup files");
    assert!(!baseline.is_empty());
    assert_eq!(file_hashes(&working).unwrap(), baseline);

    let db = open_store(&working).await;
    db.apply_migrations(NAMESPACE)
        .await
        .expect("upgrade reopened store");
    assert_eq!(raw_record(&db, MIGRATION).await["status"], "applied");
    assert_eq!(raw_record(&db, "fact:hot").await, before);
    assert_eq!(raw_record(&db, "fact:timestamp_only").await, timestamp_only);
    let migrated = raw_record(&db, "fact_access:hot").await;
    assert_eq!(migrated["source_episode"], "episode:hot");
    assert_eq!(migrated["access_count"], -9);
    assert_eq!(migrated["last_accessed"], STAMP);
    let zero = raw_record(&db, "fact_access:timestamp_only").await;
    assert_eq!(zero["access_count"], 0);
    assert_eq!(zero["last_accessed"], STAMP);
    assert!(
        db.select_one("fact_access:untouched", NAMESPACE)
            .await
            .unwrap()
            .is_none()
    );

    let writer = FactAccessStore::new(db.clone(), NAMESPACE);
    writer
        .record_fact_access("fact:hot", 3)
        .await
        .expect("increment migrated heat");
    writer
        .record_fact_access("fact:new_heat", 4)
        .await
        .expect("write first access after the cutover");
    let latest_hot = raw_record(&db, "fact_access:hot").await;
    let latest_new = raw_record(&db, "fact_access:new_heat").await;
    assert_eq!(latest_hot["access_count"], -6);
    assert_eq!(latest_new["access_count"], 4);
    assert_ne!(latest_hot["last_accessed"], STAMP);
    drop(writer);
    db.shutdown()
        .await
        .expect("close upgraded store and writer");
    drop(db);

    // The writer and its store handle are closed before the operator reopens
    // the datastore alone. No application session exists during rollback.
    let operator = open_store(&working).await;
    assert_eq!(raw_record(&operator, "fact_access:hot").await, latest_hot);
    assert_eq!(
        raw_record(&operator, "fact_access:new_heat").await,
        latest_new
    );
    assert_eq!(raw_record(&operator, "fact:hot").await, before);
    assert_eq!(
        raw_record(&operator, "fact:new_heat").await,
        new_heat_before
    );

    let first = rollback_keys(&operator, None).await;
    assert_eq!(first.len(), 128);
    rollback_batch(&operator, &first)
        .await
        .expect("commit first rollback page");
    rollback_batch(&operator, &first)
        .await
        .expect("repeat exact copy after uncertain receipt");
    let last = rollback_keys(&operator, first.last().map(String::as_str)).await;
    assert_eq!(last.len(), 2);
    rollback_batch(&operator, &last)
        .await
        .expect("commit final rollback page");
    assert!(
        rollback_keys(&operator, last.last().map(String::as_str))
            .await
            .is_empty()
    );
    operator.shutdown().await.expect("flush reconciled store");
    drop(operator);

    let rollback = open_store(&working).await;
    let mut expected = before.clone();
    expected["access_count"] = json!(-6);
    expected["last_accessed"] = latest_hot["last_accessed"].clone();
    assert_eq!(raw_record(&rollback, "fact:hot").await, expected);
    let mut expected_new = new_heat_before.clone();
    expected_new["access_count"] = json!(4);
    expected_new["last_accessed"] = latest_new["last_accessed"].clone();
    assert_eq!(raw_record(&rollback, "fact:new_heat").await, expected_new);
    assert_eq!(
        raw_record(&rollback, "fact:timestamp_only").await,
        timestamp_only
    );
    assert_eq!(raw_record(&rollback, "fact:untouched").await, untouched);
    rollback.shutdown().await.expect("close reconciled store");
    drop(rollback);

    assert_eq!(file_hashes(&backup).unwrap(), baseline);
    cold_copy(&backup, &restored).expect("restore into new directory");
    assert_eq!(file_hashes(&restored).unwrap(), baseline);
    let recovery = open_store(&restored).await;
    assert!(
        recovery
            .select_one(MIGRATION, NAMESPACE)
            .await
            .unwrap()
            .is_none()
    );
    assert!(rollback_keys(&recovery, None).await.is_empty());
    assert_eq!(raw_record(&recovery, "fact:hot").await, before);
    assert_eq!(
        raw_record(&recovery, "fact:new_heat").await,
        new_heat_before
    );
    assert_eq!(
        raw_record(&recovery, "fact:timestamp_only").await,
        timestamp_only
    );
    assert_eq!(raw_record(&recovery, "fact:untouched").await, untouched);
    recovery.shutdown().await.expect("close restored store");
}

#[tokio::test]
async fn rollback_aborts_the_whole_page_for_an_inconsistent_fact_identity() {
    let dir = tempfile::tempdir().expect("disposable store directory");
    let db = open_store(&dir.path().join("working")).await;
    db.apply_migrations(NAMESPACE)
        .await
        .expect("configure fixture");
    seed_fact(&db, "valid", 0, None).await;
    let writer = FactAccessStore::new(db.clone(), NAMESPACE);
    writer
        .record_fact_access("fact:valid", 4)
        .await
        .expect("write heat");
    drop(writer);
    db.create(
        "fact_access:orphan",
        json!({"source_episode":"episode:orphan","access_count":5}),
        NAMESPACE,
        &[],
    )
    .await
    .expect("simulate inconsistent side row");

    let result = rollback_batch(&db, &["valid".to_owned(), "orphan".to_owned()]).await;

    assert!(result.is_err());
    assert_eq!(raw_record(&db, "fact:valid").await["access_count"], 0);
    assert_eq!(
        raw_record(&db, "fact_access:valid").await["access_count"],
        4
    );
    db.shutdown().await.expect("close store");
}

#[tokio::test]
async fn rollback_rejects_a_page_larger_than_128_keys_before_copying_heat() {
    let dir = tempfile::tempdir().expect("disposable store directory");
    let db = open_store(&dir.path().join("working")).await;
    db.apply_migrations(NAMESPACE)
        .await
        .expect("configure fixture");
    seed_fact(&db, "valid", 0, None).await;
    let writer = FactAccessStore::new(db.clone(), NAMESPACE);
    writer
        .record_fact_access("fact:valid", 4)
        .await
        .expect("write heat");
    drop(writer);
    let too_many = vec!["valid".to_owned(); 129];

    let result = rollback_batch(&db, &too_many).await;

    assert!(result.is_err());
    assert_eq!(raw_record(&db, "fact:valid").await["access_count"], 0);
    db.shutdown().await.expect("close store");
}
