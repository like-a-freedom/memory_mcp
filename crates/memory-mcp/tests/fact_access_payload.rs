//! Access heat writes retain metadata without transporting the immutable source again.
mod common;
use async_trait::async_trait;
use memory_mcp::{
    MemoryError,
    memory::fact_access_store::FactAccessStore,
    storage::{DbClient, SurrealDbClient, table_scope::OwnedTable},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct MeasuredDb {
    inner: Arc<SurrealDbClient>,
    returned_bytes: AtomicUsize,
    query_count: AtomicUsize,
    request_bytes: AtomicUsize,
    elapsed_us: AtomicUsize,
    whole_record_reads: AtomicUsize,
}
#[async_trait]
impl DbClient for MeasuredDb {
    async fn query(&self, sql: &str, vars: Option<Value>, ns: &str) -> Result<Value, MemoryError> {
        self.query_count.fetch_add(1, Ordering::SeqCst);
        self.request_bytes.fetch_add(
            sql.len() + serde_json::to_vec(&vars).unwrap().len(),
            Ordering::SeqCst,
        );
        let started = std::time::Instant::now();
        let result = self.inner.query(sql, vars, ns).await?;
        self.elapsed_us
            .fetch_add(started.elapsed().as_micros() as usize, Ordering::SeqCst);
        self.returned_bytes
            .fetch_add(serde_json::to_vec(&result).unwrap().len(), Ordering::SeqCst);
        Ok(result)
    }
    async fn select_one(&self, id: &str, ns: &str) -> Result<Option<Value>, MemoryError> {
        self.whole_record_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.select_one(id, ns).await
    }
    async fn select_table(&self, table: OwnedTable, ns: &str) -> Result<Vec<Value>, MemoryError> {
        self.whole_record_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.select_table(table, ns).await
    }
    async fn create(
        &self,
        id: &str,
        data: Value,
        ns: &str,
        times: &[&str],
    ) -> Result<Value, MemoryError> {
        self.inner.create(id, data, ns, times).await
    }
    async fn update(
        &self,
        id: &str,
        data: Value,
        ns: &str,
        times: &[&str],
    ) -> Result<Value, MemoryError> {
        self.inner.update(id, data, ns, times).await
    }
    async fn apply_migrations(&self, ns: &str) -> Result<(), MemoryError> {
        self.inner.apply_migrations(ns).await
    }
}

#[tokio::test]
async fn access_write_does_not_return_the_large_source_afterimage() {
    let (_, db) = common::make_service_with_client().await;
    let content = "Falcon source details. ".repeat(10000);
    db.create("fact:access_payload", json!({"fact_id":"fact:access_payload", "fact_type":"note", "content":content, "quote":"Falcon source details.", "source_episode":"episode:access_payload", "t_valid":"2026-01-01T00:00:00Z", "t_ingested":"2026-01-01T00:00:00Z", "confidence":0.9,"index_keys":[],"entity_links":[],"scope":"org","policy_tags":[],"access_count":4,"provenance":{"source":"manual"}}), "org", &["t_valid", "t_ingested"]).await.unwrap();
    let measured = Arc::new(MeasuredDb {
        inner: db.clone(),
        returned_bytes: AtomicUsize::new(0),
        query_count: AtomicUsize::new(0),
        request_bytes: AtomicUsize::new(0),
        elapsed_us: AtomicUsize::new(0),
        whole_record_reads: AtomicUsize::new(0),
    });
    FactAccessStore::new(measured.clone(), "org")
        .record_fact_access("fact:access_payload", 1)
        .await
        .unwrap();
    let stored = db
        .select_one("fact:access_payload", "org")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["access_count"], 4);
    let stats = FactAccessStore::new(db.clone(), "org")
        .read_fact_accesses(&["fact:access_payload".into()])
        .await
        .unwrap();
    assert_eq!(stats["fact:access_payload"].access_count, 1);
    assert!(stats["fact:access_payload"].last_accessed.is_some());
    assert_eq!(stored["content"], content);
    let bytes = measured.returned_bytes.load(Ordering::SeqCst);
    println!("ACCESS_WRITE_RETURNED_BYTES {bytes}");
    assert!(
        bytes < 128,
        "heat-only write must not return the source body; observed {bytes} bytes"
    );
}

