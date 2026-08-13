//! Fire-and-forget triple extraction spawned after fact creation.
//!
//! Colocates triple-extraction logic with the rest of the episode module.
//! The `triple_extraction_semaphore` stays on
//! `ServiceContext` as shared infrastructure bounding concurrency.

use serde_json::json;

use crate::logging::LogLevel;
use crate::service::service_context::ServiceContext;

/// Spawn a bounded fire-and-forget triple extraction task.
///
/// Uses the `triple_extraction_semaphore` on `ServiceContext` to limit
/// concurrent extraction tasks to
/// [`TRIPLE_EXTRACTION_MAX_CONCURRENCY`](crate::service::TRIPLE_EXTRACTION_MAX_CONCURRENCY).
/// If the limit is reached, the task is skipped with a warning log
/// (best-effort backpressure).
pub(crate) fn spawn_triple_extraction(service: &ServiceContext, fact_id: &str, content: &str) {
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
    let entity_service = service.entity_service.clone();

    tokio::spawn(async move {
        // Hold the permit for the duration of the task.
        let _permit = permit;

        if let Ok(triples) = extractor.extract(&content, &fact_id).await {
            for triple in &triples {
                let sql = r#"
                    CREATE TYPE::thing("triple", rand::guid()) SET
                        subject = $subject,
                        predicate = $predicate,
                        object = $object,
                        confidence = $confidence,
                        source_fact_id = $source_fact_id
                "#;
                let vars = json!({
                    "subject": triple.subject,
                    "predicate": triple.predicate,
                    "object": triple.object,
                    "confidence": triple.confidence,
                    "source_fact_id": triple.source_fact_id,
                });
                let _ = entity_service.execute_query(sql, vars).await;

                if crate::service::triple_extractor::is_singleton_predicate(&triple.predicate) {
                    let _ = crate::service::conflict_resolver::resolve_conflicts_for_triple(
                        &entity_service,
                        triple,
                    )
                    .await;
                }
            }
        }
    });
}
