use std::time::Instant;

use serde_json::{Value, json};

use super::startup::EMBEDDING_STATE_RECORD_ID;
use super::{MemoryError, MemoryService, normalize_dt};
use crate::logging::LogLevel;
use crate::service::value_helpers::{json_i64, json_string};

const REEMBED_JOB_ID: &str = "embedding_job:fact_reembed";
const REEMBED_BATCH_SIZE: i32 = 100;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReembedSummary {
    pub total_facts: usize,
    pub processed_facts: usize,
    pub succeeded_facts: usize,
    pub failed_facts: usize,
}

impl MemoryService {
    pub async fn reembed_all_facts(&self) -> Result<ReembedSummary, MemoryError> {
        if !self.embedding_provider.is_enabled() {
            return Err(MemoryError::Validation(
                "reembed requires an enabled embedding provider".to_string(),
            ));
        }

        let target_signature = self.current_embedding_signature.clone().ok_or_else(|| {
            MemoryError::Validation("reembed requires an enabled embedding signature".to_string())
        })?;
        let target_dimension = self.current_embedding_dimension.ok_or_else(|| {
            MemoryError::Validation("reembed requires a resolved target dimension".to_string())
        })?;

        let started_at = Instant::now();
        let started_at_rfc3339 = chrono::Utc::now().to_rfc3339();
        let existing_job = self.load_reembed_job().await?;
        let resumable_job = existing_job.filter(|job| {
            job.get("target_signature").and_then(json_string) == Some(target_signature.as_str())
        });
        let resumed = resumable_job.is_some();
        let mut namespace_progress = resumable_job
            .as_ref()
            .and_then(|job| job.get("namespace_progress"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut summary = ReembedSummary {
            total_facts: self.count_facts_needing_reembed(&target_signature).await?,
            ..ReembedSummary::default()
        };

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.job_started")),
                (
                    "target_signature".to_string(),
                    json!(target_signature.clone()),
                ),
                ("target_dimension".to_string(), json!(target_dimension)),
                (
                    "provider".to_string(),
                    json!(self.embedding_provider.provider_name()),
                ),
                (
                    "model".to_string(),
                    json!(self.current_embedding_model.clone()),
                ),
                ("resumed".to_string(), json!(resumed)),
                ("total_facts".to_string(), json!(summary.total_facts)),
            ]),
            LogLevel::Info,
        );

        self.persist_reembed_job(
            &summary,
            &target_signature,
            target_dimension,
            &namespace_progress,
            Some(&started_at_rfc3339),
            None,
            "running",
            None,
            None,
            started_at.elapsed(),
        )
        .await?;

