//! Embedding generation service — owns embedding provider access, query/fact
//! embedding caching, and the background embedding retry pipeline.
//!
//! Concentrates embedding concerns in one place.

use std::sync::Arc;
use std::time::Instant;

use lru::LruCache;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};

use crate::error::MemoryError;
use crate::logging::{LogLevel, StdoutLogger};
use crate::models::AssembledContextItem;
use crate::service::cache::CacheKey;
use crate::service::embedding::EmbeddingProvider;
use crate::service::embedding::task_runner::BackgroundTaskRunner;
use crate::service::embedding_runtime::CachedQueryEmbedding;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use lru::LruCache;
    use serde_json::json;
    use tokio::sync::{Mutex, RwLock};

    use super::*;
    use crate::error::MemoryError;
    use crate::logging::StdoutLogger;
    use crate::service::embedding::task_runner::BackgroundTaskRunner;
    use crate::service::embedding_runtime::CachedQueryEmbedding;
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

    fn make_service(
        provider: Arc<dyn EmbeddingProvider>,
        signature: Option<&str>,
        runner: Arc<BackgroundTaskRunner>,
    ) -> EmbeddingService {
        let context_cache = Arc::new(RwLock::new(LruCache::new(
            std::num::NonZeroUsize::new(8).unwrap(),
        )));
        let query_embedding_cache = Arc::new(Mutex::new(LruCache::new(
            std::num::NonZeroUsize::new(8).unwrap(),
        )));
        EmbeddingService::new(
            Arc::new(MockDbClient::new()),
            "org",
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
        assert!(runner.try_reserve(&task_key).await);
        let deferred = service
            .generate_query_embedding_with_background("deferred input")
            .await
            .unwrap();
        assert!(deferred.is_none());
        assert_eq!(provider.calls(), 1);
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
            // Expired entry must be evicted, not served.
            cache.put(
                cache_key.clone(),
                CachedQueryEmbedding {
                    embedding: vec![1.0; 4],
                    expires_at: std::time::Instant::now() - std::time::Duration::from_secs(1),
                },
            );
        }
        assert!(service.cached_query_embedding("ttl probe").await.is_none());
        let mut cache = service.query_embedding_cache.lock().await;
        assert!(
            cache.get(&cache_key).is_none(),
            "expired entry must be evicted"
        );
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
            Arc::new(RwLock::new(LruCache::new(
                std::num::NonZeroUsize::new(8).unwrap(),
            ))),
            Arc::new(Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(8).unwrap(),
            ))),
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
