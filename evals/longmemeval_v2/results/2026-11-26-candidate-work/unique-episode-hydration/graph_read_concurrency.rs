//! Independent graph batches are bounded, ordered and cancel with the public read.
use async_trait::async_trait;
use memory_mcp::{MemoryError, service::MemoryService, storage::DbClient};
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

struct GraphFrontierDb {
    started: AtomicUsize,
    active: AtomicUsize,
    gates: [Semaphore; 5],
    fail_batch: Option<usize>,
    emit_tied_edges: bool,
}
struct ActiveRead<'a>(&'a AtomicUsize);
impl Drop for ActiveRead<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl DbClient for GraphFrontierDb {
    async fn query(&self, sql: &str, vars: Option<Value>, _: &str) -> Result<Value, MemoryError> {
        if sql.contains("FROM edge") {
            let index = self.started.fetch_add(1, Ordering::SeqCst);
            assert!(index < 5, "exactly five unchanged32-seed batches");
            let vars = vars.expect("bound graph membership");
            let expected = (index * 32..((index + 1) * 32).min(129))
                .enumerate()
                .flat_map(|(offset, seed)| {
                    [
                        (format!("entity_{offset}_table"), json!("entity")),
                        (
                            format!("entity_{offset}_key"),
                            json!(format!("frontier{seed:03}")),
                        ),
                    ]
                })
                .chain(std::iter::once((
                    "cutoff".into(),
                    json!("2026-05-01T00:00:00+00:00"),
                )))
                .collect::<serde_json::Map<_, _>>();
            assert_eq!(
                vars,
                Value::Object(expected),
                "literal sorted32-ID membership"
            );
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = ActiveRead(&self.active);
            self.gates[index].acquire().await.unwrap().forget();
            if self.fail_batch == Some(index) {
                return Err(MemoryError::Storage(format!(
                    "controlled graph failure{index}"
                )));
            }
            if self.emit_tied_edges && matches!(index, 0 | 3) {
                return Ok(
                    json!([{"in":format!("entity:frontier{:03}",index * 32),"out":"entity:tied_target","relation":"connects"}]),
                );
            }
            return Ok(json!([]));
        }
        if sql.contains("search::score(1) AS ft_score") && sql.contains("FROM fact") {
            return Ok(
                json!([{"fact_id":"fact:graphfrontier","fact_type":"observation","content":"Falcon deployment","quote":"Falcon deployment",
                "source_episode":"episode:graphfrontier","t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z",
                "confidence":0.9,"index_keys":[],"entity_links":(0..129).map(|n|format!("entity:frontier{n:03}")).collect::<Vec<_>>(),
                "scope":"org","policy_tags":[],"provenance":{"source":"manual"}}]),
            );
        }
        if self.emit_tied_edges && sql.contains("entity_links CONTAINSANY") {
            assert!(
                vars.expect("bound graph fact read")["entity_links"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("entity:tied_target"))
            );
            return Ok(
                json!([{"fact_id":"fact:tied_target","fact_type":"observation","content":"Falcon linked deployment","quote":"Falcon linked deployment",
                "source_episode":"episode:tied_target","t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z",
                "confidence":0.9,"index_keys":[],"entity_links":["entity:tied_target"],"scope":"org","policy_tags":[],"provenance":{"source":"manual"}}]),
            );
        }
        if sql.starts_with("RETURN [") {
            return Ok(json!([[]]));
        }
        Ok(json!([]))
    }
    async fn select_one(&self, _: &str, _: &str) -> Result<Option<Value>, MemoryError> {
        Ok(None)
    }
    async fn select_table(
        &self,
        _: memory_mcp::storage::table_scope::OwnedTable,
        _: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        Ok(vec![])
    }
    async fn create(&self, _: &str, _: Value, _: &str, _: &[&str]) -> Result<Value, MemoryError> {
        panic!("read must not create")
    }
    async fn update(&self, _: &str, _: Value, _: &str, _: &[&str]) -> Result<Value, MemoryError> {
        Ok(json!({}))
    }
    async fn apply_migrations(&self, _: &str) -> Result<(), MemoryError> {
        panic!("read must not migrate")
    }
}

