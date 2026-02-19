use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// Trait for safe mutex locking that handles poisoned locks gracefully.
/// For cache operations, a poisoned mutex is not fatal - we can continue operating.
pub trait SafeMutex<T> {
    fn safe_lock(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> SafeMutex<T> for Mutex<T> {
    fn safe_lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

use chrono::{DateTime, Utc};
use lru::LruCache;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::SurrealConfig;
use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{
    AccessContext, AssembleContextRequest, Edge, EntityCandidate, Episode, ExplainRequest, Fact,
    IngestRequest, InvalidateRequest,
};
use crate::storage::{DbClient, SurrealDbClient};

#[derive(thiserror::Error, Debug)]
pub enum MemoryError {
    #[error("config missing: {0}")]
    ConfigMissing(String),
    #[error("config invalid: {0}")]
    ConfigInvalid(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("validation error: {0}")]
    Validation(String),
}

const CONTEXT_CACHE_SIZE: usize = 512;

/// Core service for memory operations.
///
/// `MemoryService` provides the main business logic for the Memory MCP system,
/// including episode ingestion, entity extraction, fact management, and context assembly.
///
/// # Example
///
/// ```rust,no_run
/// use memory_mcp::MemoryService;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let service = MemoryService::new_from_env().await?;
///     // Use the service for memory operations
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct MemoryService {
    db_client: Arc<dyn DbClient>,
    namespaces: Vec<String>,
    default_namespace: String,
    logger: StdoutLogger,
    rate_limiter: Arc<RateLimiter>,
    context_cache: Arc<Mutex<LruCache<CacheKey, Vec<Value>>>>,
    analyzers: Arc<Mutex<HashMap<String, Value>>>,
    indexes: Arc<Mutex<HashMap<String, Value>>>,
    name_regex: Regex,
}

impl MemoryService {
    /// Creates a new `MemoryService` from environment variables.
    ///
    /// # Environment Variables
    ///
    /// - `SURREALDB_DB_NAME`: Database name (required)
    /// - `SURREALDB_URL`: WebSocket URL for remote connection (required for remote)
    /// - `SURREALDB_EMBEDDED`: Set to "true" for embedded RocksDB mode
    /// - `SURREALDB_DATA_DIR`: Path to RocksDB data directory (embedded mode)
    /// - `SURREALDB_NAMESPACES`: Comma-separated list of namespaces
    /// - `SURREALDB_USERNAME`: Database username
    /// - `SURREALDB_PASSWORD`: Database password
    /// - `LOG_LEVEL`: Logging level (trace, debug, info, warn, error)
    ///
    /// # Errors
    ///
    /// Returns `MemoryError::ConfigMissing` if required environment variables are missing.
    /// Returns `MemoryError::Storage` if database connection fails.
    pub async fn new_from_env() -> Result<Self, MemoryError> {
        let config = SurrealConfig::from_env()?;
        let default_namespace = config
            .namespaces
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_string());
        let db_client = SurrealDbClient::connect(&config, &default_namespace).await?;
        let service = Self::new(
            Arc::new(db_client),
            config.namespaces,
            config.log_level,
            50,
            100,
        )?;
        service.check_surrealdb_connection().await?;
        // Apply DB schema migrations on first start (no-op if none present)
        // Use the canonical ./migrations directory (repo root)
        // Delegate to the DB client which will invoke `surrealdb-migrations` runner
        service
            .db_client
            .apply_migrations(&service.default_namespace)
            .await?;
        Ok(service)
    }

    pub fn new(
        db_client: Arc<dyn DbClient>,
        namespaces: Vec<String>,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
    ) -> Result<Self, MemoryError> {
        Self::build(
            db_client,
            namespaces,
            log_level,
            rate_limit_rps,
            rate_limit_burst,
            CONTEXT_CACHE_SIZE,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_cache_size(
        db_client: Arc<dyn DbClient>,
        namespaces: Vec<String>,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
        cache_size: usize,
    ) -> Result<Self, MemoryError> {
        Self::build(
            db_client,
            namespaces,
            log_level,
            rate_limit_rps,
            rate_limit_burst,
            cache_size,
        )
    }

    fn build(
        db_client: Arc<dyn DbClient>,
        namespaces: Vec<String>,
        log_level: String,
        rate_limit_rps: i32,
        rate_limit_burst: i32,
        cache_size: usize,
    ) -> Result<Self, MemoryError> {
        if namespaces.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "namespaces cannot be empty".to_string(),
            ));
        }
        let cache_size = NonZeroUsize::new(cache_size).ok_or_else(|| {
            MemoryError::ConfigInvalid("context cache size must be > 0".to_string())
        })?;
        let logger = StdoutLogger::new(&log_level);
        Ok(Self {
            db_client,
            namespaces: namespaces.clone(),
            default_namespace: namespaces[0].clone(),
            logger,
            rate_limiter: Arc::new(RateLimiter::new(rate_limit_rps, rate_limit_burst)),
            context_cache: Arc::new(Mutex::new(LruCache::new(cache_size))),
            analyzers: Arc::new(Mutex::new(HashMap::new())),
            indexes: Arc::new(Mutex::new(HashMap::new())),
            name_regex: Regex::new(r"[A-Z][a-z]+(?:\s+[A-Z][a-z]+)+")
                .map_err(|err| MemoryError::Validation(format!("regex error: {err}")))?,
        })
    }

    /// Public helper for tool-level logging from the MCP handler.
    pub fn log_tool_event(&self, op: &str, args: Value, result: Value, level: LogLevel) {
        self.logger.log(log_event(op, args, result, None), level);
    }

    pub async fn episode_count(&self) -> Result<i32, MemoryError> {
        let mut total = 0;
        for namespace in &self.namespaces {
            total += self
                .db_client
                .select_table("episode", namespace)
                .await?
                .len() as i32;
        }
        Ok(total)
    }

    pub async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessContext>,
    ) -> Result<String, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        if request.source_type.trim().is_empty() {
            return Err(MemoryError::Validation("source_type is required".into()));
        }
        if request.source_id.trim().is_empty() {
            return Err(MemoryError::Validation("source_id is required".into()));
        }
        if request.content.trim().is_empty() {
            return Err(MemoryError::Validation("content is required".into()));
        }
        if request.scope.trim().is_empty() {
            return Err(MemoryError::Validation("scope is required".into()));
        }

        let episode_id = deterministic_episode_id(
            &request.source_type,
            &request.source_id,
            request.t_ref,
            &request.scope,
        );
        let namespace = self.namespace_for_scope(&request.scope);
        let existing = self.db_client.select_one(&episode_id, &namespace).await?;
        if existing.is_none() {
            let t_ingested = request.t_ingested.unwrap_or_else(now);
            let payload = json!({
                "episode_id": episode_id,
                "source_type": request.source_type,
                "source_id": request.source_id,
                "content": request.content,
                "t_ref": normalize_dt(request.t_ref),
                "t_ingested": normalize_dt(t_ingested),
                "scope": request.scope,
                "visibility_scope": request.visibility_scope.unwrap_or_else(|| request.scope.clone()),
                "policy_tags": request.policy_tags,
            });
            self.db_client
                .create(&episode_id, payload, &namespace)
                .await?;
        }

        self.logger.log(
            log_event(
                "ingest",
                json!({
                    "source_type": request.source_type,
                    "source_id": request.source_id,
                    "t_ref": normalize_iso(request.t_ref),
                    "scope": request.scope,
                }),
                json!({"episode_id": episode_id}),
                access.as_ref(),
            ),
            LogLevel::Info,
        );

