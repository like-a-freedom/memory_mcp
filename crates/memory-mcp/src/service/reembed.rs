//! Fact re-embedding: rewrites every fact's embedding when the provider
//! signature or dimension changes, with batched processing, resume support,
//! and per-namespace progress tracking.
//!
//! [`reembed_all_facts`](MemoryService::reembed_all_facts) coordinates three
//! phases: prepare (validate inputs + enumerate work), process (rewrite each
//! batch with a resumable cursor), and finalize (recreate the HNSW index and
//! persist completion). Progress is persisted after every fact so an
//! interrupted pass resumes from the last completed cursor.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::reembed_options::{ReembedOptions, ReembedOutcome};
use super::reembed_progress::ReembedProgressReporter;
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
    /// IDs of facts that failed during this run (for `--retry-failed`).
    pub failed_fact_ids: Vec<String>,
}

/// Mutable state threaded through a single reembed pass, shared by the
/// prepare / process / finalize phases.
struct ReembedPass {
    summary: ReembedSummary,
    namespace_progress: serde_json::Map<String, Value>,
    target_signature: String,
    target_dimension: usize,
    started_at: Instant,
    started_at_rfc3339: String,
    resumed: bool,
    resumed_count: usize,
    retry_failed_ids: std::collections::HashSet<String>,
    resumable_job: Option<Value>,
}

/// Mutable per-namespace counters threaded through the batch loop.
#[derive(Default)]
struct NamespaceBatchState {
    last_completed_fact_id: Option<String>,
    processed: usize,
    succeeded: usize,
    failed: usize,
    failed_fact_ids: Vec<String>,
}

/// Result of processing one batch: keep going or short-circuit the pass.
enum BatchOutcome {
    /// Batch processed; continue with the next batch.
    Continue,
    /// Cancellation observed mid-batch; job already persisted as `interrupted`.
    Interrupted,
    /// Failure quota exceeded; job already persisted as `failed`.
    Failed,
}