#[tokio::test]
async fn public_read_overlaps_four_graph_batches_without_starting_a_fifth() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let db = Arc::new(GraphFrontierDb {
        started: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        gates: std::array::from_fn(|_| Semaphore::new(0)),
        fail_batch: None,
        emit_tied_edges: false,
    });
    let service = MemoryService::new(db.clone(), "org".into(), "error".into(), 50, 100).unwrap();
    let request = serde_json::from_value(
        json!({"query":"Falcon","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let mut read = Box::pin(AssembleContextCapability::assemble_context_from_service(
        &service, request,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(
        db.started.load(Ordering::SeqCst),
        4,
        "four independent batches should overlap before any release"
    );
    assert_eq!(db.active.load(Ordering::SeqCst), 4);
    // Reverse completion must not change ordered processing or start the next wave.
    db.gates[3].add_permits(1);
    db.gates[2].add_permits(1);
    db.gates[1].add_permits(1);
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(db.started.load(Ordering::SeqCst), 4);
    db.gates[0].add_permits(1);
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(db.started.load(Ordering::SeqCst), 5);
    db.gates[4].add_permits(1);
    let Poll::Ready(result) = read.as_mut().poll(&mut cx) else {
        panic!("all graph reads released")
    };
    let items = result.unwrap();
    assert_eq!(items[0].fact_id, "fact:graphfrontier");
    assert_eq!(db.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn later_graph_failure_cancels_an_earlier_pending_batch() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let db = Arc::new(GraphFrontierDb {
        started: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        gates: std::array::from_fn(|_| Semaphore::new(0)),
        fail_batch: Some(3),
        emit_tied_edges: false,
    });
    let service = MemoryService::new(db.clone(), "org".into(), "error".into(), 50, 100).unwrap();
    let request = serde_json::from_value(
        json!({"query":"Falcon","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let mut read = Box::pin(AssembleContextCapability::assemble_context_from_service(
        &service, request,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(db.started.load(Ordering::SeqCst), 4);
    db.gates[3].add_permits(1);
    let Poll::Ready(Err(error)) = read.as_mut().poll(&mut cx) else {
        panic!("a failed graph read must not wait for unrelated pending batches")
    };
    assert!(
        matches!(error, MemoryError::Storage(message) if message == "controlled graph failure3")
    );
    assert_eq!(db.active.load(Ordering::SeqCst), 0);
    assert_eq!(db.started.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn cancelling_public_read_drops_all_pending_graph_batches() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let db = Arc::new(GraphFrontierDb {
        started: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        gates: std::array::from_fn(|_| Semaphore::new(0)),
        fail_batch: None,
        emit_tied_edges: false,
    });
    let service = MemoryService::new(db.clone(), "org".into(), "error".into(), 50, 100).unwrap();
    let request = serde_json::from_value(
        json!({"query":"Falcon","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let mut read = Box::pin(AssembleContextCapability::assemble_context_from_service(
        &service, request,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(db.active.load(Ordering::SeqCst), 4);
    drop(read);
    assert_eq!(db.active.load(Ordering::SeqCst), 0);
    assert_eq!(db.started.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn reverse_batch_completion_preserves_the_first_sorted_graph_path() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    let db = Arc::new(GraphFrontierDb {
        started: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        gates: std::array::from_fn(|_| Semaphore::new(0)),
        fail_batch: None,
        emit_tied_edges: true,
    });
    let service = MemoryService::new(db.clone(), "org".into(), "error".into(), 50, 100).unwrap();
    let request = serde_json::from_value(
        json!({"query":"Falcon","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let mut read = Box::pin(AssembleContextCapability::assemble_context_from_service(
        &service, request,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(read.as_mut().poll(&mut cx).is_pending());
    db.gates[3].add_permits(1);
    db.gates[2].add_permits(1);
    db.gates[1].add_permits(1);
    assert!(read.as_mut().poll(&mut cx).is_pending());
    db.gates[0].add_permits(1);
    assert!(read.as_mut().poll(&mut cx).is_pending());
    db.gates[4].add_permits(1);
    let Poll::Ready(result) = read.as_mut().poll(&mut cx) else {
        panic!("all graph reads released")
    };
    let items = result.unwrap();
    let target = items
        .iter()
        .find(|item| item.fact_id == "fact:tied_target")
        .expect("nonempty graph result must reach the public response");
    assert_eq!(
        target.provenance["graph_trace"]["path"],
        json!(["entity:frontier000", "entity:tied_target"])
    );
    assert_eq!(target.quote, "Falcon linked deployment");
}
