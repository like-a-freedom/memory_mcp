//! Embedding generation service — owns embedding provider access, query/fact
//! embedding caching, and the background embedding retry pipeline.
//!
//! Concentrates embedding concerns in one place.

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tokio::sync::Mutex;

use crate::embedding::providers::EmbeddingProvider;
use crate::embedding::providers::task_runner::{BackgroundAdmissionError, BackgroundTaskRunner};
use crate::embedding::query_cache::QueryEmbeddingCacheState;
use crate::error::MemoryError;
use crate::logging::{LogLevel, StdoutLogger};
use crate::memory::context_cache::ContextCache;
use crate::platform::context_cache_key::InvalidateContextCache;
use crate::storage::{BoundDbClient, DbClient};

/// Maximum input length accepted by embedding providers. Inputs longer than
/// this are truncated before being sent to the provider.
const MAX_EMBEDDING_INPUT_CHARS: usize = 8_000;
const MAX_BACKGROUND_QUERY_INPUT_BYTES: usize = 65_536;

/// Owns all embedding generation, caching, and background retry logic.
///
/// Held as a field on `MemoryService` and accessed by the `assemble_context`
/// pipeline, `FactService::add_fact`, `reembed`, and capability modules.
///
/// Background tasks receive a `self.clone()`: every field is `Clone` (plain
/// values, `Arc`, or `BoundDbClient`), so the clone freezes the same
/// at-spawn-time state the previous `EmbeddingBackgroundSnapshot` captured,
/// without duplicating the embedding policy methods (C4).
#[derive(Clone)]
pub(crate) struct EmbeddingService {
    db: BoundDbClient,
    logger: StdoutLogger,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    embedding_similarity_threshold: f64,
    current_embedding_signature: Option<String>,
    current_embedding_model: Option<String>,
    current_embedding_dimension: Option<usize>,
    context_cache: ContextCache,
    query_embedding_cache: Arc<Mutex<QueryEmbeddingCacheState>>,
    task_runner: Arc<BackgroundTaskRunner>,
}