        Ok(episode_id)
    }

    pub async fn explain(
        &self,
        request: ExplainRequest,
        access: Option<AccessContext>,
    ) -> Result<Vec<Value>, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        let explanations = request
            .context_pack
            .into_iter()
            .map(|item| {
                json!({
                    "content": item.content,
                    "quote": item.quote,
                    "source_episode": item.source_episode,
                })
            })
            .collect::<Vec<_>>();

        self.logger.log(
            log_event(
                "explain",
                json!({"count": explanations.len()}),
                json!({"count": explanations.len()}),
                access.as_ref(),
            ),
            LogLevel::Info,
        );

        Ok(explanations)
    }

    pub async fn extract(
        &self,
        episode_id: &str,
        access: Option<AccessContext>,
    ) -> Result<Value, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        let (record, _) = self.find_episode_record(episode_id).await?;
        if record.is_none() {
            return Err(MemoryError::NotFound(format!(
                "episode_id not found: {episode_id}"
            )));
        }
        let payload = self.extract_from_episode(episode_id).await?;
        self.logger.log(
            log_event(
                "extract",
                json!({"episode_id": episode_id}),
                json!({
                    "entities": payload["entities"].as_array().map(|v| v.len()).unwrap_or(0),
                    "facts": payload["facts"].as_array().map(|v| v.len()).unwrap_or(0),
                    "links": payload["links"].as_array().map(|v| v.len()).unwrap_or(0),
                }),
                access.as_ref(),
            ),
            LogLevel::Info,
        );
        Ok(payload)
    }

    pub async fn resolve(
        &self,
        candidate: EntityCandidate,
        access: Option<AccessContext>,
    ) -> Result<String, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        if candidate.entity_type.trim().is_empty() {
            return Err(MemoryError::Validation("entity_type is required".into()));
        }
        if candidate.canonical_name.trim().is_empty() {
            return Err(MemoryError::Validation("canonical_name is required".into()));
        }
        let namespace = self.default_namespace.clone();
        let existing = self
            .find_entity_record(&candidate.canonical_name, &namespace)
            .await?;
        if let Some(record) = existing {
            let existing_id = record
                .get("entity_id")
                .and_then(Value::as_str)
                .or_else(|| record.get("id").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            return Ok(existing_id);
        }

        let entity_id = deterministic_entity_id(&candidate.entity_type, &candidate.canonical_name);
        let aliases = candidate
            .aliases
            .into_iter()
            .filter(|alias| !alias.trim().is_empty())
            .map(|alias| normalize_text(&alias))
            .collect::<Vec<_>>();

        let payload = json!({
            "entity_id": entity_id,
            "entity_type": candidate.entity_type,
            "canonical_name": candidate.canonical_name,
            "aliases": aliases.clone(),
        });
        self.db_client
            .create(&entity_id, payload, &namespace)
            .await?;
        if !aliases.is_empty() {
            let statement = format!("UPDATE {} SET aliases = $aliases RETURN *", entity_id);
            self.db_client
                .query(&statement, Some(json!({"aliases": aliases})), &namespace)
                .await?;
        }

        Ok(entity_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_fact(
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
        provenance: Value,
    ) -> Result<String, MemoryError> {
        if fact_type.trim().is_empty() {
            return Err(MemoryError::Validation("fact_type is required".into()));
        }
        if content.trim().is_empty() {
            return Err(MemoryError::Validation("content is required".into()));
        }
        if quote.trim().is_empty() {
            return Err(MemoryError::Validation("quote is required".into()));
        }
        if source_episode.trim().is_empty() {
            return Err(MemoryError::Validation("source_episode is required".into()));
        }
        if scope.trim().is_empty() {
            return Err(MemoryError::Validation("scope is required".into()));
        }

        let fact_id = deterministic_fact_id(fact_type, content, source_episode, t_valid);
        let namespace = self.namespace_for_scope(scope);
        let existing = self.db_client.select_one(&fact_id, &namespace).await?;
        if existing.is_none() {
            let t_ingested = now();
            let payload = json!({
                "fact_id": fact_id,
                "fact_type": fact_type,
                "content": content,
                "quote": quote,
                "source_episode": source_episode,
                "t_valid": normalize_dt(t_valid),
                "t_ingested": normalize_dt(t_ingested),
                "t_invalid": Value::Null,
                "t_invalid_ingested": Value::Null,
                "confidence": confidence,
                "entity_links": entity_links,
                "scope": scope,
                "policy_tags": policy_tags,
                "provenance": provenance,
            });
            self.db_client.create(&fact_id, payload, &namespace).await?;
            self.context_cache.safe_lock().clear();
        }
        Ok(fact_id)
    }

    pub async fn invalidate(
        &self,
        request: InvalidateRequest,
        access: Option<AccessContext>,
    ) -> Result<String, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        let (record, namespace) = self.find_fact_record(&request.fact_id).await?;
        let namespace =
            namespace.ok_or_else(|| MemoryError::NotFound("fact_id not found".into()))?;
        let mut updated =
            record.ok_or_else(|| MemoryError::NotFound("fact_id not found".into()))?;
        updated.insert(
            "t_invalid".to_string(),
            json!(normalize_dt(request.t_invalid)),
        );
        updated.insert("t_invalid_ingested".to_string(), json!(normalize_dt(now())));
        self.db_client
            .update(&request.fact_id, Value::Object(updated), &namespace)
            .await?;
        self.context_cache.safe_lock().clear();
        Ok("ok".to_string())
    }

    pub async fn assemble_context(
        &self,
        request: AssembleContextRequest,
    ) -> Result<Vec<Value>, MemoryError> {
        let access = AccessContext::from_payload(request.access.clone());

        // Log high-level request start
        self.logger.log(
            log_event(
                "assemble_context.start",
                json!({"scope": request.scope, "query": request.query, "budget": request.budget}),
                json!({}),
                access.as_ref(),
            ),
            LogLevel::Info,
        );

        self.enforce_rate_limit(access.as_ref())?;
        if request.scope.trim().is_empty() {
            return Err(MemoryError::Validation("scope is required".into()));
        }
        let cutoff = request.as_of.unwrap_or_else(now);
        let access = access.unwrap_or_else(|| AccessContext {
            allowed_scopes: Some(vec![request.scope.clone()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        });
        if !self.is_scope_allowed(&request.scope, &access) {
            return Ok(vec![]);
        }

        let cache_key = CacheKey::new(
            &request.query,
            &request.scope,
            cutoff,
            request.budget,
            access.allowed_tags.clone(),
        );
        let cached = {
            let mut cache = self.context_cache.safe_lock();
            cache.get(&cache_key).cloned()
        };
        if let Some(cached) = cached {
            // Cache hit is useful to see at Info level for observability
            self.logger.log(
                log_event(
                    "assemble_context.cache_hit",
                    json!({"scope": request.scope, "query": request.query}),
                    json!({"count": cached.len()}),
                    Some(&access),
                ),
                LogLevel::Info,
            );
            return Ok(cached.clone());
        }

        let namespace = self.namespace_for_scope(&request.scope);
        let cutoff_iso = normalize_iso(cutoff);
        let cleaned_query = preprocess_search_query(&request.query);
        let query_opt = if cleaned_query.is_empty() {
            None
        } else {
            Some(cleaned_query.as_str())
        };

        // Use DB-side filtering for scope, cutoff, query, and limit (query pushdown)
        let fact_records = self
            .db_client
            .select_facts_filtered(
                &namespace,
                &request.scope,
                &cutoff_iso,
                query_opt,
                request.budget,
            )
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

        // Only filter by policy_tags in memory (complex set intersection)
        let mut active = Vec::new();
        for record in fact_records {
            if let Some(fact) = fact_from_record(&record) {
                // Tag filtering only - scope/cutoff/query done in DB
                if !fact.policy_tags.is_empty()
                    && let Some(allowed_tags) = &access.allowed_tags
                {
                    let allowed: std::collections::HashSet<_> = allowed_tags.iter().collect();
                    if !fact.policy_tags.iter().any(|tag| allowed.contains(tag)) {
                        continue;
                    }
                }
                active.push(fact);
            }
        }
        active.sort_by(|a, b| {
            b.t_valid
                .cmp(&a.t_valid)
                .then_with(|| b.fact_id.cmp(&a.fact_id))
        });

        let mut results = Vec::new();
        for fact in active.into_iter().take(request.budget.max(1) as usize) {
            results.push(json!({
                "fact_id": fact.fact_id,
                "content": fact.content,
                "quote": fact.quote,
                "source_episode": fact.source_episode,
                "confidence": decayed_confidence(&fact, cutoff),
                "provenance": fact.provenance,
                "rationale": format!("matched scope={} and active at {}", request.scope, cutoff.date_naive()),
            }));
        }

        self.context_cache
            .safe_lock()
            .put(cache_key, results.clone());

        // Log cache set and returned results count
        self.logger.log(
            log_event(
                "assemble_context.cache_set",
                json!({"scope": request.scope}),
                json!({"count": results.len()}),
                Some(&access),
            ),
            LogLevel::Debug,
        );

        Ok(results)
    }

    pub async fn extract_from_episode(&self, episode_id: &str) -> Result<Value, MemoryError> {
        self.logger.log(
            log_event(
                "extract_from_episode.start",
                json!({"episode_id": episode_id}),
                json!({}),
                None,
            ),
            LogLevel::Info,
        );

        let (record, namespace) = self.find_episode_record(episode_id).await?;
        let namespace =
            namespace.ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;
        let record = record.ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;
        let episode = episode_from_record(&record)
            .ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;

        let entities = self.extract_entities(&episode.content).await?;
        let facts = self.extract_facts(&episode).await?;
        let mut links = Vec::new();

        self.logger.log(
            log_event(
                "extract_from_episode.done",
                json!({"episode_id": episode_id}),
                json!({"entities": entities.len(), "facts": facts.len()}),
                None,
            ),
            LogLevel::Info,
        );

        let edge_ingested = now();

        for entity in &entities {
            links.push(json!({
                "entity_id": entity["entity_id"].clone(),
                "episode_id": episode_id,
            }));
            let edge = Edge {
                from_id: entity["entity_id"].as_str().unwrap_or("").to_string(),
                relation: "mentioned_in".to_string(),
                to_id: episode_id.to_string(),
                strength: 1.0,
                confidence: 0.9,
                provenance: json!({"source_episode": episode_id}),
                t_valid: episode.t_ref,
                t_ingested: edge_ingested,
                t_invalid: None,
                t_invalid_ingested: None,
            };
            self.store_edge(&edge, &namespace).await?;
        }

        for fact in &facts {
            for entity in &entities {
                let edge = Edge {
                    from_id: entity["entity_id"].as_str().unwrap_or("").to_string(),
                    relation: "involved_in".to_string(),
                    to_id: fact["fact_id"].as_str().unwrap_or("").to_string(),
                    strength: 0.8,
                    confidence: 0.8,
                    provenance: json!({"source_episode": episode_id}),
                    t_valid: episode.t_ref,
                    t_ingested: edge_ingested,
                    t_invalid: None,
                    t_invalid_ingested: None,
                };
                self.store_edge(&edge, &namespace).await?;
            }
        }

        let entity_ids = entities
            .iter()
            .filter_map(|entity| entity.get("entity_id").and_then(Value::as_str))
            .map(String::from)
            .collect::<Vec<_>>();
        self.update_communities(&entity_ids, &episode.scope).await?;

        Ok(json!({
            "episode_id": episode_id,
            "entities": entities,
            "facts": facts,
            "links": links,
        }))
    }

    pub async fn resolve_person(&self, name: &str) -> Result<String, MemoryError> {
        self.resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: name.to_string(),
                aliases: Vec::new(),
            },
            None,
        )
        .await
    }

    pub async fn resolve_company(&self, name: &str) -> Result<String, MemoryError> {
        self.resolve(
            EntityCandidate {
                entity_type: "company".to_string(),
                canonical_name: name.to_string(),
                aliases: Vec::new(),
            },
            None,
        )
        .await
    }

    pub async fn relate(
        &self,
        from_id: &str,
        relation: &str,
        to_id: &str,
    ) -> Result<(), MemoryError> {
        let edge = Edge {
            from_id: from_id.to_string(),
            relation: relation.to_string(),
            to_id: to_id.to_string(),
            strength: 1.0,
            confidence: 0.8,
            provenance: json!({"source": "manual"}),
            t_valid: now(),
            t_ingested: now(),
            t_invalid: None,
            t_invalid_ingested: None,
        };
        self.store_edge(&edge, &self.default_namespace).await?;
        Ok(())
    }

    pub fn register_analyzer(&self, name: &str, config: Value) {
        self.analyzers.safe_lock().insert(name.to_string(), config);
    }

    pub fn register_index(&self, name: &str, config: Value) {
        self.indexes.safe_lock().insert(name.to_string(), config);
    }

    pub async fn create_task(
        &self,
        title: &str,
        due_date: Option<DateTime<Utc>>,
    ) -> Result<Value, MemoryError> {
        self.logger.log(
            log_event(
                "create_task.start",
                json!({"title": title}),
                json!({}),
                None,
            ),
            LogLevel::Info,
        );

        let record = self
            .db_client
            .create(
                "task",
                json!({
                    "status": "pending_confirmation",
                    "title": title,
                    "due_date": due_date.map(normalize_dt),
                }),
                &self.default_namespace,
            )
            .await?;
        let id = record
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("task:unknown")
            .to_string();

        self.logger.log(
            log_event(
                "create_task.done",
                json!({"title": title}),
                json!({"id": id}),
                None,
            ),
            LogLevel::Info,
        );

        Ok(json!({
            "id": id,
            "status": "pending_confirmation",
            "title": title,
            "due_date": due_date.map(normalize_iso),
        }))
    }

    pub fn send_message_draft(&self, to: &str, subject: &str, body: &str) -> Value {
        json!({
            "status": "pending_confirmation",
            "to": to,
            "subject": subject,
            "body": body,
        })
    }

    pub fn schedule_meeting(&self, title: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Value {
        json!({
            "status": "pending_confirmation",
            "title": title,
            "start": normalize_iso(start),
            "end": normalize_iso(end),
        })
    }

    pub fn update_metric(&self, name: &str, value: f64) -> Value {
        json!({
            "status": "ok",
            "metric": name,
            "value": value,
        })
    }

    pub async fn ui_promises(&self) -> Result<Vec<Value>, MemoryError> {
        self.logger.log(
            log_event("ui_promises.start", json!({}), json!({}), None),
            LogLevel::Debug,
        );
        let mut records = Vec::new();
        for namespace in &self.namespaces {
            records.extend(self.db_client.select_table("fact", namespace).await?);
        }
        let filtered: Vec<Value> = records
            .into_iter()
            .filter(|record| record.get("fact_type").and_then(Value::as_str) == Some("promise"))
            .map(|record| {
                json!({
                    "content": record.get("content"),
                    "quote": record.get("quote"),
                    "source_episode": record.get("source_episode"),
                })
            })
            .collect();
        self.logger.log(
            log_event(
                "ui_promises.done",
                json!({}),
                json!({"count": filtered.len()}),
                None,
            ),
            LogLevel::Info,
        );
        Ok(filtered)
    }

    pub async fn ui_metrics(&self) -> Result<Vec<Value>, MemoryError> {
        self.logger.log(
            log_event("ui_metrics.start", json!({}), json!({}), None),
            LogLevel::Debug,
        );
        let mut records = Vec::new();
        for namespace in &self.namespaces {
            records.extend(self.db_client.select_table("fact", namespace).await?);
        }
        let filtered: Vec<Value> = records
            .into_iter()
            .filter(|record| record.get("fact_type").and_then(Value::as_str) == Some("metric"))
            .map(|record| {
                json!({
                    "content": record.get("content"),
                    "quote": record.get("quote"),
                    "source_episode": record.get("source_episode"),
                })
            })
            .collect();
        self.logger.log(
            log_event(
                "ui_metrics.done",
                json!({}),
                json!({"count": filtered.len()}),
                None,
            ),
            LogLevel::Info,
        );
        Ok(filtered)
    }

    pub async fn ui_tasks(&self) -> Result<Vec<Value>, MemoryError> {
        self.logger.log(
            log_event("ui_tasks.start", json!({}), json!({}), None),
            LogLevel::Debug,
        );
        let records = self
            .db_client
            .select_table("task", &self.default_namespace)
            .await?;
        let out: Vec<Value> = records
            .into_iter()
            .map(|mut record| {
                if let Some(id) = record.get("id").and_then(Value::as_str) {
                    record["id"] = Value::String(id.to_string());
                }
                record
            })
            .collect();
        self.logger.log(
            log_event(
                "ui_tasks.done",
                json!({}),
                json!({"count": out.len()}),
                None,
            ),
            LogLevel::Info,
        );
        Ok(out)
    }

    pub async fn get_surrealdb_config(&self) -> Result<Value, MemoryError> {
        Ok(json!({
            "namespaces": self.namespaces,
        }))
    }

    async fn check_surrealdb_connection(&self) -> Result<(), MemoryError> {
        let _ = self
            .db_client
            .select_table("event_log", &self.default_namespace)
            .await?;
        Ok(())
    }

    #[allow(dead_code)]
    async fn apply_migrations(&self) -> Result<(), MemoryError> {
        let migrations_dir = migrations_dir()?;
        if !migrations_dir.exists() {
            return Ok(());
        }
        let mut entries = std::fs::read_dir(migrations_dir)
            .map_err(|err| MemoryError::Storage(format!("read migrations failed: {err}")))?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("surql") {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .map_err(|err| MemoryError::Storage(format!("read migration failed: {err}")))?;
            for statement in content.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                let result = self
                    .db_client
                    .query(statement, None, &self.default_namespace)
                    .await;
                if let Err(err) = result {
                    if is_ignorable_migration_error(statement, &err) {
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    fn namespace_for_scope(&self, scope: &str) -> String {
        if self.namespaces.contains(&scope.to_string()) {
            return scope.to_string();
        }
        if scope.starts_with("personal") && self.namespaces.contains(&"personal".to_string()) {
            return "personal".to_string();
        }
        if scope.starts_with("private") && self.namespaces.contains(&"private".to_string()) {
            return "private".to_string();
        }
        if scope.starts_with("org") && self.namespaces.contains(&"org".to_string()) {
            return "org".to_string();
        }
        self.default_namespace.clone()
    }

    async fn find_episode_record(
        &self,
        episode_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        // Debug: searching for episode across namespaces
        self.logger.log(
            log_event(
                "find_episode_record.start",
                json!({"episode_id": episode_id}),
                json!({}),
                None,
            ),
            LogLevel::Debug,
        );

        for namespace in &self.namespaces {
            self.logger.log(
                log_event(
                    "find_episode_record.scan",
                    json!({"episode_id": episode_id, "namespace": namespace}),
                    json!({}),
                    None,
                ),
                LogLevel::Trace,
            );
            let record = self.db_client.select_one(episode_id, namespace).await?;
            if let Some(Value::Object(map)) = record {
                self.logger.log(
                    log_event(
                        "find_episode_record.found",
                        json!({"episode_id": episode_id}),
                        json!({"namespace": namespace, "fields": map.len()}),
                        None,
                    ),
                    LogLevel::Info,
                );
                return Ok((Some(map), Some(namespace.clone())));
            }
        }
        self.logger.log(
            log_event(
                "find_episode_record.missing",
                json!({"episode_id": episode_id}),
                json!({}),
                None,
            ),
            LogLevel::Debug,
        );
        Ok((None, None))
    }

    async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        self.logger.log(
            log_event(
                "find_fact_record.start",
                json!({"fact_id": fact_id}),
                json!({}),
                None,
            ),
            LogLevel::Debug,
        );
        for namespace in &self.namespaces {
            self.logger.log(
                log_event(
                    "find_fact_record.scan",
                    json!({"fact_id": fact_id, "namespace": namespace}),
                    json!({}),
                    None,
                ),
                LogLevel::Trace,
            );
            let record = self.db_client.select_one(fact_id, namespace).await?;
            if let Some(Value::Object(map)) = record {
                self.logger.log(
                    log_event(
                        "find_fact_record.found",
                        json!({"fact_id": fact_id}),
                        json!({"namespace": namespace, "fields": map.len()}),
                        None,
                    ),
                    LogLevel::Info,
                );
                return Ok((Some(map), Some(namespace.clone())));
            }
        }
        Ok((None, None))
    }

    async fn find_entity_record(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<Option<serde_json::Map<String, Value>>, MemoryError> {
        let normalized = normalize_text(name);
        self.logger.log(
            log_event(
                "find_entity_record.start",
                json!({"name": name, "normalized": normalized}),
                json!({"namespace": namespace}),
                None,
            ),
            LogLevel::Debug,
        );
        let records = self.db_client.select_table("entity", namespace).await?;
        for record in records {
            if let Value::Object(map) = record {
                let canonical = map
                    .get("canonical_name")
                    .and_then(Value::as_str)
                    .map(normalize_text)
                    .unwrap_or_default();
                let aliases = map
                    .get("aliases")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(string_from_value)
                            .map(|value| normalize_text(&value))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if normalized == canonical || aliases.contains(&normalized) {
                    self.logger.log(
                        log_event(
                            "find_entity_record.found",
                            json!({"name": name}),
                            json!({"namespace": namespace, "canonical": canonical}),
                            None,
                        ),
                        LogLevel::Info,
                    );
                    return Ok(Some(map));
                }
            }
        }
        Ok(None)
    }

    fn is_scope_allowed(&self, scope: &str, access: &AccessContext) -> bool {
        if let Some(scopes) = &access.allowed_scopes
            && !scopes.contains(&scope.to_string())
        {
            if let Some(cross) = &access.cross_scope_allow {
                let allowed = cross
                    .iter()
                    .any(|pair| pair.from == "*" && pair.to == scope);
                if !allowed {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }

    async fn store_edge(&self, edge: &Edge, namespace: &str) -> Result<(), MemoryError> {
        let edge_id =
            deterministic_edge_id(&edge.from_id, &edge.relation, &edge.to_id, edge.t_valid);
        self.logger.log(
            log_event(
                "store_edge.start",
                json!({"edge_id": edge_id, "from": edge.from_id, "to": edge.to_id, "relation": edge.relation}),
                json!({}),
                None,
            ),
            LogLevel::Debug,
        );
        let existing = self.db_client.select_one(&edge_id, namespace).await?;
        if existing.is_some() {
            self.logger.log(
                log_event(
                    "store_edge.skip",
                    json!({"edge_id": edge_id}),
                    json!({"reason": "duplicate"}),
                    None,
                ),
                LogLevel::Debug,
            );
            return Ok(());
        }
        let payload = json!({
            "edge_id": edge_id,
            "from_id": edge.from_id,
            "relation": edge.relation,
            "to_id": edge.to_id,
            "strength": edge.strength,
            "confidence": edge.confidence,
            "provenance": edge.provenance,
            "t_valid": normalize_dt(edge.t_valid),
            "t_ingested": normalize_dt(edge.t_ingested),
            "t_invalid": edge.t_invalid.map(normalize_dt),
            "t_invalid_ingested": edge.t_invalid_ingested.map(normalize_dt),
        });
        self.db_client.create(&edge_id, payload, namespace).await?;
        self.logger.log(
            log_event(
                "store_edge.created",
                json!({"edge_id": edge_id}),
                json!({}),
                None,
            ),
            LogLevel::Info,
        );
        Ok(())
    }

    async fn extract_entities(&self, content: &str) -> Result<Vec<Value>, MemoryError> {
        let mut entities = Vec::new();
        let candidates: std::collections::HashSet<_> = self
            .name_regex
            .find_iter(content)
            .map(|mat| mat.as_str().to_string())
            .collect();

        self.logger.log(
            log_event(
                "extract_entities.start",
                json!({"candidates": candidates.len()}),
                json!({}),
                None,
            ),
            LogLevel::Debug,
        );

        for candidate in candidates {
            let entity_type = if candidate.contains("Corp") || candidate.contains("Inc") {
                "company"
            } else {
                "person"
            };
            let entity_id = self
                .resolve(
                    EntityCandidate {
                        entity_type: entity_type.to_string(),
                        canonical_name: candidate.clone(),
                        aliases: Vec::new(),
                    },
                    None,
                )
                .await?;
            entities.push(json!({
                "entity_id": entity_id,
                "type": entity_type,
                "canonical_name": candidate,
            }));
        }

        self.logger.log(
            log_event(
                "extract_entities.done",
                json!({"count": entities.len()}),
                json!({}),
                None,
            ),
            LogLevel::Debug,
        );

        Ok(entities)
    }

    async fn extract_facts(&self, episode: &Episode) -> Result<Vec<Value>, MemoryError> {
        let mut facts = Vec::new();
        let normalized = episode.content.to_lowercase();

        self.logger.log(
            log_event(
                "extract_facts.start",
                json!({"episode_id": episode.episode_id}),
                json!({}),
                None,
            ),
            LogLevel::Debug,
        );

        if normalized.contains("arr") || episode.content.contains('$') {
            let fact_id = self
                .add_fact(
                    "metric",
                    &episode.content,
                    &episode.content,
                    &episode.episode_id,
                    episode.t_ref,
                    &episode.scope,
                    0.7,
                    Vec::new(),
                    Vec::new(),
                    json!({"source_episode": episode.episode_id}),
                )
                .await?;
            facts.push(json!({"fact_id": fact_id, "type": "metric"}));
        }
        // Enhanced promise detection: support Russian and various English patterns ("will do", "I will", "I'll", "going to ...")
        // Narrow English matches to promise-like verbs to avoid false positives (e.g., "will happen")
        static PROMISE_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let promise_re = PROMISE_RE.get_or_init(|| {
            Regex::new(r"\b(i will|i'll|will\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule)|going to\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule))\b")
                .expect("promise regex is valid")
        });
        if normalized.contains("сделаю") || promise_re.is_match(&normalized) {
            let fact_id = self
                .add_fact(
                    "promise",
                    &episode.content,
                    &episode.content,
                    &episode.episode_id,
                    episode.t_ref,
                    &episode.scope,
                    0.7,
                    Vec::new(),
                    Vec::new(),
                    json!({"source_episode": episode.episode_id}),
                )
                .await?;
            facts.push(json!({"fact_id": fact_id, "type": "promise"}));
        }

        self.logger.log(
            log_event(
                "extract_facts.done",
                json!({"episode_id": episode.episode_id}),
                json!({"count": facts.len()}),
                None,
            ),
            LogLevel::Debug,
        );

        Ok(facts)
    }

    async fn update_communities(
        &self,
        entity_ids: &[String],
        scope: &str,
    ) -> Result<(), MemoryError> {
        if entity_ids.len() < 2 {
            return Ok(());
        }
        let community_id = deterministic_community_id(entity_ids);
        let mut names = Vec::new();
        let records = self
            .db_client
            .select_table("entity", &self.default_namespace)
            .await?;
        for record in records {
            if let Value::Object(map) = record {
                let entity_id = map
                    .get("entity_id")
                    .and_then(Value::as_str)
                    .or_else(|| map.get("id").and_then(Value::as_str))
                    .unwrap_or("");
                if entity_ids.contains(&entity_id.to_string()) {
                    names.push(
                        map.get("canonical_name")
                            .and_then(Value::as_str)
                            .unwrap_or(entity_id)
                            .to_string(),
                    );
                }
            }
        }
        let summary = if !names.is_empty() {
            names.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        } else {
            entity_ids
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        let payload = json!({
            "community_id": community_id,
            "member_entities": entity_ids,
            "summary": summary,
            "updated_at": normalize_dt(now()),
        });
        let namespace = self.namespace_for_scope(scope);
        let existing = self.db_client.select_one(&community_id, &namespace).await?;
        if existing.is_some() {
            self.db_client
                .update(&community_id, payload, &namespace)
                .await?;
        } else {
            self.db_client
                .create(&community_id, payload, &namespace)
                .await?;
        }
        Ok(())
    }

    pub async fn find_intro_chain(
        &self,
        target_name: &str,
        max_hops: i32,
        as_of: Option<DateTime<Utc>>,
    ) -> Result<Vec<String>, MemoryError> {
        let target_id = self.find_entity_by_name(target_name).await?;
        let Some(target_id) = target_id else {
            return Ok(vec![]);
        };

        let cutoff = as_of.unwrap_or_else(now);
        let cutoff_iso = normalize_iso(cutoff);

        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for namespace in &self.namespaces {
            for record in self
                .db_client
                .select_edges_filtered(namespace, &cutoff_iso)
                .await?
            {
                if let Value::Object(map) = record
                    && let (Some(from_id), Some(to_id)) = (
                        map.get("from_id").and_then(Value::as_str),
                        map.get("to_id").and_then(Value::as_str),
                    )
                {
                    graph
                        .entry(from_id.to_string())
                        .or_default()
                        .push(to_id.to_string());
                }
            }
        }

        for neighbors in graph.values_mut() {
            neighbors.sort();
        }

        let mut start_ids = graph.keys().cloned().collect::<Vec<_>>();
        start_ids.sort();
        for start_id in start_ids {
            if start_id == target_id {
                return Ok(vec![start_id]);
            }
            if let Some(path) = bfs_path(&graph, &start_id, &target_id, max_hops) {
                return Ok(path);
            }
        }
        Ok(vec![])
    }

    pub async fn invalidate_metric_if_superseded(
        &self,
        new_value: f64,
        old_fact_id: &str,
        t_invalid: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let (record, _) = self.find_fact_record(old_fact_id).await?;
        if record.is_none() {
            return Ok(());
        }
        self.invalidate(
            InvalidateRequest {
                fact_id: old_fact_id.to_string(),
                reason: format!("Superseded by {new_value}"),
                t_invalid,
            },
            None,
        )
        .await?;
        Ok(())
    }

    pub fn cbor_round_trip(&self, payload: &Value) -> Result<Value, MemoryError> {
        let encoded = serde_cbor::to_vec(payload)
            .map_err(|err| MemoryError::Storage(format!("cbor encode error: {err}")))?;
        let decoded: Value = serde_cbor::from_slice(&encoded)
            .map_err(|err| MemoryError::Storage(format!("cbor decode error: {err}")))?;
        Ok(decoded)
    }

    async fn find_entity_by_name(&self, name: &str) -> Result<Option<String>, MemoryError> {
        let record = self
            .find_entity_record(name, &self.default_namespace)
            .await?;
        Ok(record.and_then(|map| {
            map.get("entity_id")
                .and_then(Value::as_str)
                .or_else(|| map.get("id").and_then(Value::as_str))
                .map(str::to_string)
        }))
    }

    fn enforce_rate_limit(&self, access: Option<&AccessContext>) -> Result<(), MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    query: String,
    scope: String,
    cutoff: String,
    budget: i32,
    tags: Option<Vec<String>>,
}

impl CacheKey {
    fn new(
        query: &str,
        scope: &str,
        cutoff: DateTime<Utc>,
        budget: i32,
        tags: Option<Vec<String>>,
    ) -> Self {
        let mut tags = tags;
        if let Some(ref mut tag_list) = tags {
            tag_list.sort();
        }
        Self {
            query: normalize_text(query),
            scope: scope.to_string(),
            cutoff: bucket_to_hour(cutoff), // Bucket by hour for better cache hit rate
            budget,
            tags,
        }
    }
}

#[derive(Debug)]
struct RateLimiter {
    rps: i32,
    burst: i32,
    tokens: Mutex<HashMap<String, f64>>,
    last: Mutex<HashMap<String, Instant>>,
}

impl RateLimiter {
    fn new(rps: i32, burst: i32) -> Self {
        Self {
            rps: rps.max(1),
            burst: burst.max(1),
            tokens: Mutex::new(HashMap::new()),
            last: Mutex::new(HashMap::new()),
        }
    }

    fn allow(&self, key: &str) -> bool {
        let mut tokens = self.tokens.safe_lock();
        let mut last = self.last.safe_lock();
        let now = Instant::now();
        let last_time = last.entry(key.to_string()).or_insert(now);
        let elapsed = now.duration_since(*last_time).as_secs_f64();
        *last_time = now;
        let entry = tokens.entry(key.to_string()).or_insert(self.burst as f64);
        let refill = elapsed * self.rps as f64;
        *entry = (*entry + refill).min(self.burst as f64);
        if *entry < 1.0 {
            return false;
        }
        *entry -= 1.0;
        true
    }
}

fn log_event(
    op: &str,
    args: Value,
    result: Value,
    access: Option<&AccessContext>,
) -> HashMap<String, Value> {
    let mut event = HashMap::new();
    event.insert("op".to_string(), Value::String(op.to_string()));
    event.insert("args".to_string(), args);
    event.insert("result".to_string(), result);
    if let Some(access) = access {
        event.insert("access".to_string(), serialize_access(access));
    }
    event
}

fn serialize_access(access: &AccessContext) -> Value {
    json!({
        "caller_id": access.caller_id,
        "allowed_scopes": access.allowed_scopes,
        "allowed_tags": access.allowed_tags,
        "session_vars": access.session_vars,
        "transport": access.transport,
        "content_type": access.content_type,
        "cross_scope_allow": access.cross_scope_allow,
    })
}

fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn string_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get("String") {
                return Some(s.clone());
            }
            if let Some(Value::String(s)) = map.get("Strand") {
                return Some(s.clone());
            }
            if let Some(Value::Object(inner)) = map.get("Strand")
                && let Some(Value::String(s)) = inner.get("String")
            {
                return Some(s.clone());
            }
            None
        }
        _ => None,
    }
}

fn normalize_dt(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn normalize_iso(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

/// Preprocess a search query: strip episode references (episode:xxx),
/// boolean operators (OR/AND/NOT), quoted phrases, and collapse whitespace.
/// Returns cleaned words joined by spaces, suitable for full-text search.
pub fn preprocess_search_query(raw: &str) -> String {
    static EPISODE_REF: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static QUOTED: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let episode_re = EPISODE_REF.get_or_init(|| {
        Regex::new(r"(?i)episode:[a-z0-9_-]+").expect("episode_ref regex is valid")
    });
    let quoted_re =
        QUOTED.get_or_init(|| Regex::new(r#""([^"]*)""#).expect("quoted regex is valid"));

    let s = episode_re.replace_all(raw, " ");
    let s = quoted_re.replace_all(&s, " $1 ");

    s.split_whitespace()
        .filter(|w| {
            let upper = w.to_uppercase();
            // Drop boolean operators and very short noise tokens
            upper != "OR" && upper != "AND" && upper != "NOT" && w.len() >= 2
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bucket cutoff to the start of the hour for better cache hit rate
fn bucket_to_hour(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:00:00Z").to_string()
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

fn parse_iso(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

fn deterministic_episode_id(
    source_type: &str,
    source_id: &str,
    t_ref: DateTime<Utc>,
    scope: &str,
) -> String {
    let payload = format!(
        "{}|{}|{}|{}",
        normalize_text(source_type),
        normalize_text(source_id),
        normalize_iso(t_ref),
        normalize_text(scope),
    );
    format!("episode:{}", hash_prefix(&payload))
}

fn deterministic_entity_id(entity_type: &str, canonical_name: &str) -> String {
    let payload = format!(
        "{}|{}",
        normalize_text(entity_type),
        normalize_text(canonical_name)
    );
    format!("entity:{}", hash_prefix(&payload))
}

fn deterministic_fact_id(
    fact_type: &str,
    content: &str,
    source_episode: &str,
    t_valid: DateTime<Utc>,
) -> String {
    let payload = format!(
        "{}|{}|{}|{}",
        normalize_text(fact_type),
        normalize_text(content),
        normalize_text(source_episode),
        normalize_iso(t_valid),
    );
    format!("fact:{}", hash_prefix(&payload))
}

fn deterministic_community_id(member_entities: &[String]) -> String {
    let mut members = member_entities.to_vec();
    members.sort();
    format!("community:{}", hash_prefix(&members.join("|")))
}

fn deterministic_edge_id(
    from_id: &str,
    relation: &str,
    to_id: &str,
    t_valid: DateTime<Utc>,
) -> String {
    let payload = format!(
        "{}|{}|{}|{}",
        normalize_text(from_id),
        normalize_text(relation),
        normalize_text(to_id),
        normalize_iso(t_valid),
    );
    format!("edge:{}", hash_prefix(&payload))
}

fn hash_prefix(payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)[..24].to_string()
}

#[allow(dead_code)]
fn is_ignorable_migration_error(statement: &str, err: &MemoryError) -> bool {
    let message = format!("{err}").to_lowercase();
    if message.contains("already exists") || message.contains("already defined") {
        return true;
    }
    let trimmed = statement.trim().to_lowercase();
    if trimmed.starts_with("define table") && message.contains("defined") {
        return true;
    }
    if trimmed.starts_with("define field") && message.contains("defined") {
        return true;
    }
    // Also ignore some benign errors returned by remote engines about indexes/fields
    if message.contains("index already exists") || message.contains("index exists") {
        return true;
    }
    false
}

pub(crate) fn migrations_dir() -> Result<PathBuf, MemoryError> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // First try: src/migrations in the same directory as Cargo.toml
    let src_migrations = manifest_dir.join("src/migrations");
    if src_migrations.exists() {
        return Ok(src_migrations);
    }

    // Second try: migrations in the same directory as Cargo.toml
    let root_migrations = manifest_dir.join("migrations");
    if root_migrations.exists() {
        return Ok(root_migrations);
    }

    // Fallback to parent directories
    let root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| MemoryError::Storage("cannot locate repo root".into()))?;

    // Expect a single canonical relative path for migrations: repo_root/migrations
    // This must be a relative directory inside the repository root (no absolute paths).
    Ok(root.join("migrations"))
}

fn episode_from_record(record: &serde_json::Map<String, Value>) -> Option<Episode> {
    Some(Episode {
        episode_id: record.get("episode_id")?.as_str()?.to_string(),
        source_type: record.get("source_type")?.as_str()?.to_string(),
        source_id: record.get("source_id")?.as_str()?.to_string(),
        content: record.get("content")?.as_str()?.to_string(),
        t_ref: parse_iso(record.get("t_ref")?.as_str()?)?,
        t_ingested: parse_iso(record.get("t_ingested")?.as_str()?)?,
        scope: record.get("scope")?.as_str()?.to_string(),
        visibility_scope: record
            .get("visibility_scope")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        policy_tags: record
            .get("policy_tags")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn fact_from_record(record: &Value) -> Option<Fact> {
    let map = record.as_object()?;
    let t_valid_str = map.get("t_valid")?.as_str()?;
    let t_valid = parse_iso(t_valid_str)?;
    let t_ingested = map
        .get("t_ingested")
        .and_then(Value::as_str)
        .and_then(parse_iso)
        .unwrap_or(t_valid);
    Some(Fact {
        fact_id: map.get("fact_id")?.as_str()?.to_string(),
        fact_type: map.get("fact_type")?.as_str()?.to_string(),
        content: map.get("content")?.as_str()?.to_string(),
        quote: map.get("quote")?.as_str()?.to_string(),
        source_episode: map.get("source_episode")?.as_str()?.to_string(),
        t_valid,
        t_ingested,
        t_invalid: map
            .get("t_invalid")
            .and_then(Value::as_str)
            .and_then(parse_iso),
        t_invalid_ingested: map
            .get("t_invalid_ingested")
            .and_then(Value::as_str)
            .and_then(parse_iso),
        confidence: map.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
        entity_links: map
            .get("entity_links")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        scope: map
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        policy_tags: map
            .get("policy_tags")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        provenance: map.get("provenance").cloned().unwrap_or(Value::Null),
    })
}

fn decayed_confidence(fact: &Fact, now: DateTime<Utc>) -> f64 {
    let half_life_days = if fact.fact_type == "metric" || fact.fact_type == "promise" {
        365.0
    } else {
        180.0
    };
    let delta_days = (now - fact.t_valid).num_days().max(0) as f64;
    let decay = 0.5_f64.powf(delta_days / half_life_days);
    (fact.confidence * decay * 10000.0).round() / 10000.0
}

fn bfs_path(
    graph: &HashMap<String, Vec<String>>,
    start: &str,
    target: &str,
    max_hops: i32,
) -> Option<Vec<String>> {
    let mut queue: VecDeque<(String, Vec<String>)> = VecDeque::new();
    let mut visited = HashMap::new();
    queue.push_back((start.to_string(), vec![start.to_string()]));
    visited.insert(start.to_string(), true);

    while let Some((node, path)) = queue.pop_front() {
        if (path.len() as i32) - 1 > max_hops {
            continue;
        }
        if let Some(neighbors) = graph.get(&node) {
            for neighbor in neighbors {
                if visited.contains_key(neighbor) {
                    continue;
                }
                if neighbor == target {
                    let mut new_path = path.clone();
                    new_path.push(neighbor.clone());
                    return Some(new_path);
                }
                visited.insert(neighbor.clone(), true);
                let mut new_path = path.clone();
                new_path.push(neighbor.clone());
                queue.push_back((neighbor.clone(), new_path));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AccessPayload;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};

    #[derive(Default)]
    struct FakeDbClient {
        namespaces: Mutex<HashMap<String, FakeNamespaceStore>>,
    }

    #[derive(Default, Clone)]
    struct FakeNamespaceStore {
        tables: HashMap<String, HashMap<String, Value>>,
        counters: HashMap<String, i32>,
    }

    impl FakeDbClient {
        fn store(&self, namespace: &str) -> FakeNamespaceStore {
            self.namespaces
                .lock()
                .unwrap()
                .get(namespace)
                .cloned()
                .unwrap_or_default()
        }

        fn set_store(&self, namespace: &str, store: FakeNamespaceStore) {
            self.namespaces
                .lock()
                .unwrap()
                .insert(namespace.to_string(), store);
        }

        fn table_from_id(record_id: &str) -> String {
            record_id.split(':').next().unwrap_or(record_id).to_string()
        }
    }

    #[async_trait]
    impl crate::storage::DbClient for FakeDbClient {
        async fn select_one(
            &self,
            record_id: &str,
            namespace: &str,
        ) -> Result<Option<Value>, MemoryError> {
            let store = self.store(namespace);
            let table = Self::table_from_id(record_id);
            Ok(store
                .tables
                .get(&table)
                .and_then(|table| table.get(record_id).cloned()))
        }

        async fn select_table(
            &self,
            table: &str,
            namespace: &str,
        ) -> Result<Vec<Value>, MemoryError> {
            let store = self.store(namespace);
            Ok(store
                .tables
                .get(table)
                .map(|table| table.values().cloned().collect())
                .unwrap_or_default())
        }

        async fn create(
            &self,
            record_id: &str,
            mut content: Value,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            let mut store = self.store(namespace);
            let mut record_id = record_id.to_string();
            let table = if record_id.contains(':') {
                Self::table_from_id(&record_id)
            } else {
                let counter = store.counters.entry(record_id.clone()).or_insert(0);
                *counter += 1;
                let table = record_id.clone();
                record_id = format!("{table}:{}", counter);
                table
            };
            if let Value::Object(ref mut map) = content {
                map.insert("id".to_string(), Value::String(record_id.clone()));
            }
            store
                .tables
                .entry(table)
                .or_default()
                .insert(record_id.clone(), content.clone());
            self.set_store(namespace, store);
            Ok(content)
        }

        async fn update(
            &self,
            record_id: &str,
            content: Value,
            namespace: &str,
        ) -> Result<Value, MemoryError> {
            let mut store = self.store(namespace);
            let table = Self::table_from_id(record_id);
            let entry = store
                .tables
                .entry(table)
                .or_default()
                .entry(record_id.to_string())
                .or_insert(Value::Object(Default::default()));
            if let (Value::Object(map), Value::Object(update)) = (entry, content.clone()) {
                for (key, value) in update {
                    map.insert(key, value);
                }
                map.insert("id".to_string(), Value::String(record_id.to_string()));
            }
            self.set_store(namespace, store);
            Ok(content)
        }

        async fn query(
            &self,
            _sql: &str,
            _vars: Option<Value>,
            _namespace: &str,
        ) -> Result<Value, MemoryError> {
            Ok(Value::Null)
        }

        async fn select_facts_filtered(
            &self,
            namespace: &str,
            scope: &str,
            cutoff: &str,
            query_contains: Option<&str>,
            limit: i32,
        ) -> Result<Vec<Value>, MemoryError> {
            let store = self.store(namespace);
            let facts = store
                .tables
                .get("fact")
                .map(|t| t.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();

            let query_lower = query_contains.map(|q| q.to_lowercase());

            let mut filtered: Vec<Value> = facts
                .into_iter()
                .filter(|f| {
                    let f_scope = f.get("scope").and_then(Value::as_str).unwrap_or_default();
                    let t_valid = f.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                    let t_ingested = f
                        .get("t_ingested")
                        .and_then(Value::as_str)
                        .unwrap_or(t_valid);
                    let t_invalid = f.get("t_invalid").and_then(Value::as_str);
                    let t_invalid_ingested = f.get("t_invalid_ingested").and_then(Value::as_str);

                    if f_scope != scope {
                        return false;
                    }
                    if t_valid > cutoff {
                        return false;
                    }
                    if t_ingested > cutoff {
                        return false;
                    }
                    if let Some(inv) = t_invalid
                        && !inv.is_empty()
                        && inv <= cutoff
                    {
                        let invalid_known = t_invalid_ingested
                            .map(|ingested| !ingested.is_empty() && ingested <= cutoff)
                            .unwrap_or(true);
                        if invalid_known {
                            return false;
                        }
                    }
                    // Per-word OR matching (mirrors SurrealDB full-text search semantics)
                    if let Some(ref q) = query_lower {
                        let content = f
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_lowercase();
                        let words: Vec<&str> =
                            q.split_whitespace().filter(|w| w.len() >= 2).collect();
                        if words.is_empty() {
                            if !content.contains(q) {
                                return false;
                            }
                        } else {
                            let any_match = words.iter().any(|w| content.contains(w));
                            if !any_match {
                                return false;
                            }
                        }
                    }
                    true
                })
                .collect();

            filtered.sort_by(|a, b| {
                let ta = a.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                let tb = b.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                tb.cmp(ta)
            });

            filtered.truncate(limit.max(1) as usize);
            Ok(filtered)
        }

        async fn select_edges_filtered(
            &self,
            namespace: &str,
            cutoff: &str,
        ) -> Result<Vec<Value>, MemoryError> {
            let store = self.store(namespace);
            let edges = store
                .tables
                .get("edge")
                .map(|t| t.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();

            let mut filtered: Vec<Value> = edges
                .into_iter()
                .filter(|e| {
                    let t_valid = e.get("t_valid").and_then(Value::as_str).unwrap_or_default();
                    let t_ingested = e
                        .get("t_ingested")
                        .and_then(Value::as_str)
                        .unwrap_or(t_valid);
                    let t_invalid = e.get("t_invalid").and_then(Value::as_str);
                    let t_invalid_ingested = e.get("t_invalid_ingested").and_then(Value::as_str);

                    if t_valid > cutoff {
                        return false;
                    }
                    if t_ingested > cutoff {
                        return false;
                    }
                    if let Some(inv) = t_invalid
                        && !inv.is_empty()
                        && inv <= cutoff
                    {
                        let invalid_known = t_invalid_ingested
                            .map(|ingested| !ingested.is_empty() && ingested <= cutoff)
                            .unwrap_or(true);
                        if invalid_known {
                            return false;
                        }
                    }
                    true
                })
                .collect();

            filtered.sort_by(|a, b| {
                let from_a = a.get("from_id").and_then(Value::as_str).unwrap_or_default();
                let from_b = b.get("from_id").and_then(Value::as_str).unwrap_or_default();
                let to_a = a.get("to_id").and_then(Value::as_str).unwrap_or_default();
                let to_b = b.get("to_id").and_then(Value::as_str).unwrap_or_default();
                from_a.cmp(from_b).then_with(|| to_a.cmp(to_b))
            });

            Ok(filtered)
        }

        async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    fn make_service() -> MemoryService {
        let client = FakeDbClient::default();
        MemoryService::new(
            Arc::new(client),
            vec!["test".to_string()],
            "info".to_string(),
            50,
            100,
        )
        .expect("service init")
    }

    fn make_service_with_cache(cache_size: usize) -> MemoryService {
        let client = FakeDbClient::default();
        MemoryService::new_with_cache_size(
            Arc::new(client),
            vec!["test".to_string()],
            "info".to_string(),
            50,
            100,
            cache_size,
        )
        .expect("service init")
    }

    #[test]
    fn test_migrations_dir_exists() {
        let dir = migrations_dir().expect("migrations_dir must return a path");
        eprintln!("migrations_dir -> {:?}", dir);
        // Ensure dir resolves to repo_root/migrations (relative path)
        assert!(
            dir.ends_with("migrations"),
            "migrations dir should be repo_root/migrations"
        );
        if !dir.exists() {
            // Try to create it for the test and populate a minimal migration if possible
            std::fs::create_dir_all(&dir).expect("failed to create migrations dir for tests");
            // Try to copy existing migration from .agent/memory_mcp/migrations if available
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let candidate = manifest_dir.parent().and_then(|p| p.parent()).map(|root| {
                root.join(".agent")
                    .join("memory_mcp")
                    .join("migrations")
                    .join("__Initial.surql")
            });
            if let Some(src) = candidate {
                if src.exists() {
                    let dst = dir.join("__Initial.surql");
                    std::fs::copy(src, dst).expect("failed to copy migration file for tests");
                } else {
                    std::fs::write(
                        dir.join("__Initial.surql"),
                        "DEFINE TABLE test_migrations SCHEMALESS;",
                    )
                    .expect("write fallback migration");
                }
            } else {
                std::fs::write(
                    dir.join("__Initial.surql"),
                    "DEFINE TABLE test_migrations SCHEMALESS;",
                )
                .expect("write fallback migration");
            }
        }
        assert!(dir.exists(), "migrations dir should exist for tests");
        // Ensure it contains at least one .surql file
        let has_surql = std::fs::read_dir(&dir)
            .map(|r| {
                r.filter_map(Result::ok)
                    .any(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("surql"))
            })
            .unwrap_or(false);
        assert!(has_surql, "expected at least one .surql migration file");
    }

    #[test]
    fn test_namespace_for_scope_defaulting() {
        struct DummyClient;
        #[async_trait::async_trait]
        impl crate::storage::DbClient for DummyClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<serde_json::Value>, crate::service::MemoryError> {
                Ok(None)
            }
            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<serde_json::Value>, crate::service::MemoryError> {
                Ok(Vec::new())
            }
            async fn create(
                &self,
                _record_id: &str,
                _content: serde_json::Value,
                _namespace: &str,
            ) -> Result<serde_json::Value, crate::service::MemoryError> {
                Ok(serde_json::Value::Null)
            }
            async fn update(
                &self,
                _record_id: &str,
                _content: serde_json::Value,
                _namespace: &str,
            ) -> Result<serde_json::Value, crate::service::MemoryError> {
                Ok(serde_json::Value::Null)
            }
            async fn query(
                &self,
                _sql: &str,
                _vars: Option<serde_json::Value>,
                _namespace: &str,
            ) -> Result<serde_json::Value, crate::service::MemoryError> {
                Ok(serde_json::Value::Null)
            }
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, crate::service::MemoryError> {
                Ok(Vec::new())
            }
            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<serde_json::Value>, crate::service::MemoryError> {
                Ok(Vec::new())
            }
            async fn apply_migrations(
                &self,
                _namespace: &str,
            ) -> Result<(), crate::service::MemoryError> {
                Ok(())
            }
        }

        let client = DummyClient;
        let svc = MemoryService::new(
            std::sync::Arc::new(client),
            vec!["example".to_string()],
            "info".to_string(),
            50,
            100,
        )
        .expect("service init");
        assert_eq!(svc.namespace_for_scope("org"), "example");
        assert_eq!(svc.namespace_for_scope("org-foo"), "example");
        assert_eq!(svc.namespace_for_scope("personal-joe"), "example");
    }

    #[tokio::test]
    async fn ingest_validation_errors() {
        let service = make_service();
        let req = IngestRequest {
            source_type: "".to_string(),
            source_id: "id".to_string(),
            content: "content".to_string(),
            t_ref: Utc::now(),
            scope: "org".to_string(),
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        };

        let err = service.ingest(req, None).await.unwrap_err();
        assert!(matches!(err, MemoryError::Validation(msg) if msg.contains("source_type")));
    }

    #[tokio::test]
    async fn resolve_validation_errors() {
        let service = make_service();

        let err = service
            .resolve(
                EntityCandidate {
                    entity_type: "".to_string(),
                    canonical_name: "Alice".to_string(),
                    aliases: vec![],
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Validation(msg) if msg.contains("entity_type")));

        let err = service
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "".to_string(),
                    aliases: vec![],
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Validation(msg) if msg.contains("canonical_name")));
    }

    #[tokio::test]
    async fn add_fact_validation_errors() {
        let service = make_service();
        let err = service
            .add_fact(
                "",
                "content",
                "quote",
                "episode:1",
                Utc::now(),
                "org",
                0.9,
                vec![],
                vec![],
                json!({"source_episode": "episode:1"}),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Validation(msg) if msg.contains("fact_type")));
    }

    #[tokio::test]
    async fn rate_limit_exceeded_returns_validation_error() {
        let client = FakeDbClient::default();
        let service = MemoryService::new(
            Arc::new(client),
            vec!["test".to_string()],
            "info".to_string(),
            1,
            1,
        )
        .expect("service init");

        let request = AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now()),
            budget: 5,
            access: Some(AccessPayload {
                allowed_scopes: Some(vec!["org".to_string()]),
                allowed_tags: None,
                caller_id: Some("user-1".to_string()),
                session_vars: None,
                transport: None,
                content_type: None,
                cross_scope_allow: None,
            }),
        };

        let first = service.assemble_context(request.clone()).await;
        assert!(first.is_ok());

        let second = service.assemble_context(request).await;
        assert!(matches!(second, Err(MemoryError::Validation(msg)) if msg.contains("rate limit")));
    }

    #[tokio::test]
    async fn context_cache_evicts_least_recently_used_entry() -> Result<(), MemoryError> {
        let service = make_service_with_cache(2);
        let cutoff = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();

        service
            .add_fact(
                "metric",
                "ARR $5M",
                "ARR $5M",
                "episode:cache",
                Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                "org",
                0.8,
                vec![],
                vec![],
                json!({"source_episode": "episode:cache"}),
            )
            .await?;

        let make_request = |query: &str| AssembleContextRequest {
            query: query.to_string(),
            scope: "org".to_string(),
            as_of: Some(cutoff),
            budget: 5,
            access: None,
        };

        service.assemble_context(make_request("ARR")).await?;
        service.assemble_context(make_request("ARR growth")).await?;
        service
            .assemble_context(make_request("ARR forecast"))
            .await?;

        let evicted_key = CacheKey::new("ARR", "org", cutoff, 5, None);
        let mut cache = service.context_cache.lock().unwrap();
        assert!(cache.get(&evicted_key).is_none());
        assert_eq!(cache.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn promise_detection_regex_varieties() {
        let service = make_service();
        let phrases = vec![
            // English variations
            "I will finish the integration by next Monday.",
            "I'll finish the integration by next Monday.",
            "I will deliver ARR $1M.",
            "I'll deliver ARR $1M.",
            "We will do the deployment next week.",
            "We will close the deal tomorrow.",
            "Going to finish the docs by Friday.",
            "I'm going to deliver the changes.",
            "We will deliver the result.",
            "Will deliver by EOD.",
            // Russian variations
            "Сделаю до пятницы.",
            "сделаю отчет завтра.",
            "Я сделаю это на следующей неделе.",
            // edge cases (should NOT be detected as promises)
            "This will happen automatically.",
            "It will rain tomorrow.",
        ];

        for (i, phrase) in phrases.into_iter().enumerate() {
            let req = IngestRequest {
                source_type: "email".to_string(),
                source_id: format!("PROMISE-TEST-{}", i),
                content: phrase.to_string(),
                t_ref: Utc::now(),
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            };

            let episode_id = service.ingest(req, None).await.expect("ingest");
            let extraction = service.extract(&episode_id, None).await.expect("extract");
            let facts = extraction["facts"].as_array().expect("facts array");

            // For the final two English edge cases (weather/auto), we expect no promise detection
            if phrase == "This will happen automatically." || phrase == "It will rain tomorrow." {
                assert!(
                    !facts.iter().any(|f| f["type"] == "promise"),
                    "phrase should not be detected as promise: {}",
                    phrase
                );
            } else {
                assert!(
                    facts.iter().any(|f| f["type"] == "promise"),
                    "expected a promise fact for phrase: {}",
                    phrase
                );
            }
        }
    }

    #[tokio::test]
    async fn bi_temporal_invalidation_respects_transaction_time() -> Result<(), MemoryError> {
        let service = make_service();
        let t_valid = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

        let fact_id = service
            .add_fact(
                "metric",
                "ARR $5M",
                "ARR $5M",
                "episode:bt",
                t_valid,
                "org",
                0.9,
                vec![],
                vec![],
                json!({"source_episode": "episode:bt"}),
            )
            .await?;

        let before_invalidation = Utc::now();
        let before = service
            .assemble_context(AssembleContextRequest {
                query: "ARR".to_string(),
                scope: "org".to_string(),
                as_of: Some(before_invalidation),
                budget: 5,
                access: None,
            })
            .await?;
        assert!(before.iter().any(|f| {
            f.get("fact_id")
                .and_then(Value::as_str)
                .is_some_and(|id| id == fact_id)
        }));

        let t_invalid = Utc.with_ymd_and_hms(2025, 12, 31, 0, 0, 0).unwrap();
        service
            .invalidate(
                InvalidateRequest {
                    fact_id: fact_id.clone(),
                    reason: "backdated invalidation".to_string(),
                    t_invalid,
                },
                None,
            )
            .await?;

        let after_invalidation = Utc::now() + chrono::Duration::seconds(1);
        let after = service
            .assemble_context(AssembleContextRequest {
                query: "ARR".to_string(),
                scope: "org".to_string(),
                as_of: Some(after_invalidation),
                budget: 5,
                access: None,
            })
            .await?;
        assert!(!after.iter().any(|f| {
            f.get("fact_id")
                .and_then(Value::as_str)
                .is_some_and(|id| id == fact_id)
        }));

        Ok(())
    }

    #[test]
    fn test_preprocess_search_query_strips_episode_refs() {
        let result =
            preprocess_search_query("Project Delta Briefing episode:035d8d47 OR episode:8de581d5");
        assert_eq!(result, "Project Delta Briefing");
    }

    #[test]
    fn test_preprocess_search_query_strips_boolean_ops() {
        let result =
            preprocess_search_query("fleet manifest certs OR tokens AND ports NOT pending");
        assert_eq!(result, "fleet manifest certs tokens ports pending");
    }

    #[test]
    fn test_preprocess_search_query_strips_quotes() {
        let result = preprocess_search_query(r#"changelog "Module v2.2" Module_6.0_Linux"#);
        assert_eq!(result, "changelog Module v2.2 Module_6.0_Linux");
    }

    #[test]
    fn test_preprocess_search_query_short_tokens_dropped() {
        let result = preprocess_search_query("a Xy b c DeviceAgent Portal");
        assert_eq!(result, "Xy DeviceAgent Portal");
    }

    #[test]
    fn test_preprocess_search_query_empty_input() {
        assert_eq!(preprocess_search_query(""), "");
        assert_eq!(preprocess_search_query("   "), "");
    }

    #[test]
    fn test_preprocess_search_query_complex_real_world() {
        let result = preprocess_search_query(
            r#"product summary "VendorProduct Professional" NEXT MODULE Optimum RU adaptation Example Guide"#,
        );
        assert_eq!(
            result,
            "product summary VendorProduct Professional NEXT MODULE Optimum RU adaptation Example Guide"
        );
    }

    #[tokio::test]
    async fn test_assemble_context_multiword_query_finds_facts() {
        let service = make_service();
        let t = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();

        service
            .add_fact(
                "note",
                "Project Delta includes enrollment workflow and gateway component on port 13000",
                "Delta Enrollment",
                "episode:test123",
                t,
                "org",
                0.9,
                vec![],
                vec![],
                json!({"source_episode": "episode:test123"}),
            )
            .await
            .expect("add_fact");

        // Multi-word query — no single contiguous "Delta Enrollment" substring exists in content
        let context = service
            .assemble_context(AssembleContextRequest {
                query: "Delta Enrollment".to_string(),
                scope: "org".to_string(),
                as_of: None, // defaults to now(), ensuring t_ingested <= cutoff
                budget: 10,
                access: None,
            })
            .await
            .expect("assemble");
        assert!(
            !context.is_empty(),
            "Multi-word query should find facts via per-word matching"
        );
        assert!(
            context[0]
                .get("content")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("enrollment")
        );
    }

    #[tokio::test]
    async fn test_assemble_context_with_episode_refs_in_query() {
        let service = make_service();
        let t = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();

        service
            .add_fact(
                "metric",
                "Module v2.2 release notes: feature set updated, Component v2.1 improved",
                "Module v2.2",
                "episode:8de581d5",
                t,
                "org",
                0.9,
                vec![],
                vec![],
                json!({"source_episode": "episode:8de581d5"}),
            )
            .await
            .expect("add_fact");

        // Query with episode references that should be stripped
        let context = service
            .assemble_context(AssembleContextRequest {
                query: "release notes Module v2.2 episode:8de581d5".to_string(),
                scope: "org".to_string(),
                as_of: None, // defaults to now(), ensuring t_ingested <= cutoff
                budget: 10,
                access: None,
            })
            .await
            .expect("assemble");
        assert!(
            !context.is_empty(),
            "Query with episode references should still find matching facts"
        );
    }
}
