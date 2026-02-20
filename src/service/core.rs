//! MemoryService implementation - core service orchestration.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Value, json};

use crate::config::SurrealConfig;
use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{
    AccessContext, AssembleContextRequest, EntityCandidate, ExplainRequest, IngestRequest,
    InvalidateRequest,
};
use crate::storage::{DbClient, SurrealDbClient};

use super::cache::{CacheKey, SafeMutex};
use super::error::MemoryError;
use super::ids::{deterministic_entity_id, deterministic_episode_id, deterministic_fact_id};
use super::validation::{validate_entity_candidate, validate_fact_input, validate_ingest_request};

/// Core service for memory operations.
#[derive(Clone)]
pub struct MemoryService {
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) namespaces: Vec<String>,
    pub(crate) default_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    pub(crate) context_cache: Arc<Mutex<LruCache<CacheKey, Vec<Value>>>>,
    pub(crate) analyzers: Arc<Mutex<HashMap<String, Value>>>,
    pub(crate) indexes: Arc<Mutex<HashMap<String, Value>>>,
    pub(crate) name_regex: Regex,
}

use lru::LruCache;

impl MemoryService {
    /// Creates a new `MemoryService` from environment variables.
    pub async fn new_from_env() -> Result<Self, MemoryError> {
        let config = SurrealConfig::from_env()?;

        // Startup log: emit effective DB storage location so misconfigured
        // working directories are easy to diagnose. Use Info level so the
        // message appears by default in normal runs.
        let effective_data_dir = config.data_dir_or_default();
        let startup_logger = crate::logging::StdoutLogger::new(&config.log_level);
        let mut startup_event = std::collections::HashMap::new();
        startup_event.insert("op".to_string(), serde_json::json!("startup"));
        startup_event.insert(
            "db_mode".to_string(),
            serde_json::json!(if config.embedded { "embedded" } else { "remote" }),
        );
        // Only include the effective data dir for embedded mode to avoid
        // exposing unnecessary fields for remote mode.
        if config.embedded {
            startup_event.insert("effective_data_dir".to_string(), serde_json::json!(effective_data_dir.clone()));
        } else if let Some(url) = &config.url {
            startup_event.insert("url".to_string(), serde_json::json!(url));
        }
        startup_event.insert("namespaces".to_string(), serde_json::json!(config.namespaces));
        startup_event.insert("db_name".to_string(), serde_json::json!(config.db_name));
        startup_logger.log(startup_event, crate::logging::LogLevel::Info);

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
        service
            .db_client
            .apply_migrations(&service.default_namespace)
            .await?;
        Ok(service)
    }