async fn characterize_write(label: &str, body_repetitions: usize, key_count: usize) {
    let (_, db) = common::make_service_with_client().await;
    let content = "Cobalt source observation. ".repeat(body_repetitions);
    let keys = (0..key_count)
        .map(|n| format!("cobalt-dimension-{n}"))
        .collect::<Vec<_>>();
    let key_bytes = keys.iter().map(String::len).sum::<usize>();
    let id = "fact:cost_probe";
    db.create(id, json!({"fact_id":id,"fact_type":"note","content":content,"quote":"Cobalt source observation.","source_episode":"episode:cost_probe","t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","confidence":0.9,"index_keys":keys,"entity_links":[],"scope":"org","policy_tags":[],"access_count":4,"provenance":{"source":"manual"}}), "org", &["t_valid","t_ingested"]).await.unwrap();
    let measured = Arc::new(MeasuredDb {
        inner: db.clone(),
        returned_bytes: AtomicUsize::new(0),
        query_count: AtomicUsize::new(0),
        request_bytes: AtomicUsize::new(0),
        elapsed_us: AtomicUsize::new(0),
        whole_record_reads: AtomicUsize::new(0),
    });
    let store = FactAccessStore::new(db.clone(), "org");
    store.record_fact_access(id, 4).await.unwrap();
    FactAccessStore::new(measured.clone(), "org")
        .record_fact_access(id, 1)
        .await
        .unwrap();
    let row = db.select_one(id, "org").await.unwrap().unwrap();
    assert_eq!(row["access_count"], 4);
    let stats = store.read_fact_accesses(&[id.into()]).await.unwrap();
    assert_eq!(stats[id].access_count, 5);
    assert!(stats[id].last_accessed.is_some());
    assert_eq!(row["content"], content);
    assert_eq!(row["index_keys"], json!(keys));
    assert_eq!(measured.query_count.load(Ordering::SeqCst), 1);
    assert_eq!(measured.whole_record_reads.load(Ordering::SeqCst), 0);
    assert!(measured.returned_bytes.load(Ordering::SeqCst) < 128);
    assert!(measured.request_bytes.load(Ordering::SeqCst) < 2048);
    println!(
        "ACCESS_COST {}",
        json!({"case":label,"fact_id":id,"content_bytes":content.len(),"index_keys":key_count,"index_key_bytes":key_bytes,"queries":measured.query_count.load(Ordering::SeqCst),"request_bytes":measured.request_bytes.load(Ordering::SeqCst),"returned_bytes":measured.returned_bytes.load(Ordering::SeqCst),"elapsed_us":measured.elapsed_us.load(Ordering::SeqCst),"statistics_verified":true})
    );
}

#[tokio::test]
async fn small_body_small_keys_access_cost() {
    characterize_write("small-body-small-keys", 1, 1).await;
}
#[tokio::test]
async fn large_body_small_keys_access_cost() {
    characterize_write("large-body-small-keys", 10000, 1).await;
}
#[tokio::test]
async fn small_body_large_keys_access_cost() {
    characterize_write("small-body-large-keys", 1, 8192).await;
}
#[tokio::test]
async fn large_body_large_keys_access_cost() {
    characterize_write("large-body-large-keys", 10000, 8192).await;
}

#[cfg(feature = "eval-support")]
mod isolation_prototype {
    use super::*;
    use memory_mcp::config::SurrealConfigBuilder;
    use std::{io::Write, path::Path, time::Instant};

    const NAMESPACE: &str = "main";
    const BODY: &str = "Cobalt source observation. ";
    const STAMP: &str = "2026-11-27T12:00:00Z";
    const PAIRS: usize = 20;
    const LOOKUP: &str = "SELECT source_episode, access_count, last_accessed \
        FROM type::record('fact_access_prototype', $fact_key)";
    const UPDATE: &str = "UPDATE type::record('fact_access_prototype', $fact_key) SET \
        access_count = IF $boost > 0 THEN \
            IF access_count > $int_max - $boost THEN $int_max ELSE access_count + $boost END \
        ELSE IF $boost < 0 THEN \
            IF access_count < $int_min - $boost THEN $int_min ELSE access_count + $boost END \
        ELSE access_count END, \
        last_accessed = type::datetime($stamp) RETURN NONE";

    fn emit(row: Value) {
        println!("HEAT_PROTOTYPE {row}");
        std::io::stdout().flush().expect("retain attempt receipt");
    }

