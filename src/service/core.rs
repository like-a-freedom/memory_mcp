//! MemoryService implementation - core service orchestration.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::config::SurrealConfig;
use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{
    AccessContext, AssembleContextRequest, AssembledContextItem, EntityCandidate, ExplainItem,
    ExplainRequest, ExtractResult, IngestRequest, InvalidateRequest,
};
use crate::storage::{DbClient, GraphDirection, SurrealDbClient};

use super::cache::{CacheKey, SafeMutex};
use super::embedding::NullEmbedder;
use super::entity_extraction::RegexEntityExtractor;
use super::error::MemoryError;
use super::ids::{deterministic_entity_id, deterministic_episode_id, deterministic_fact_id};
use super::validation::{validate_entity_candidate, validate_fact_input, validate_ingest_request};
use super::{EmbeddingProvider, EntityExtractor};

/// Core service for memory operations.
#[derive(Clone)]
pub struct MemoryService {
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) namespaces: Vec<String>,
    pub(crate) default_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    pub(crate) context_cache: Arc<Mutex<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    pub(crate) analyzers: Arc<Mutex<HashMap<String, Value>>>,
    pub(crate) indexes: Arc<Mutex<HashMap<String, Value>>>,
    pub(crate) embedder: Arc<dyn EmbeddingProvider>,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
}

use lru::LruCache;

/// Build a startup versions event payload used for diagnostic logging.
/// Extracted to a helper so it can be unit-tested independently of logger I/O.
fn build_startup_versions_event(
    client_version: &str,
    server_version: Option<&str>,
) -> std::collections::HashMap<String, serde_json::Value> {
    let mut m = std::collections::HashMap::new();
    m.insert("op".to_string(), json!("startup.versions"));
    m.insert("client_version".to_string(), json!(client_version));
    if let Some(sv) = server_version {
        m.insert("surrealdb_server_version".to_string(), json!(sv));
    }
    m
}

