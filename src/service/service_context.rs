use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use lru::LruCache;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{AssembledContextItem, FactId};
use crate::service::cache::CacheKey;
use crate::service::embedding::EmbeddingProvider;
use crate::service::embedding::task_runner::BackgroundTaskRunner;
use crate::service::embedding_runtime::CachedQueryEmbedding;
use crate::service::entity::EntityService;
use crate::service::entity_extraction::EntityExtractor;
use crate::service::entity_resolution::EntityResolver;
use crate::service::error::MemoryError;
use crate::service::explanation::ExplanationService;
use crate::service::fact::FactService;
use crate::service::ingestion::IngestionService;
use crate::service::triple_extractor::TripleExtractor;
use crate::service::util::RateLimiter;
use crate::storage::DbClient;

/// Shared context passed to capability modules.
///
/// Contains the infrastructure dependencies that all capabilities need,
/// without exposing the full `MemoryService` surface. Capabilities and the
/// `assemble_context` pipeline read from this struct exclusively.
pub struct ServiceContext {
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) namespaces: Vec<String>,
    pub(crate) default_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    pub(crate) ingestion_service: IngestionService,
    pub(crate) explanation_service: ExplanationService,
    pub(crate) entity_resolver: EntityResolver,
    pub(crate) entity_service: EntityService,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
    pub(crate) embedding_provider: Arc<dyn EmbeddingProvider>,
    pub(crate) embedding_similarity_threshold: f64,
    pub(crate) current_embedding_signature: Option<String>,
    pub(crate) current_embedding_model: Option<String>,
    pub(crate) current_embedding_dimension: Option<usize>,
    pub(crate) task_runner: Arc<BackgroundTaskRunner>,
    pub(crate) fact_service: FactService,
    pub(crate) query_embedding_cache: Arc<Mutex<LruCache<String, CachedQueryEmbedding>>>,
    pub(crate) triple_extractor: Arc<dyn TripleExtractor>,
    pub(crate) context_cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    pub(crate) claim_store: Option<Arc<dyn crate::storage::claims::ClaimStore>>,
    pub(crate) query_logging_enabled: bool,
    pub(crate) query_log_retention_days: u32,
    /// Claim service reference for extract reconciliation.
    pub(crate) claim_service: crate::service::claims::project::ClaimService,
}