    fn first_access_sql() -> String {
        format!(
            "BEGIN TRANSACTION; \
             LET $existing = SELECT VALUE id FROM type::record('fact_access_prototype', $fact_key); \
             IF array::len($existing) > 0 {{ {UPDATE}; }} ELSE {{ \
               LET $source = SELECT VALUE source_episode FROM type::record('fact', $fact_key); \
               IF array::len($source) > 0 {{ \
                 CREATE type::record('fact_access_prototype', $fact_key) \
                 SET source_episode = $source[0], access_count = $boost, \
                     last_accessed = type::datetime($stamp) RETURN NONE; \
               }}; \
             }}; \
             COMMIT TRANSACTION;"
        )
    }

    fn write_vars(key: &str, boost: i64) -> Option<Value> {
        Some(json!({
            "fact_key":key, "boost":boost, "int_min":i64::MIN, "int_max":i64::MAX, "stamp":STAMP
        }))
    }

    // Frozen legacy writer used as the comparison arm after production moved
    // heat off the fact row. The registered binary called that adapter directly.
    async fn legacy_write(db: &dyn DbClient, key: &str) -> Result<Value, MemoryError> {
        db.query(
            "UPDATE type::record('fact', $fact_id) SET \
                 access_count = IF access_count IS NONE OR access_count IS NULL THEN $boost \
                 ELSE IF $boost > 0 THEN \
                   IF access_count > $int_max - $boost THEN $int_max \
                   ELSE access_count + $boost END \
                 ELSE IF $boost < 0 THEN \
                   IF access_count < $int_min - $boost THEN $int_min \
                   ELSE access_count + $boost END \
                 ELSE access_count END, \
                 last_accessed = type::datetime($last_accessed) RETURN NONE",
            Some(json!({
                "fact_id": key,
                "boost": 1,
                "int_max": i64::MAX,
                "int_min": i64::MIN,
                "last_accessed": memory_mcp::service::normalize_dt(chrono::Utc::now()),
            })),
            NAMESPACE,
        )
        .await
    }

    async fn connect(path: &Path) -> Arc<SurrealDbClient> {
        let config = SurrealConfigBuilder::new()
            .embedded(true)
            .credentials("root", "root")
            .db_name("fact_access_prototype")
            .namespace(NAMESPACE)
            .data_dir(path.to_str().expect("UTF-8 temporary fixture path"))
            .build()
            .expect("build disposable configuration");
        Arc::new(
            SurrealDbClient::connect(&config)
                .await
                .expect("open SurrealKV"),
        )
    }

    fn measured(db: &Arc<SurrealDbClient>) -> Arc<MeasuredDb> {
        Arc::new(MeasuredDb {
            inner: Arc::clone(db),
            returned_bytes: 0.into(),
            query_count: 0.into(),
            request_bytes: 0.into(),
            elapsed_us: 0.into(),
            whole_record_reads: 0.into(),
        })
    }

    async fn seed(db: &SurrealDbClient, key: &str, count: i64, index_keys: &[String]) -> Value {
        let id = format!("fact:{key}");
        db.create(
            &id,
            json!({"fact_id":id,"fact_type":"note","content":BODY,"quote":"Cobalt source observation.",
                "source_episode":format!("episode:{key}"),"t_valid":"2026-01-01T00:00:00Z",
                "t_ingested":"2026-01-01T00:00:00Z","confidence":0.9,"index_keys":index_keys,
                "entity_links":[],"scope":"org","policy_tags":[],"access_count":count,
                "provenance":{"source":"manual"}}),
            NAMESPACE,
            &["t_valid", "t_ingested"],
        )
        .await
        .expect("seed immutable fact");
        fact(db, key).await
    }

    async fn fact(db: &SurrealDbClient, key: &str) -> Value {
        db.select_one(&format!("fact:{key}"), NAMESPACE)
            .await
            .expect("read fact for verification")
            .expect("fixture exists")
    }

    async fn hydrate(db: &dyn DbClient, key: &str) -> Value {
        db.query(LOOKUP, Some(json!({"fact_key":key})), NAMESPACE)
            .await
            .expect("project heat by exact record key")
    }

    fn assert_heat(rows: &Value, key: &str, count: i64) {
        let rows = rows.as_array().expect("heat projection returns rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["source_episode"], format!("episode:{key}"));
        assert_eq!(rows[0]["access_count"], count);
        let timestamp = rows[0]["last_accessed"].as_str().expect("heat timestamp");
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(timestamp).unwrap(),
            chrono::DateTime::parse_from_rfc3339(STAMP).unwrap()
        );
    }