        for namespace in &self.namespaces {
            let mut last_completed_fact_id = resumable_job
                .as_ref()
                .and_then(|job| namespace_last_completed_fact_id(job, namespace));
            let (mut namespace_processed, mut namespace_succeeded, mut namespace_failed) =
                existing_namespace_counters(&namespace_progress, namespace);

            self.write_embedding_state(namespace, "rebuilding", None, Some(REEMBED_JOB_ID))
                .await?;

            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("reembed.namespace_started")),
                    ("namespace".to_string(), json!(namespace)),
                    (
                        "resume_cursor".to_string(),
                        json!(last_completed_fact_id.clone()),
                    ),
                ]),
                LogLevel::Info,
            );

            loop {
                let batch = self
                    .db_client
                    .select_facts_needing_reembed(
                        namespace,
                        &target_signature,
                        last_completed_fact_id.as_deref(),
                        REEMBED_BATCH_SIZE,
                    )
                    .await?;

                self.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("reembed.batch_fetched")),
                        ("namespace".to_string(), json!(namespace)),
                        ("count".to_string(), json!(batch.len())),
                        (
                            "after_cursor".to_string(),
                            json!(last_completed_fact_id.clone()),
                        ),
                    ]),
                    LogLevel::Debug,
                );

                if batch.is_empty() {
                    break;
                }

                for fact in batch {
                    let fact_id = fact
                        .get("fact_id")
                        .and_then(json_string)
                        .ok_or_else(|| MemoryError::Validation("missing fact_id".to_string()))?
                        .to_string();

                    self.logger.log(
                        std::collections::HashMap::from([
                            ("op".to_string(), json!("reembed.fact_rewrite_started")),
                            ("namespace".to_string(), json!(namespace)),
                            ("fact_id".to_string(), json!(fact_id.clone())),
                        ]),
                        LogLevel::Debug,
                    );

                    match self
                        .rewrite_fact_embedding(
                            namespace,
                            fact,
                            &target_signature,
                            target_dimension,
                        )
                        .await
                    {
                        Ok(updated_fact_id) => {
                            summary.processed_facts += 1;
                            summary.succeeded_facts += 1;
                            namespace_processed += 1;
                            namespace_succeeded += 1;
                            last_completed_fact_id = Some(updated_fact_id.clone());
                            update_namespace_progress(
                                &mut namespace_progress,
                                namespace,
                                "running",
                                namespace_processed,
                                namespace_succeeded,
                                namespace_failed,
                                last_completed_fact_id.as_deref(),
                            );
                            self.persist_reembed_job(
                                &summary,
                                &target_signature,
                                target_dimension,
                                &namespace_progress,
                                Some(&started_at_rfc3339),
                                None,
                                "running",
                                Some(namespace),
                                None,
                                started_at.elapsed(),
                            )
                            .await?;

                            self.logger.log(
                                std::collections::HashMap::from([
                                    ("op".to_string(), json!("reembed.cursor_advanced")),
                                    ("namespace".to_string(), json!(namespace)),
                                    (
                                        "last_completed_fact_id".to_string(),
                                        json!(last_completed_fact_id.clone()),
                                    ),
                                ]),
                                LogLevel::Debug,
                            );
                        }
                        Err(err) => {
                            summary.processed_facts += 1;
                            summary.failed_facts += 1;
                            namespace_processed += 1;
                            namespace_failed += 1;
                            update_namespace_progress(
                                &mut namespace_progress,
                                namespace,
                                "failed",
                                namespace_processed,
                                namespace_succeeded,
                                namespace_failed,
                                last_completed_fact_id.as_deref(),
                            );
                            self.write_embedding_state(
                                namespace,
                                "failed",
                                None,
                                Some(REEMBED_JOB_ID),
                            )
                            .await?;
                            let error_message = format!(
                                "reembed failed for fact {fact_id}; fix the provider and rerun `memory_mcp reembed`: {err}"
                            );
                            self.persist_reembed_job(
                                &summary,
                                &target_signature,
                                target_dimension,
                                &namespace_progress,
                                Some(&started_at_rfc3339),
                                Some(&chrono::Utc::now().to_rfc3339()),
                                "failed",
                                Some(namespace),
                                Some(&error_message),
                                started_at.elapsed(),
                            )
                            .await?;
                            self.logger.log(
                                std::collections::HashMap::from([
                                    ("op".to_string(), json!("reembed.fact_failed")),
                                    ("namespace".to_string(), json!(namespace)),
                                    ("fact_id".to_string(), json!(fact_id.clone())),
                                    ("error".to_string(), json!(err.to_string())),
                                ]),
                                LogLevel::Warn,
                            );
                            self.logger.log(
                                std::collections::HashMap::from([
                                    ("op".to_string(), json!("reembed.job_failed")),
                                    (
                                        "processed_facts".to_string(),
                                        json!(summary.processed_facts),
                                    ),
                                    (
                                        "succeeded_facts".to_string(),
                                        json!(summary.succeeded_facts),
                                    ),
                                    ("failed_facts".to_string(), json!(summary.failed_facts)),
                                    ("total_facts".to_string(), json!(summary.total_facts)),
                                    (
                                        "facts_per_second".to_string(),
                                        json!(facts_per_second(
                                            started_at.elapsed(),
                                            summary.processed_facts
                                        )),
                                    ),
                                    (
                                        "duration_ms".to_string(),
                                        json!(started_at.elapsed().as_millis() as u64),
                                    ),
                                    (
                                        "provider".to_string(),
                                        json!(self.embedding_provider.provider_name()),
                                    ),
                                    (
                                        "model".to_string(),
                                        json!(self.current_embedding_model.clone()),
                                    ),
                                    ("target_dimension".to_string(), json!(target_dimension)),
                                    (
                                        "target_signature".to_string(),
                                        json!(target_signature.clone()),
                                    ),
                                    ("resumed".to_string(), json!(resumed)),
                                ]),
                                LogLevel::Warn,
                            );
                            return Err(MemoryError::Storage(format!(
                                "reembed failed for fact {fact_id}: {err}"
                            )));
                        }
                    }
                }

                self.log_reembed_progress(namespace, &summary, started_at.elapsed());
            }

            update_namespace_progress(
                &mut namespace_progress,
                namespace,
                "completed",
                namespace_processed,
                namespace_succeeded,
                namespace_failed,
                last_completed_fact_id.as_deref(),
            );
            self.write_embedding_state(
                namespace,
                "ready",
                Some(&target_signature),
                Some(REEMBED_JOB_ID),
            )
            .await?;
            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("reembed.namespace_completed")),
                    ("namespace".to_string(), json!(namespace)),
                    (
                        "last_completed_fact_id".to_string(),
                        json!(last_completed_fact_id.clone()),
                    ),
                ]),
                LogLevel::Info,
            );
        }

        let finished_at = chrono::Utc::now().to_rfc3339();
        self.persist_reembed_job(
            &summary,
            &target_signature,
            target_dimension,
            &namespace_progress,
            Some(&started_at_rfc3339),
            Some(&finished_at),
            "completed",
            None,
            None,
            started_at.elapsed(),
        )
        .await?;
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.job_completed")),
                (
                    "processed_facts".to_string(),
                    json!(summary.processed_facts),
                ),
                (
                    "succeeded_facts".to_string(),
                    json!(summary.succeeded_facts),
                ),
                ("failed_facts".to_string(), json!(summary.failed_facts)),
                ("total_facts".to_string(), json!(summary.total_facts)),
                (
                    "facts_per_second".to_string(),
                    json!(facts_per_second(
                        started_at.elapsed(),
                        summary.processed_facts
                    )),
                ),
                (
                    "duration_ms".to_string(),
                    json!(started_at.elapsed().as_millis() as u64),
                ),
                (
                    "provider".to_string(),
                    json!(self.embedding_provider.provider_name()),
                ),
                (
                    "model".to_string(),
                    json!(self.current_embedding_model.clone()),
                ),
                ("target_dimension".to_string(), json!(target_dimension)),
                ("target_signature".to_string(), json!(target_signature)),
                ("resumed".to_string(), json!(resumed)),
            ]),
            LogLevel::Info,
        );

        Ok(summary)
    }

    async fn load_reembed_job(&self) -> Result<Option<Value>, MemoryError> {
        self.db_client
            .select_one(REEMBED_JOB_ID, self.default_namespace.as_str())
            .await
    }

    async fn count_facts_needing_reembed(
        &self,
        target_signature: &str,
    ) -> Result<usize, MemoryError> {
        let mut total = 0;
        for namespace in &self.namespaces {
            total += self
                .db_client
                .count_facts_needing_reembed(namespace, target_signature)
                .await?;
        }
        Ok(total)
    }

    async fn rewrite_fact_embedding(
        &self,
        namespace: &str,
        fact: Value,
        target_signature: &str,
        target_dimension: usize,
    ) -> Result<String, MemoryError> {
        let mut updated = fact
            .as_object()
            .cloned()
            .ok_or_else(|| MemoryError::Validation("fact record must be an object".to_string()))?;
        let fact_id = updated
            .get("fact_id")
            .and_then(json_string)
            .ok_or_else(|| MemoryError::Validation("missing fact_id".to_string()))?
            .to_string();
        let fact_type = updated
            .get("fact_type")
            .and_then(json_string)
            .ok_or_else(|| MemoryError::Validation("missing fact_type".to_string()))?;
        let content = updated
            .get("content")
            .and_then(json_string)
            .ok_or_else(|| MemoryError::Validation("missing content".to_string()))?;
        let quote = updated
            .get("quote")
            .and_then(json_string)
            .ok_or_else(|| MemoryError::Validation("missing quote".to_string()))?;

        let embedding = self
            .generate_embedding(&MemoryService::build_fact_embedding_input(
                fact_type, content, quote,
            ))
            .await?
            .ok_or_else(|| {
                MemoryError::Validation(
                    "reembed requires an enabled embedding provider".to_string(),
                )
            })?;

        if embedding.len() != target_dimension {
            return Err(MemoryError::Validation(format!(
                "embedding dimension mismatch: provider returned {}, expected {target_dimension}",
                embedding.len()
            )));
        }

        updated.insert("embedding".to_string(), json!(embedding));
        updated.insert(
            "embedding_provider".to_string(),
            json!(self.embedding_provider.provider_name()),
        );
        if let Some(model) = &self.current_embedding_model {
            updated.insert("embedding_model".to_string(), json!(model));
        }
        updated.insert("embedding_dimension".to_string(), json!(target_dimension));
        updated.insert("embedding_signature".to_string(), json!(target_signature));
        updated.insert(
            "embedding_updated_at".to_string(),
            json!(normalize_dt(chrono::Utc::now())),
        );

        self.db_client
            .update(&fact_id, Value::Object(updated), namespace)
            .await?;
        Ok(fact_id)
    }

    async fn write_embedding_state(
        &self,
        namespace: &str,
        status: &str,
        active_signature: Option<&str>,
        last_job_id: Option<&str>,
    ) -> Result<(), MemoryError> {
        let mut payload = serde_json::Map::from_iter([
            ("status".to_string(), json!(status)),
            (
                "provider".to_string(),
                json!(self.embedding_provider.provider_name()),
            ),
            (
                "model".to_string(),
                json!(self.current_embedding_model.clone()),
            ),
            (
                "dimension".to_string(),
                json!(self.current_embedding_dimension),
            ),
            (
                "updated_at".to_string(),
                json!(chrono::Utc::now().to_rfc3339()),
            ),
        ]);
        if let Some(active_signature) = active_signature {
            payload.insert("active_signature".to_string(), json!(active_signature));
        }
        if let Some(last_job_id) = last_job_id {
            payload.insert("last_job_id".to_string(), json!(last_job_id));
        }

        if self
            .db_client
            .select_one(EMBEDDING_STATE_RECORD_ID, namespace)
            .await?
            .is_some()
        {
            self.db_client
                .update(EMBEDDING_STATE_RECORD_ID, Value::Object(payload), namespace)
                .await?;
        } else {
            self.db_client
                .create(EMBEDDING_STATE_RECORD_ID, Value::Object(payload), namespace)
                .await?;
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn persist_reembed_job(
        &self,
        summary: &ReembedSummary,
        target_signature: &str,
        target_dimension: usize,
        namespace_progress: &serde_json::Map<String, Value>,
        started_at: Option<&str>,
        finished_at: Option<&str>,
        status: &str,
        current_namespace: Option<&str>,
        last_error: Option<&str>,
        elapsed: std::time::Duration,
    ) -> Result<(), MemoryError> {
        let payload = json!({
            "job_id": REEMBED_JOB_ID,
            "status": status,
            "target_signature": target_signature,
            "provider": self.embedding_provider.provider_name(),
            "model": self.current_embedding_model.clone(),
            "dimension": target_dimension,
            "namespaces": self.namespaces.clone(),
            "requested_at": started_at,
            "total_facts": summary.total_facts,
            "processed_facts": summary.processed_facts,
            "succeeded_facts": summary.succeeded_facts,
            "failed_facts": summary.failed_facts,
            "facts_per_second": facts_per_second(elapsed, summary.processed_facts),
            "eta_seconds": eta_seconds(elapsed, summary.total_facts, summary.processed_facts),
            "current_namespace": current_namespace,
            "namespace_progress": namespace_progress,
            "last_error": last_error,
            "started_at": started_at,
            "updated_at": chrono::Utc::now().to_rfc3339(),
            "finished_at": finished_at,
        });

        if self
            .db_client
            .select_one(REEMBED_JOB_ID, self.default_namespace.as_str())
            .await?
            .is_some()
        {
            self.db_client
                .update(REEMBED_JOB_ID, payload, self.default_namespace.as_str())
                .await?;
        } else {
            self.db_client
                .create(REEMBED_JOB_ID, payload, self.default_namespace.as_str())
                .await?;
        }

        Ok(())
    }

    fn log_reembed_progress(
        &self,
        namespace: &str,
        summary: &ReembedSummary,
        elapsed: std::time::Duration,
    ) {
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.progress")),
                ("namespace".to_string(), json!(namespace)),
                (
                    "processed_facts".to_string(),
                    json!(summary.processed_facts),
                ),
                (
                    "succeeded_facts".to_string(),
                    json!(summary.succeeded_facts),
                ),
                ("failed_facts".to_string(), json!(summary.failed_facts)),
                ("total_facts".to_string(), json!(summary.total_facts)),
                (
                    "facts_per_second".to_string(),
                    json!(facts_per_second(elapsed, summary.processed_facts)),
                ),
                (
                    "eta_seconds".to_string(),
                    json!(eta_seconds(
                        elapsed,
                        summary.total_facts,
                        summary.processed_facts
                    )),
                ),
            ]),
            LogLevel::Info,
        );
    }
}

