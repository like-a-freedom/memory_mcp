//! Context reads await a bounded wave of independent access writes.
use async_trait::async_trait;
use memory_mcp::{
    MemoryError,
    service::MemoryService,
    storage::{DbClient, table_scope::OwnedTable},
};
use serde_json::{Value, json};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};
use tokio::sync::Semaphore;

struct AccessDb {
    started: AtomicUsize,
    active: AtomicUsize,
    gates: [Semaphore; 5],
    fail_write: Option<usize>,
    completed: AtomicUsize,
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl DbClient for AccessDb {
    async fn query(&self, sql: &str, _: Option<Value>, _: &str) -> Result<Value, MemoryError> {
        if sql.starts_with("UPDATE type::record('fact'") && sql.contains("access_count") {
            let n = self.started.fetch_add(1, Ordering::SeqCst);
            assert!(n < 5, "one write per returned fact");
            self.active.fetch_add(1, Ordering::SeqCst);
            let _guard = Active(&self.active);
            self.gates[n].acquire().await.unwrap().forget();
            self.completed.fetch_add(1, Ordering::SeqCst);
            if self.fail_write == Some(n) {
                return Err(MemoryError::Storage("controlled access failure".into()));
            }
            return Ok(json!([]));
        }
        if sql.contains("search::score(1) AS ft_score") && sql.contains("FROM fact") {
            return Ok(Value::Array((0..5).map(|n|json!({"fact_id":format!("fact:archive{n}"),"fact_type":"observation","content":format!("Archive instrument {n} records distinct measurement {n}"),"quote":format!("Archive instrument {n}"),"source_episode":format!("episode:archive{n}"),"t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","confidence":0.9,"index_keys":["Archive"],"entity_links":[],"scope":"org","policy_tags":[],"provenance":{"source":"manual"}})).collect()));
        }
        Ok(json!([]))
    }
    async fn select_one(&self, _: &str, _: &str) -> Result<Option<Value>, MemoryError> {
        Ok(None)
    }
    async fn select_table(&self, _: OwnedTable, _: &str) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }
    async fn create(&self, _: &str, _: Value, _: &str, _: &[&str]) -> Result<Value, MemoryError> {
        panic!("read does not create")
    }
    async fn update(&self, _: &str, _: Value, _: &str, _: &[&str]) -> Result<Value, MemoryError> {
        panic!("access writes use atomic SQL")
    }
    async fn apply_migrations(&self, _: &str) -> Result<(), MemoryError> {
        panic!("read does not migrate")
    }
}
fn database() -> Arc<AccessDb> {
    Arc::new(AccessDb {
        started: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        gates: std::array::from_fn(|_| Semaphore::new(0)),
        fail_write: None,
        completed: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn public_read_awaits_four_access_writes_before_starting_the_next_wave() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let db = database();
    let service = MemoryService::new(db.clone(), "org".into(), "error".into(), 50, 100).unwrap();
    let req = serde_json::from_value(
        json!({"query":"Archive","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let mut read = Box::pin(AssembleContextCapability::assemble_context_from_service(
        &service, req,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(
        db.started.load(Ordering::SeqCst),
        4,
        "independent writes overlap, with a limit of four"
    );
    assert_eq!(db.active.load(Ordering::SeqCst), 4);
    db.gates[3].add_permits(1);
    db.gates[2].add_permits(1);
    db.gates[1].add_permits(1);
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(
        db.started.load(Ordering::SeqCst),
        4,
        "await the entire owned wave"
    );
    db.gates[0].add_permits(1);
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(db.started.load(Ordering::SeqCst), 5);
    db.gates[4].add_permits(1);
    let Poll::Ready(result) = read.as_mut().poll(&mut cx) else {
        panic!("every write released")
    };
    assert_eq!(result.unwrap().len(), 5);
    assert_eq!(db.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_drops_all_pending_access_writes() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let db = database();
    let service = MemoryService::new(db.clone(), "org".into(), "error".into(), 50, 100).unwrap();
    let req = serde_json::from_value(
        json!({"query":"Archive","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let mut read = Box::pin(AssembleContextCapability::assemble_context_from_service(
        &service, req,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(db.active.load(Ordering::SeqCst), 4);
    drop(read);
    assert_eq!(db.active.load(Ordering::SeqCst), 0);
    assert_eq!(db.started.load(Ordering::SeqCst), 4);
    assert_eq!(db.completed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_access_does_not_cancel_siblings_or_skip_later_facts() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let db = Arc::new(AccessDb {
        started: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        gates: std::array::from_fn(|_| Semaphore::new(1)),
        fail_write: Some(1),
        completed: AtomicUsize::new(0),
    });
    let service = MemoryService::new(db.clone(), "org".into(), "error".into(), 50, 100).unwrap();
    let req = serde_json::from_value(
        json!({"query":"Archive","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, req)
        .await
        .unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(db.started.load(Ordering::SeqCst), 5);
    assert_eq!(db.completed.load(Ordering::SeqCst), 5);
    assert_eq!(db.active.load(Ordering::SeqCst), 0);
}

mod common;
#[tokio::test]
async fn public_read_durably_records_all_distinct_fact_accesses_before_returning() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let (service, db) = common::make_service_with_client().await;
    for n in 0..5 {
        db.create(&format!("fact:archive{n}"),json!({"fact_id":format!("fact:archive{n}"),"fact_type":"observation","content":format!("Archive instrument {n} records distinct measurement {n}"),"quote":format!("Archive instrument {n}"),"source_episode":format!("episode:archive{n}"),"t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","confidence":0.9,"index_keys":["Archive"],"entity_links":[],"scope":"org","policy_tags":[],"provenance":{"source":"manual"}}),"org",&["t_valid","t_ingested"]).await.unwrap();
    }
    let req = serde_json::from_value(
        json!({"query":"Archive","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, req)
        .await
        .unwrap();
    assert_eq!(items.len(), 5);
    let records = db
        .query(
            "SELECT fact_id, access_count, last_accessed FROM fact ORDER BY fact_id ASC",
            None,
            "org",
        )
        .await
        .unwrap();
    let actual = records
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["fact_id"].as_str().unwrap().to_owned(),
                r["access_count"].as_i64().unwrap(),
                r["last_accessed"].is_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        vec![
            ("fact:archive0".into(), 1, true),
            ("fact:archive1".into(), 1, true),
            ("fact:archive2".into(), 1, true),
            ("fact:archive3".into(), 1, true),
            ("fact:archive4".into(), 1, true)
        ]
    );
}