impl MemoryService {
    /// Creates a new `MemoryService` from environment variables.
    pub async fn new_from_env() -> Result<Self, MemoryError> {
        let config = SurrealConfig::from_env()?;
        let default_namespace = config
            .default_namespace()
            .ok_or_else(|| MemoryError::ConfigInvalid("namespaces cannot be empty".to_string()))?;

        let effective_data_dir = config.data_dir_or_default();
        let startup_logger = crate::logging::StdoutLogger::new(&config.log_level);
        let mut startup_event = std::collections::HashMap::new();
        startup_event.insert("op".to_string(), serde_json::json!("startup"));
        startup_event.insert(
            "db_mode".to_string(),
            serde_json::json!(if config.embedded {
                "embedded"
            } else {
                "remote"
            }),
        );
        startup_event.insert(
            "namespaces".to_string(),
            serde_json::json!(config.namespaces.clone()),
        );
        if config.embedded {
            startup_event.insert(
                "data_dir".to_string(),
                serde_json::json!(effective_data_dir),
            );
        } else if let Some(url) = &config.url {
            startup_event.insert("url".to_string(), serde_json::json!(url));
        }
        startup_logger.log(startup_event, crate::logging::LogLevel::Info);

        let db_client = SurrealDbClient::connect(&config, default_namespace).await?;
        let server_version = match db_client.server_version(default_namespace).await {
            Ok(version) => version,
            Err(err) => {
                let mut event = std::collections::HashMap::new();
                event.insert(
                    "op".to_string(),
                    serde_json::json!("startup.version_probe_failed"),
                );
                event.insert("error".to_string(), serde_json::json!(err.to_string()));
                startup_logger.log(event, crate::logging::LogLevel::Warn);
                None
            }
        };

        let client_version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown");
        let versions_event =
            build_startup_versions_event(client_version, server_version.as_deref());
        startup_logger.log(versions_event, crate::logging::LogLevel::Info);

        let service = Self::new(
            Arc::new(db_client),
            config.namespaces,
            config.log_level,
            50,
            100,
        )?;
        service
            .db_client
            .apply_migrations(&service.default_namespace)
            .await?;
        service.check_surrealdb_connection().await?;
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
            embedder: Arc::new(NullEmbedder),
            entity_extractor: Arc::new(RegexEntityExtractor::new()?),
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
            let embedding = self.embedder.embed_text(&request.content).await?;
            let mut payload = json!({
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
            if let (Some(embedding), Some(object)) = (embedding, payload.as_object_mut()) {
                object.insert("embedding".to_string(), json!(embedding));
            }
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
    ) -> Result<Vec<ExplainItem>, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        let mut explanations = Vec::with_capacity(request.context_pack.len());
        for item in request.context_pack {
            explanations.push(self.build_explain_item(item).await?);
        }

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
    ) -> Result<ExtractResult, MemoryError> {
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
                    "entities": payload.entities.len(),
                    "facts": payload.facts.len(),
                    "links": payload.links.len(),
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
        let normalized = super::normalize_text(&candidate.canonical_name);
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
        let embedding = self.embedder.embed_text(&candidate.canonical_name).await?;
        let aliases = candidate
            .aliases
            .into_iter()
            .filter(|alias| !alias.trim().is_empty())
            .map(|alias| super::normalize_text(&alias))
            .collect::<Vec<_>>();

        let mut payload = json!({
            "entity_id": entity_id,
            "entity_type": candidate.entity_type,
            "canonical_name": candidate.canonical_name,
            "canonical_name_normalized": normalized,
            "aliases": aliases.clone(),
        });
        if let (Some(embedding), Some(object)) = (embedding, payload.as_object_mut()) {
            object.insert("embedding".to_string(), json!(embedding));
        }
        self.db_client
            .create(&entity_id, payload, &namespace)
            .await?;

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
        provenance: Value,
    ) -> Result<String, MemoryError> {
        validate_fact_input(fact_type, content, quote, source_episode, scope)?;

        let fact_id = deterministic_fact_id(fact_type, content, source_episode, t_valid);
        let namespace = self.namespace_for_scope(scope);
        let existing = self.db_client.select_one(&fact_id, &namespace).await?;
        if existing.is_none() {
            let t_ingested = super::query::now();
            let embedding = self.embedder.embed_text(content).await?;
            let mut payload = json!({
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
                "provenance": provenance,
            });
            if let (Some(embedding), Some(object)) = (embedding, payload.as_object_mut()) {
                object.insert("embedding".to_string(), json!(embedding));
            }
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
    ) -> Result<Vec<AssembledContextItem>, MemoryError> {
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

        let mut frontier = vec![target_id.clone()];
        let mut visited = std::collections::HashSet::from([target_id.clone()]);
        let mut next_hop: HashMap<String, String> = HashMap::new();
        let mut candidate_starts = BTreeSet::new();

        for _ in 0..max_hops {
            let mut next_frontier = Vec::new();

            for node_id in &frontier {
                for namespace in &self.namespaces {
                    for record in self
                        .db_client
                        .select_edge_neighbors(
                            namespace,
                            node_id,
                            &cutoff_iso,
                            GraphDirection::Incoming,
                        )
                        .await?
                    {
                        if let Value::Object(map) = record
                            && let (Some(from_id), Some(to_id)) = (
                                map.get("from_id").and_then(string_from_value),
                                map.get("to_id").and_then(string_from_value),
                            )
                            && visited.insert(from_id.clone())
                        {
                            next_hop.insert(from_id.clone(), to_id);
                            candidate_starts.insert(from_id.clone());
                            next_frontier.push(from_id);
                        }
                    }
                }
            }

            if next_frontier.is_empty() {
                break;
            }

            next_frontier.sort();
            next_frontier.dedup();
            frontier = next_frontier;
        }

        let Some(start_id) = candidate_starts.into_iter().next() else {
            return Ok(vec![]);
        };

        let mut path = vec![start_id.clone()];
        let mut current = start_id;

        while let Some(next) = next_hop.get(&current).cloned() {
            path.push(next.clone());
            if next == target_id {
                return Ok(path);
            }
            current = next;
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
        Ok(self
            .db_client
            .select_entity_lookup(namespace, &normalized)
            .await?
            .and_then(|record| record.as_object().cloned()))
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
                .and_then(string_from_value)
                .or_else(|| map.get("id").and_then(string_from_value))
        }))
    }

    async fn build_explain_item(&self, item: ExplainItem) -> Result<ExplainItem, MemoryError> {
        let (record, _) = self.find_episode_record(&item.source_episode).await?;
        let Some(record) = record else {
            return Ok(item);
        };

        let Some(episode) = super::episode::episode_from_record(&record) else {
            return Ok(item);
        };

        Ok(ExplainItem {
            content: if item.content.is_empty() {
                episode.content.clone()
            } else {
                item.content
            },
            quote: item.quote,
            source_episode: item.source_episode,
            scope: Some(episode.scope.clone()),
            t_ref: Some(episode.t_ref),
            t_ingested: Some(episode.t_ingested),
            provenance: json!({
                "source_episode": episode.episode_id,
                "source_type": episode.source_type,
                "source_id": episode.source_id,
            }),
            citation_context: Some(episode.content),
        })
    }
}

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