fn namespace_last_completed_fact_id(job: &Value, namespace: &str) -> Option<String> {
    job.get("namespace_progress")
        .and_then(Value::as_object)
        .and_then(|progress| progress.get(namespace))
        .and_then(|value| value.get("last_completed_fact_id"))
        .and_then(json_string)
        .map(ToString::to_string)
}

fn existing_namespace_counters(
    namespace_progress: &serde_json::Map<String, Value>,
    namespace: &str,
) -> (usize, usize, usize) {
    let Some(entry) = namespace_progress.get(namespace).and_then(Value::as_object) else {
        return (0, 0, 0);
    };

    (
        entry.get("processed_facts").and_then(json_i64).unwrap_or(0) as usize,
        entry.get("succeeded_facts").and_then(json_i64).unwrap_or(0) as usize,
        entry.get("failed_facts").and_then(json_i64).unwrap_or(0) as usize,
    )
}

fn update_namespace_progress(
    namespace_progress: &mut serde_json::Map<String, Value>,
    namespace: &str,
    status: &str,
    processed_facts: usize,
    succeeded_facts: usize,
    failed_facts: usize,
    last_completed_fact_id: Option<&str>,
) {
    namespace_progress.insert(
        namespace.to_string(),
        json!({
            "status": status,
            "processed_facts": processed_facts,
            "succeeded_facts": succeeded_facts,
            "failed_facts": failed_facts,
            "last_completed_fact_id": last_completed_fact_id,
            "updated_at": chrono::Utc::now().to_rfc3339(),
        }),
    );
}