    async fn check_first_access(db: &SurrealDbClient, first_sql: &str) -> Value {
        let key = "prototype_contract";
        let original = seed(db, key, 0, &[]).await;
        assert_eq!(hydrate(db, key).await, json!([]));
        db.query(first_sql, write_vars("prototype_absent", 1), NAMESPACE)
            .await
            .expect("absent fact is a no-op");
        assert_eq!(hydrate(db, "prototype_absent").await, json!([]));
        assert!(
            db.select_one("fact:prototype_absent", NAMESPACE)
                .await
                .unwrap()
                .is_none()
        );

        let (one, three) = tokio::join!(
            db.query(first_sql, write_vars(key, 1), NAMESPACE),
            db.query(first_sql, write_vars(key, 3), NAMESPACE)
        );
        one.expect("first access");
        three.expect("concurrent first access");
        assert_heat(&hydrate(db, key).await, key, 4);
        db.query(UPDATE, write_vars(key, i64::MAX), NAMESPACE)
            .await
            .unwrap();
        assert_heat(&hydrate(db, key).await, key, i64::MAX);
        db.query(UPDATE, write_vars(key, -i64::MAX), NAMESPACE)
            .await
            .unwrap();
        db.query(UPDATE, write_vars(key, i64::MIN), NAMESPACE)
            .await
            .unwrap();
        db.query(UPDATE, write_vars(key, -1), NAMESPACE)
            .await
            .unwrap();
        db.query(UPDATE, write_vars(key, 0), NAMESPACE)
            .await
            .unwrap();
        assert_heat(&hydrate(db, key).await, key, i64::MIN);
        assert_eq!(fact(db, key).await, original);
        original
    }

    fn report(arm: &str, pair: usize, key_count: usize, elapsed_us: u128, measured: &MeasuredDb) {
        assert_eq!(measured.whole_record_reads.load(Ordering::SeqCst), 0);
        emit(
            json!({"event":"complete", "arm":arm, "pair":pair, "index_keys":key_count,
            "wall_us":elapsed_us, "db_us":measured.elapsed_us.load(Ordering::SeqCst),
            "queries":measured.query_count.load(Ordering::SeqCst),
            "request_bytes":measured.request_bytes.load(Ordering::SeqCst),
            "returned_bytes":measured.returned_bytes.load(Ordering::SeqCst)}),
        );
    }

    async fn compare_key_count(db: &Arc<SurrealDbClient>, first_sql: &str, key_count: usize) {
        let keys = (0..key_count)
            .map(|n| format!("cobalt-dimension-{n}"))
            .collect::<Vec<_>>();
        let legacy = format!("legacy_{key_count}");
        let isolated = format!("isolated_{key_count}");
        let cold = format!("first_{key_count}");
        let mut legacy_original = seed(db, &legacy, 4, &keys).await;
        legacy_original
            .as_object_mut()
            .unwrap()
            .remove("last_accessed");
        let isolated_original = seed(db, &isolated, 0, &keys).await;
        let cold_original = seed(db, &cold, 0, &keys).await;
        db.query(first_sql, write_vars(&isolated, 4), NAMESPACE)
            .await
            .unwrap();
        for pair in 0..PAIRS {
            let arms = if pair % 2 == 0 {
                ["legacy", "isolated"]
            } else {
                ["isolated", "legacy"]
            };
            for arm in arms {
                let measured = measured(db);
                let vars = write_vars(&isolated, 1);
                emit(json!({"event":"start", "arm":arm, "pair":pair, "index_keys":key_count}));
                let started = Instant::now();
                if arm == "legacy" {
                    legacy_write(measured.as_ref(), &legacy)
                        .await
                        .expect("legacy heat write");
                    report(
                        arm,
                        pair,
                        key_count,
                        started.elapsed().as_micros(),
                        &measured,
                    );
                    assert_eq!(measured.query_count.load(Ordering::SeqCst), 1);
                } else {
                    let before = hydrate(measured.as_ref(), &isolated).await;
                    measured
                        .query(UPDATE, vars, NAMESPACE)
                        .await
                        .expect("isolated heat write");
                    report(
                        arm,
                        pair,
                        key_count,
                        started.elapsed().as_micros(),
                        &measured,
                    );
                    assert_heat(&before, &isolated, 4 + pair as i64);
                    assert_eq!(measured.query_count.load(Ordering::SeqCst), 2);
                }
            }

            let measured = measured(db);
            let vars = write_vars(&cold, 1);
            emit(
                json!({"event":"start", "arm":"first_access", "pair":pair, "index_keys":key_count}),
            );
            let started = Instant::now();
            measured
                .query(first_sql, vars, NAMESPACE)
                .await
                .expect("first-access transaction");
            report(
                "first_access",
                pair,
                key_count,
                started.elapsed().as_micros(),
                &measured,
            );
            assert_eq!(measured.query_count.load(Ordering::SeqCst), 1);
            assert_heat(&hydrate(db.as_ref(), &cold).await, &cold, 1);
            if pair + 1 < PAIRS {
                db.query(
                    "DELETE type::record('fact_access_prototype', $fact_key) RETURN NONE",
                    Some(json!({"fact_key":cold})),
                    NAMESPACE,
                )
                .await
                .expect("reset disposable side record");
            }
        }
        let mut legacy_after = fact(db, &legacy).await;
        assert_eq!(legacy_after["access_count"], 4 + PAIRS);
        assert!(
            legacy_after
                .as_object_mut()
                .unwrap()
                .remove("last_accessed")
                .unwrap()
                .is_string()
        );
        legacy_after["access_count"] = json!(4);
        assert_eq!(legacy_after, legacy_original);
        assert_eq!(fact(db, &isolated).await, isolated_original);
        assert_eq!(fact(db, &cold).await, cold_original);
    }

