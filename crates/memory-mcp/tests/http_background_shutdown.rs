#![cfg(all(
    feature = "prometheus",
    feature = "streamable-http",
    feature = "test-fixtures"
))]

#[tokio::test]
async fn runtime_shutdown_cancels_upkeep_and_attempts_embedding_join() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use memory_mcp::error::MemoryError;
    use memory_mcp::http::runtime::bootstrap::join_background_owners_for_test;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    let cancel_upkeep = CancellationToken::new();
    let coordinator_shutdown_called = Arc::new(AtomicBool::new(false));
    let (coordinator_attempted_tx, coordinator_attempted_rx) = oneshot::channel();
    let (upkeep_attempted_tx, upkeep_attempted_rx) = oneshot::channel();

    let coordinator_cancel = cancel_upkeep.clone();
    let coordinator_shutdown = coordinator_shutdown_called.clone();
    let coordinator_join = async move {
        assert!(
            coordinator_cancel.is_cancelled(),
            "upkeep cancellation is signaled before the coordinator join is polled"
        );
        assert!(
            coordinator_shutdown.load(Ordering::SeqCst),
            "the embedding coordinator is shut down before joining"
        );
        let _ = coordinator_attempted_tx.send(());
        Err(MemoryError::Storage(
            "synthetic coordinator join failure".to_string(),
        ))
    };

    let upkeep_cancel = cancel_upkeep.clone();
    let upkeep_join = async move {
        assert!(upkeep_cancel.is_cancelled());
        let _ = upkeep_attempted_tx.send(());
        Err(MemoryError::Storage(
            "synthetic upkeep join failure".to_string(),
        ))
    };

    let shutdown_flag = coordinator_shutdown_called.clone();
    let result = join_background_owners_for_test(
        cancel_upkeep.clone(),
        move || shutdown_flag.store(true, Ordering::SeqCst),
        coordinator_join,
        upkeep_join,
    )
    .await;

    assert!(matches!(
        result,
        Err(MemoryError::Storage(message)) if message.contains("synthetic coordinator join failure")
    ));
    coordinator_attempted_rx
        .await
        .expect("coordinator join is attempted");
    upkeep_attempted_rx.await.expect("upkeep join is attempted");
}
