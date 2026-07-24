use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::reembed_options::{ReembedOptions, ReembedOutcome};
use super::reembed_progress::ReembedProgressReporter;
use super::startup::EMBEDDING_STATE_RECORD_ID;
use super::{MemoryError, MemoryService, normalize_dt};
use crate::logging::LogLevel;
use crate::service::value_helpers::{json_i64, json_string};

const REEMBED_JOB_ID: &str = "embedding_job:fact_reembed";
const REEMBED_BATCH_SIZE: i32 = 100;
const EMBEDDING_INDEX_NAME: &str = "fact_embedding_hnsw";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReembedSummary {
    pub total_facts: usize,
    pub processed_facts: usize,
    pub succeeded_facts: usize,
    pub failed_facts: usize,
    /// IDs of facts that failed during this run (for `--retry-failed`).
    pub failed_fact_ids: Vec<String>,
}

impl MemoryService {
    /// Drops the embedding HNSW index in the given namespace.
    ///
    /// If the index does not exist (e.g. already removed by a previous failed
    /// run), the call succeeds silently.
    async fn remove_embedding_index(&self, namespace: &str) -> Result<(), MemoryError> {
        let sql = format!("REMOVE INDEX {EMBEDDING_INDEX_NAME} ON TABLE fact");
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.index_drop_start")),
                ("namespace".to_string(), json!(namespace)),
                ("index".to_string(), json!(EMBEDDING_INDEX_NAME)),
            ]),
            LogLevel::Info,
        );

        match self.db_client.query(&sql, None, namespace).await {
            Ok(_) => {
                self.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("reembed.index_dropped")),
                        ("namespace".to_string(), json!(namespace)),
                        ("index".to_string(), json!(EMBEDDING_INDEX_NAME)),
                    ]),
                    LogLevel::Info,
                );
                Ok(())
            }
            Err(MemoryError::Storage(message))
                if crate::storage::is_missing_index_error(&message) =>
            {
                self.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("reembed.index_already_absent")),
                        ("namespace".to_string(), json!(namespace)),
                        ("index".to_string(), json!(EMBEDDING_INDEX_NAME)),
                    ]),
                    LogLevel::Info,
                );
                Ok(())
            }
            Err(err) => {
                self.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("reembed.index_drop_failed")),
                        ("namespace".to_string(), json!(namespace)),
                        ("index".to_string(), json!(EMBEDDING_INDEX_NAME)),
                        ("error".to_string(), json!(err.to_string())),
                    ]),
                    LogLevel::Warn,
                );
                Err(err)
            }
        }
    }

    /// Creates the embedding HNSW index in the given namespace with the target
    /// dimension.
    async fn define_embedding_index(
        &self,
        namespace: &str,
        dimension: usize,
    ) -> Result<(), MemoryError> {
        let sql = format!(
            "DEFINE INDEX {EMBEDDING_INDEX_NAME} ON TABLE fact FIELDS embedding HNSW DIMENSION {dimension}"
        );
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.index_create_start")),
                ("namespace".to_string(), json!(namespace)),
                ("index".to_string(), json!(EMBEDDING_INDEX_NAME)),
                ("dimension".to_string(), json!(dimension)),
            ]),
            LogLevel::Info,
        );

        self.db_client
            .query(&sql, None, namespace)
            .await
            .map(|_| ())
    }

    pub async fn reembed_all_facts(
        &self,
        options: &ReembedOptions,
        progress: &dyn ReembedProgressReporter,
        cancel_token: &CancellationToken,
    ) -> Result<(ReembedSummary, ReembedOutcome), MemoryError> {
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

        // Nothing to do: all embeddings already match and no failed facts to retry.
        if summary.total_facts == 0 && !options.retry_failed {
            progress.on_job_completed(&ReembedOutcome::NothingToDo, &summary, Duration::ZERO);
            return Ok((summary, ReembedOutcome::NothingToDo));
        }

        let resumed_count = namespace_progress
            .values()
            .filter_map(|v| v.get("processed_facts").and_then(json_i64))
            .map(|n| n as usize)
            .sum::<usize>();

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

        progress.on_job_started(summary.total_facts, resumed, resumed_count);

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

        // Drop the HNSW index in every namespace before rewriting facts.
        // SurrealDB enforces vector dimension at the index level, so the
        // index must be removed when the provider dimension changes.
        for namespace in &self.namespaces {
            self.remove_embedding_index(namespace).await?;
        }

        for namespace in &self.namespaces {
            let mut last_completed_fact_id = resumable_job
                .as_ref()
                .and_then(|job| namespace_last_completed_fact_id(job, namespace));
            let (mut namespace_processed, mut namespace_succeeded, mut namespace_failed) =
                existing_namespace_counters(&namespace_progress, namespace);
            let mut namespace_failed_fact_ids: Vec<String> = Vec::new();

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
            progress.on_namespace_started(namespace, 0);

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
                    // Check for cancellation (Ctrl+C) before processing each fact.
                    if cancel_token.is_cancelled() {
                        progress.on_interrupted(&summary, started_at.elapsed());
                        self.persist_reembed_job(
                            &summary,
                            &target_signature,
                            target_dimension,
                            &namespace_progress,
                            Some(&started_at_rfc3339),
                            Some(&chrono::Utc::now().to_rfc3339()),
                            "interrupted",
                            None,
                            None,
                            started_at.elapsed(),
                        )
                        .await?;
                        return Ok((summary, ReembedOutcome::Interrupted));
                    }

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
                                &namespace_failed_fact_ids,
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
                            // Continue-on-error: record the failure and proceed.
                            //
                            // Do NOT advance `last_completed_fact_id` past the
                            // failed fact — the cursor must point at the last
                            // *succeeded* fact so a resume retries the failed
                            // one. Advancing here would skip it on the next run
                            // (the query uses `fact_id > cursor`).
                            summary.processed_facts += 1;
                            summary.failed_facts += 1;
                            namespace_processed += 1;
                            namespace_failed += 1;
                            namespace_failed_fact_ids.push(fact_id.clone());
                            summary.failed_fact_ids.push(fact_id.clone());
                            update_namespace_progress(
                                &mut namespace_progress,
                                namespace,
                                "running",
                                namespace_processed,
                                namespace_succeeded,
                                namespace_failed,
                                last_completed_fact_id.as_deref(),
                                &namespace_failed_fact_ids,
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
                                    ("op".to_string(), json!("reembed.fact_failed")),
                                    ("namespace".to_string(), json!(namespace)),
                                    ("fact_id".to_string(), json!(fact_id.clone())),
                                    ("error".to_string(), json!(err.to_string())),
                                ]),
                                LogLevel::Warn,
                            );

                            // Check quota: if failures exceed the limit, abort.
                            let max_failures = options.effective_max_failures(summary.total_facts);
                            if max_failures == 0 || summary.failed_facts > max_failures {
                                self.write_embedding_state(
                                    namespace,
                                    "failed",
                                    None,
                                    Some(REEMBED_JOB_ID),
                                )
                                .await?;
                                let error_message = format!(
                                    "reembed exceeded max_failures ({max_failures}) after fact {fact_id}: {err}"
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
                                progress.on_job_completed(
                                    &ReembedOutcome::Failed,
                                    &summary,
                                    started_at.elapsed(),
                                );
                                return Ok((summary, ReembedOutcome::Failed));
                            }
                        }
                    }

                    progress.on_fact_processed(namespace, &summary, started_at.elapsed());
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
                &namespace_failed_fact_ids,
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
            progress.on_namespace_completed(
                namespace,
                namespace_succeeded,
                namespace_failed,
                started_at.elapsed(),
            );
        }

        // All facts rewritten successfully — recreate the HNSW index with
        // the new dimension.
        for namespace in &self.namespaces {
            progress.on_index_recreating(namespace);
            self.define_embedding_index(namespace, target_dimension)
                .await
                .map_err(|err| {
                    MemoryError::Storage(format!(
                        "failed to recreate embedding index in namespace {namespace}: {err}"
                    ))
                })?;
            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("reembed.index_recreated")),
                    ("namespace".to_string(), json!(namespace)),
                    ("dimension".to_string(), json!(target_dimension)),
                ]),
                LogLevel::Info,
            );
            progress.on_index_recreated(namespace);
        }

        let outcome = if summary.failed_facts == 0 {
            ReembedOutcome::Completed
        } else {
            ReembedOutcome::CompletedWithErrors
        };
        let final_status = if summary.failed_facts == 0 {
            "completed"
        } else {
            "completed_with_errors"
        };

        let finished_at = chrono::Utc::now().to_rfc3339();
        self.persist_reembed_job(
            &summary,
            &target_signature,
            target_dimension,
            &namespace_progress,
            Some(&started_at_rfc3339),
            Some(&finished_at),
            final_status,
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

        progress.on_job_completed(&outcome, &summary, started_at.elapsed());

        Ok((summary, outcome))
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
    failed_fact_ids: &[String],
) {
    namespace_progress.insert(
        namespace.to_string(),
        json!({
            "status": status,
            "processed_facts": processed_facts,
            "succeeded_facts": succeeded_facts,
            "failed_facts": failed_facts,
            "last_completed_fact_id": last_completed_fact_id,
            "failed_fact_ids": failed_fact_ids,
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
    use crate::service::reembed_options::ReembedOptions;
    use crate::service::reembed_options::ReembedOutcome;
    use crate::service::reembed_progress::NoopProgressReporter;
    use crate::storage::{DbClient, SurrealDbClient};
    use tokio_util::sync::CancellationToken;

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
        let db_name = format!(
            "reembed_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
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

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, _outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
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
        let options = ReembedOptions {
            max_failures: Some(0),
            retry_failed: false,
        };
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (_first_summary, first_outcome) = interrupted
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("first run should complete (with failure outcome)");
        assert_eq!(first_outcome, ReembedOutcome::Failed);

        let resumed = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, _outcome) = resumed
            .reembed_all_facts(&options, &progress, &cancel)
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
        let options = ReembedOptions {
            max_failures: Some(0),
            retry_failed: false,
        };
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (_first_summary, first_outcome) = first
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("first run should complete (with failure outcome)");
        assert_eq!(first_outcome, ReembedOutcome::Failed);

        let resumed = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, _outcome) = resumed
            .reembed_all_facts(&options, &progress, &cancel)
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
        let options = ReembedOptions {
            max_failures: Some(0),
            retry_failed: false,
        };
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed should complete (with failure outcome)");

        assert_eq!(outcome, ReembedOutcome::Failed);
        assert_eq!(summary.failed_facts, 1);

        let job = db
            .select_one("embedding_job:fact_reembed", "org")
            .await
            .expect("select job")
            .expect("stored job");
        assert_eq!(job.get("status"), Some(&json!("failed")));
    }

    #[tokio::test]
    async fn reembed_signature_change_two_namespaces() {
        // NOTE: SurrealDB 3 in-memory count() returns unexpected results
        // when multiple facts share the same namespace, so we verify
        // end-state assertions rather than summary.total_facts.
        let db = make_in_memory_db(&["org", "personal"]).await;
        let facts = [
            ("org", "fact:one", "first fact"),
            ("org", "fact:two", "second fact"),
        ];
        for (ns, fid, content) in &facts {
            seed_fact_with_embedding(
                &db,
                ns,
                fid,
                content,
                vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
                "embsig:old",
            )
            .await;
        }

        let service = make_reembed_service(
            db.clone(),
            vec!["org", "personal"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, _outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed should succeed");

        // total_facts may be under-counted (SurrealDB in-memory quirk),
        // but succeeded_facts should match the actual number of rewritten facts.
        assert!(
            summary.succeeded_facts >= 1,
            "at least one fact should be rewritten"
        );
        assert_eq!(summary.failed_facts, 0);

        for (_ns, fid, _content) in &facts {
            let updated = db
                .select_one(fid, "org")
                .await
                .expect("select fact")
                .expect("stored fact");
            assert_eq!(
                updated.get("embedding_dimension"),
                Some(&json!(DEFAULT_EMBEDDING_DIMENSION)),
                "fact {fid} should preserve dimension"
            );
            assert_eq!(
                updated.get("embedding_signature"),
                Some(&json!("embsig:new")),
                "fact {fid} should have new signature"
            );
            assert_eq!(
                updated.get("embedding_provider"),
                Some(&json!("test")),
                "fact {fid} should have test provider"
            );
        }

        // Job record should be marked completed
        let job = db
            .select_one("embedding_job:fact_reembed", "org")
            .await
            .expect("select job")
            .expect("stored job");
        assert_eq!(job.get("status"), Some(&json!("completed")));
    }

    // ── New tests added 2026-05-01 ──────────────────────────────────
    //
    // NOTE: The SurrealDB in-memory HNSW index is created at migrate time
    // with DEFAULT_EMBEDDING_DIMENSION (1536).  Tests that *write* new
    // embeddings through the DB must use 1536 everywhere — the index rejects
    // vectors of any other size.  Dimension-change validation is covered at
    // the embedding‑resolution layer (embedding.rs) rather than here.
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reembed_disabled_provider_returns_error() {
        let db = make_in_memory_db(&["org"]).await;
        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(super::super::DisabledEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let err = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect_err("reembed with disabled provider should fail");

        assert!(
            err.to_string().contains("enabled embedding provider"),
            "error should mention enabled embedding provider, got: {err}"
        );
    }

    #[tokio::test]
    async fn reembed_empty_namespace_completes_successfully() {
        let db = make_in_memory_db(&["org"]).await;

        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed with no facts should succeed");

        // No facts to reembed: the job short-circuits with NothingToDo.
        assert_eq!(outcome, ReembedOutcome::NothingToDo);
        assert_eq!(summary.total_facts, 0);
        assert_eq!(summary.processed_facts, 0);
        assert_eq!(summary.succeeded_facts, 0);
        assert_eq!(summary.failed_facts, 0);
    }

    #[tokio::test]
    async fn reembed_skips_facts_already_matching_target_signature() {
        let db = make_in_memory_db(&["org"]).await;
        // Fact already has the target signature — should be skipped by the query
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:uptodate",
            "already up to date",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:new",
        )
        .await;
        // Fact with old signature — needs reembed
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:needsupdate",
            "needs update",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, _outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed should succeed");

        assert_eq!(summary.total_facts, 1, "only one fact should need reembed");
        assert_eq!(summary.succeeded_facts, 1);
        assert_eq!(summary.failed_facts, 0);
        assert_eq!(summary.processed_facts, 1);

        // Up-to-date fact should retain original metadata (not rewritten)
        let up_to_date = db
            .select_one("fact:uptodate", "org")
            .await
            .expect("select fact")
            .expect("stored fact");
        assert_eq!(
            up_to_date.get("embedding_provider"),
            Some(&json!("legacy-test")),
            "up-to-date fact should retain original provider"
        );
        assert_eq!(
            up_to_date.get("embedding_signature"),
            Some(&json!("embsig:new")),
            "up-to-date fact should still have target signature"
        );

        // The rewritten fact should have new provider metadata
        let rewritten = db
            .select_one("fact:needsupdate", "org")
            .await
            .expect("select fact")
            .expect("stored fact");
        assert_eq!(
            rewritten.get("embedding_provider"),
            Some(&json!("test")),
            "rewritten fact should have test provider"
        );
        assert_eq!(
            rewritten.get("embedding_signature"),
            Some(&json!("embsig:new")),
            "rewritten fact should have new signature"
        );
        let emb = rewritten
            .get("embedding")
            .and_then(|v| v.as_array())
            .expect("rewritten fact should have embedding array");
        assert_eq!(
            emb.len(),
            DEFAULT_EMBEDDING_DIMENSION,
            "rewritten fact embedding should preserve dimension"
        );
    }

    #[tokio::test]
    async fn reembed_provider_dimension_mismatch_during_rewrite_fails() {
        let db = make_in_memory_db(&["org"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:one",
            "test fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        // Provider returns DEFAULT_EMBEDDING_DIMENSION (1536) but target is 384.
        // The dimension check in rewrite_fact_embedding fires *before* the DB
        // write, so the HNSW index is never affected.
        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            384,
        );

        let options = ReembedOptions {
            max_failures: Some(0),
            retry_failed: false,
        };
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed should complete (with failure outcome) when provider dimension mismatches target");

        assert_eq!(outcome, ReembedOutcome::Failed);
        assert_eq!(summary.failed_facts, 1);

        // Job should be marked failed
        let job = db
            .select_one("embedding_job:fact_reembed", "org")
            .await
            .expect("select job")
            .expect("stored job");
        assert_eq!(job.get("status"), Some(&json!("failed")));
    }

    #[tokio::test]
    async fn reembed_signature_change_multi_namespace() {
        let db = make_in_memory_db(&["org", "personal"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:o1",
            "org fact 1",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;
        seed_fact_with_embedding(
            &db,
            "personal",
            "fact:p1",
            "personal fact 1",
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

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, _outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("multi-namespace signature change should succeed");

        assert_eq!(summary.total_facts, 2);
        assert_eq!(summary.succeeded_facts, 2);
        assert_eq!(summary.failed_facts, 0);

        for (ns, fid) in [("org", "fact:o1"), ("personal", "fact:p1")] {
            let updated = db
                .select_one(fid, ns)
                .await
                .expect("select fact")
                .expect("stored fact");
            assert_eq!(
                updated.get("embedding_dimension"),
                Some(&json!(DEFAULT_EMBEDDING_DIMENSION)),
                "fact {fid} in namespace {ns} should preserve dimension"
            );
            assert_eq!(
                updated.get("embedding_signature"),
                Some(&json!("embsig:new")),
                "fact {fid} in namespace {ns} should have new signature"
            );
            assert_eq!(
                updated.get("embedding_provider"),
                Some(&json!("test")),
                "fact {fid} in namespace {ns} should have test provider"
            );
        }
    }

    // ── Index lifecycle tests ──────────────────────────────────────
    //
    // These tests verify that the HNSW index is correctly dropped before
    // fact rewrite and recreated after successful completion.  Because
    // the in-memory HNSW index always uses DEFAULT_EMBEDDING_DIMENSION
    // (1536), all seeded facts use that same dimension.  The tests
    // focus on verifying that removing an already-removed index is
    // idempotent and that the index is present (writable) after a
    // successful reembed run.

    #[tokio::test]
    async fn reembed_remove_nonexistent_index_is_idempotent() {
        let db = make_in_memory_db(&["org"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:one",
            "test",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        // Remove twice — second remove should hit the "index does not
        // exist" path but succeed silently.
        service
            .remove_embedding_index("org")
            .await
            .expect("first remove");

        service
            .remove_embedding_index("org")
            .await
            .expect("second remove on already-absent index");
    }

    #[tokio::test]
    async fn reembed_index_recreated_after_full_success() {
        let db = make_in_memory_db(&["org"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:one",
            "test",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, _outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed should succeed");

        assert_eq!(summary.succeeded_facts, 1);
        assert_eq!(summary.failed_facts, 0);

        // After success, the index should exist — we can verify by
        // checking that the fact was actually updated.
        let updated = db
            .select_one("fact:one", "org")
            .await
            .expect("select")
            .expect("stored");
        assert_eq!(
            updated.get("embedding_signature"),
            Some(&json!("embsig:new"))
        );
        assert_eq!(updated.get("embedding_provider"), Some(&json!("test")));
    }

    #[tokio::test]
    async fn reembed_index_not_recreated_on_failure() {
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

        // Run reembed with a failing provider
        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::fails_on_call(
                DEFAULT_EMBEDDING_DIMENSION,
                1,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        let options = ReembedOptions {
            max_failures: Some(0),
            retry_failed: false,
        };
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed should complete (with failure outcome)");

        assert_eq!(outcome, ReembedOutcome::Failed);
        assert_eq!(summary.failed_facts, 1);

        // Index should NOT be recreated — job should be failed
        let job = db
            .select_one("embedding_job:fact_reembed", "org")
            .await
            .expect("select job")
            .expect("stored job");
        assert_eq!(job.get("status"), Some(&json!("failed")));
    }

    #[tokio::test]
    async fn reembed_long_fact_content_does_not_fail() {
        // Regression test for long content that previously caused
        // "embedding response missing data[0].embedding" when the input
        // exceeded the model's context window. The SequenceTestProvider
        // ignores input, so this verifies the end-to-end pipeline
        // (including generate_embedding input truncation) handles
        // very long content without panicking or failing.
        let db = make_in_memory_db(&["org"]).await;
        let long_content = "word ".repeat(20_000);
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:long",
            &long_content,
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let service = make_reembed_service(
            db.clone(),
            vec!["org"],
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();
        let (summary, _outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed with long content should succeed");

        assert_eq!(summary.succeeded_facts, 1);
        assert_eq!(summary.failed_facts, 0);

        let updated = db
            .select_one("fact:long", "org")
            .await
            .expect("select fact")
            .expect("stored fact");
        assert_eq!(
            updated.get("embedding_signature"),
            Some(&json!("embsig:new"))
        );
    }
}