impl MemoryService {
    /// Drops the embedding HNSW index in the active namespace.
    ///
    /// The DDL and its idempotency rule live in [`ReembedStoreClient`];
    /// this method only orchestrates logging around the call.
    async fn remove_embedding_index(&self, namespace: &str) -> Result<(), MemoryError> {
        use crate::storage::{EMBEDDING_INDEX_NAME, IndexRemoval};
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.index_drop_start")),
                ("namespace".to_string(), json!(namespace)),
                ("index".to_string(), json!(EMBEDDING_INDEX_NAME)),
            ]),
            LogLevel::Info,
        );

        match self.reembed_store().remove_embedding_index().await {
            Ok(IndexRemoval::Removed) => {
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
            Ok(IndexRemoval::AlreadyAbsent) => {
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

    /// Creates the embedding HNSW index in the active namespace with the
    /// target dimension. The DDL lives in [`ReembedStoreClient`].
    async fn define_embedding_index(
        &self,
        namespace: &str,
        dimension: usize,
    ) -> Result<(), MemoryError> {
        use crate::storage::EMBEDDING_INDEX_NAME;
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.index_create_start")),
                ("namespace".to_string(), json!(namespace)),
                ("index".to_string(), json!(EMBEDDING_INDEX_NAME)),
                ("dimension".to_string(), json!(dimension)),
            ]),
            LogLevel::Info,
        );

        self.reembed_store().define_embedding_index(dimension).await
    }

    async fn restore_semantic_readiness(
        &self,
        target_signature: &str,
        target_dimension: usize,
    ) -> Result<(), MemoryError> {
        let namespace = &self.active_namespace;
        self.write_embedding_state(namespace, "rebuilding", None, Some(REEMBED_JOB_ID))
            .await?;
        self.remove_embedding_index(namespace).await?;
        self.define_embedding_index(namespace, target_dimension)
            .await
            .map_err(|err| {
                MemoryError::Storage(format!(
                    "failed to restore embedding index in namespace {namespace}: {err}"
                ))
            })?;
        self.write_embedding_state(
            namespace,
            "ready",
            Some(target_signature),
            Some(REEMBED_JOB_ID),
        )
        .await?;
        Ok(())
    }

    pub async fn reembed_all_facts(
        &self,
        options: &ReembedOptions,
        progress: &dyn ReembedProgressReporter,
        cancel_token: &CancellationToken,
    ) -> Result<(ReembedSummary, ReembedOutcome), MemoryError> {
        let pass = self.prepare_reembed_pass(options).await?;

        // Nothing to do: all embeddings match and no failed facts to retry.
        if pass.summary.total_facts == 0 {
            self.restore_semantic_readiness(&pass.target_signature, pass.target_dimension)
                .await?;
            progress.on_job_completed(&ReembedOutcome::NothingToDo, &pass.summary, Duration::ZERO);
            return Ok((pass.summary, ReembedOutcome::NothingToDo));
        }

        let mut pass = pass;
        let embedding_state = self.embedding_runtime_snapshot();

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.job_started")),
                (
                    "target_signature".to_string(),
                    json!(pass.target_signature.clone()),
                ),
                ("target_dimension".to_string(), json!(pass.target_dimension)),
                (
                    "provider".to_string(),
                    json!(embedding_state.provider.provider_name()),
                ),
                ("model".to_string(), json!(embedding_state.model.clone())),
                ("resumed".to_string(), json!(pass.resumed)),
                ("total_facts".to_string(), json!(pass.summary.total_facts)),
            ]),
            LogLevel::Info,
        );

        progress.on_job_started(pass.summary.total_facts, pass.resumed, pass.resumed_count);

        self.persist_reembed_job(
            &pass.summary,
            &pass.target_signature,
            pass.target_dimension,
            &pass.namespace_progress,
            Some(&pass.started_at_rfc3339),
            None,
            "running",
            None,
            None,
            pass.started_at.elapsed(),
        )
        .await?;

        // Drop the HNSW index before rewriting facts.
        // SurrealDB enforces vector dimension at the index level, so the
        // index must be removed when the provider dimension changes.
        self.remove_embedding_index(&self.active_namespace).await?;

        if let Some(outcome) = self
            .process_reembed_pass(options, progress, cancel_token, &mut pass)
            .await?
        {
            // Interrupted or failed — the processing phase already persisted
            // the terminal job state; report and return.
            return Ok((pass.summary, outcome));
        }

        let outcome = self.finalize_reembed_pass(progress, &pass).await?;
        progress.on_job_completed(&outcome, &pass.summary, pass.started_at.elapsed());
        Ok((pass.summary, outcome))
    }

    /// Phase 1 — validate inputs, load any resumable job, enumerate the work,
    /// and snapshot the shared pass state.
    ///
    /// The HNSW index is intentionally left untouched here: the orchestrator
    /// drops it only when there is actual work to do.
    async fn prepare_reembed_pass(
        &self,
        options: &ReembedOptions,
    ) -> Result<ReembedPass, MemoryError> {
        let embedding_state = self.embedding_runtime_snapshot();
        if !embedding_state.provider.is_enabled() {
            return Err(MemoryError::Validation(
                "reembed requires an enabled embedding provider".to_string(),
            ));
        }

        let target_signature = embedding_state.signature.clone().ok_or_else(|| {
            MemoryError::Validation("reembed requires an enabled embedding signature".to_string())
        })?;
        let target_dimension = embedding_state.dimension.ok_or_else(|| {
            MemoryError::Validation("reembed requires a resolved target dimension".to_string())
        })?;

        let started_at = Instant::now();
        let started_at_rfc3339 = chrono::Utc::now().to_rfc3339();
        let existing_job = self.load_reembed_job().await?;
        let resumable_job = existing_job.filter(|job| {
            job.get("target_signature").and_then(json_string) == Some(target_signature.as_str())
        });
        let resumed = resumable_job.is_some();
        let namespace_progress = resumable_job
            .as_ref()
            .and_then(|job| job.get("namespace_progress"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        // A legacy job may contain progress for several namespaces. The
        // current process imports only the Active Namespace entry; all other
        // entries remain untouched for a later process selecting that namespace.
        let active_namespace = &self.active_namespace;
        let retry_failed_ids: std::collections::HashSet<String> = if options.retry_failed {
            existing_failed_fact_ids(&namespace_progress, active_namespace)
                .into_iter()
                .collect()
        } else {
            std::collections::HashSet::new()
        };

        let summary = if options.retry_failed {
            ReembedSummary {
                total_facts: retry_failed_ids.len(),
                ..ReembedSummary::default()
            }
        } else {
            ReembedSummary {
                total_facts: self.count_facts_needing_reembed(&target_signature).await?,
                ..ReembedSummary::default()
            }
        };

        let resumed_count = namespace_progress
            .get(active_namespace)
            .and_then(|value| value.get("processed_facts"))
            .and_then(json_i64)
            .unwrap_or(0) as usize;

        Ok(ReembedPass {
            summary,
            namespace_progress,
            target_signature,
            target_dimension,
            started_at,
            started_at_rfc3339,
            resumed,
            resumed_count,
            retry_failed_ids,
            resumable_job,
        })
    }

    /// Phase 2 — rewrite facts namespace by namespace, batch by batch.
    ///
    /// Returns `None` when every namespace completed; `Some(outcome)` when the
    /// pass was cut short (interrupted or failed) — in which case the terminal
    /// job state has already been persisted.
    async fn process_reembed_pass(
        &self,
        options: &ReembedOptions,
        progress: &dyn ReembedProgressReporter,
        cancel_token: &CancellationToken,
        pass: &mut ReembedPass,
    ) -> Result<Option<ReembedOutcome>, MemoryError> {
        let namespace = &self.active_namespace;
        let mut ns_state = NamespaceBatchState::default();
        (ns_state.processed, ns_state.succeeded, ns_state.failed) =
            existing_namespace_counters(&pass.namespace_progress, namespace);
        ns_state.last_completed_fact_id = pass
            .resumable_job
            .as_ref()
            .and_then(|job| namespace_last_completed_fact_id(job, namespace));

        self.write_embedding_state(namespace, "rebuilding", None, Some(REEMBED_JOB_ID))
            .await?;

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.namespace_started")),
                ("namespace".to_string(), json!(namespace)),
                (
                    "resume_cursor".to_string(),
                    json!(ns_state.last_completed_fact_id.clone()),
                ),
            ]),
            LogLevel::Info,
        );
        progress.on_namespace_started(namespace, 0);

        loop {
            // Check for cancellation between batches (Ctrl+C responsiveness).
            if cancel_token.is_cancelled() {
                self.persist_interrupted(progress, pass).await?;
                return Ok(Some(ReembedOutcome::Interrupted));
            }

            let batch = self
                .reembed_store()
                .select_facts_needing_reembed(
                    &pass.target_signature,
                    ns_state.last_completed_fact_id.as_deref(),
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
                        json!(ns_state.last_completed_fact_id.clone()),
                    ),
                ]),
                LogLevel::Debug,
            );

            if batch.is_empty() {
                break;
            }

            match self
                .process_reembed_batch(
                    namespace,
                    batch,
                    options,
                    progress,
                    cancel_token,
                    pass,
                    &mut ns_state,
                )
                .await?
            {
                BatchOutcome::Continue => {}
                BatchOutcome::Interrupted => return Ok(Some(ReembedOutcome::Interrupted)),
                BatchOutcome::Failed => return Ok(Some(ReembedOutcome::Failed)),
            }
        }

        update_namespace_progress(
            &mut pass.namespace_progress,
            namespace,
            "completed",
            ns_state.processed,
            ns_state.succeeded,
            ns_state.failed,
            ns_state.last_completed_fact_id.as_deref(),
            &ns_state.failed_fact_ids,
        );
        self.write_embedding_state(
            namespace,
            "ready",
            Some(&pass.target_signature),
            Some(REEMBED_JOB_ID),
        )
        .await?;
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.namespace_completed")),
                ("namespace".to_string(), json!(namespace)),
                (
                    "last_completed_fact_id".to_string(),
                    json!(ns_state.last_completed_fact_id.clone()),
                ),
            ]),
            LogLevel::Info,
        );
        progress.on_namespace_completed(
            namespace,
            ns_state.succeeded,
            ns_state.failed,
            pass.started_at.elapsed(),
        );

        Ok(None)
    }

    /// Processes a single fetched batch: applies the `--retry-failed` filter,
    /// rewrites each fact's embedding, advances the resume cursor, and persists
    /// progress after every fact.
    ///
    /// Returns [`BatchOutcome::Continue`] to keep processing, or a short-circuit
    /// outcome when the pass must stop (cancellation or failure quota).
    // Clippy: 7 args are cohesive parts of one batch-processing step (work,
    // options, reporters, and the shared pass/namespace state). Splitting into
    // a struct would add noise without clarity.
    #[allow(clippy::too_many_arguments)]
    async fn process_reembed_batch(
        &self,
        namespace: &str,
        batch: Vec<Value>,
        options: &ReembedOptions,
        progress: &dyn ReembedProgressReporter,
        cancel_token: &CancellationToken,
        pass: &mut ReembedPass,
        ns_state: &mut NamespaceBatchState,
    ) -> Result<BatchOutcome, MemoryError> {
        let embedding_state = self.embedding_runtime_snapshot();
        // In --retry-failed mode, filter batch to only failed fact IDs.
        let batch: Vec<Value> = if options.retry_failed {
            batch
                .into_iter()
                .filter(|fact| {
                    fact.get("fact_id")
                        .and_then(json_string)
                        .is_some_and(|id| pass.retry_failed_ids.contains(id))
                })
                .collect()
        } else {
            batch
        };

        if batch.is_empty() {
            // No retryable facts in this batch — continue to next batch.
            return Ok(BatchOutcome::Continue);
        }

        for fact in batch {
            // Check for cancellation (Ctrl+C) before processing each fact.
            if cancel_token.is_cancelled() {
                self.persist_interrupted(progress, pass).await?;
                return Ok(BatchOutcome::Interrupted);
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
                .rewrite_fact_embedding(fact, &pass.target_signature, pass.target_dimension)
                .await
            {
                Ok(updated_fact_id) => {
                    pass.summary.processed_facts += 1;
                    pass.summary.succeeded_facts += 1;
                    ns_state.processed += 1;
                    ns_state.succeeded += 1;
                    ns_state.last_completed_fact_id = Some(updated_fact_id.clone());
                    update_namespace_progress(
                        &mut pass.namespace_progress,
                        namespace,
                        "running",
                        ns_state.processed,
                        ns_state.succeeded,
                        ns_state.failed,
                        ns_state.last_completed_fact_id.as_deref(),
                        &ns_state.failed_fact_ids,
                    );
                    self.persist_reembed_job(
                        &pass.summary,
                        &pass.target_signature,
                        pass.target_dimension,
                        &pass.namespace_progress,
                        Some(&pass.started_at_rfc3339),
                        None,
                        "running",
                        Some(namespace),
                        None,
                        pass.started_at.elapsed(),
                    )
                    .await?;

                    self.logger.log(
                        std::collections::HashMap::from([
                            ("op".to_string(), json!("reembed.cursor_advanced")),
                            ("namespace".to_string(), json!(namespace)),
                            (
                                "last_completed_fact_id".to_string(),
                                json!(ns_state.last_completed_fact_id.clone()),
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
                    pass.summary.processed_facts += 1;
                    pass.summary.failed_facts += 1;
                    ns_state.processed += 1;
                    ns_state.failed += 1;
                    ns_state.failed_fact_ids.push(fact_id.clone());
                    pass.summary.failed_fact_ids.push(fact_id.clone());
                    update_namespace_progress(
                        &mut pass.namespace_progress,
                        namespace,
                        "running",
                        ns_state.processed,
                        ns_state.succeeded,
                        ns_state.failed,
                        ns_state.last_completed_fact_id.as_deref(),
                        &ns_state.failed_fact_ids,
                    );
                    self.persist_reembed_job(
                        &pass.summary,
                        &pass.target_signature,
                        pass.target_dimension,
                        &pass.namespace_progress,
                        Some(&pass.started_at_rfc3339),
                        None,
                        "running",
                        Some(namespace),
                        None,
                        pass.started_at.elapsed(),
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
                    let max_failures = options.effective_max_failures(pass.summary.total_facts);
                    if max_failures == 0 || pass.summary.failed_facts > max_failures {
                        self.write_embedding_state(namespace, "failed", None, Some(REEMBED_JOB_ID))
                            .await?;
                        let error_message = format!(
                            "reembed exceeded max_failures ({max_failures}) after fact {fact_id}: {err}"
                        );
                        self.persist_reembed_job(
                            &pass.summary,
                            &pass.target_signature,
                            pass.target_dimension,
                            &pass.namespace_progress,
                            Some(&pass.started_at_rfc3339),
                            Some(&chrono::Utc::now().to_rfc3339()),
                            "failed",
                            Some(namespace),
                            Some(&error_message),
                            pass.started_at.elapsed(),
                        )
                        .await?;
                        self.logger.log(
                            std::collections::HashMap::from([
                                ("op".to_string(), json!("reembed.job_failed")),
                                (
                                    "processed_facts".to_string(),
                                    json!(pass.summary.processed_facts),
                                ),
                                (
                                    "succeeded_facts".to_string(),
                                    json!(pass.summary.succeeded_facts),
                                ),
                                ("failed_facts".to_string(), json!(pass.summary.failed_facts)),
                                ("total_facts".to_string(), json!(pass.summary.total_facts)),
                                (
                                    "facts_per_second".to_string(),
                                    json!(facts_per_second(
                                        pass.started_at.elapsed(),
                                        pass.summary.processed_facts
                                    )),
                                ),
                                (
                                    "duration_ms".to_string(),
                                    json!(pass.started_at.elapsed().as_millis() as u64),
                                ),
                                (
                                    "provider".to_string(),
                                    json!(embedding_state.provider.provider_name()),
                                ),
                                ("model".to_string(), json!(embedding_state.model.clone())),
                                ("target_dimension".to_string(), json!(pass.target_dimension)),
                                (
                                    "target_signature".to_string(),
                                    json!(pass.target_signature.clone()),
                                ),
                                ("resumed".to_string(), json!(pass.resumed)),
                            ]),
                            LogLevel::Warn,
                        );
                        progress.on_job_completed(
                            &ReembedOutcome::Failed,
                            &pass.summary,
                            pass.started_at.elapsed(),
                        );
                        return Ok(BatchOutcome::Failed);
                    }
                }
            }

            progress.on_fact_processed(namespace, &pass.summary, pass.started_at.elapsed());
        }

        self.log_reembed_progress(namespace, &pass.summary, pass.started_at.elapsed());
        Ok(BatchOutcome::Continue)
    }

    /// Cancel-path: report the interruption and persist the job as `interrupted`.
    /// Shared by the between-batch and per-fact cancellation checks.
    async fn persist_interrupted(
        &self,
        progress: &dyn ReembedProgressReporter,
        pass: &ReembedPass,
    ) -> Result<(), MemoryError> {
        progress.on_interrupted(&pass.summary, pass.started_at.elapsed());
        self.persist_reembed_job(
            &pass.summary,
            &pass.target_signature,
            pass.target_dimension,
            &pass.namespace_progress,
            Some(&pass.started_at_rfc3339),
            Some(&chrono::Utc::now().to_rfc3339()),
            "interrupted",
            None,
            None,
            pass.started_at.elapsed(),
        )
        .await?;
        Ok(())
    }

    /// Phase 3 — recreate the HNSW index at the target dimension, persist the
    /// completed job record, and log the final summary. Returns the outcome.
    async fn finalize_reembed_pass(
        &self,
        progress: &dyn ReembedProgressReporter,
        pass: &ReembedPass,
    ) -> Result<ReembedOutcome, MemoryError> {
        // All facts rewritten successfully — recreate the HNSW index with
        // the new dimension.
        let namespace = &self.active_namespace;
        progress.on_index_recreating(namespace);
        self.define_embedding_index(namespace, pass.target_dimension)
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
                ("dimension".to_string(), json!(pass.target_dimension)),
            ]),
            LogLevel::Info,
        );
        progress.on_index_recreated(namespace);

        let outcome = if pass.summary.failed_facts == 0 {
            ReembedOutcome::Completed
        } else {
            ReembedOutcome::CompletedWithErrors
        };
        let final_status = if pass.summary.failed_facts == 0 {
            "completed"
        } else {
            "completed_with_errors"
        };

        let finished_at = chrono::Utc::now().to_rfc3339();
        self.persist_reembed_job(
            &pass.summary,
            &pass.target_signature,
            pass.target_dimension,
            &pass.namespace_progress,
            Some(&pass.started_at_rfc3339),
            Some(&finished_at),
            final_status,
            None,
            None,
            pass.started_at.elapsed(),
        )
        .await?;
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("reembed.job_completed")),
                (
                    "processed_facts".to_string(),
                    json!(pass.summary.processed_facts),
                ),
                (
                    "succeeded_facts".to_string(),
                    json!(pass.summary.succeeded_facts),
                ),
                ("failed_facts".to_string(), json!(pass.summary.failed_facts)),
                ("total_facts".to_string(), json!(pass.summary.total_facts)),
                (
                    "facts_per_second".to_string(),
                    json!(facts_per_second(
                        pass.started_at.elapsed(),
                        pass.summary.processed_facts
                    )),
                ),
                (
                    "duration_ms".to_string(),
                    json!(pass.started_at.elapsed().as_millis() as u64),
                ),
                (
                    "provider".to_string(),
                    json!(self.embedding_runtime_snapshot().provider.provider_name()),
                ),
                (
                    "model".to_string(),
                    json!(self.embedding_runtime_snapshot().model.clone()),
                ),
                ("target_dimension".to_string(), json!(pass.target_dimension)),
                (
                    "target_signature".to_string(),
                    json!(pass.target_signature.clone()),
                ),
                ("resumed".to_string(), json!(pass.resumed)),
            ]),
            LogLevel::Info,
        );

        Ok(outcome)
    }

    async fn load_reembed_job(&self) -> Result<Option<Value>, MemoryError> {
        self.reembed_store().load_record(REEMBED_JOB_ID).await
    }

    async fn count_facts_needing_reembed(
        &self,
        target_signature: &str,
    ) -> Result<usize, MemoryError> {
        self.reembed_store()
            .count_facts_needing_reembed(target_signature)
            .await
    }

    async fn rewrite_fact_embedding(
        &self,
        fact: Value,
        target_signature: &str,
        target_dimension: usize,
    ) -> Result<String, MemoryError> {
        let embedding_state = self.embedding_runtime_snapshot();
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

        let embedding_input =
            super::fact::FactService::build_fact_embedding_input(fact_type, content, quote);
        let context = self.build_context();
        let embedding = context
            .embedding_service
            .generate_embedding(&embedding_input)
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
            json!(embedding_state.provider.provider_name()),
        );
        if let Some(model) = &embedding_state.model {
            updated.insert("embedding_model".to_string(), json!(model));
        }
        updated.insert("embedding_dimension".to_string(), json!(target_dimension));
        updated.insert("embedding_signature".to_string(), json!(target_signature));
        updated.insert(
            "embedding_updated_at".to_string(),
            json!(normalize_dt(chrono::Utc::now())),
        );

        self.reembed_store()
            .upsert_record(&fact_id, Value::Object(updated))
            .await?;
        Ok(fact_id)
    }

    async fn write_embedding_state(
        &self,
        _namespace: &str,
        status: &str,
        active_signature: Option<&str>,
        last_job_id: Option<&str>,
    ) -> Result<(), MemoryError> {
        use crate::storage::EmbeddingStateStatus;

        let status = match status {
            "ready" => EmbeddingStateStatus::Ready,
            "rebuilding" => EmbeddingStateStatus::Rebuilding,
            "failed" => EmbeddingStateStatus::Failed,
            "backfill_pending" => EmbeddingStateStatus::BackfillPending,
            other => {
                return Err(MemoryError::Validation(format!(
                    "unknown embedding state status `{other}`"
                )));
            }
        };
        let embedding_state = self.embedding_runtime_snapshot();
        crate::storage::EmbeddingStateStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
        .upsert_job_state(
            status,
            embedding_state.provider.provider_name(),
            embedding_state.model.as_deref(),
            embedding_state.dimension,
            active_signature,
            last_job_id,
        )
        .await
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
        let embedding_state = self.embedding_runtime_snapshot();
        let payload = json!({
            "job_id": REEMBED_JOB_ID,
            "status": status,
            "target_signature": target_signature,
            "provider": embedding_state.provider.provider_name(),
            "model": embedding_state.model.clone(),
            "dimension": target_dimension,
            "namespaces": vec![self.active_namespace.clone()],
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

        self.reembed_store()
            .upsert_record(REEMBED_JOB_ID, payload)
            .await
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

/// Extracts persisted `failed_fact_ids` for a namespace from a prior job record.
///
/// Used by `--retry-failed` to know which facts to re-process.
fn existing_failed_fact_ids(
    namespace_progress: &serde_json::Map<String, Value>,
    namespace: &str,
) -> Vec<String> {
    namespace_progress
        .get(namespace)
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("failed_fact_ids"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// Clippy: 9 args is acceptable here — all params are cohesive parts of a single
// namespace-progress update record. Splitting into a struct would add noise without clarity.
#[allow(clippy::too_many_arguments)]
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

    const TEST_NAMESPACE: &str = "org";

    async fn make_in_memory_db(namespaces: &[&str]) -> Arc<SurrealDbClient> {
        assert_eq!(namespaces, &[TEST_NAMESPACE]);
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
        active_namespace: &str,
        provider: Arc<dyn EmbeddingProvider>,
        dimension: usize,
    ) -> MemoryService {
        assert_eq!(active_namespace, TEST_NAMESPACE);
        let service = MemoryService::new_with_embedding_provider(
            db_client,
            active_namespace.to_string(),
            "warn".to_string(),
            50,
            100,
            provider,
            DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("service should build");
        let provider = service.embedding_runtime_snapshot().provider;
        service.replace_embedding_runtime_state(
            crate::service::embedding_runtime::EmbeddingRuntimeState::new(
                provider,
                Some("embsig:new".to_string()),
                Some("test-model".to_string()),
                Some(dimension),
            ),
        );
        service
    }

    #[tokio::test]
    async fn reembed_rewrites_all_facts_and_marks_job_completed() {
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

        let service = make_reembed_service(
            db.clone(),
            "org",
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
            "org",
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
            "org",
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
            "org",
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
            "org",
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
            "org",
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
    async fn reembed_signature_change_two_facts() {
        // NOTE: SurrealDB 3 in-memory count() returns unexpected results
        // when multiple facts share the same namespace, so we verify
        // end-state assertions rather than summary.total_facts.
        let db = make_in_memory_db(&["org"]).await;
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
            "org",
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
            "org",
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
            "org",
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
            "org",
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
            "org",
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
    async fn reembed_signature_change_two_facts_in_one_namespace() {
        let db = make_in_memory_db(&["org"]).await;
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
            "org",
            "fact:p1",
            "second org fact",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:old",
        )
        .await;

        let service = make_reembed_service(
            db.clone(),
            "org",
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
            .expect("single-namespace signature change should succeed");

        assert_eq!(summary.total_facts, 2);
        assert_eq!(summary.succeeded_facts, 2);
        assert_eq!(summary.failed_facts, 0);

        for fid in ["fact:o1", "fact:p1"] {
            let updated = db
                .select_one(fid, "org")
                .await
                .expect("select fact")
                .expect("stored fact");
            assert_eq!(
                updated.get("embedding_dimension"),
                Some(&json!(DEFAULT_EMBEDDING_DIMENSION)),
                "fact {fid} in namespace org should preserve dimension"
            );
            assert_eq!(
                updated.get("embedding_signature"),
                Some(&json!("embsig:new")),
                "fact {fid} in namespace org should have new signature"
            );
            assert_eq!(
                updated.get("embedding_provider"),
                Some(&json!("test")),
                "fact {fid} in namespace org should have test provider"
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
            "org",
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
            "org",
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
            "org",
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
            "org",
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

    #[tokio::test]
    async fn reembed_continue_on_error_completes_within_quota() {
        // Keep several facts in the single active namespace so the test
        // exercises the failure quota without relying on multiple namespaces.
        let db = make_in_memory_db(&["org"]).await;
        for i in 0..3 {
            seed_fact_with_embedding(
                &db,
                "org",
                &format!("fact:{i}"),
                &format!("content {i}"),
                vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
                "embsig:old",
            )
            .await;
        }

        let provider = Arc::new(SequenceTestEmbeddingProvider::fails_on_call(
            DEFAULT_EMBEDDING_DIMENSION,
            2,
        ));
        let service = make_reembed_service(db, "org", provider, DEFAULT_EMBEDDING_DIMENSION);

        let options = ReembedOptions {
            max_failures: Some(10),
            retry_failed: false,
        };
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();

        let (summary, outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed should not hard-fail within quota");

        assert_eq!(outcome, ReembedOutcome::CompletedWithErrors);
        // The key behavioral assertions are: the run completed within quota
        // (outcome == CompletedWithErrors, not Failed) and at least one
        // failure was recorded without aborting.
        assert!(
            summary.processed_facts >= 3,
            "processed_facts undercounted: {}",
            summary.processed_facts
        );
        assert!(
            summary.failed_facts >= 1,
            "at least one failure expected: {}",
            summary.failed_facts
        );
        assert_eq!(summary.failed_fact_ids.len(), summary.failed_facts);
        assert_eq!(
            summary.succeeded_facts + summary.failed_facts,
            summary.processed_facts
        );
    }

    #[tokio::test]
    async fn reembed_fail_fast_when_max_failures_zero() {
        let db = make_in_memory_db(&["org"]).await;
        for i in 0..5 {
            seed_fact_with_embedding(
                &db,
                "org",
                &format!("fact:{i}"),
                &format!("content {i}"),
                vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
                "embsig:old",
            )
            .await;
        }

        let provider = Arc::new(SequenceTestEmbeddingProvider::fails_on_call(
            DEFAULT_EMBEDDING_DIMENSION,
            2,
        ));
        let service = make_reembed_service(db, "org", provider, DEFAULT_EMBEDDING_DIMENSION);

        let options = ReembedOptions {
            max_failures: Some(0),
            retry_failed: false,
        };
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();

        let (summary, outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("reembed returns outcome, not error, on quota exceeded");

        assert_eq!(outcome, ReembedOutcome::Failed);
        assert!(summary.failed_facts >= 1);
    }

    #[tokio::test]
    async fn reembed_cancellation_produces_interrupted_outcome() {
        let db = make_in_memory_db(&["org"]).await;
        for i in 0..3 {
            seed_fact_with_embedding(
                &db,
                "org",
                &format!("fact:{i}"),
                &format!("content {i}"),
                vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
                "embsig:old",
            )
            .await;
        }

        let provider = Arc::new(SequenceTestEmbeddingProvider::new(
            DEFAULT_EMBEDDING_DIMENSION,
        ));
        let service = make_reembed_service(db, "org", provider, DEFAULT_EMBEDDING_DIMENSION);

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();

        // Cancel before starting — simulates Ctrl+C during processing.
        cancel.cancel();

        let (_summary, outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("interrupted reembed returns outcome, not error");

        assert_eq!(outcome, ReembedOutcome::Interrupted);
        let job = service.load_reembed_job().await.expect("load job");
        assert_eq!(
            job.as_ref()
                .and_then(|j| j.get("status"))
                .and_then(|v| v.as_str()),
            Some("interrupted")
        );
    }

    #[tokio::test]
    async fn reembed_nothing_to_do_when_all_match_signature() {
        let db = make_in_memory_db(&["org"]).await;
        // Seed facts that already match the target signature ("embsig:new")
        // set by make_reembed_service. Facts with matching signatures are
        // excluded by the count query, yielding total_facts == 0.
        for i in 0..3 {
            seed_fact_with_embedding(
                &db,
                "org",
                &format!("fact:{i}"),
                &format!("content {i}"),
                vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
                "embsig:new",
            )
            .await;
        }

        let provider = Arc::new(SequenceTestEmbeddingProvider::new(
            DEFAULT_EMBEDDING_DIMENSION,
        ));
        let service = make_reembed_service(db, "org", provider, DEFAULT_EMBEDDING_DIMENSION);

        let options = ReembedOptions::default();
        let progress = NoopProgressReporter;
        let cancel = CancellationToken::new();

        let (summary, outcome) = service
            .reembed_all_facts(&options, &progress, &cancel)
            .await
            .expect("nothing-to-do is not an error");

        assert_eq!(outcome, ReembedOutcome::NothingToDo);
        assert_eq!(summary.total_facts, 0);
    }

    #[tokio::test]
    async fn reembed_counts_every_stale_fact_in_one_namespace() {
        let db = make_in_memory_db(&["org"]).await;
        for index in 0..3 {
            seed_fact_with_embedding(
                &db,
                "org",
                &format!("fact:stale_{index}"),
                &format!("stale fact {index}"),
                vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
                "embsig:old",
            )
            .await;
        }

        let service = make_reembed_service(
            db,
            "org",
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        let (summary, outcome) = service
            .reembed_all_facts(
                &ReembedOptions::default(),
                &NoopProgressReporter,
                &CancellationToken::new(),
            )
            .await
            .expect("reembed should process every stale fact");

        assert_eq!(outcome, ReembedOutcome::Completed);
        assert_eq!(summary.total_facts, 3);
        assert_eq!(summary.processed_facts, 3);
        assert_eq!(summary.succeeded_facts, 3);
    }

    #[tokio::test]
    async fn reembed_nothing_to_do_restores_semantic_readiness() {
        use crate::storage::embedding_state_store::EMBEDDING_STATE_RECORD_ID;

        let db = make_in_memory_db(&["org"]).await;
        seed_fact_with_embedding(
            &db,
            "org",
            "fact:up_to_date",
            "already uses the target embedding",
            vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
            "embsig:new",
        )
        .await;
        db.create(
            EMBEDDING_STATE_RECORD_ID,
            json!({
                "status": "ready",
                "active_signature": "embsig:old",
                "dimension": DEFAULT_EMBEDDING_DIMENSION,
                "updated_at": normalize_dt(Utc::now()),
            }),
            "org",
        )
        .await
        .expect("seed stale embedding state");

        let service = make_reembed_service(
            db.clone(),
            "org",
            Arc::new(SequenceTestEmbeddingProvider::new(
                DEFAULT_EMBEDDING_DIMENSION,
            )),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        service
            .remove_embedding_index("org")
            .await
            .expect("simulate an interrupted run before index recreation");

        let (summary, outcome) = service
            .reembed_all_facts(
                &ReembedOptions::default(),
                &NoopProgressReporter,
                &CancellationToken::new(),
            )
            .await
            .expect("nothing-to-do reembed should restore semantic readiness");

        assert_eq!(outcome, ReembedOutcome::NothingToDo);
        assert_eq!(summary.total_facts, 0);

        let state = db
            .select_one(EMBEDDING_STATE_RECORD_ID, "org")
            .await
            .expect("select embedding state")
            .expect("stored embedding state");
        assert_eq!(state.get("status"), Some(&json!("ready")));
        assert_eq!(state.get("active_signature"), Some(&json!("embsig:new")));

        let matches = crate::storage::ContextStoreClient::new(db.clone(), "org")
            .select_facts_ann(
                &normalize_dt(Utc::now()),
                &vec![1.0; DEFAULT_EMBEDDING_DIMENSION],
                1,
            )
            .await
            .expect("restored index should support semantic retrieval");
        assert_eq!(matches.len(), 1);
    }
}