impl EmbeddingService {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        db_client: Arc<dyn DbClient>,
        namespace: impl Into<String>,
        logger: StdoutLogger,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        embedding_similarity_threshold: f64,
        current_embedding_signature: Option<String>,
        current_embedding_model: Option<String>,
        current_embedding_dimension: Option<usize>,
        context_cache: ContextCache,
        query_embedding_cache: Arc<Mutex<QueryEmbeddingCacheState>>,
        task_runner: Arc<BackgroundTaskRunner>,
    ) -> Self {
        Self {
            db: BoundDbClient::new(db_client, namespace),
            logger,
            embedding_provider,
            embedding_similarity_threshold,
            current_embedding_signature,
            current_embedding_model,
            current_embedding_dimension,
            context_cache,
            query_embedding_cache,
            task_runner,
        }
    }

    pub(crate) fn embedding_provider(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.embedding_provider
    }

    pub(crate) fn embedding_similarity_threshold(&self) -> f64 {
        self.embedding_similarity_threshold
    }

    pub(crate) fn current_embedding_signature(&self) -> Option<&str> {
        self.current_embedding_signature.as_deref()
    }

    pub(crate) fn current_embedding_model(&self) -> Option<&str> {
        self.current_embedding_model.as_deref()
    }

    pub(crate) fn current_embedding_dimension(&self) -> Option<usize> {
        self.current_embedding_dimension
    }

    pub(crate) fn should_defer_embedding_retry(&self, err: &MemoryError) -> bool {
        crate::embedding::runtime::is_transient_embedding_error(err)
            && crate::embedding::runtime::is_remote_embedding_provider(
                self.embedding_provider.provider_name(),
            )
    }

    pub(crate) async fn generate_embedding(
        &self,
        input: &str,
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        generate_with_provider(&self.embedding_provider, &self.logger, input).await
    }

    pub(crate) async fn generate_query_embedding_with_background(
        &self,
        input: &str,
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        if let Some(embedding) = self.cached_query_embedding(input).await {
            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.query_cache_hit")),
                    (
                        "provider".to_string(),
                        json!(self.embedding_provider.provider_name()),
                    ),
                    ("input_chars".to_string(), json!(input.chars().count())),
                ]),
                LogLevel::Debug,
            );
            return Ok(Some(embedding));
        }

        let task_key = self.background_query_task_key(input);
        if self.background_embedding_task_inflight(&task_key).await {
            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.query_deferred_inflight")),
                    (
                        "provider".to_string(),
                        json!(self.embedding_provider.provider_name()),
                    ),
                    ("input_chars".to_string(), json!(input.chars().count())),
                ]),
                LogLevel::Debug,
            );
            return Ok(None);
        }

        match self.generate_embedding(input).await {
            Ok(Some(embedding)) => {
                self.store_query_embedding(input, embedding.clone()).await;
                Ok(Some(embedding))
            }
            Ok(None) => Ok(None),
            Err(err) if self.should_defer_embedding_retry(&err) => {
                self.enqueue_background_query_embedding(input).await;
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) async fn cached_query_embedding(&self, input: &str) -> Option<Vec<f64>> {
        let cache_key = self.query_embedding_cache_key(input);
        let mut cache = self.query_embedding_cache.lock().await;
        cache.get(&cache_key, std::time::Instant::now())
    }

    /// Enqueues a background fact-embedding retry after a transient provider
    /// failure during fact creation. Self-terminating: bounded retry loop.
    pub(crate) async fn enqueue_background_fact_embedding(
        &self,
        namespace: String,
        fact_id: String,
        input: String,
    ) {
        let task_key = self.background_fact_task_key(&namespace, &fact_id);
        let provider_input_bytes = effective_embedding_input_bytes(&input);
        let retained_bytes = provider_input_bytes
            .checked_add(namespace.len())
            .and_then(|bytes| bytes.checked_add(fact_id.len()))
            .and_then(|bytes| bytes.checked_add(task_key.len()));
        let Some(retained_bytes) = retained_bytes else {
            self.log_background_refusal("fact", BackgroundAdmissionError::ByteCapacity);
            return;
        };
        let reservation = match self.task_runner.try_admit(&task_key, retained_bytes) {
            Ok(reservation) => reservation,
            Err(BackgroundAdmissionError::Duplicate) => {
                self.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("embedding.background_deduped")),
                        ("kind".to_string(), json!("fact")),
                        ("namespace".to_string(), json!(namespace)),
                        ("fact_id".to_string(), json!(fact_id)),
                    ]),
                    LogLevel::Debug,
                );
                return;
            }
            Err(reason) => {
                self.log_background_refusal("fact", reason);
                return;
            }
        };
        let provider_input = effective_embedding_input(&input);

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("embedding.background_enqueued")),
                ("kind".to_string(), json!("fact")),
                ("namespace".to_string(), json!(namespace.clone())),
                ("fact_id".to_string(), json!(fact_id.clone())),
                ("input_bytes".to_string(), json!(input.len())),
                ("retained_bytes".to_string(), json!(retained_bytes)),
            ]),
            LogLevel::Info,
        );

        let service = self.clone();
        if let Err(reason) = self
            .task_runner
            .spawn(reservation, async move {
                service
                    .run_background_fact_embedding_task(namespace, fact_id, provider_input)
                    .await;
            })
            .await
        {
            self.log_background_refusal("fact", reason);
        }
    }

    fn background_fact_task_key(&self, namespace: &str, fact_id: &str) -> String {
        let signature = self
            .current_embedding_signature
            .as_deref()
            .unwrap_or(self.embedding_provider.provider_name());
        format!("fact:{signature}:{namespace}:{fact_id}")
    }

    fn background_query_task_key(&self, input: &str) -> String {
        let signature = self
            .current_embedding_signature
            .as_deref()
            .unwrap_or(self.embedding_provider.provider_name());
        format!(
            "query:{signature}:{}",
            self.query_embedding_cache_key(input)
        )
    }

    fn query_embedding_cache_key(&self, input: &str) -> String {
        let signature = self
            .current_embedding_signature
            .as_deref()
            .unwrap_or(self.embedding_provider.provider_name());
        crate::shared::ids::hash_prefix(&format!(
            "{signature}|{}",
            crate::shared::search::normalize_text(input)
        ))
    }

    async fn store_query_embedding(&self, input: &str, embedding: Vec<f64>) {
        let cache_key = self.query_embedding_cache_key(input);
        self.store_query_embedding_by_key(cache_key, embedding)
            .await;
    }

    async fn store_query_embedding_by_key(&self, cache_key: String, embedding: Vec<f64>) {
        let mut cache = self.query_embedding_cache.lock().await;
        let _outcome = cache.insert(cache_key, embedding, std::time::Instant::now());
    }

    async fn background_embedding_task_inflight(&self, task_key: &str) -> bool {
        self.task_runner.is_inflight(task_key).await
    }

    async fn enqueue_background_query_embedding(&self, input: &str) {
        if input.len() > MAX_BACKGROUND_QUERY_INPUT_BYTES {
            self.log_background_refusal("query", BackgroundAdmissionError::Oversized);
            return;
        }

        let provider_input_bytes = effective_embedding_input_bytes(input);
        let cache_key = self.query_embedding_cache_key(input);
        let task_key = self.background_query_task_key(input);
        let retained_bytes = provider_input_bytes
            .checked_add(cache_key.len())
            .and_then(|bytes| bytes.checked_add(task_key.len()));
        let Some(retained_bytes) = retained_bytes else {
            self.log_background_refusal("query", BackgroundAdmissionError::ByteCapacity);
            return;
        };
        let reservation = match self.task_runner.try_admit(&task_key, retained_bytes) {
            Ok(reservation) => reservation,
            Err(reason) => {
                self.log_background_refusal("query", reason);
                return;
            }
        };
        let provider_input = effective_embedding_input(input);

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("embedding.background_enqueued")),
                ("kind".to_string(), json!("query")),
                ("input_bytes".to_string(), json!(input.len())),
                ("retained_bytes".to_string(), json!(retained_bytes)),
            ]),
            LogLevel::Info,
        );

        let service = self.clone();
        if let Err(reason) = self
            .task_runner
            .spawn(reservation, async move {
                service
                    .run_background_query_embedding_task(provider_input, cache_key)
                    .await;
            })
            .await
        {
            self.log_background_refusal("query", reason);
        }
    }

    fn log_background_refusal(&self, kind: &str, reason: BackgroundAdmissionError) {
        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("embedding.background_refused")),
                ("kind".to_string(), json!(kind)),
                ("reason".to_string(), json!(format!("{reason:?}"))),
            ]),
            LogLevel::Debug,
        );
    }

    // ─── Background task execution ──────────────────────────────────────
    //
    // These methods run inside spawned tasks on a cloned service instance.

    async fn run_background_fact_embedding_task(
        self,
        namespace: String,
        fact_id: String,
        input: String,
    ) {
        let outcome = self
            .run_background_fact_embedding_task_inner(&namespace, &fact_id, &input)
            .await;

        if let Err(err) = outcome {
            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.background_failed")),
                    ("kind".to_string(), json!("fact")),
                    ("namespace".to_string(), json!(namespace)),
                    ("fact_id".to_string(), json!(fact_id)),
                    ("error".to_string(), json!(err.to_string())),
                ]),
                LogLevel::Warn,
            );
        }
    }

    async fn run_background_fact_embedding_task_inner(
        &self,
        namespace: &str,
        fact_id: &str,
        input: &str,
    ) -> Result<(), MemoryError> {
        for attempt in 1..=crate::embedding::runtime::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS {
            match self.generate_embedding(input).await {
                Ok(Some(embedding)) => {
                    self.store_embedding_on_fact(fact_id, embedding).await?;
                    self.logger.log(
                        std::collections::HashMap::from([
                            ("op".to_string(), json!("embedding.background_succeeded")),
                            ("kind".to_string(), json!("fact")),
                            ("namespace".to_string(), json!(namespace)),
                            ("fact_id".to_string(), json!(fact_id)),
                            ("attempt".to_string(), json!(attempt)),
                        ]),
                        LogLevel::Info,
                    );
                    return Ok(());
                }
                Ok(None) => return Ok(()),
                Err(err)
                    if self.should_defer_embedding_retry(&err)
                        && attempt
                            < crate::embedding::runtime::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS =>
                {
                    let delay =
                        crate::embedding::runtime::background_embedding_retry_delay(attempt);
                    self.logger.log(
                        std::collections::HashMap::from([
                            ("op".to_string(), json!("embedding.background_retry")),
                            ("kind".to_string(), json!("fact")),
                            ("namespace".to_string(), json!(namespace)),
                            ("fact_id".to_string(), json!(fact_id)),
                            ("attempt".to_string(), json!(attempt)),
                            ("delay_ms".to_string(), json!(delay.as_millis() as u64)),
                            ("error".to_string(), json!(err.to_string())),
                        ]),
                        LogLevel::Warn,
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(err) => return Err(err),
            }
        }

        Ok(())
    }

    async fn run_background_query_embedding_task(self, input: String, cache_key: String) {
        let outcome = self
            .run_background_query_embedding_task_inner(&input, cache_key)
            .await;

        if let Err(err) = outcome {
            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.background_failed")),
                    ("kind".to_string(), json!("query")),
                    ("input_bytes".to_string(), json!(input.len())),
                    ("error".to_string(), json!(err.to_string())),
                ]),
                LogLevel::Warn,
            );
        }
    }

    async fn run_background_query_embedding_task_inner(
        &self,
        input: &str,
        cache_key: String,
    ) -> Result<(), MemoryError> {
        for attempt in 1..=crate::embedding::runtime::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS {
            match self.generate_embedding(input).await {
                Ok(Some(embedding)) => {
                    self.store_query_embedding_by_key(cache_key, embedding)
                        .await;
                    self.logger.log(
                        std::collections::HashMap::from([
                            ("op".to_string(), json!("embedding.background_succeeded")),
                            ("kind".to_string(), json!("query")),
                            ("input_chars".to_string(), json!(input.chars().count())),
                            ("attempt".to_string(), json!(attempt)),
                        ]),
                        LogLevel::Info,
                    );
                    return Ok(());
                }
                Ok(None) => return Ok(()),
                Err(err)
                    if self.should_defer_embedding_retry(&err)
                        && attempt
                            < crate::embedding::runtime::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS =>
                {
                    let delay =
                        crate::embedding::runtime::background_embedding_retry_delay(attempt);
                    self.logger.log(
                        std::collections::HashMap::from([
                            ("op".to_string(), json!("embedding.background_retry")),
                            ("kind".to_string(), json!("query")),
                            ("input_chars".to_string(), json!(input.chars().count())),
                            ("attempt".to_string(), json!(attempt)),
                            ("delay_ms".to_string(), json!(delay.as_millis() as u64)),
                            ("error".to_string(), json!(err.to_string())),
                        ]),
                        LogLevel::Warn,
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(err) => return Err(err),
            }
        }

        Ok(())
    }

    /// Write a vector this service already generated.
    ///
    /// The one place that writes without going through `generate_and_update`,
    /// and the reason is the retry loop: `run_background_fact_embedding_task`
    /// regenerates on every attempt, so the vector is the thing being retried
    /// and cannot be generated once and reused. Collapsing the two would
    /// remove the ability to retry generation at all.
    ///
    /// The generation contract still holds — the vector came from
    /// `generate_embedding`, so the input limit and the enabled check applied
    /// when it was made. What this skips is only the *naming* of the policy in
    /// one place, and the policy is named on line below.
    async fn store_embedding_on_fact(
        &self,
        fact_id: &str,
        embedding: Vec<f64>,
    ) -> Result<(), MemoryError> {
        let Some(identity) = self.current_vector_identity() else {
            return Err(MemoryError::Validation(
                "no resolved embedding target for background embedding".into(),
            ));
        };
        let adapter = crate::embedding::infra::FactVectorAdapter::new(
            self.db.db.clone(),
            self.db.namespace(),
        );
        crate::embedding::api::update_canonical_vector(
            &adapter,
            fact_id,
            embedding,
            &identity,
            chrono::Utc::now(),
            // The retry only ever skipped a fact whose stored
            // signature already matched the runtime, so a stale
            // vector is replaced rather than preserved.
            crate::embedding::api::VectorWritePolicy::ReplaceStale,
        )
        .await?;
        self.context_cache.invalidate_context_cache().await;
        Ok(())
    }

    /// Resolved target identity for a vector produced by the
    /// current provider, or `None` when the runtime has not
    /// resolved one.
    fn current_vector_identity(&self) -> Option<crate::embedding::api::VectorIdentity> {
        Some(crate::embedding::api::VectorIdentity {
            provider: self.embedding_provider.provider_name().to_owned(),
            model: self.current_embedding_model.clone(),
            dimension: self.current_embedding_dimension?,
            signature: self.current_embedding_signature.clone()?,
        })
    }
}

/// Generate one vector, or `None` when the provider is disabled.
///
fn effective_embedding_input_bytes(input: &str) -> usize {
    input
        .char_indices()
        .nth(MAX_EMBEDDING_INPUT_CHARS)
        .map_or(input.len(), |(byte_index, _)| byte_index)
}

fn effective_embedding_input(input: &str) -> String {
    input.chars().take(MAX_EMBEDDING_INPUT_CHARS).collect()
}

/// Free rather than only a method on [`EmbeddingService`] because two
/// adapters need it and only one of them is the service. The input limit,
/// the enabled check, the stage timer and the four log events all live
/// here, and a second implementation of any of them would be a second
/// place to forget one — which is what `service/embedding_recovery.rs`
/// was doing when it called `provider.embed()` directly.
async fn generate_with_provider(
    embedding_provider: &Arc<dyn EmbeddingProvider>,
    logger: &StdoutLogger,
    input: &str,
) -> Result<Option<Vec<f64>>, MemoryError> {
    let effective_input: String = if input.len() > MAX_EMBEDDING_INPUT_CHARS {
        let truncated: String = input.chars().take(MAX_EMBEDDING_INPUT_CHARS).collect();
        logger.log(
            crate::platform::log_event::log_event(
                "embedding.input_truncated",
                json!({
                    "original_chars": input.chars().count(),
                    "truncated_chars": truncated.chars().count(),
                    "limit": MAX_EMBEDDING_INPUT_CHARS,
                }),
                json!({}),
                None,
                None,
                None,
            ),
            LogLevel::Warn,
        );
        truncated
    } else {
        input.to_string()
    };

    let timer = Instant::now();
    let provider = embedding_provider.provider_name();
    let args = json!({
        "provider": provider,
        "input_chars": effective_input.chars().count(),
    });

    if !embedding_provider.is_enabled() {
        let mut result = crate::platform::log_event::build_embedding_log_result(0, None);
        if let Some(map) = result.as_object_mut() {
            map.insert("status".to_string(), json!("disabled"));
        }
        logger.log(
            crate::platform::log_event::log_event(
                "embedding.generate.skipped",
                crate::platform::log_event::log_args_with_duration(args, timer.elapsed()),
                result,
                None,
                None,
                None,
            ),
            LogLevel::Debug,
        );
        return Ok(None);
    }

    // The provider call is the one stage here that can be slow for reasons
    // outside this process — a model server, a network hop, a queue behind
    // someone else's inference. Everything above it is bookkeeping, so
    // this is the duration that separates "our code got slower" from
    // "the thing we call got slower".
    // Scoped to the call. Bound at function scope the guard would stay
    // alive through the logging below, so `embedding_provider` would
    // measure the call plus the bookkeeping that follows it — the exact
    // split between "the thing we call" and "our own code" that the stage
    // exists to provide.
    let embedded = {
        let _provider_stage =
            crate::shared::observability::StageTimer::new("extract", "embedding_provider");
        embedding_provider.embed(&effective_input).await
    };
    match embedded {
        Ok(embedding) => {
            logger.log(
                crate::platform::log_event::log_event(
                    "embedding.generate.done",
                    crate::platform::log_event::log_args_with_duration(args, timer.elapsed()),
                    crate::platform::log_event::build_embedding_log_result(
                        1,
                        Some(embedding.len()),
                    ),
                    None,
                    None,
                    None,
                ),
                LogLevel::Info,
            );
            Ok(Some(embedding))
        }
        Err(err) => {
            let mut result = crate::platform::log_event::build_embedding_log_result(0, None);
            if let Some(map) = result.as_object_mut() {
                map.insert("error".to_string(), json!(err.to_string()));
            }
            logger.log(
                crate::platform::log_event::log_event(
                    "embedding.generate.error",
                    crate::platform::log_event::log_args_with_duration(args, timer.elapsed()),
                    result,
                    None,
                    None,
                    None,
                ),
                LogLevel::Warn,
            );
            Err(err)
        }
    }
}

/// Generation over one provider, for a caller that holds a provider rather
/// than a whole service.
///
/// Recovery is that caller: it is handed a provider that may differ from the
/// one the service was built with — a re-probed remote target, or a local
/// model that swapped in while the service was running. This wrapper exists so
/// that holding a provider is not the same as being allowed to skip the
/// generation contract.
///
/// It is a separate type from the [`EmbeddingService`] adapter because the two
/// carry different state, not because the contract differs. This one has a
/// provider and a logger; the service adapter has those plus a resolved
/// identity and two caches, none of which generation reads.
pub(crate) struct ProviderGeneration {
    provider: Arc<dyn EmbeddingProvider>,
    logger: StdoutLogger,
}

impl ProviderGeneration {
    pub(crate) fn new(provider: Arc<dyn EmbeddingProvider>, logger: StdoutLogger) -> Self {
        Self { provider, logger }
    }
}

/// The production adapter of [`crate::embedding::api::EmbeddingGeneration`],
/// and the reason the trait exists: a caller that generates through a port
/// cannot skip the input limit or the disabled check, because both live in
/// `generate_with_provider` and there is no other way in.
#[async_trait::async_trait]
impl crate::embedding::api::EmbeddingGeneration for EmbeddingService {
    async fn generate(
        &self,
        input: &str,
    ) -> Result<crate::embedding::api::GenerationOutcome, MemoryError> {
        Ok(to_generation_outcome(self.generate_embedding(input).await?))
    }
}

#[async_trait::async_trait]
impl crate::embedding::api::EmbeddingGeneration for ProviderGeneration {
    async fn generate(
        &self,
        input: &str,
    ) -> Result<crate::embedding::api::GenerationOutcome, MemoryError> {
        Ok(to_generation_outcome(
            generate_with_provider(&self.provider, &self.logger, input).await?,
        ))
    }
}

/// Say that "no vector" is a reason rather than an absence.
///
/// `generate_with_provider` returns `None` in exactly one case — the provider
/// is configured but disabled — and has no other way to decline. That is what
/// makes this mapping total rather than a guess: there is no second `None` for
/// a future change to fall through.
fn to_generation_outcome(generated: Option<Vec<f64>>) -> crate::embedding::api::GenerationOutcome {
    use crate::embedding::api::{GenerationOutcome, SkipReason};
    match generated {
        Some(vector) => GenerationOutcome::Generated(vector),
        None => GenerationOutcome::Skipped(SkipReason::ProviderDisabled),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::{Mutex, RwLock};

    use super::*;
    use crate::embedding::providers::task_runner::BackgroundTaskRunner;
    use crate::embedding::query_cache::QueryEmbeddingCacheState;
    use crate::error::MemoryError;
    use crate::logging::StdoutLogger;
    use crate::service::mock_db::MockDbClient;

    /// Deterministic provider: returns a fixed-dimension vector whose first
    /// component encodes the input, so tests can distinguish calls.
    struct ScriptedProvider {
        dimension: usize,
        enabled: bool,
        failures: AtomicUsize,
        calls: AtomicUsize,
        fail_with: MemoryError,
    }

    impl ScriptedProvider {
        fn new(dimension: usize) -> Self {
            Self {
                dimension,
                enabled: true,
                failures: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
                fail_with: MemoryError::Transient("synthetic outage".to_string()),
            }
        }

        fn disabled(dimension: usize) -> Self {
            Self {
                enabled: false,
                ..Self::new(dimension)
            }
        }

        fn fail_permanently(mut self) -> Self {
            self.failures.store(u32::MAX as usize, Ordering::SeqCst);
            self.fail_with = MemoryError::Storage("permanent failure".to_string());
            self
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for ScriptedProvider {
        fn is_enabled(&self) -> bool {
            self.enabled
        }

        fn provider_name(&self) -> &'static str {
            "scripted"
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let remaining = self.failures.load(Ordering::SeqCst);
            if remaining > 0 {
                self.failures.fetch_sub(1, Ordering::SeqCst);
                return Err(self.fail_with.clone());
            }
            let mut vector = vec![0.0; self.dimension];
            vector[0] = input.chars().count() as f64;
            Ok(vector)
        }
    }

    /// Remote-named variant of [`ScriptedProvider`] for retry-policy tests.
    struct ScriptedRemoteProvider(AtomicUsize);

    #[async_trait]
    impl EmbeddingProvider for ScriptedRemoteProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "openai-compatible"
        }

        fn dimension(&self) -> usize {
            4
        }

        async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
            let remaining = self.0.load(Ordering::SeqCst);
            if remaining > 0 {
                self.0.fetch_sub(1, Ordering::SeqCst);
                return Err(MemoryError::Transient("synthetic outage".to_string()));
            }
            let mut vector = vec![0.0; 4];
            vector[0] = input.chars().count() as f64;
            Ok(vector)
        }
    }

    fn make_query_cache() -> Arc<Mutex<QueryEmbeddingCacheState>> {
        Arc::new(Mutex::new(QueryEmbeddingCacheState::new(
            std::num::NonZeroUsize::new(8).unwrap(),
            std::num::NonZeroUsize::new(2 * 1024 * 1024).unwrap(),
            crate::embedding::runtime::query_embedding_cache_ttl(),
        )))
    }

    fn make_service(
        provider: Arc<dyn EmbeddingProvider>,
        signature: Option<&str>,
        runner: Arc<BackgroundTaskRunner>,
    ) -> EmbeddingService {
        make_service_in_namespace("org", provider, signature, runner)
    }

    fn make_service_in_namespace(
        namespace: &str,
        provider: Arc<dyn EmbeddingProvider>,
        signature: Option<&str>,
        runner: Arc<BackgroundTaskRunner>,
    ) -> EmbeddingService {
        let context_cache = Arc::new(RwLock::new(
            crate::platform::context_cache::ContextCacheState::new(
                std::num::NonZeroUsize::new(8).unwrap(),
                std::num::NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
            ),
        ));
        let query_embedding_cache = make_query_cache();
        EmbeddingService::new(
            Arc::new(MockDbClient::new()),
            namespace,
            StdoutLogger::new("warn"),
            provider,
            0.8,
            signature.map(str::to_string),
            Some("test-model".to_string()),
            Some(4),
            context_cache,
            query_embedding_cache,
            runner,
        )
    }

    #[test]
    fn task_keys_are_scoped_by_signature_and_kind() {
        let runner = Arc::new(BackgroundTaskRunner::new());
        let with_sig = make_service(
            Arc::new(ScriptedProvider::new(4)),
            Some("sig-a"),
            runner.clone(),
        );
        let without_sig = make_service(Arc::new(ScriptedProvider::new(4)), None, runner.clone());

        // Signature present: explicit in the key. Absent: provider name is
        // the fallback, so the same input maps to different keys.
        assert_eq!(
            with_sig.background_fact_task_key("org", "fact:1"),
            "fact:sig-a:org:fact:1"
        );
        assert_eq!(
            without_sig.background_fact_task_key("org", "fact:1"),
            "fact:scripted:org:fact:1"
        );

        let query_key = with_sig.background_query_task_key("some input");
        assert!(
            query_key.starts_with("query:sig-a:"),
            "query key should carry the signature prefix, got {query_key}"
        );
        // The query task key embeds the cache key, so it is deterministic.
        assert_eq!(query_key, with_sig.background_query_task_key("some input"));
        // Fact and query keys for the same logical work never collide.
        assert_ne!(
            with_sig.background_fact_task_key("org", "some input"),
            with_sig.background_query_task_key("some input")
        );
    }

    #[tokio::test]
    async fn same_fact_id_in_distinct_namespaces_is_not_deduplicated() {
        struct BlockingProvider {
            started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        }

        #[async_trait]
        impl EmbeddingProvider for BlockingProvider {
            fn is_enabled(&self) -> bool {
                true
            }

            fn provider_name(&self) -> &'static str {
                "openai-compatible"
            }

            fn dimension(&self) -> usize {
                4
            }

            async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
                if let Some(started) = self.started.lock().expect("started lock").take() {
                    let _ = started.send(());
                }
                if let Some(release) = self.release.lock().await.take() {
                    let _ = release.await;
                }
                Ok(vec![0.0; 4])
            }
        }

        let runner = Arc::new(BackgroundTaskRunner::new());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let provider = Arc::new(BlockingProvider {
            started: std::sync::Mutex::new(Some(started_tx)),
            release: Mutex::new(Some(release_rx)),
        });
        let tenant_a = make_service_in_namespace(
            "tenant-a",
            provider.clone(),
            Some("signature-a"),
            runner.clone(),
        );
        let tenant_b =
            make_service_in_namespace("tenant-b", provider, Some("signature-a"), runner.clone());

        tenant_a
            .enqueue_background_fact_embedding(
                "tenant-a".to_string(),
                "fact:shared".to_string(),
                "tenant A fact".to_string(),
            )
            .await;
        tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .expect("tenant A provider call starts")
            .expect("provider sends start signal");
        tenant_b
            .enqueue_background_fact_embedding(
                "tenant-b".to_string(),
                "fact:shared".to_string(),
                "tenant B fact".to_string(),
            )
            .await;

        assert_eq!(
            runner.resource_snapshot().admitted_tasks,
            2,
            "the same fact record key in two namespaces must occupy distinct admissions"
        );
        let _ = release_tx.send(());
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("both namespace-specific retries join");
        assert_eq!(
            runner.resource_snapshot(),
            crate::embedding::providers::task_runner::BackgroundTaskSnapshot::default()
        );
    }

    #[test]
    fn query_cache_key_normalizes_input_and_scopes_by_signature() {
        let sig_a = make_service(
            Arc::new(ScriptedProvider::new(4)),
            Some("sig-a"),
            Arc::new(BackgroundTaskRunner::new()),
        );
        let sig_b = make_service(
            Arc::new(ScriptedProvider::new(4)),
            Some("sig-b"),
            Arc::new(BackgroundTaskRunner::new()),
        );
        let unnamed = make_service(
            Arc::new(ScriptedProvider::new(4)),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );

        // Whitespace and case variants normalize to the same key.
        assert_eq!(
            sig_a.query_embedding_cache_key("  The  Quick \n Brown  "),
            sig_a.query_embedding_cache_key("the quick brown")
        );
        // Distinct texts get distinct keys.
        assert_ne!(
            sig_a.query_embedding_cache_key("the quick brown"),
            sig_a.query_embedding_cache_key("jumps over the fox")
        );
        // A different embedding signature isolates the caches.
        assert_ne!(
            sig_a.query_embedding_cache_key("the quick brown"),
            sig_b.query_embedding_cache_key("the quick brown")
        );
        // No signature: provider name scopes the key instead.
        assert_ne!(
            sig_a.query_embedding_cache_key("the quick brown"),
            unnamed.query_embedding_cache_key("the quick brown")
        );
    }

    #[test]
    fn should_defer_retry_only_for_transient_remote_failures() {
        let local_transient = make_service(
            Arc::new(ScriptedProvider::new(4)),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );
        let remote = make_service(
            Arc::new(ScriptedRemoteProvider(AtomicUsize::new(1))),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );

        let transient = MemoryError::Transient("outage".to_string());
        let permanent = MemoryError::Storage("boom".to_string());

        assert!(!local_transient.should_defer_embedding_retry(&transient));
        assert!(remote.should_defer_embedding_retry(&transient));
        assert!(!remote.should_defer_embedding_retry(&permanent));
    }

    #[tokio::test]
    async fn generate_embedding_returns_none_when_provider_disabled() {
        let service = make_service(
            Arc::new(ScriptedProvider::disabled(4)),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );
        let result = service.generate_embedding("hello").await.unwrap();
        assert!(result.is_none());
    }

    /// The two adapters of `EmbeddingGeneration` both carry the input limit,
    /// because both call one function. This test is the proof, and it is the
    /// evidence that the seam closed the hole `service/embedding_recovery.rs`
    /// was carrying when it called `provider.embed()` directly: a direct call
    /// sends the caller's text, and this asserts the provider does not
    /// receive it.
    #[tokio::test]
    async fn the_generation_port_truncates_before_the_provider_sees_the_text() {
        struct RecordingProvider {
            seen: std::sync::Mutex<Vec<String>>,
        }

        #[async_trait]
        impl EmbeddingProvider for RecordingProvider {
            fn is_enabled(&self) -> bool {
                true
            }
            fn provider_name(&self) -> &'static str {
                "recording"
            }
            fn dimension(&self) -> usize {
                4
            }
            async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
                self.seen.lock().expect("seen lock").push(input.to_owned());
                Ok(vec![0.0; 4])
            }
        }

        let provider = Arc::new(RecordingProvider {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let long = "\u{e9}".repeat(MAX_EMBEDDING_INPUT_CHARS + 500);

        // Adapter one: the provider-only port recovery uses.
        let port = ProviderGeneration::new(provider.clone(), StdoutLogger::new("warn"));
        assert!(
            matches!(
                crate::embedding::api::EmbeddingGeneration::generate(&port, &long)
                    .await
                    .expect("generation completes"),
                crate::embedding::api::GenerationOutcome::Generated(_)
            ),
            "an enabled provider produces a vector, whatever the input length"
        );

        // Adapter two: the service, which the other three callers use.
        let service = make_service(
            provider.clone(),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );
        let _ = crate::embedding::api::EmbeddingGeneration::generate(&service, &long)
            .await
            .expect("generation completes");

        let seen = provider.seen.lock().expect("seen lock").clone();
        assert_eq!(seen.len(), 2, "both adapters reached the provider");
        for (index, received) in seen.iter().enumerate() {
            assert_eq!(
                received.chars().count(),
                MAX_EMBEDDING_INPUT_CHARS,
                "adapter {index} sent the caller's text instead of the truncated one"
            );
        }
    }

    /// The other half of the port's contract: a disabled provider is a
    /// configuration, not a failure, and it is reported as a reason a metric
    /// can label rather than as an absent value.
    #[tokio::test]
    async fn the_generation_port_reports_a_disabled_provider_as_a_skip() {
        let service = make_service(
            Arc::new(ScriptedProvider::disabled(4)),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );
        assert_eq!(
            crate::embedding::api::EmbeddingGeneration::generate(&service, "hello")
                .await
                .expect("a disabled provider is not an error"),
            crate::embedding::api::GenerationOutcome::Skipped(
                crate::embedding::api::SkipReason::ProviderDisabled
            )
        );
    }

    #[tokio::test]
    async fn generate_embedding_propagates_provider_errors() {
        let provider = Arc::new(ScriptedProvider::new(4).fail_permanently());
        let service = make_service(
            provider.clone(),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );
        let result = service.generate_embedding("hello").await;
        assert!(matches!(result, Err(MemoryError::Storage(_))));
        assert_eq!(provider.calls(), 1);
    }

    #[tokio::test]
    async fn same_query_signature_uses_cached_embedding_once() {
        let provider = Arc::new(ScriptedProvider::new(4));
        let service = make_service(
            provider.clone(),
            Some("signature-a"),
            Arc::new(BackgroundTaskRunner::new()),
        );

        let first = service
            .generate_query_embedding_with_background("repeat this query")
            .await
            .expect("first provider call should succeed");
        let second = service
            .generate_query_embedding_with_background("repeat this query")
            .await
            .expect("cached lookup should succeed");

        assert_eq!(second, first);
        assert_eq!(provider.calls(), 1);
    }

    #[tokio::test]
    async fn different_provider_signature_never_reuses_embedding() {
        let shared_cache = make_query_cache();
        let provider_a = Arc::new(ScriptedProvider::new(4));
        let provider_b = Arc::new(ScriptedProvider::new(4));
        let mut service_a = make_service(
            provider_a.clone(),
            Some("signature-a"),
            Arc::new(BackgroundTaskRunner::new()),
        );
        let mut service_b = make_service(
            provider_b.clone(),
            Some("signature-b"),
            Arc::new(BackgroundTaskRunner::new()),
        );
        service_a.query_embedding_cache = shared_cache.clone();
        service_b.query_embedding_cache = shared_cache;

        service_a
            .generate_query_embedding_with_background("same normalized query")
            .await
            .expect("first signature should produce an embedding");
        service_b
            .generate_query_embedding_with_background("same normalized query")
            .await
            .expect("second signature should produce its own embedding");

        assert_eq!(provider_a.calls(), 1);
        assert_eq!(provider_b.calls(), 1);
    }

    #[tokio::test]
    async fn query_embedding_uses_cache_and_defers_inflight_tasks() {
        let provider = Arc::new(ScriptedProvider::new(4));
        let runner = Arc::new(BackgroundTaskRunner::new());
        let service = make_service(provider.clone(), None, runner.clone());

        // First call: miss, provider invoked, embedding cached.
        let first = service
            .generate_query_embedding_with_background("repeat me")
            .await
            .unwrap();
        assert_eq!(first.as_ref().unwrap().len(), 4);
        assert_eq!(provider.calls(), 1);

        // Second call: cache hit, provider not re-invoked.
        let second = service
            .generate_query_embedding_with_background("repeat me")
            .await
            .unwrap();
        assert_eq!(second, first);
        assert_eq!(provider.calls(), 1);

        // A new input that is already reserved as an inflight background task
        // is deferred (Ok(None)) instead of racing the provider.
        let task_key = service.background_query_task_key("deferred input");
        let _reservation = runner
            .try_admit(&task_key, 0)
            .expect("query retry is reserved");
        let deferred = service
            .generate_query_embedding_with_background("deferred input")
            .await
            .unwrap();
        assert!(deferred.is_none());
        assert_eq!(provider.calls(), 1);
    }

    #[tokio::test]
    async fn fact_retry_retains_only_bounded_input_and_owns_running_work() {
        struct BlockingProvider {
            started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
            release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        }

        #[async_trait]
        impl EmbeddingProvider for BlockingProvider {
            fn is_enabled(&self) -> bool {
                true
            }

            fn provider_name(&self) -> &'static str {
                "openai-compatible"
            }

            fn dimension(&self) -> usize {
                4
            }

            async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
                if let Some(started) = self.started.lock().expect("started lock").take() {
                    let _ = started.send(input.to_owned());
                }
                if let Some(release) = self.release.lock().await.take() {
                    let _ = release.await;
                }
                Ok(vec![0.0; 4])
            }
        }

        let namespace = "org";
        let fact_id = "fact:123";
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let runner = Arc::new(BackgroundTaskRunner::new());
        let service = make_service(
            Arc::new(BlockingProvider {
                started: std::sync::Mutex::new(Some(started_tx)),
                release: Mutex::new(Some(release_rx)),
            }),
            Some("signature-a"),
            runner.clone(),
        );
        let input = "é".repeat(9_000);
        let task_key = service.background_fact_task_key(namespace, fact_id);
        let expected_retained_bytes =
            "é".repeat(8_000).len() + namespace.len() + fact_id.len() + task_key.len();

        service
            .enqueue_background_fact_embedding(namespace.to_string(), fact_id.to_string(), input)
            .await;
        let provider_input = tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .expect("background fact provider call starts")
            .expect("provider sends its input");

        assert_eq!(provider_input, "é".repeat(8_000));
        assert_eq!(
            runner.resource_snapshot(),
            crate::embedding::providers::task_runner::BackgroundTaskSnapshot {
                admitted_tasks: 1,
                running_tasks: 1,
                retained_bytes: expected_retained_bytes,
            }
        );

        let _ = release_tx.send(());
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("background fact task joins");
        assert_eq!(
            runner.resource_snapshot(),
            crate::embedding::providers::task_runner::BackgroundTaskSnapshot::default()
        );
    }

    #[tokio::test]
    async fn multibyte_input_is_accounted_by_utf8_bytes() {
        struct BlockingProvider {
            started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
            release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        }

        #[async_trait]
        impl EmbeddingProvider for BlockingProvider {
            fn is_enabled(&self) -> bool {
                true
            }

            fn provider_name(&self) -> &'static str {
                "openai-compatible"
            }

            fn dimension(&self) -> usize {
                4
            }

            async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
                if let Some(started) = self.started.lock().expect("started lock").take() {
                    let _ = started.send(input.to_owned());
                }
                if let Some(release) = self.release.lock().await.take() {
                    let _ = release.await;
                }
                Ok(vec![0.0; 4])
            }
        }

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let runner = Arc::new(BackgroundTaskRunner::new());
        let service = make_service(
            Arc::new(BlockingProvider {
                started: std::sync::Mutex::new(Some(started_tx)),
                release: Mutex::new(Some(release_rx)),
            }),
            Some("signature-a"),
            runner.clone(),
        );
        let input = "é".repeat(12);
        let cache_key = service.query_embedding_cache_key(&input);
        let task_key = service.background_query_task_key(&input);
        let expected_retained_bytes = input.len() + cache_key.len() + task_key.len();

        service.enqueue_background_query_embedding(&input).await;
        let provider_input = tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .expect("background provider call starts")
            .expect("provider sends its input");

        assert_eq!(provider_input, input);
        assert_eq!(
            runner.resource_snapshot().retained_bytes,
            expected_retained_bytes
        );

        let _ = release_tx.send(());
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("background provider task joins");
    }

    #[tokio::test]
    async fn long_query_uses_original_normalized_identity_not_provider_prefix() {
        struct RecordingRemoteProvider {
            calls: AtomicUsize,
            background_input: std::sync::Mutex<Option<String>>,
        }

        #[async_trait]
        impl EmbeddingProvider for RecordingRemoteProvider {
            fn is_enabled(&self) -> bool {
                true
            }

            fn provider_name(&self) -> &'static str {
                "openai-compatible"
            }

            fn dimension(&self) -> usize {
                4
            }

            async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(MemoryError::Transient("synthetic outage".to_string()));
                }
                *self.background_input.lock().expect("background input lock") =
                    Some(input.to_owned());
                Ok(vec![0.25; 4])
            }
        }

        let provider = Arc::new(RecordingRemoteProvider {
            calls: AtomicUsize::new(0),
            background_input: std::sync::Mutex::new(None),
        });
        let runner = Arc::new(BackgroundTaskRunner::new());
        let service = make_service(provider.clone(), Some("signature-a"), runner.clone());
        let original = "x".repeat(8_001);

        assert!(
            service
                .generate_query_embedding_with_background(&original)
                .await
                .expect("transient query failure is deferred")
                .is_none()
        );
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("background query retry joins");

        assert_eq!(
            provider
                .background_input
                .lock()
                .expect("background input lock")
                .as_deref(),
            Some("x".repeat(8_000).as_str())
        );
        assert!(service.cached_query_embedding(&original).await.is_some());
        assert!(
            service
                .cached_query_embedding(&"x".repeat(8_000))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn provider_outage_does_not_grow_admitted_resources() {
        let runner = Arc::new(BackgroundTaskRunner::new());
        let service = make_service(
            Arc::new(ScriptedRemoteProvider(AtomicUsize::new(100))),
            None,
            runner.clone(),
        );

        assert!(
            service
                .generate_query_embedding_with_background("same outage query")
                .await
                .expect("first transient provider error is deferred")
                .is_none()
        );
        assert!(
            service
                .generate_query_embedding_with_background("same outage query")
                .await
                .expect("in-flight retry avoids another provider call")
                .is_none()
        );
        assert!(
            service
                .generate_query_embedding_with_background("same outage query")
                .await
                .expect("repeated outage does not enqueue another retry")
                .is_none()
        );
        let snapshot = runner.resource_snapshot();

        assert_eq!(snapshot.admitted_tasks, 1);
        assert!(snapshot.running_tasks <= 1);
        assert!(snapshot.retained_bytes <= 262_144);
        runner.shutdown();
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("shutdown cancels the outage retry");
        assert_eq!(runner.resource_snapshot(), Default::default());
    }

    #[tokio::test]
    async fn refused_request_spawns_no_waiter() {
        let provider = Arc::new(ScriptedRemoteProvider(AtomicUsize::new(100)));
        let runner = Arc::new(BackgroundTaskRunner::with_limits(
            crate::embedding::providers::task_runner::BackgroundEmbeddingLimits {
                max_admitted_tasks: 0,
                ..Default::default()
            },
        ));
        let service = make_service(provider.clone(), None, runner.clone());

        assert!(
            service
                .generate_query_embedding_with_background("retry refused")
                .await
                .expect("transient foreground failure remains deferred")
                .is_none()
        );
        assert_eq!(provider.0.load(Ordering::SeqCst), 99);
        assert_eq!(
            runner.resource_snapshot(),
            crate::embedding::providers::task_runner::BackgroundTaskSnapshot::default()
        );

        runner.shutdown();
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("refused work creates no tracked waiter");
    }

    #[tokio::test]
    async fn query_at_background_byte_limit_is_admitted() {
        struct PendingProvider;

        #[async_trait]
        impl EmbeddingProvider for PendingProvider {
            fn is_enabled(&self) -> bool {
                true
            }

            fn provider_name(&self) -> &'static str {
                "openai-compatible"
            }

            fn dimension(&self) -> usize {
                4
            }

            async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
                std::future::pending().await
            }
        }

        let runner = Arc::new(BackgroundTaskRunner::new());
        let service = make_service(Arc::new(PendingProvider), None, runner.clone());
        let input = "é".repeat(32_768);
        assert_eq!(input.len(), 65_536);

        service.enqueue_background_query_embedding(&input).await;

        assert_eq!(runner.resource_snapshot().admitted_tasks, 1);
        assert!(runner.resource_snapshot().retained_bytes <= 262_144);
        runner.shutdown();
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("shutdown cancels the bounded query retry");
        assert_eq!(runner.resource_snapshot(), Default::default());
    }

    #[tokio::test]
    async fn oversized_query_does_not_admit_background_retry() {
        let provider = Arc::new(ScriptedRemoteProvider(AtomicUsize::new(100)));
        let runner = Arc::new(BackgroundTaskRunner::new());
        let service = make_service(provider.clone(), None, runner.clone());
        let input = "é".repeat(32_769);

        assert!(
            service
                .generate_query_embedding_with_background(&input)
                .await
                .expect("foreground transient failure remains deferred")
                .is_none()
        );
        assert_eq!(input.len(), 65_538);
        assert_eq!(provider.0.load(Ordering::SeqCst), 99);
        assert_eq!(
            runner.resource_snapshot(),
            crate::embedding::providers::task_runner::BackgroundTaskSnapshot::default()
        );

        runner.shutdown();
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("no background query work remains");
    }

    #[tokio::test]
    async fn query_embedding_defers_transient_remote_errors() {
        let provider = Arc::new(ScriptedRemoteProvider(AtomicUsize::new(1)));
        let service = make_service(
            provider.clone(),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );

        // Transient remote failure: deferred to a background retry, the call
        // itself succeeds with no embedding.
        let deferred = service
            .generate_query_embedding_with_background("flaky")
            .await
            .unwrap();
        assert!(deferred.is_none());

        // Same input right after: the background task is inflight, so the
        // foreground path defers again instead of hammering the provider.
        let second = service
            .generate_query_embedding_with_background("flaky")
            .await
            .unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn cached_query_embedding_evicts_expired_entries() {
        let service = make_service(
            Arc::new(ScriptedProvider::new(4)),
            None,
            Arc::new(BackgroundTaskRunner::new()),
        );
        let cache_key = service.query_embedding_cache_key("ttl probe");
        {
            let mut cache = service.query_embedding_cache.lock().await;
            let insertion_time = std::time::Instant::now()
                - crate::embedding::runtime::query_embedding_cache_ttl()
                - std::time::Duration::from_secs(1);
            assert_eq!(
                cache.insert(cache_key.clone(), vec![1.0; 4], insertion_time),
                crate::embedding::query_cache::QueryCacheInsertOutcome::Stored
            );
        }
        assert!(service.cached_query_embedding("ttl probe").await.is_none());
        let cache = service.query_embedding_cache.lock().await;
        assert!(cache.is_empty(), "expired entry must be purged");
    }

    #[tokio::test]
    async fn store_embedding_on_fact_rejects_dimension_mismatch() {
        let provider = Arc::new(MismatchedProvider {
            dimension: 4,
            returned: 2,
        });
        let db = Arc::new(MockDbClient::new().expect_select_one(
            "fact:1",
            // A stale signature so the write path is not short-circuited
            // before dimension validation.
            Some(json!({"fact_id": "fact:1", "embedding_signature": "sig-previous"})),
        ));
        let service = EmbeddingService::new(
            db,
            "org",
            StdoutLogger::new("warn"),
            provider,
            0.8,
            Some("sig-a".to_string()),
            Some("test-model".to_string()),
            Some(4),
            Arc::new(RwLock::new(
                crate::platform::context_cache::ContextCacheState::new(
                    std::num::NonZeroUsize::new(8).unwrap(),
                    std::num::NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
                ),
            )),
            make_query_cache(),
            Arc::new(BackgroundTaskRunner::new()),
        );

        // `store_embedding_on_fact` reads the record, validates the vector
        // against the provider dimension, and rejects mismatches before write.
        let err = service
            .store_embedding_on_fact("fact:1", vec![1.0, 2.0])
            .await
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::Validation(ref msg) if msg.contains("dimension mismatch")),
            "expected dimension mismatch, got {err:?}"
        );
    }

    struct MismatchedProvider {
        dimension: usize,
        returned: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for MismatchedProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "mismatched"
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
            Ok(vec![0.5; self.returned])
        }
    }
}
