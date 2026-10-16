//! Stock-level background worker.
//!
//! Periodically re-reads the knowledge inventory and stamps it as gauges,
//! so the product dashboard's stock panels keep their data without a
//! human opening the lifecycle view. The lifecycle dashboard read stamps
//! the same gauges under its own operation label; both are levels, set
//! rather than accumulated.

use std::sync::Arc;

use tokio::time::{self, Duration as TokioDuration};
use tokio_util::sync::CancellationToken;

use super::LifecycleHandles;
use crate::error::MemoryError;
use crate::memory::lifecycle_types::StockLevels;

/// Spawns the stock-level worker background task.
///
/// The task runs until `shutdown` is cancelled, at which point it exits
/// cleanly after completing any in-flight pass.
pub(crate) fn spawn_stock_worker(
    service: Arc<dyn crate::storage::DbClient>,
    active_namespace: String,
    logger: crate::logging::StdoutLogger,
    policy: crate::memory::lifecycle_workers::LifecyclePolicy,
    interval_secs: u64,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(TokioDuration::from_secs(interval_secs));

        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::Value::String("lifecycle.stock.start".to_string()),
        );
        event.insert(
            "interval_secs".to_string(),
            serde_json::Value::Number(serde_json::Number::from(interval_secs)),
        );
        logger.log(event, crate::logging::LogLevel::Info);

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {}
            }
            match run_stock_pass(&LifecycleHandles {
                db_client: service.clone(),
                active_namespace: &active_namespace,
                logger: &logger,
                policy,
                claim_store: None,
            })
            .await
            {
                Ok(levels) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::Value::String("lifecycle.stock.complete".to_string()),
                    );
                    event.insert(
                        "active_facts".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(levels.active_facts)),
                    );
                    event.insert(
                        "archival_candidates".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(
                            levels.archival_candidates,
                        )),
                    );
                    event.insert(
                        "communities".to_string(),
                        serde_json::Value::Number(serde_json::Number::from(levels.communities)),
                    );
                    logger.log(event, crate::logging::LogLevel::Info);
                }
                Err(e) => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::Value::String("lifecycle.stock.error".to_string()),
                    );
                    event.insert(
                        "error".to_string(),
                        serde_json::Value::String(format!("{}", e)),
                    );
                    logger.log(event, e.log_level());
                }
            }
        }
    })
}

/// Runs one stock-level pass, stamping the inventory gauges.
///
/// Read-only: the levels are what the store currently holds, stamped
/// under the `lifecycle_stock` operation so the periodic pass is
/// distinguishable from the dashboard read on the same gauge.
pub(crate) async fn run_stock_pass(
    service: &LifecycleHandles<'_>,
) -> Result<StockLevels, MemoryError> {
    let mut operation_metrics = crate::observability::OperationMetrics::new("lifecycle_stock");
    let levels = service.read_stock_levels().await?;

    // A stock, not a flow: levels read from the store are set rather than
    // added — see `record_stock`.
    operation_metrics.record_stock("active_facts", levels.active_facts);
    operation_metrics.record_stock("archival_candidates", levels.archival_candidates);
    operation_metrics.record_stock("communities", levels.communities);
    operation_metrics.success();
    Ok(levels)
}