impl ServiceContext {
    /// Scans all namespaces for a record by its ID, returning the payload and owning namespace.
    pub(crate) async fn find_record_by_id(
        &self,
        record_id: &str,
    ) -> Result<
        (
            Option<serde_json::Map<String, serde_json::Value>>,
            Option<String>,
        ),
        MemoryError,
    > {
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(record_id, namespace).await?;
            if let Some(serde_json::Value::Object(map)) = record {
                return Ok((Some(map), Some(namespace.clone())));
            }
        }
        Ok((None, None))
    }

    /// Enforces rate limit based on the caller ID in the access payload.
    pub(crate) fn enforce_rate_limit(
        &self,
        access: Option<&crate::models::AccessPayload>,
    ) -> Result<(), MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
    }

    /// Returns the namespace for a given scope.
    pub(crate) fn namespace_for_scope(&self, scope: &str) -> Result<String, MemoryError> {
        crate::service::MemoryScope::parse(scope)?.namespace(&self.namespaces)
    }

    /// Returns the episode record for the given episode ID.
    pub(crate) async fn find_episode_record(
        &self,
        episode_id: &str,
    ) -> Result<
        (
            Option<serde_json::Map<String, serde_json::Value>>,
            Option<String>,
        ),
        MemoryError,
    > {
        self.find_record_by_id(episode_id).await
    }

    /// Returns the fact record for the given fact ID.
    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<
        (
            Option<serde_json::Map<String, serde_json::Value>>,
            Option<String>,
        ),
        MemoryError,
    > {
        self.find_record_by_id(fact_id).await
    }

    /// Records fact access (access_count, last_accessed) for recency scoring.
    pub(crate) async fn record_fact_access(
        &self,
        fact_id: &str,
        boost: i64,
    ) -> Result<(), MemoryError> {
        self.explanation_service
            .record_fact_access(fact_id, boost)
            .await
    }

    /// Whether query logging is enabled for this service.
    pub(crate) fn is_query_logging_enabled(&self) -> bool {
        self.query_logging_enabled
    }

    /// Number of days to retain query logs before pruning.
    pub(crate) fn query_log_retention_days(&self) -> u32 {
        self.query_log_retention_days
    }

    /// Public helper for tool-level logging.
    pub(crate) fn log_tool_event(
        &self,
        op: &str,
        args: Value,
        result: Value,
        level: LogLevel,
        request_id: Option<&str>,
    ) {
        self.logger.log(
            crate::service::log_event(op, args, result, None, request_id, None),
            level,
        );
    }

    /// Public helper for tool-level logging with duration.
    pub(crate) fn log_tool_event_with_duration(
        &self,
        op: &str,
        args: Value,
        result: Value,
        level: LogLevel,
        duration: std::time::Duration,
        request_id: Option<&str>,
    ) {
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.logger.log(
            crate::service::log_event(op, args, result, None, request_id, Some(duration_ms)),
            level,
        );
    }

    /// Returns the context store handle (the db client).
    pub(crate) fn context_store(&self) -> &dyn crate::storage::ContextStore {
        &self.db_client
    }

    /// Returns the context access log handle (the db client).
    pub(crate) fn context_access_log(&self) -> &dyn crate::storage::ContextAccessLog {
        &self.db_client
    }

    /// Finds an entity record by ID across all namespaces.
    pub(crate) async fn find_entity_record_by_id(
        &self,
        entity_id: &str,
    ) -> Result<Option<Value>, MemoryError> {
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(entity_id, namespace).await?;
            if record.is_some() {
                return Ok(record);
            }
        }
        Ok(None)
    }

    /// Returns the project associated with a source episode, if any.
    async fn project_for_source_episode(
        &self,
        source_episode: &str,
    ) -> Result<Option<String>, MemoryError> {
        let (record, _) = self.find_episode_record(source_episode).await?;
        Ok(record
            .as_ref()
            .and_then(|map| map.get("project"))
            .and_then(crate::service::value_helpers::string_from_value))
    }

    pub(crate) fn build_fact_embedding_input(
        fact_type: &str,
        content: &str,
        quote: &str,
    ) -> String {
        format!("{fact_type}\n{content}\n{quote}")
    }

    /// Validates embedding dimension and builds `EmbeddingPayload` for
    /// passing to `FactService::create_fact`.
    fn build_embedding_payload(
        &self,
        embedding: Vec<f64>,
    ) -> Result<crate::service::fact::EmbeddingPayload, MemoryError> {
        let expected_dim = self
            .current_embedding_dimension
            .unwrap_or_else(|| self.embedding_provider.dimension());
        if embedding.len() != expected_dim {
            return Err(MemoryError::Validation(format!(
                "embedding dimension mismatch: provider returned {}, expected {expected_dim}",
                embedding.len()
            )));
        }
        Ok(crate::service::fact::EmbeddingPayload {
            embedding,
            provider: self.embedding_provider.provider_name().to_string(),
            model: self.current_embedding_model.clone(),
            dimension: expected_dim,
            signature: self.current_embedding_signature.clone(),
            updated_at: crate::service::normalize_dt(crate::service::query::now()),
        })
    }

    /// Adds a new fact.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn add_fact(
        &self,
        fact_type: &str,
        content: &str,
        quote: &str,
        source_episode: &str,
        t_valid: DateTime<Utc>,
        scope: &str,
        confidence: f64,
        entity_links: Vec<String>,
        policy_tags: Vec<String>,
        provenance: crate::models::Provenance,
    ) -> Result<String, MemoryError> {
        crate::service::util::validate_fact_input(
            fact_type,
            content,
            quote,
            source_episode,
            scope,
        )?;

        let namespace = self.namespace_for_scope(scope)?;
        let project = self.project_for_source_episode(source_episode).await?;

        // Pre-fetch entity records to avoid async closures.
        let mut entity_map: std::collections::HashMap<String, (String, Vec<String>)> =
            std::collections::HashMap::new();
        for entity_id in &entity_links {
            let entity_record = self.find_entity_record_by_id(entity_id).await?;
            if let Some(map) = entity_record.as_ref().and_then(Value::as_object) {
                let canonical = map
                    .get("canonical_name")
                    .and_then(Value::as_str)
                    .unwrap_or(entity_id.as_str())
                    .to_string();
                let aliases = map
                    .get("aliases")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                entity_map.insert(entity_id.clone(), (canonical, aliases));
            }
        }

        // Pre-fetch episode source_id.
        let (episode_record, _) = self.find_episode_record(source_episode).await?;
        let episode_source_id = episode_record
            .as_ref()
            .and_then(|map| map.get("source_id"))
            .and_then(crate::service::value_helpers::string_from_value);

        let entity_lookup = |entity_id: &str| Ok(entity_map.get(entity_id).cloned());
        let source_reference_lookup = |_episode_id: &str| Ok(episode_source_id.clone());

        let index_keys = self
            .fact_service
            .build_index_keys(
                content,
                source_episode,
                &provenance,
                &entity_links,
                t_valid,
                entity_lookup,
                source_reference_lookup,
            )
            .await?;

        // Prepare embedding input and generate or defer.
        let embedding_input = Self::build_fact_embedding_input(fact_type, content, quote);
        let mut deferred_embedding_input = None;
        let embedding_fields = match self.generate_embedding(&embedding_input).await {
            Ok(Some(embedding)) => {
                self.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("embedding.generate.success")),
                        (
                            "provider".to_string(),
                            json!(self.embedding_provider.provider_name()),
                        ),
                        ("fact_type".to_string(), json!(fact_type)),
                    ]),
                    LogLevel::Info,
                );
                Some(self.build_embedding_payload(embedding)?)
            }
            Ok(None) => None,
            Err(err) => {
                self.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("embedding.write_skipped")),
                        (
                            "provider".to_string(),
                            json!(self.embedding_provider.provider_name()),
                        ),
                        ("error".to_string(), json!(err.to_string())),
                        ("fact_type".to_string(), json!(fact_type)),
                    ]),
                    LogLevel::Warn,
                );
                if self.should_defer_embedding_retry(&err) {
                    deferred_embedding_input = Some(embedding_input.clone());
                }
                None
            }
        };

        // Create fact record via FactService.
        let fact_id = self
            .fact_service
            .create_fact(
                fact_type,
                content,
                quote,
                source_episode,
                t_valid,
                scope,
                confidence,
                &entity_links,
                &policy_tags,
                &provenance,
                &namespace,
                project.as_deref(),
                embedding_fields,
                index_keys,
            )
            .await?;

        // Invalidate caches immediately after ingestion to ensure isolation.
        crate::service::cache::invalidate_cache_by_scope(&self.context_cache, scope).await;

        // Background processes: triple extraction and pending embedding retries.
        self.spawn_triple_extraction(&fact_id, content, &namespace);

        // Synchronous claim projection for deterministic extract visibility.
        let claim_svc = self.claim_service.clone();
        let claim_fact_id = FactId::from(fact_id.clone());
        let claim_episode_id = crate::models::EpisodeId::from(source_episode.to_string());
        let claim_content = content.to_string();
        let claim_scope = scope.to_string();
        let claim_project = project.clone();
        let claim_entity_links = entity_links.clone();
        let claim_t_valid = t_valid;
        let claim_params = crate::service::claims::project::FactPersistedParams {
            namespace: &namespace,
            fact_id: &claim_fact_id,
            source_episode_id: &claim_episode_id,
            content: &claim_content,
            scope: &claim_scope,
            project: claim_project.as_deref(),
            entity_links: &claim_entity_links,
            t_valid: claim_t_valid,
        };
        match claim_svc.after_fact_persisted(&claim_params).await {
            Ok(summary) => claim_svc.record_post_fact_success(
                &namespace,
                &summary.fact_id,
                summary.claims_projected,
                summary.claims_skipped,
            ),
            Err(error) => claim_svc.record_post_fact_failure(&namespace, &claim_fact_id, &error),
        }

        // Enqueue background embedding after claim projection to ensure test invariants.
        if let Some(input) = deferred_embedding_input {
            self.enqueue_background_fact_embedding(namespace.clone(), fact_id.clone(), input)
                .await;
        }

        Ok(fact_id)
    }

    /// Spawn a fire-and-forget triple extraction task.
    fn spawn_triple_extraction(&self, fact_id: &str, content: &str, namespace: &str) {
        let extractor = self.triple_extractor.clone();
        let fact_id = fact_id.to_string();
        let content = content.to_string();
        let namespace = namespace.to_string();
        let entity_service = self.entity_service.clone();

        tokio::spawn(async move {
            if let Ok(triples) = extractor.extract(&content, &fact_id).await {
                for triple in &triples {
                    let sql = r#"
                        CREATE TYPE::thing("triple", rand::guid()) SET
                            namespace = $ns,
                            subject = $subject,
                            predicate = $predicate,
                            object = $object,
                            confidence = $confidence,
                            source_fact_id = $source_fact_id
                    "#;
                    let vars = serde_json::json!({
                        "ns": namespace,
                        "subject": triple.subject,
                        "predicate": triple.predicate,
                        "object": triple.object,
                        "confidence": triple.confidence,
                        "source_fact_id": triple.source_fact_id,
                    });
                    let _ = entity_service.execute_query(sql, vars, &namespace).await;

                    if crate::service::triple_extractor::is_singleton_predicate(&triple.predicate) {
                        let _ = crate::service::conflict_resolver::resolve_conflicts_for_triple(
                            &entity_service,
                            &namespace,
                            triple,
                        )
                        .await;
                    }
                }
            }
        });
    }

    pub(crate) async fn generate_embedding(
        &self,
        input: &str,
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        const MAX_EMBEDDING_INPUT_CHARS: usize = 8_000;
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

    fn should_defer_embedding_retry(&self, err: &MemoryError) -> bool {
        crate::service::is_transient_embedding_error(err)
            && crate::service::is_remote_embedding_provider(self.embedding_provider.provider_name())
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

    async fn enqueue_background_fact_embedding(
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

        let snapshot = self.embedding_background_snapshot();
        tokio::spawn(async move {
            snapshot
                .run_background_fact_embedding_task(task_key, namespace, fact_id, input)
                .await;
        });
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

        let snapshot = self.embedding_background_snapshot();
        tokio::spawn(async move {
            snapshot
                .run_background_query_embedding_task(task_key, input)
                .await;
        });
    }

    /// Captures the `Arc`-based fields needed by background embedding tasks.
    fn embedding_background_snapshot(&self) -> EmbeddingBackgroundSnapshot {
        EmbeddingBackgroundSnapshot {
            db_client: self.db_client.clone(),
            logger: self.logger.clone(),
            embedding_provider: self.embedding_provider.clone(),
            current_embedding_signature: self.current_embedding_signature.clone(),
            context_cache: self.context_cache.clone(),
            query_embedding_cache: self.query_embedding_cache.clone(),
            task_runner: self.task_runner.clone(),
        }
    }
}

/// Owned snapshot of the fields required by background embedding tasks.
///
/// Background tasks cannot borrow `&ServiceContext` across `.await` points
/// in a spawned task, so they receive an owned snapshot of the `Arc` fields
/// they need.
struct EmbeddingBackgroundSnapshot {
    db_client: Arc<dyn DbClient>,
    logger: StdoutLogger,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    current_embedding_signature: Option<String>,
    context_cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    query_embedding_cache: Arc<Mutex<LruCache<String, CachedQueryEmbedding>>>,
    task_runner: Arc<BackgroundTaskRunner>,
}

impl EmbeddingBackgroundSnapshot {
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
                    self.store_embedding_on_fact(namespace, fact_id, embedding)
                        .await?;
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

    async fn store_embedding_on_fact(
        &self,
        namespace: &str,
        fact_id: &str,
        embedding: Vec<f64>,
    ) -> Result<(), MemoryError> {
        let Some(Value::Object(mut record)) = self.db_client.select_one(fact_id, namespace).await?
        else {
            return Err(MemoryError::NotFound(format!(
                "fact_id not found for background embedding: {fact_id}"
            )));
        };

        if let Some(current_signature) = self.current_embedding_signature.as_deref()
            && record.get("embedding_signature").and_then(Value::as_str) == Some(current_signature)
        {
            return Ok(());
        }

        let scope = record
            .get("scope")
            .and_then(crate::service::value_helpers::string_from_value)
            .unwrap_or_else(|| namespace.to_string());
        self.insert_current_embedding_fields(&mut record, embedding)?;
        self.db_client
            .update(fact_id, Value::Object(record), namespace)
            .await?;
        crate::service::cache::invalidate_cache_by_scope(&self.context_cache, &scope).await;
        Ok(())
    }

    pub(crate) async fn generate_embedding(
        &self,
        input: &str,
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        const MAX_EMBEDDING_INPUT_CHARS: usize = 8_000;
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

    fn should_defer_embedding_retry(&self, err: &MemoryError) -> bool {
        crate::service::is_transient_embedding_error(err)
            && crate::service::is_remote_embedding_provider(self.embedding_provider.provider_name())
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

    async fn release_background_embedding_task(&self, task_key: &str) {
        self.task_runner.release(task_key).await;
    }
}

impl crate::service::apps::graph::GraphContext for ServiceContext {
    fn app_store(&self) -> &dyn crate::storage::AppStore {
        &self.db_client
    }
    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}
