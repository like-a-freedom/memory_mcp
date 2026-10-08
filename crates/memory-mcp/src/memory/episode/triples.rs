//! Fire-and-forget triple extraction spawned after fact creation.
//!
//! Colocates triple-extraction logic with the rest of the episode module.
//! The `triple_extraction_semaphore` stays on the service container as
//! shared infrastructure bounding concurrency.

use serde_json::json;

use crate::logging::LogLevel;
use crate::memory::capabilities::deps::ExtractDeps;

/// Spawn a bounded fire-and-forget triple extraction task.
///
/// Uses the `triple_extraction_semaphore` on the service container to limit
/// concurrent extraction tasks to
/// [`TRIPLE_EXTRACTION_MAX_CONCURRENCY`](crate::service::TRIPLE_EXTRACTION_MAX_CONCURRENCY).
/// If the limit is reached, the task is skipped with a warning log
/// (best-effort backpressure).
pub(crate) fn spawn_triple_extraction(service: &ExtractDeps, fact_id: &str, content: &str) {
    let permit = match service
        .triple_extraction_semaphore
        .clone()
        .try_acquire_owned()
    {
        Ok(permit) => permit,
        Err(_) => {
            service.logger.log(
                std::collections::HashMap::from([
                    (
                        "op".to_string(),
                        json!("triple_extraction.skipped_concurrency_limit"),
                    ),
                    ("fact_id".to_string(), json!(fact_id)),
                ]),
                LogLevel::Warn,
            );
            return;
        }
    };

    let extractor = service.triple_extractor.clone();
    let fact_id = fact_id.to_string();
    let content = content.to_string();
    let triple_store = service.triple_store();

    tokio::spawn(async move {
        // Hold the permit for the duration of the task.
        let _permit = permit;

        if let Ok(triples) = extractor.extract(&content, &fact_id).await {
            for triple in &triples {
                if let Err(error) = triple_store
                    .create_triple(
                        &triple.subject,
                        &triple.predicate,
                        &triple.object,
                        triple.confidence,
                        &triple.source_fact_id,
                    )
                    .await
                {
                    crate::logging::emit_best_effort_failure(
                        "triple_extraction.persist_failed",
                        &error,
                    );
                }

                if crate::shared::triple_extractor::is_singleton_predicate(&triple.predicate)
                    && let Err(error) =
                        crate::knowledge::conflict_resolver::resolve_conflicts_for_triple(
                            &triple_store,
                            triple,
                        )
                        .await
                {
                    crate::logging::emit_best_effort_failure(
                        "triple_extraction.reconcile_failed",
                        &error,
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::error::MemoryError;
    use crate::memory::capabilities::deps::ExtractDeps;
    use crate::service::MemoryService;
    use crate::shared::triple_extractor::{SemanticTriple, TripleExtractor};

    /// One triple with a non-singleton predicate, so only the persist path —
    /// not the conflict resolver — runs.
    struct OneTriple;

    #[async_trait::async_trait]
    impl TripleExtractor for OneTriple {
        async fn extract(
            &self,
            _text: &str,
            source_fact_id: &str,
        ) -> Result<Vec<SemanticTriple>, MemoryError> {
            Ok(vec![SemanticTriple {
                subject: "Alice".to_string(),
                predicate: "authored".to_string(),
                object: "spec".to_string(),
                confidence: 0.9,
                source_fact_id: source_fact_id.to_string(),
            }])
        }
    }

    /// A triple write that fails is best-effort: the spawned task still
    /// returns, and the discarded error is recorded rather than lost.
    #[tokio::test]
    async fn a_failed_triple_persist_is_logged() {
        let sink = crate::logging::capture::install();
        let db = Arc::new(
            crate::service::mock_db::MockDbClient::new().expect_create_with(|| {
                Err(MemoryError::Storage("triple write failed".to_string()))
            }),
        );
        let service = MemoryService::new(db, "org".to_string(), "warn".to_string(), 50, 100)
            .expect("service");
        let mut deps = ExtractDeps::from(&service);
        deps.triple_extractor = Arc::new(OneTriple);
        deps.logger = crate::logging::StdoutLogger::new("warn");

        spawn_triple_extraction(&deps, "fact:1", "Alice authored the spec.");

        // The work runs in a spawned task, so the test polls the capture under
        // a bound rather than sleeping.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if sink
                    .lines()
                    .iter()
                    .any(|line| line.contains("op=triple_extraction.persist_failed"))
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the persisted failure must be recorded within the bound");
    }
}
