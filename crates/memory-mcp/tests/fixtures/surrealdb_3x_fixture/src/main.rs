use std::{env, error::Error};

use surrealdb::engine::local::{Db, RocksDb, SurrealKv};
use surrealdb::Surreal;

const NAMESPACE: &str = "probe";
const DATABASE: &str = "memory";

const SCHEMA: &str = "DEFINE TABLE tenant SCHEMAFULL; DEFINE TABLE known SCHEMALESS; DEFINE FIELD value ON known TYPE string; DEFINE FIELD occurred_at ON known TYPE datetime; DEFINE INDEX known_value ON known FIELDS value; CREATE known:1 SET value = 'surrealdb-3x-data-survives', occurred_at = d'2025-01-02T03:04:05Z'; CREATE known:2 SET value = 'second-record', occurred_at = d'2025-02-03T04:05:06Z';";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        return Err("usage: sdb3-fixture <rocksdb|surrealkv> <seed|verify|hold> <path>".into());
    }
    let engine = args[1].as_str();
    let mode = args[2].as_str();
    let path = args[3].as_str();

    match (engine, mode) {
        ("rocksdb", "seed") => seed(Surreal::new::<RocksDb>(path).await?).await?,
        ("surrealkv", "seed") => seed(Surreal::new::<SurrealKv>(path).await?).await?,
        ("rocksdb", "verify") => verify(Surreal::new::<RocksDb>(path).await?).await?,
        ("surrealkv", "verify") => verify(Surreal::new::<SurrealKv>(path).await?).await?,
        ("rocksdb", "hold") => hold(Surreal::new::<RocksDb>(path).await?).await?,
        ("surrealkv", "hold") => hold(Surreal::new::<SurrealKv>(path).await?).await?,
        _ => return Err(format!("unknown engine/mode: {engine}/{mode}").into()),
    }

    println!("{mode} passed for {engine} at {path}");
    Ok(())
}

async fn seed(db: Surreal<Db>) -> Result<(), Box<dyn Error>> {
    db.use_ns(NAMESPACE).use_db(DATABASE).await?;
    db.query(SCHEMA).await?.check()?;
    Ok(())
}

async fn hold(db: Surreal<Db>) -> Result<(), Box<dyn Error>> {
    let ready_file = env::var("SURREALDB3_FIXTURE_READY_FILE")?;
    std::fs::write(ready_file, "ready")?;
    let _db = db;
    loop {
        std::thread::park();
    }
}

async fn verify(db: Surreal<Db>) -> Result<(), Box<dyn Error>> {
    db.use_ns(NAMESPACE).use_db(DATABASE).await?;

    let mut response = db
        .query("RETURN (SELECT VALUE value FROM known:1)[0] = 'surrealdb-3x-data-survives'")
        .await?
        .check()?;
    let first_record: Vec<bool> = response.take(0)?;
    assert_eq!(first_record, [true], "known record was lost or changed");

    let mut response = db
        .query("RETURN (SELECT VALUE value FROM known:2)[0] = 'second-record'")
        .await?
        .check()?;
    let second_record: Vec<bool> = response.take(0)?;
    assert_eq!(second_record, [true], "second known record was lost or changed");

    let mut response = db.query("INFO FOR TABLE known").await?.check()?;
    let schema: Vec<serde_json::Value> = response.take(0)?;
    let schema_text = serde_json::to_string(&schema)?;
    assert!(schema_text.contains("known_value"), "managed index is missing: {schema_text}");
    assert!(schema_text.contains("occurred_at"), "field schema is missing: {schema_text}");
    Ok(())
}