    async fn open_prototype(path: &Path) -> Arc<SurrealDbClient> {
        let db = connect(path).await;
        db.apply_migrations(NAMESPACE)
            .await
            .expect("existing fact schema");
        db.query("DEFINE TABLE fact_access_prototype SCHEMAFULL; \
            DEFINE FIELD source_episode ON fact_access_prototype TYPE string; \
            DEFINE FIELD access_count ON fact_access_prototype TYPE int; \
            DEFINE FIELD last_accessed ON fact_access_prototype TYPE option<datetime>; \
            DEFINE INDEX source_episode_recent ON fact_access_prototype FIELDS source_episode, last_accessed;",
            None, NAMESPACE).await.expect("define table only in disposable store");
        db
    }

    #[tokio::test]
    #[ignore = "deterministic contract of disposable prototype SQL"]
    async fn fact_access_prototype_contract() {
        let directory = tempfile::tempdir().expect("disposable SurrealKV store");
        let db = open_prototype(directory.path()).await;
        let original = check_first_access(db.as_ref(), &first_access_sql()).await;
        db.shutdown().await.expect("close the original engine");
        drop(db);
        let reopened = connect(directory.path()).await;
        assert_heat(
            &hydrate(reopened.as_ref(), "prototype_contract").await,
            "prototype_contract",
            i64::MIN,
        );
        assert_eq!(
            fact(reopened.as_ref(), "prototype_contract").await,
            original
        );
        reopened
            .shutdown()
            .await
            .expect("close the reopened engine");
    }

    #[tokio::test]
    #[ignore = "registered native SurrealKV comparison; use the fixed trial protocol"]
    async fn fact_access_storage_isolation_prototype() {
        assert_eq!(BODY.len(), 27);
        let directory = tempfile::tempdir().expect("disposable SurrealKV store");
        let db = open_prototype(directory.path()).await;
        let first_sql = first_access_sql();
        let contract_fact = check_first_access(db.as_ref(), &first_sql).await;
        emit(json!({"event":"contract_preflight", "status":"passed"}));
        compare_key_count(&db, &first_sql, 1).await;
        compare_key_count(&db, &first_sql, 8192).await;

        db.shutdown().await.expect("close the original engine");
        drop(db);
        let reopened = connect(directory.path()).await;
        assert_heat(
            &hydrate(reopened.as_ref(), "prototype_contract").await,
            "prototype_contract",
            i64::MIN,
        );
        assert_eq!(
            fact(reopened.as_ref(), "prototype_contract").await,
            contract_fact
        );
        for (key, count) in [
            ("isolated_1", 24),
            ("isolated_8192", 24),
            ("first_1", 1),
            ("first_8192", 1),
        ] {
            assert_heat(&hydrate(reopened.as_ref(), key).await, key, count);
        }
        reopened
            .shutdown()
            .await
            .expect("close the reopened engine");
        emit(json!({"event":"contract_reopen", "status":"passed"}));
    }
}
