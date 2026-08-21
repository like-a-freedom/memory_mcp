//! Embedding generation service — owns embedding provider access, query/fact
//! embedding caching, and the background embedding retry pipeline.
//!
//! Concentrates embedding concerns in one place.

use std::sync::Arc;
use std::time::Instant;

use lru::LruCache;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::AssembledContextItem;
use crate::service::cache::CacheKey;
use crate::service::embedding::EmbeddingProvider;
use crate::service::embedding::task_runner::BackgroundTaskRunner;
use crate::service::embedding_runtime::CachedQueryEmbedding;
use crate::service::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Maximum input length accepted by embedding providers. Inputs longer than
/// this are truncated before being sent to the provider.
const MAX_EMBEDDING_INPUT_CHARS: usize = 8_000;

/// Owns all embedding generation, caching, and background retry logic.
///
/// Held as a field on `ServiceContext` and accessed by the `assemble_context`
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
    context_cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    query_embedding_cache: Arc<Mutex<LruCache<String, CachedQueryEmbedding>>>,
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
        context_cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
        query_embedding_cache: Arc<Mutex<LruCache<String, CachedQueryEmbedding>>>,
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
        crate::service::is_transient_embedding_error(err)
            && crate::service::is_remote_embedding_provider(self.embedding_provider.provider_name())
    }

    pub(crate) async fn generate_embedding(
        &self,
        input: &str,
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        let effective_input: String = if input.len() > MAX_EMBEDDING_INPUT_CHARS {
            let truncated: String = input.chars().take(MAX_EMBEDDING_INPUT_CHARS).collect();
            self.logger.log(
                crate::service::log_event(
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
        let provider = self.embedding_provider.provider_name();
        let args = json!({
            "provider": provider,
            "input_chars": effective_input.chars().count(),
        });

        if !self.embedding_provider.is_enabled() {
            let mut result = crate::service::core::build_embedding_log_result(0, None);
            if let Some(map) = result.as_object_mut() {
                map.insert("status".to_string(), json!("disabled"));
            }
            self.logger.log(
                crate::service::log_event(
                    "embedding.generate.skipped",
                    crate::service::log_args_with_duration(args, timer.elapsed()),
                    result,
                    None,
                    None,
                    None,
                ),
                LogLevel::Debug,
            );
            return Ok(None);
        }

        match self.embedding_provider.embed(&effective_input).await {
            Ok(embedding) => {
                self.logger.log(
                    crate::service::log_event(
                        "embedding.generate.done",
                        crate::service::log_args_with_duration(args, timer.elapsed()),
                        crate::service::core::build_embedding_log_result(1, Some(embedding.len())),
                        None,
                        None,
                        None,
                    ),
                    LogLevel::Info,
                );
                Ok(Some(embedding))
            }
            Err(err) => {
                let mut result = crate::service::core::build_embedding_log_result(0, None);
                if let Some(map) = result.as_object_mut() {
                    map.insert("error".to_string(), json!(err.to_string()));
                }
                self.logger.log(
                    crate::service::log_event(
                        "embedding.generate.error",
                        crate::service::log_args_with_duration(args, timer.elapsed()),
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
                self.enqueue_background_query_embedding(input.to_string())
                    .await;
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) async fn cached_query_embedding(&self, input: &str) -> Option<Vec<f64>> {
        let cache_key = self.query_embedding_cache_key(input);
        let mut cache = self.query_embedding_cache.lock().await;
        let now = std::time::Instant::now();
        if let Some(entry) = cache.get(&cache_key).cloned() {
            if entry.expires_at > now {
                return Some(entry.embedding);
            }
            cache.pop(&cache_key);
        }

        None
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
        if !self.try_reserve_background_embedding_task(&task_key).await {
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

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("embedding.background_enqueued")),
                ("kind".to_string(), json!("fact")),
                ("namespace".to_string(), json!(namespace.clone())),
                ("fact_id".to_string(), json!(fact_id.clone())),
            ]),
            LogLevel::Info,
        );

        // Self-terminating task: `run_background_fact_embedding_task` runs a
        // bounded retry loop (DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS) and exits
        // on success, `Ok(None)`, or a non-retryable error. It is not an
        // infinite loop, so no CancellationToken is needed. The `task_runner`
        // reservation is released when the task completes. The clone freezes
        // the embedding policy state at spawn time.
        let service = self.clone();
        tokio::spawn(async move {
            service
                .run_background_fact_embedding_task(task_key, namespace, fact_id, input)
                .await;
        });
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
        crate::service::hash_prefix(&format!(
            "{signature}|{}",
            crate::service::normalize_text(input)
        ))
    }

    async fn store_query_embedding(&self, input: &str, embedding: Vec<f64>) {
        let cache_key = self.query_embedding_cache_key(input);
        let mut cache = self.query_embedding_cache.lock().await;
        cache.put(
            cache_key,
            crate::service::CachedQueryEmbedding {
                embedding,
                expires_at: std::time::Instant::now() + crate::service::query_embedding_cache_ttl(),
            },
        );
    }

    async fn background_embedding_task_inflight(&self, task_key: &str) -> bool {
        self.task_runner.is_inflight(task_key).await
    }

    async fn try_reserve_background_embedding_task(&self, task_key: &str) -> bool {
        self.task_runner.try_reserve(task_key).await
    }

    async fn enqueue_background_query_embedding(&self, input: String) {
        let task_key = self.background_query_task_key(&input);
        if !self.try_reserve_background_embedding_task(&task_key).await {
            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.background_deduped")),
                    ("kind".to_string(), json!("query")),
                    ("input_chars".to_string(), json!(input.chars().count())),
                ]),
                LogLevel::Debug,
            );
            return;
        }

        self.logger.log(
            std::collections::HashMap::from([
                ("op".to_string(), json!("embedding.background_enqueued")),
                ("kind".to_string(), json!("query")),
                ("input_chars".to_string(), json!(input.chars().count())),
            ]),
            LogLevel::Info,
        );

        // Self-terminating task: `run_background_query_embedding_task` runs a
        // bounded retry loop (DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS) and exits
        // on success, `Ok(None)`, or a non-retryable error. It is not an
        // infinite loop, so no CancellationToken is needed. The `task_runner`
        // reservation is released when the task completes. The clone freezes
        // the embedding policy state at spawn time.
        let service = self.clone();
        tokio::spawn(async move {
            service
                .run_background_query_embedding_task(task_key, input)
                .await;
        });
    }

    // ─── Background task execution ──────────────────────────────────────
    //
    // These methods run inside spawned tasks on a cloned service instance.

    async fn run_background_fact_embedding_task(
        self,
        task_key: String,
        namespace: String,
        fact_id: String,
        input: String,
    ) {
        let outcome = self
            .run_background_fact_embedding_task_inner(&namespace, &fact_id, &input)
            .await;
        self.release_background_embedding_task(&task_key).await;

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
        for attempt in 1..=crate::service::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS {
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
                        && attempt < crate::service::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS =>
                {
                    let delay = crate::service::background_embedding_retry_delay(attempt);
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

    async fn run_background_query_embedding_task(self, task_key: String, input: String) {
        let outcome = self.run_background_query_embedding_task_inner(&input).await;
        self.release_background_embedding_task(&task_key).await;

        if let Err(err) = outcome {
            self.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.background_failed")),
                    ("kind".to_string(), json!("query")),
                    ("input_chars".to_string(), json!(input.chars().count())),
                    ("error".to_string(), json!(err.to_string())),
                ]),
                LogLevel::Warn,
            );
        }
    }

    async fn run_background_query_embedding_task_inner(
        &self,
        input: &str,
    ) -> Result<(), MemoryError> {
        for attempt in 1..=crate::service::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS {
            match self.generate_embedding(input).await {
                Ok(Some(embedding)) => {
                    self.store_query_embedding(input, embedding).await;
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
                        && attempt < crate::service::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS =>
                {
                    let delay = crate::service::background_embedding_retry_delay(attempt);
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

    fn insert_current_embedding_fields(
        &self,
        payload: &mut serde_json::Map<String, Value>,
        embedding: Vec<f64>,
    ) -> Result<(), MemoryError> {
        let expected_dim = self.embedding_provider.dimension();
        if embedding.len() != expected_dim {
            return Err(MemoryError::Validation(format!(
                "embedding dimension mismatch: provider returned {}, expected {expected_dim}",
                embedding.len()
            )));
        }
        payload.insert("embedding".to_string(), json!(embedding));
        payload.insert(
            "embedding_provider".to_string(),
            json!(self.embedding_provider.provider_name()),
        );
        if let Some(signature) = &self.current_embedding_signature {
            payload.insert("embedding_signature".to_string(), json!(signature));
        }
        payload.insert(
            "embedding_updated_at".to_string(),
            json!(crate::service::normalize_dt(crate::service::query::now())),
        );
        Ok(())
    }

    async fn store_embedding_on_fact(
        &self,
        fact_id: &str,
        embedding: Vec<f64>,
    ) -> Result<(), MemoryError> {
        let Some(Value::Object(mut record)) = self.db.select_one(fact_id).await? else {
            return Err(MemoryError::NotFound(format!(
                "fact_id not found for background embedding: {fact_id}"
            )));
        };

        if let Some(current_signature) = self.current_embedding_signature.as_deref()
            && record.get("embedding_signature").and_then(Value::as_str) == Some(current_signature)
        {
            return Ok(());
        }

        self.insert_current_embedding_fields(&mut record, embedding)?;
        self.db.update(fact_id, Value::Object(record)).await?;
        crate::service::cache::invalidate_cache(&self.context_cache).await;
        Ok(())
    }

    async fn release_background_embedding_task(&self, task_key: &str) {
        self.task_runner.release(task_key).await;
    }
}