    /// Creates a new service instance.
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
            super::CONTEXT_CACHE_SIZE,
        )
    }

    #[cfg(test)]
    #[allow(dead_code)]
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
        let cache_size = std::num::NonZeroUsize::new(cache_size).ok_or_else(|| {
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

    /// Public helper for tool-level logging.
    pub fn log_tool_event(&self, op: &str, args: Value, result: Value, level: LogLevel) {
        self.logger.log(log_event(op, args, result, None), level);
    }

    /// Returns the total count of episodes.
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

    /// Ingests a new episode.
    pub async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessContext>,
    ) -> Result<String, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        validate_ingest_request(&request)?;

        let episode_id = deterministic_episode_id(
            &request.source_type,
            &request.source_id,
            request.t_ref,
            &request.scope,
        );
        let namespace = self.namespace_for_scope(&request.scope);
        let existing = self.db_client.select_one(&episode_id, &namespace).await?;
        if existing.is_none() {
            let t_ingested = request.t_ingested.unwrap_or_else(super::query::now);
            let payload = json!({
                "episode_id": episode_id,
                "source_type": request.source_type,
                "source_id": request.source_id,
                "content": request.content,
                "t_ref": super::normalize_dt(request.t_ref),
                "t_ingested": super::normalize_dt(t_ingested),
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
                    "t_ref": super::normalize_dt(request.t_ref),
                    "scope": request.scope,
                }),
                json!({"episode_id": episode_id}),
                access.as_ref(),
            ),
            LogLevel::Info,
        );

        Ok(episode_id)
    }

    /// Provides explanations for context items.
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

    /// Extracts entities and facts from an episode.
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
        let payload = super::episode::extract_from_episode(self, episode_id).await?;
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

    /// Resolves an entity candidate.
    pub async fn resolve(
        &self,
        candidate: EntityCandidate,
        access: Option<AccessContext>,
    ) -> Result<String, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        validate_entity_candidate(&candidate)?;
        let namespace = self.default_namespace.clone();
        let existing = self
            .find_entity_record(&candidate.canonical_name, &namespace)
            .await?;
        if let Some(record) = existing {
            let existing_id = record
                .get("entity_id")
                .and_then(string_from_value)
                .or_else(|| record.get("id").and_then(string_from_value))
                .unwrap_or_default();
            return Ok(existing_id);
        }

        let entity_id = deterministic_entity_id(&candidate.entity_type, &candidate.canonical_name);
        let aliases = candidate
            .aliases
            .into_iter()
            .filter(|alias| !alias.trim().is_empty())
            .map(|alias| super::normalize_text(&alias))
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

    /// Adds a new fact.
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
        _provenance: Value,
    ) -> Result<String, MemoryError> {
        validate_fact_input(fact_type, content, quote, source_episode, scope)?;

        let fact_id = deterministic_fact_id(fact_type, content, source_episode, t_valid);
        let namespace = self.namespace_for_scope(scope);
        let existing = self.db_client.select_one(&fact_id, &namespace).await?;
        if existing.is_none() {
            let t_ingested = super::query::now();
            let payload = json!({
                "fact_id": fact_id.clone(),
                "fact_type": fact_type,
                "content": content,
                "quote": quote,
                "source_episode": source_episode,
                "t_valid": super::normalize_dt(t_valid),
                "t_ingested": super::normalize_dt(t_ingested),
                "confidence": confidence,
                "entity_links": entity_links,
                "scope": scope,
                "policy_tags": policy_tags,
                "provenance": json!({}),
            });
            let created = self.db_client.create(&fact_id, payload, &namespace).await?;
            if created.is_null() {
                return Err(MemoryError::Storage(
                    "failed to persist fact record".to_string(),
                ));
            }
            super::cache::invalidate_cache_by_scope(&self.context_cache, scope);
        }
        Ok(fact_id)
    }

    /// Invalidates a fact.
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

        let scope = updated
            .get("scope")
            .and_then(string_from_value)
            .unwrap_or_else(|| namespace.clone());

        updated.insert(
            "t_invalid".to_string(),
            json!(super::normalize_dt(request.t_invalid)),
        );
        updated.insert(
            "t_invalid_ingested".to_string(),
            json!(super::normalize_dt(super::query::now())),
        );
        self.db_client
            .update(&request.fact_id, Value::Object(updated), &namespace)
            .await?;
        super::cache::invalidate_cache_by_scope(&self.context_cache, &scope);
        Ok("ok".to_string())
    }

    /// Assembles context for a query.
    pub async fn assemble_context(
        &self,
        request: AssembleContextRequest,
    ) -> Result<Vec<Value>, MemoryError> {
        super::context::assemble_context(self, request).await
    }

    /// Resolves a person entity.
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

    /// Resolves a company entity.
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

    /// Creates a relationship edge between two entities.
    pub async fn relate(
        &self,
        from_id: &str,
        relation: &str,
        to_id: &str,
    ) -> Result<(), MemoryError> {
        use crate::models::Edge;
        let edge = Edge {
            from_id: from_id.to_string(),
            relation: relation.to_string(),
            to_id: to_id.to_string(),
            strength: 1.0,
            confidence: 0.8,
            provenance: json!({"source": "manual"}),
            t_valid: super::query::now(),
            t_ingested: super::query::now(),
            t_invalid: None,
            t_invalid_ingested: None,
        };
        super::episode::store_edge(self, &edge, &self.default_namespace).await
    }

    /// Registers an analyzer.
    pub fn register_analyzer(&self, name: &str, config: Value) {
        self.analyzers.safe_lock().insert(name.to_string(), config);
    }

    /// Registers an index.
    pub fn register_index(&self, name: &str, config: Value) {
        self.indexes.safe_lock().insert(name.to_string(), config);
    }

    /// Creates a task.
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
                    "due_date": due_date.map(super::normalize_dt),
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
            "due_date": due_date.map(super::normalize_dt),
        }))
    }

    /// Creates a message draft.
    pub fn send_message_draft(&self, to: &str, subject: &str, body: &str) -> Value {
        json!({
            "status": "pending_confirmation",
            "to": to,
            "subject": subject,
            "body": body,
        })
    }

    /// Creates a meeting draft.
    pub fn schedule_meeting(&self, title: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> Value {
        json!({
            "status": "pending_confirmation",
            "title": title,
            "start": super::normalize_dt(start),
            "end": super::normalize_dt(end),
        })
    }

    /// Updates a metric.
    pub fn update_metric(&self, name: &str, value: f64) -> Value {
        json!({
            "status": "ok",
            "metric": name,
            "value": value,
        })
    }

    /// Retrieves promise facts.
    pub async fn ui_promises(&self) -> Result<Vec<Value>, MemoryError> {
        self.fetch_facts_by_type("promise").await
    }

    /// Retrieves metric facts.
    pub async fn ui_metrics(&self) -> Result<Vec<Value>, MemoryError> {
        self.fetch_facts_by_type("metric").await
    }

    /// Fetches facts filtered by type.
    async fn fetch_facts_by_type(&self, fact_type: &str) -> Result<Vec<Value>, MemoryError> {
        let op_name = format!("ui_{fact_type}s");
        self.logger.log(
            log_event(&format!("{op_name}.start"), json!({}), json!({}), None),
            LogLevel::Debug,
        );
        let mut records = Vec::new();
        for namespace in &self.namespaces {
            records.extend(self.db_client.select_table("fact", namespace).await?);
        }
        let filtered: Vec<Value> = records
            .into_iter()
            .filter(|record| record.get("fact_type").and_then(Value::as_str) == Some(fact_type))
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
                &format!("{op_name}.done"),
                json!({}),
                json!({"count": filtered.len()}),
                None,
            ),
            LogLevel::Info,
        );
        Ok(filtered)
    }

    /// Retrieves task drafts.
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
                for key in ["id", "title", "status", "due_date"] {
                    if let Some(value) = record.get(key).and_then(string_from_value) {
                        record[key] = Value::String(value);
                    }
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

    /// Retrieves SurrealDB config.
    pub async fn get_surrealdb_config(&self) -> Result<Value, MemoryError> {
        Ok(json!({
            "namespaces": self.namespaces.clone(),
        }))
    }

    /// Finds an introduction chain.
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

        let cutoff = as_of.unwrap_or_else(super::query::now);
        let cutoff_iso = super::normalize_dt(cutoff);

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

    /// Invalidates a superseded metric.
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

    /// Performs a CBOR round-trip.
    pub fn cbor_round_trip(&self, payload: &Value) -> Result<Value, MemoryError> {
        let encoded = serde_cbor::to_vec(payload)
            .map_err(|err| MemoryError::Storage(format!("cbor encode error: {err}")))?;
        let decoded: Value = serde_cbor::from_slice(&encoded)
            .map_err(|err| MemoryError::Storage(format!("cbor decode error: {err}")))?;
        Ok(decoded)
    }

    // ==================== Private Helpers ====================

    async fn check_surrealdb_connection(&self) -> Result<(), MemoryError> {
        let _ = self
            .db_client
            .select_table("event_log", &self.default_namespace)
            .await?;
        Ok(())
    }

    pub(crate) fn namespace_for_scope(&self, scope: &str) -> String {
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

    pub(crate) async fn find_episode_record(
        &self,
        episode_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(episode_id, namespace).await?;
            if let Some(Value::Object(map)) = record {
                return Ok((Some(map), Some(namespace.clone())));
            }
        }
        Ok((None, None))
    }

    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(fact_id, namespace).await?;
            if let Some(Value::Object(map)) = record {
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
        let normalized = super::normalize_text(name);
        let records = self.db_client.select_table("entity", namespace).await?;
        for record in records {
            if let Value::Object(map) = record {
                let canonical = map
                    .get("canonical_name")
                    .and_then(string_from_value)
                    .map(|value| super::normalize_text(&value))
                    .unwrap_or_default();
                let aliases: Vec<String> = map
                    .get("aliases")
                    .and_then(|value| {
                        value
                            .as_array()
                            .or_else(|| value.get("Array").and_then(Value::as_array))
                    })
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(string_from_value)
                            .map(|value| super::normalize_text(&value))
                            .collect()
                    })
                    .unwrap_or_default();
                if normalized == canonical || aliases.contains(&normalized) {
                    return Ok(Some(map));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn is_scope_allowed(&self, scope: &str, access: &AccessContext) -> bool {
        if let Some(scopes) = &access.allowed_scopes
            && !scopes.contains(&scope.to_string())
        {
            return access.cross_scope_allow.as_ref().is_some_and(|cross| {
                cross
                    .iter()
                    .any(|pair| pair.from == "*" && pair.to == scope)
            });
        }
        true
    }

    pub(crate) fn enforce_rate_limit(
        &self,
        access: Option<&AccessContext>,
    ) -> Result<(), MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
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
}

// ==================== Rate Limiter ====================

pub(crate) struct RateLimiter {
    rps: f64,
    burst: f64,
    tokens: Mutex<HashMap<String, f64>>,
    last: Mutex<HashMap<String, Instant>>,
}

impl RateLimiter {
    pub(crate) fn new(rps: i32, burst: i32) -> Self {
        Self {
            rps: (rps.max(1)) as f64,
            burst: (burst.max(1)) as f64,
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
        let entry = tokens.entry(key.to_string()).or_insert(self.burst);
        let refill = elapsed * self.rps;
        *entry = (*entry + refill).min(self.burst);
        if *entry < 1.0 {
            return false;
        }
        *entry -= 1.0;
        true
    }
}

// ==================== Helper Functions ====================

pub(crate) fn log_event(
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

#[must_use]
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

fn bfs_path(
    graph: &HashMap<String, Vec<String>>,
    start: &str,
    target: &str,
    max_hops: i32,
) -> Option<Vec<String>> {
    use std::collections::HashSet;

    let mut queue: std::collections::VecDeque<(String, Vec<String>)> =
        std::collections::VecDeque::new();
    let mut visited = HashSet::new();
    queue.push_back((start.to_string(), vec![start.to_string()]));
    visited.insert(start.to_string());

    while let Some((node, path)) = queue.pop_front() {
        if (path.len() as i32) - 1 > max_hops {
            continue;
        }
        if let Some(neighbors) = graph.get(&node) {
            for neighbor in neighbors {
                if visited.contains(neighbor) {
                    continue;
                }
                if neighbor == target {
                    let mut new_path = path.clone();
                    new_path.push(neighbor.clone());
                    return Some(new_path);
                }
                visited.insert(neighbor.clone());
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
    use crate::models::{AccessContext, AccessScopeAllow};
    use serde_json::json;

    // ==================== Rate Limiter Tests ====================

    #[test]
    fn rate_limiter_allows_burst() {
        let limiter = RateLimiter::new(10, 5);
        // Should allow burst of 5 requests
        for _ in 0..5 {
            assert!(limiter.allow("test-user"));
        }
    }

    #[test]
    fn rate_limiter_blocks_after_burst() {
        let limiter = RateLimiter::new(10, 2);
        assert!(limiter.allow("test-user"));
        assert!(limiter.allow("test-user"));
        // Third request should be blocked
        assert!(!limiter.allow("test-user"));
    }

    #[test]
    fn rate_limiter_different_users_independent() {
        let limiter = RateLimiter::new(10, 1);
        assert!(limiter.allow("user-1"));
        assert!(!limiter.allow("user-1"));
        // Different user should be allowed
        assert!(limiter.allow("user-2"));
    }

    // ==================== Helper Function Tests ====================

    #[test]
    fn log_event_creates_expected_structure() {
        let event = log_event(
            "test_op",
            json!({"key": "value"}),
            json!({"result": "ok"}),
            None,
        );
        assert_eq!(event.get("op").unwrap().as_str(), Some("test_op"));
        assert_eq!(
            event.get("args").unwrap().get("key").unwrap().as_str(),
            Some("value")
        );
        assert_eq!(
            event.get("result").unwrap().get("result").unwrap().as_str(),
            Some("ok")
        );
    }

    #[test]
    fn log_event_includes_access_when_provided() {
        let access = AccessContext {
            caller_id: Some("test-caller".to_string()),
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        let event = log_event("test_op", json!({}), json!({}), Some(&access));
        let access_event = event.get("access").unwrap();
        assert_eq!(
            access_event.get("caller_id").unwrap().as_str(),
            Some("test-caller")
        );
    }

    #[test]
    fn serialize_access_includes_all_fields() {
        let access = AccessContext {
            caller_id: Some("caller".to_string()),
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: Some(vec!["tag1".to_string()]),
            session_vars: Some(json!({"key": "value"})),
            transport: Some("http".to_string()),
            content_type: Some("application/json".to_string()),
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }]),
        };
        let serialized = serialize_access(&access);
        assert!(serialized.get("caller_id").is_some());
        assert!(serialized.get("allowed_scopes").is_some());
        assert!(serialized.get("allowed_tags").is_some());
        assert!(serialized.get("session_vars").is_some());
        assert!(serialized.get("transport").is_some());
        assert!(serialized.get("content_type").is_some());
        assert!(serialized.get("cross_scope_allow").is_some());
    }

    #[test]
    fn string_from_value_handles_string() {
        let value = json!("test");
        assert_eq!(string_from_value(&value), Some("test".to_string()));
    }

    #[test]
    fn string_from_value_handles_strand() {
        let value = json!({"Strand": "test"});
        assert_eq!(string_from_value(&value), Some("test".to_string()));
    }

    #[test]
    fn string_from_value_handles_nested_strand() {
        let value = json!({"Strand": {"String": "test"}});
        assert_eq!(string_from_value(&value), Some("test".to_string()));
    }

    #[test]
    fn string_from_value_returns_none_for_other_types() {
        assert_eq!(string_from_value(&json!(123)), None);
        assert_eq!(string_from_value(&json!(true)), None);
        assert_eq!(string_from_value(&json!(null)), None);
        assert_eq!(string_from_value(&json!([1, 2, 3])), None);
    }

    #[test]
    fn bfs_path_finds_direct_connection() {
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), vec!["B".to_string()]);
        graph.insert("B".to_string(), vec![]);

        let path = bfs_path(&graph, "A", "B", 5);
        assert_eq!(path, Some(vec!["A".to_string(), "B".to_string()]));
    }

    #[test]
    fn bfs_path_finds_indirect_connection() {
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), vec!["B".to_string()]);
        graph.insert("B".to_string(), vec!["C".to_string()]);
        graph.insert("C".to_string(), vec![]);

        let path = bfs_path(&graph, "A", "C", 5);
        assert_eq!(
            path,
            Some(vec!["A".to_string(), "B".to_string(), "C".to_string()])
        );
    }

    #[test]
    fn bfs_path_respects_max_hops() {
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), vec!["B".to_string()]);
        graph.insert("B".to_string(), vec!["C".to_string()]);
        graph.insert("C".to_string(), vec!["D".to_string()]);

        // Path A->B->C->D has 3 edges
        // With max_hops=1, we can only reach B
        let path = bfs_path(&graph, "A", "D", 1);
        assert_eq!(path, None);

        // With max_hops=3, we can reach D
        let path = bfs_path(&graph, "A", "D", 3);
        assert!(path.is_some());
    }

    #[test]
    fn bfs_path_returns_none_for_unreachable() {
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), vec!["B".to_string()]);
        graph.insert("B".to_string(), vec![]);
        graph.insert("C".to_string(), vec![]); // Unreachable from A

        let path = bfs_path(&graph, "A", "C", 5);
        assert_eq!(path, None);
    }

    #[test]
    fn bfs_path_returns_single_element_for_same_node() {
        // Note: Current implementation doesn't handle start==target specially
        // It will return None since we only check neighbors
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), vec![]);

        let path = bfs_path(&graph, "A", "A", 5);
        // Current behavior: returns None for same node
        assert_eq!(path, None);
    }
}