fn facts_per_second(elapsed: std::time::Duration, processed_facts: usize) -> f64 {
    if elapsed.as_secs_f64() > 0.0 {
        processed_facts as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    }
}

fn eta_seconds(
    elapsed: std::time::Duration,
    total_facts: usize,
    processed_facts: usize,
) -> Option<u64> {
    if processed_facts == 0 || processed_facts >= total_facts {
        return None;
    }

    let fps = facts_per_second(elapsed, processed_facts);
    if fps <= f64::EPSILON {
        return None;
    }

    Some(((total_facts.saturating_sub(processed_facts)) as f64 / fps).ceil() as u64)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::json;

    use super::super::{EmbeddingProvider, MemoryError, MemoryService, normalize_dt};
    use crate::config::{DEFAULT_EMBEDDING_DIMENSION, DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD};
    use crate::storage::{DbClient, SurrealDbClient};

    struct SequenceTestEmbeddingProvider {
        dimension: usize,
        fail_on_call: Option<usize>,
        call_count: AtomicUsize,
    }

    impl SequenceTestEmbeddingProvider {
        fn new(dimension: usize) -> Self {
            Self {
                dimension,
                fail_on_call: None,
                call_count: AtomicUsize::new(0),
            }
        }

        fn fails_on_call(dimension: usize, fail_on_call: usize) -> Self {
            Self {
                dimension,
                fail_on_call: Some(fail_on_call),
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for SequenceTestEmbeddingProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "test"
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
            let call = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_on_call == Some(call) {
                return Err(MemoryError::Storage(format!(
                    "synthetic reembed failure on call {call}"
                )));
            }

            let mut embedding = vec![0.0; self.dimension];
            if let Some(first) = embedding.first_mut() {
                *first = 1.0;
            }
            Ok(embedding)
        }
    }

    async fn make_in_memory_db(namespaces: &[&str]) -> Arc<SurrealDbClient> {
        let namespaces = namespaces
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces("reembed_test", &namespaces, "warn")
                .await
                .expect("connect in memory db"),
        );

        for namespace in &namespaces {
            db_client
                .apply_migrations(namespace)
                .await
                .expect("apply migrations");
        }

        db_client
    }

    async fn seed_fact_with_embedding(
        db_client: &Arc<SurrealDbClient>,
        namespace: &str,
        fact_id: &str,
        content: &str,
        embedding: Vec<f64>,
        signature: &str,
    ) {
        let now = normalize_dt(Utc::now());
        db_client
            .create(
                fact_id,
                json!({
                    "fact_id": fact_id,
                    "fact_type": "note",
                    "content": content,
                    "quote": content,
                    "source_episode": "episode:seed",
                    "t_valid": now,
                    "t_ingested": now,
                    "confidence": 0.9,
                    "index_keys": [],
                    "access_count": 0,
                    "entity_links": [],
                    "scope": namespace,
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:seed"},
                    "embedding": embedding,
                    "embedding_provider": "legacy-test",
                    "embedding_model": "legacy-model",
                    "embedding_dimension": embedding.len(),
                    "embedding_signature": signature,
                    "embedding_updated_at": now,
                }),
                namespace,
            )
            .await
            .expect("seed fact should succeed");
    }

    fn make_reembed_service(
        db_client: Arc<SurrealDbClient>,
        namespaces: Vec<&str>,
        provider: Arc<dyn EmbeddingProvider>,
        dimension: usize,
    ) -> MemoryService {
        let mut service = MemoryService::new_with_embedding_provider(
            db_client,
            namespaces
                .into_iter()
                .map(|value| value.to_string())
                .collect(),
            "warn".to_string(),
            50,
            100,
            provider,
            DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        )
        .expect("service should build");
        service.current_embedding_signature = Some("embsig:new".to_string());
        service.current_embedding_model = Some("test-model".to_string());
        service.current_embedding_dimension = Some(dimension);
        service
    }

    #[tokio::test]
    async fn reembed_rewrites_all_facts_and_marks_job_completed() {
        let db = make_in_memory_db(&["org", "personal"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:one",
            "first fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;
        seed_fact_with_embedding(
            &db,
            "personal",
            "fact:two",
            "second fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let service = make_reembed_service(
            db.clone(),
            vec!["org", "personal"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        let summary = service
            .reembed_all_facts()
            .await
            .expect("reembed should succeed");

        assert_eq!(summary.total_facts, 2);
        assert_eq!(summary.failed_facts, 0);

        let updated = db
            .select_one("fact:one", "org")
            .await
            .expect("select fact")
            .expect("stored fact");
        assert_eq!(
            updated.get("embedding_dimension"),
            Some(&json!(DEFAULT_EMBEDDING_DIMENSION))
        );
        assert_eq!(
            updated.get("embedding_signature"),
            Some(&json!("embsig:new"))
        );

        let job = db
            .select_one("embedding_job:fact_reembed", "org")
            .await
            .expect("select job")
            .expect("stored job");
        assert_eq!(job.get("status"), Some(&json!("completed")));
    }

    #[tokio::test]
    async fn reembed_resume_after_restart_uses_persisted_job_state() {
        let db = make_in_memory_db(&["org"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:one",
            "first fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:two",
            "second fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let interrupted = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::fails_on_call(
                DEFAULT_EMBEDDING_DIMENSION,
                2,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        interrupted
            .reembed_all_facts()
            .await
            .expect_err("first run should stop after one fact");

        let resumed = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        let summary = resumed
            .reembed_all_facts()
            .await
            .expect("resume should succeed");

        assert_eq!(summary.succeeded_facts, 1);

        let job = db
            .select_one("embedding_job:fact_reembed", "org")
            .await
            .expect("select job")
            .expect("stored job");
        assert_eq!(job.get("status"), Some(&json!("completed")));
    }

    #[tokio::test]
    async fn reembed_resume_after_failure_retries_failed_fact_instead_of_skipping_it() {
        let db = make_in_memory_db(&["org"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:one",
            "first fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:two",
            "second fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let first = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::fails_on_call(
                DEFAULT_EMBEDDING_DIMENSION,
                2,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        first
            .reembed_all_facts()
            .await
            .expect_err("first run should fail on second fact");

        let resumed = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        let summary = resumed
            .reembed_all_facts()
            .await
            .expect("resume should succeed");

        assert_eq!(summary.succeeded_facts, 1);
        let updated = db
            .select_one("fact:two", "org")
            .await
            .expect("select fact")
            .expect("stored fact");
        assert_eq!(
            updated.get("embedding_signature"),
            Some(&json!("embsig:new"))
        );
    }

    #[tokio::test]
    async fn reembed_failure_marks_job_failed() {
        let db = make_in_memory_db(&["org"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:bad",
            "bad fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::fails_on_call(
                DEFAULT_EMBEDDING_DIMENSION,
                1,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        let error = service
            .reembed_all_facts()
            .await
            .expect_err("reembed should fail");

        assert!(error.to_string().contains("reembed failed"));

        let job = db
            .select_one("embedding_job:fact_reembed", "org")
            .await
            .expect("select job")
            .expect("stored job");
        assert_eq!(job.get("status"), Some(&json!("failed")));
    }
}