#[cfg(test)]
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
    use crate::models::EntityCandidate;
    use crate::models::{AccessContext, AccessScopeAllow};
    use serde_json::json;

    #[test]
    fn rate_limiter_allows_burst() {
        let limiter = RateLimiter::new(10, 5);
        for _ in 0..5 {
            assert!(limiter.allow("test-user"));
        }
    }

    #[test]
    fn rate_limiter_blocks_after_burst() {
        let limiter = RateLimiter::new(10, 2);
        assert!(limiter.allow("test-user"));
        assert!(limiter.allow("test-user"));
        assert!(!limiter.allow("test-user"));
    }

    #[test]
    fn rate_limiter_different_users_independent() {
        let limiter = RateLimiter::new(10, 1);
        assert!(limiter.allow("user-1"));
        assert!(!limiter.allow("user-1"));
        assert!(limiter.allow("user-2"));
    }

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

        let path = bfs_path(&graph, "A", "D", 1);
        assert_eq!(path, None);

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
    fn build_startup_versions_event_includes_both_versions() {
        let evt = build_startup_versions_event("0.1.0", Some("SurrealDB 3.0.0"));
        assert_eq!(evt.get("op").unwrap().as_str(), Some("startup.versions"));
        assert_eq!(evt.get("client_version").unwrap().as_str(), Some("0.1.0"));
        assert_eq!(
            evt.get("surrealdb_server_version").unwrap().as_str(),
            Some("SurrealDB 3.0.0")
        );
    }

    #[test]
    fn build_startup_versions_event_omits_server_when_none() {
        let evt = build_startup_versions_event("0.1.0", None);
        assert_eq!(evt.get("op").unwrap().as_str(), Some("startup.versions"));
        assert_eq!(evt.get("client_version").unwrap().as_str(), Some("0.1.0"));
        assert!(!evt.contains_key("surrealdb_server_version"));
    }

    #[test]
    fn bfs_path_returns_single_element_for_same_node() {
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), vec![]);

        let path = bfs_path(&graph, "A", "A", 5);
        assert_eq!(path, None);
    }

    #[test]
    fn namespace_for_scope_returns_exact_match() {
        let service = create_test_service(vec!["org", "personal"]);
        assert_eq!(service.namespace_for_scope("org"), "org");
        assert_eq!(service.namespace_for_scope("personal"), "personal");
    }

    #[test]
    fn namespace_for_scope_returns_default_for_unknown() {
        let service = create_test_service(vec!["org", "personal"]);
        assert_eq!(service.namespace_for_scope("unknown"), "org");
    }

    #[test]
    fn namespace_for_scope_handles_personal_prefix() {
        let service = create_test_service(vec!["org", "personal"]);
        assert_eq!(service.namespace_for_scope("personal-work"), "personal");
    }

    #[test]
    fn namespace_for_scope_handles_org_prefix() {
        let service = create_test_service(vec!["org", "personal"]);
        assert_eq!(service.namespace_for_scope("org-team"), "org");
    }

    #[test]
    fn namespace_for_scope_handles_private_prefix() {
        let service = create_test_service(vec!["org", "private"]);
        assert_eq!(service.namespace_for_scope("private-notes"), "private");
    }

    fn create_test_service(namespaces: Vec<&str>) -> MemoryService {
        use crate::storage::DbClient;
        use std::sync::Arc;

        struct MockDbClient;

        #[async_trait::async_trait]
        impl DbClient for MockDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_embedding(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _embedding: &[f32],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        MemoryService::new(
            Arc::new(MockDbClient),
            namespaces.iter().map(|s| s.to_string()).collect(),
            "warn".to_string(),
            50,
            100,
        )
        .unwrap()
    }

    #[test]
    fn is_scope_allowed_returns_true_when_no_restrictions() {
        let service = create_test_service(vec!["org"]);
        let access = AccessContext::default();
        assert!(service.is_scope_allowed("org", &access));
    }

    #[test]
    fn is_scope_allowed_returns_true_for_allowed_scope() {
        let service = create_test_service(vec!["org"]);
        let access = AccessContext {
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        assert!(service.is_scope_allowed("org", &access));
    }

    #[test]
    fn is_scope_allowed_returns_false_for_disallowed_scope() {
        let service = create_test_service(vec!["org"]);
        let access = AccessContext {
            allowed_scopes: Some(vec!["personal".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        assert!(!service.is_scope_allowed("org", &access));
    }

    #[test]
    fn is_scope_allowed_allows_with_cross_scope_wildcard() {
        let service = create_test_service(vec!["org"]);
        let access = AccessContext {
            allowed_scopes: Some(vec!["personal".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }]),
        };
        assert!(service.is_scope_allowed("org", &access));
    }

    #[test]
    fn enforce_rate_limit_allows_without_caller_id() {
        let service = create_test_service(vec!["org"]);
        let access = AccessContext::default();
        assert!(service.enforce_rate_limit(Some(&access)).is_ok());
    }

    #[test]
    fn enforce_rate_limit_allows_within_limit() {
        let service = create_test_service(vec!["org"]);
        let access = AccessContext {
            caller_id: Some("user-1".to_string()),
            ..Default::default()
        };
        assert!(service.enforce_rate_limit(Some(&access)).is_ok());
    }

    #[test]
    fn enforce_rate_limit_accepts_none() {
        let service = create_test_service(vec!["org"]);
        assert!(service.enforce_rate_limit(None).is_ok());
    }

    #[test]
    fn cbor_round_trip_preserves_value() {
        let service = create_test_service(vec!["org"]);
        let original = json!({"key": "value", "nested": {"num": 42}});
        let round_tripped = service.cbor_round_trip(&original).unwrap();
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn cbor_round_trip_handles_arrays() {
        let service = create_test_service(vec!["org"]);
        let original = json!([1, 2, 3, "test", {"key": "value"}]);
        let round_tripped = service.cbor_round_trip(&original).unwrap();
        assert_eq!(original, round_tripped);
    }

    #[tokio::test]
    async fn resolve_uses_indexed_entity_lookup_instead_of_table_scan() {
        use std::sync::Arc;

        struct LookupOnlyDbClient;

        #[async_trait::async_trait]
        impl DbClient for LookupOnlyDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                panic!("resolve should not scan the entity table")
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_embedding(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _embedding: &[f32],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                panic!("find_intro_chain should not bulk-load all edges")
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                node_id: &str,
                _cutoff: &str,
                direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(direction, GraphDirection::Incoming);

                Ok(match node_id {
                    "entity:openai" => {
                        vec![json!({"from_id": "entity:bob", "to_id": "entity:openai"})]
                    }
                    "entity:bob" => vec![json!({"from_id": "entity:alice", "to_id": "entity:bob"})],
                    _ => vec![],
                })
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(match normalized_name {
                    "dima ivanov" => Some(json!({"entity_id": "entity:existing"})),
                    "openai" => Some(json!({"entity_id": "entity:openai"})),
                    _ => None,
                })
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                panic!("resolve should not create when indexed lookup finds a record")
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = MemoryService::new(
            Arc::new(LookupOnlyDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let resolved = service
            .resolve(
                EntityCandidate {
                    entity_type: "person".to_string(),
                    canonical_name: "Dima Ivanov".to_string(),
                    aliases: vec![],
                },
                None,
            )
            .await
            .unwrap();

        assert_eq!(resolved, "entity:existing");
    }

    #[tokio::test]
    async fn find_intro_chain_uses_db_side_neighbor_lookups() {
        use std::sync::Arc;

        struct TraversalDbClient;

        #[async_trait::async_trait]
        impl DbClient for TraversalDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_embedding(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _embedding: &[f32],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                panic!("find_intro_chain should not materialize the full edge table")
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                node_id: &str,
                _cutoff: &str,
                direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(direction, GraphDirection::Incoming);

                Ok(match node_id {
                    "entity:openai" => {
                        vec![json!({"from_id": "entity:bob", "to_id": "entity:openai"})]
                    }
                    "entity:bob" => vec![json!({"from_id": "entity:alice", "to_id": "entity:bob"})],
                    _ => vec![],
                })
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                assert_eq!(normalized_name, "openai");
                Ok(Some(json!({"entity_id": "entity:openai"})))
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = MemoryService::new(
            Arc::new(TraversalDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let chain = service.find_intro_chain("OpenAI", 3, None).await.unwrap();

        assert_eq!(
            chain,
            vec![
                "entity:alice".to_string(),
                "entity:bob".to_string(),
                "entity:openai".to_string(),
            ]
        );
    }
}
