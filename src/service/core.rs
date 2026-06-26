//! MemoryService implementation - core service orchestration.

use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{
    AccessPayload, AssembleContextRequest, AssembledContextItem, EntityCandidate, ExplainItem,
    ExplainRequest, ExtractResult, GraphHubEntity, GraphInsights, IngestRequest, InvalidateRequest,
    ProvenanceSource,
};
use crate::storage::GraphDirection;

use super::error::MemoryError;
use super::ingest::prepare_ingest_request;
use super::util::{deterministic_entity_id, deterministic_episode_id, deterministic_fact_id};
use super::util::{validate_entity_candidate, validate_fact_input, validate_ingest_request};
use super::value_helpers::{json_i64, string_from_value};

mod builder;
mod helpers;
pub use builder::MemoryService;
pub(crate) use helpers::*;

const MAX_GRAPH_INSIGHT_LINKED_ENTITIES: usize = 3;
const MAX_GRAPH_INSIGHT_HUBS: i32 = 3;
const MAX_GRAPH_INSIGHT_CONNECTIONS: usize = 5;

impl MemoryService {
    /// Public helper for tool-level logging.
    pub(crate) fn log_tool_event(
        &self,
        op: &str,
        args: Value,
        result: Value,
        level: LogLevel,
        request_id: Option<&str>,
    ) {
        self.logger
            .log(log_event(op, args, result, None, request_id, None), level);
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
            log_event(op, args, result, None, request_id, Some(duration_ms)),
            level,
        );
    }

    pub(crate) fn build_fact_embedding_input(
        fact_type: &str,
        content: &str,
        quote: &str,
    ) -> String {
        format!("{fact_type}\n{content}\n{quote}")
    }

    pub(crate) fn insert_current_embedding_fields(
        &self,
        payload: &mut serde_json::Map<String, Value>,
        embedding: Vec<f64>,
    ) -> Result<(), MemoryError> {
        let expected_dim = self
            .current_embedding_dimension
            .unwrap_or_else(|| self.embedding_provider.dimension());
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
        if let Some(model) = &self.current_embedding_model {
            payload.insert("embedding_model".to_string(), json!(model));
        }
        payload.insert("embedding_dimension".to_string(), json!(expected_dim));
        if let Some(signature) = &self.current_embedding_signature {
            payload.insert("embedding_signature".to_string(), json!(signature));
        }
        payload.insert(
            "embedding_updated_at".to_string(),
            json!(super::normalize_dt(super::query::now())),
        );

        Ok(())
    }

    /// Returns the total count of episodes.
    pub async fn episode_count(&self) -> Result<i32, MemoryError> {
        let mut total = 0;
        let sql = "SELECT count() FROM episode GROUP ALL";
        for namespace in &self.namespaces {
            let result = self.db_client.query(sql, None, namespace).await?;
            let count = result
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|obj| obj.get("count"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            total += count;
        }
        Ok(total)
    }

    /// Ingests a new episode.
    pub async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        let ingest_transport = super::ingest::detect_ingest_transport(&request.content);
        let original_source_id = request.source_id.clone();
        let original_content_len = request.content.len();
        self.logger.log(
            log_event(
                "ingest.prepare",
                json!({
                    "source_type": request.source_type,
                    "source_id": request.source_id,
                    "scope": request.scope,
                    "project": request.project,
                    "transport": ingest_transport,
                }),
                json!({}),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Debug,
        );
        let request = prepare_ingest_request(request).await?;
        self.logger.log(
            log_event(
                "ingest.prepared",
                json!({
                    "scope": request.scope,
                    "project": request.project,
                    "transport": ingest_transport,
                    "source_id_rewritten": request.source_id != original_source_id,
                }),
                json!({
                    "source_id": request.source_id,
                    "content_len": request.content.len(),
                    "original_content_len": original_content_len,
                }),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Trace,
        );
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
            let mut payload = serde_json::Map::from_iter([
                ("episode_id".to_string(), json!(episode_id)),
                ("source_type".to_string(), json!(request.source_type)),
                ("source_id".to_string(), json!(request.source_id)),
                ("content".to_string(), json!(request.content)),
                (
                    "t_ref".to_string(),
                    json!(super::normalize_dt(request.t_ref)),
                ),
                (
                    "t_ingested".to_string(),
                    json!(super::normalize_dt(t_ingested)),
                ),
                ("scope".to_string(), json!(request.scope.clone())),
                (
                    "visibility_scope".to_string(),
                    json!(
                        request
                            .visibility_scope
                            .unwrap_or_else(|| request.scope.clone())
                    ),
                ),
                ("policy_tags".to_string(), json!(request.policy_tags)),
            ]);
            if let Some(project) = request.project.clone() {
                payload.insert("project".to_string(), json!(project));
            }
            self.db_client
                .create(&episode_id, Value::Object(payload), &namespace)
                .await?;
        } else {
            self.logger.log(
                log_event(
                    "ingest.duplicate",
                    json!({
                        "episode_id": episode_id,
                        "source_id": request.source_id,
                        "scope": request.scope,
                    }),
                    json!({"status": "existing_episode_reused"}),
                    access.as_ref(),
                    None,
                    None,
                ),
                LogLevel::Debug,
            );
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
                None,
                None,
            ),
            LogLevel::Info,
        );

        Ok(episode_id)
    }

    /// Provides explanations for context items.
    /// Provides explanations for context items with batched graph insights.
    ///
    /// Phase 1: resolves episodes and facts for all items, collecting entity_links.
    /// Phase 2: computes shared graph insights once for the entire batch.
    /// Phase 3: builds individual explain items using cached provenance lookups.
    pub async fn explain(
        &self,
        request: ExplainRequest,
        access: Option<AccessPayload>,
    ) -> Result<Vec<ExplainItem>, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;

        // --- Phase 1: resolve episodes / facts, collect all entity_links ---
        struct ResolvedItem {
            item: ExplainItem,
            episode: Option<crate::models::Episode>,
            entity_links: Vec<String>,
            fact_namespace: Option<String>,
        }

        let mut resolved = Vec::with_capacity(request.context_pack.len());
        let mut all_entity_links: HashSet<String> = HashSet::new();

        for item in request.context_pack {
            if item.source_episode.is_empty() {
                return Err(MemoryError::Validation(
                    "source_episode is required for explain items".into(),
                ));
            }
            let (record, _) = self.find_episode_record(&item.source_episode).await?;
            let episode = record
                .as_ref()
                .and_then(super::episode::episode_from_record);

            let (entity_links, fact_namespace) = if let Some(ref fact_id) = item.fact_id {
                let (fact_record, namespace) = self.find_fact_record(fact_id).await?;
                let links = fact_record
                    .and_then(|r| {
                        r.get("entity_links").and_then(|v| v.as_array()).map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect::<Vec<_>>()
                        })
                    })
                    .unwrap_or_default();
                for link in &links {
                    all_entity_links.insert(link.clone());
                }
                (links, namespace)
            } else {
                (Vec::new(), None)
            };

            resolved.push(ResolvedItem {
                item,
                episode,
                entity_links,
                fact_namespace,
            });
        }

        // --- Phase 2: shared graph insights (computed once for the batch) ---
        let entity_links_vec: Vec<String> = all_entity_links.into_iter().collect();
        let first_namespace = resolved
            .iter()
            .find_map(|r| {
                r.fact_namespace.clone().or_else(|| {
                    r.episode
                        .as_ref()
                        .map(|ep| self.namespace_for_scope(&ep.scope))
                })
            })
            .unwrap_or_else(|| self.default_namespace.clone());
        let shared_insights = self
            .build_graph_insights_batched(&entity_links_vec, &first_namespace)
            .await?;

        // --- Phase 3: build explain items with cached provenance ---
        let mut episode_via_entity_cache: HashMap<String, Vec<crate::models::Episode>> =
            HashMap::new();
        let mut explanations = Vec::with_capacity(resolved.len());

        for resolved_item in resolved {
            // Track fact access regardless of whether the episode is found
            if let Some(ref fact_id) = resolved_item.item.fact_id
                && let Err(err) = self.record_fact_access(fact_id, 3).await
            {
                self.logger.log(
                    log_event(
                        "explain.access_track_error",
                        json!({"fact_id": fact_id}),
                        json!({"error": err.to_string()}),
                        access.as_ref(),
                        None,
                        None,
                    ),
                    LogLevel::Warn,
                );
            }

            let Some(episode) = resolved_item.episode else {
                explanations.push(resolved_item.item);
                continue;
            };

            let namespace = resolved_item
                .fact_namespace
                .unwrap_or_else(|| self.namespace_for_scope(&episode.scope));

            let all_sources = self
                .collect_provenance_sources_cached(
                    &episode,
                    &resolved_item.entity_links,
                    &namespace,
                    &mut episode_via_entity_cache,
                )
                .await?;

            let explanation = ExplainItem {
                fact_id: resolved_item.item.fact_id,
                content: if resolved_item.item.content.is_empty() {
                    episode.content.clone()
                } else {
                    resolved_item.item.content
                },
                quote: resolved_item.item.quote,
                source_episode: resolved_item.item.source_episode,
                scope: Some(episode.scope.clone()),
                t_ref: Some(episode.t_ref),
                t_ingested: Some(episode.t_ingested),
                provenance: json!({
                    "source_episode": episode.episode_id,
                    "source_type": episode.source_type,
                    "source_id": episode.source_id,
                }),
                citation_context: Some(episode.content.clone()),
                all_sources,
                graph_insights: shared_insights.clone(),
            };

            explanations.push(explanation);
        }

        self.logger.log(
            log_event(
                "explain",
                json!({"count": explanations.len()}),
                json!({"count": explanations.len()}),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Info,
        );

        Ok(explanations)
    }

    /// Extracts entities and facts from an episode.
    ///
    /// # Arguments
    ///
    /// * `episode_id` - The episode to extract from.
    /// * `access` - Optional access context for authorization.
    /// * `zero_shot_labels` - Optional custom entity labels for GLiNER extraction.
    ///   When provided, these labels override the default NER configuration.
    pub async fn extract(
        &self,
        episode_id: &str,
        access: Option<AccessPayload>,
        zero_shot_labels: Option<&[String]>,
    ) -> Result<ExtractResult, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        let timer = Instant::now(); // extract
        let (record, _) = self.find_episode_record(episode_id).await?;
        if record.is_none() {
            return Err(MemoryError::NotFound(format!(
                "episode_id not found: {episode_id}"
            )));
        }
        let episode = record.as_ref().and_then(super::episode_from_record);
        let payload =
            super::episode::extract_from_episode(self, episode_id, zero_shot_labels).await?;
        self.logger.log(
            log_event(
                "extract",
                log_args_with_duration(json!({"episode_id": episode_id}), timer.elapsed()),
                super::episode::build_extract_log_result(
                    episode.as_ref(),
                    payload.entities.len(),
                    &payload.facts,
                    payload.links.len(),
                    payload.warnings.len(),
                ),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Info,
        );
        Ok(payload)
    }

    /// Resolves an entity candidate.
    pub async fn resolve(
        &self,
        candidate: EntityCandidate,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        validate_entity_candidate(&candidate)?;
        let namespace = self.default_namespace.clone();
        let normalized = super::normalize_text(&candidate.canonical_name);

        // Check if entity already exists by name
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
            "canonical_name_normalized": normalized,
            "aliases": aliases.clone(),
        });

        // Attempt to create the entity. If it already exists (race condition),
        // fetch and return the existing entity ID.
        match self.db_client.create(&entity_id, payload, &namespace).await {
            Ok(_) => Ok(entity_id),
            Err(MemoryError::Storage(msg)) if msg.contains("already exists") => {
                // Race condition: another request created the entity concurrently.
                // Fetch and return the existing entity.
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
                // Fallback: return the deterministic ID even if we couldn't fetch
                Ok(entity_id)
            }
            Err(err) => Err(err),
        }
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
            let project = self.project_for_source_episode(source_episode).await?;
            let embedding_input = Self::build_fact_embedding_input(fact_type, content, quote);
            let index_keys = self
                .build_fact_index_keys(content, source_episode, &provenance, &entity_links, t_valid)
                .await?;
            let mut payload = serde_json::Map::from_iter([
                ("fact_id".to_string(), json!(fact_id.clone())),
                ("fact_type".to_string(), json!(fact_type)),
                ("content".to_string(), json!(content)),
                ("quote".to_string(), json!(quote)),
                ("source_episode".to_string(), json!(source_episode)),
                ("t_valid".to_string(), json!(super::normalize_dt(t_valid))),
                (
                    "t_ingested".to_string(),
                    json!(super::normalize_dt(t_ingested)),
                ),
                ("confidence".to_string(), json!(confidence)),
                ("index_keys".to_string(), json!(index_keys)),
                ("access_count".to_string(), json!(0)),
                ("entity_links".to_string(), json!(entity_links)),
                ("scope".to_string(), json!(scope)),
                ("policy_tags".to_string(), json!(policy_tags)),
                ("provenance".to_string(), provenance),
            ]);
            if let Some(project) = project {
                payload.insert("project".to_string(), json!(project));
            }

            let mut deferred_embedding_input = None;

            match self.generate_embedding(&embedding_input).await {
                Ok(Some(embedding)) => {
                    self.insert_current_embedding_fields(&mut payload, embedding)?;
                }
                Ok(None) => {}
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
                }
            }

            let created = self
                .db_client
                .create(&fact_id, Value::Object(payload), &namespace)
                .await?;
            if created.is_null() {
                return Err(MemoryError::Storage(
                    "failed to persist fact record".to_string(),
                ));
            }
            super::cache::invalidate_cache_by_scope(&self.context_cache, scope).await;
            if let Some(input) = deferred_embedding_input {
                self.enqueue_background_fact_embedding(namespace, fact_id.clone(), input)
                    .await;
            }
        }
        Ok(fact_id)
    }

    pub(crate) async fn record_fact_access(
        &self,
        fact_id: &str,
        boost: i64,
    ) -> Result<(), MemoryError> {
        let (record, namespace) = self.find_fact_record(fact_id).await?;
        let Some(namespace) = namespace else {
            return Ok(());
        };
        let Some(mut record) = record else {
            return Ok(());
        };

        let access_count = record
            .get("access_count")
            .and_then(json_i64)
            .unwrap_or(0)
            .saturating_add(boost);
        record.insert("access_count".to_string(), json!(access_count));
        record.insert(
            "last_accessed".to_string(),
            json!(super::normalize_dt(super::query::now())),
        );

        self.db_client
            .update(fact_id, Value::Object(record), &namespace)
            .await?;

        Ok(())
    }

    async fn build_fact_index_keys(
        &self,
        content: &str,
        source_episode: &str,
        provenance: &Value,
        entity_links: &[String],
        t_valid: DateTime<Utc>,
    ) -> Result<Vec<String>, MemoryError> {
        let mut keys = HashSet::new();

        for entity_id in entity_links {
            let Some(record) = self.find_entity_record_by_id(entity_id).await? else {
                continue;
            };
            let Some(map) = record.as_object() else {
                continue;
            };

            if let Some(name) = map.get("canonical_name").and_then(string_from_value) {
                let normalized = super::normalize_text(&name);
                if !normalized.is_empty() {
                    keys.insert(normalized);
                }
            }

            if let Some(aliases) = map.get("aliases").and_then(Value::as_array) {
                for alias in aliases.iter().filter_map(string_from_value) {
                    let normalized = super::normalize_text(&alias);
                    if !normalized.is_empty() {
                        keys.insert(normalized);
                    }
                }
            }
        }

        keys.extend(extract_temporal_index_keys(content, t_valid));
        keys.extend(reference_index_terms(content));
        for source_reference in self
            .collect_fact_source_references(source_episode, provenance)
            .await?
        {
            keys.extend(reference_index_terms(&source_reference));
        }

        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort();
        Ok(keys)
    }

    async fn collect_fact_source_references(
        &self,
        source_episode: &str,
        provenance: &Value,
    ) -> Result<Vec<String>, MemoryError> {
        let mut references = Vec::new();
        let mut seen = HashSet::new();

        if let Some(source_id) = provenance_source_id(provenance) {
            let normalized = super::normalize_text(&source_id);
            if !normalized.is_empty() && seen.insert(normalized) {
                references.push(source_id);
            }
        }

        let (episode_record, _) = self.find_episode_record(source_episode).await?;
        if let Some(source_id) = episode_record
            .as_ref()
            .and_then(|map| map.get("source_id"))
            .and_then(string_from_value)
        {
            let normalized = super::normalize_text(&source_id);
            if !normalized.is_empty() && seen.insert(normalized) {
                references.push(source_id);
            }
        }

        Ok(references)
    }

    pub(crate) async fn generate_embedding(
        &self,
        input: &str,
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        // Defensive truncation: no embedding model supports more than ~32000
        // characters (≈8000 tokens for English, smaller for many other
        // languages). Extremely long inputs cause remote APIs to return empty
        // responses (e.g. OpenRouter missing data[0].embedding).
        const MAX_EMBEDDING_INPUT_CHARS: usize = 8_000;
        let effective_input: String = if input.len() > MAX_EMBEDDING_INPUT_CHARS {
            let truncated: String = input.chars().take(MAX_EMBEDDING_INPUT_CHARS).collect();
            self.logger.log(
                log_event(
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
            let mut result = build_embedding_log_result(0, None);
            if let Some(map) = result.as_object_mut() {
                map.insert("status".to_string(), json!("disabled"));
            }
            self.logger.log(
                log_event(
                    "embedding.generate.skipped",
                    log_args_with_duration(args, timer.elapsed()),
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
                    log_event(
                        "embedding.generate.done",
                        log_args_with_duration(args, timer.elapsed()),
                        build_embedding_log_result(1, Some(embedding.len())),
                        None,
                        None,
                        None,
                    ),
                    LogLevel::Info,
                );
                Ok(Some(embedding))
            }
            Err(err) => {
                let mut result = build_embedding_log_result(0, None);
                if let Some(map) = result.as_object_mut() {
                    map.insert("error".to_string(), json!(err.to_string()));
                }
                self.logger.log(
                    log_event(
                        "embedding.generate.error",
                        log_args_with_duration(args, timer.elapsed()),
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
        super::is_transient_embedding_error(err)
            && super::is_remote_embedding_provider(self.embedding_provider.provider_name())
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

    async fn cached_query_embedding(&self, input: &str) -> Option<Vec<f64>> {
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
            super::CachedQueryEmbedding {
                embedding,
                expires_at: std::time::Instant::now() + super::query_embedding_cache_ttl(),
            },
        );
    }

    async fn background_embedding_task_inflight(&self, task_key: &str) -> bool {
        self.background_embedding_inflight
            .lock()
            .await
            .contains(task_key)
    }

    async fn try_reserve_background_embedding_task(&self, task_key: &str) -> bool {
        self.background_embedding_inflight
            .lock()
            .await
            .insert(task_key.to_string())
    }

    async fn release_background_embedding_task(&self, task_key: &str) {
        self.background_embedding_inflight
            .lock()
            .await
            .remove(task_key);
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

        let service = self.clone();
        tokio::spawn(async move {
            service
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

        let service = self.clone();
        tokio::spawn(async move {
            service
                .run_background_query_embedding_task(task_key, input)
                .await;
        });
    }

    async fn run_background_fact_embedding_task(
        &self,
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
        for attempt in 1..=super::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS {
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
                        && attempt < super::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS =>
                {
                    let delay = super::background_embedding_retry_delay(attempt);
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

    async fn run_background_query_embedding_task(&self, task_key: String, input: String) {
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
        for attempt in 1..=super::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS {
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
                        && attempt < super::DEFAULT_BACKGROUND_EMBEDDING_ATTEMPTS =>
                {
                    let delay = super::background_embedding_retry_delay(attempt);
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
            .and_then(string_from_value)
            .unwrap_or_else(|| namespace.to_string());
        self.insert_current_embedding_fields(&mut record, embedding)?;
        self.db_client
            .update(fact_id, Value::Object(record), namespace)
            .await?;
        super::cache::invalidate_cache_by_scope(&self.context_cache, &scope).await;
        Ok(())
    }

    /// Invalidates a fact.
    pub async fn invalidate(
        &self,
        request: InvalidateRequest,
        access: Option<AccessPayload>,
    ) -> Result<(), MemoryError> {
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
        super::cache::invalidate_cache_by_scope(&self.context_cache, &scope).await;
        Ok(())
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
        self.resolve_entity_by_type("person", name).await
    }

    /// Resolves a company entity.
    pub async fn resolve_company(&self, name: &str) -> Result<String, MemoryError> {
        self.resolve_entity_by_type("company", name).await
    }

    /// Resolves a location entity.
    pub async fn resolve_location(&self, name: &str) -> Result<String, MemoryError> {
        self.resolve_entity_by_type("location", name).await
    }

    /// Resolves a product entity.
    pub async fn resolve_product(&self, name: &str) -> Result<String, MemoryError> {
        self.resolve_entity_by_type("product", name).await
    }

    /// Resolves an event entity.
    pub async fn resolve_event(&self, name: &str) -> Result<String, MemoryError> {
        self.resolve_entity_by_type("event", name).await
    }

    /// Resolves a concept entity.
    pub async fn resolve_concept(&self, name: &str) -> Result<String, MemoryError> {
        self.resolve_entity_by_type("concept", name).await
    }

    /// Internal helper: resolves an entity by its type string and canonical name.
    async fn resolve_entity_by_type(
        &self,
        entity_type: &str,
        name: &str,
    ) -> Result<String, MemoryError> {
        self.resolve(
            EntityCandidate {
                entity_type: entity_type.to_string(),
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
        use crate::models::{Edge, EdgeOrigin};
        let edge = Edge {
            in_id: from_id.to_string(),
            relation: relation.to_string(),
            out_id: to_id.to_string(),
            origin: EdgeOrigin::Inferred,
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
        let mut visited = HashSet::from([target_id.clone()]);
        let mut next_hop: HashMap<String, String> = HashMap::new();
        let mut discovered_nodes = HashSet::new();
        let mut nodes_with_predecessors = HashSet::new();

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
                            && let (Some(in_id), Some(out_id)) = (
                                map.get("in").and_then(string_from_value),
                                map.get("out").and_then(string_from_value),
                            )
                            && visited.insert(in_id.clone())
                        {
                            next_hop.insert(in_id.clone(), out_id);
                            discovered_nodes.insert(in_id.clone());
                            nodes_with_predecessors.insert(node_id.clone());
                            next_frontier.push(in_id);
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

        let mut candidate_paths = discovered_nodes
            .into_iter()
            .filter(|node_id| !nodes_with_predecessors.contains(node_id))
            .filter_map(|start_id| build_intro_chain_from_start(&start_id, &target_id, &next_hop))
            .collect::<Vec<_>>();

        candidate_paths
            .sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));

        let Some(best_path) = candidate_paths.into_iter().next() else {
            return Ok(vec![]);
        };

        Ok(best_path)
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

    /// Returns the namespace for a given scope.
    ///
    /// Normalizes scope to lowercase for prefix matching.
    /// Returns default namespace for unknown scopes with a warning log.
    #[must_use]
    pub fn namespace_for_scope(&self, scope: &str) -> String {
        let (ns, fell_back) = resolve_namespace(&self.namespaces, &self.default_namespace, scope);
        if fell_back {
            let mut event = std::collections::HashMap::new();
            event.insert(
                "op".to_string(),
                serde_json::Value::String("scope.namespace_fallback".to_string()),
            );
            event.insert(
                "scope".to_string(),
                serde_json::Value::String(scope.to_string()),
            );
            event.insert(
                "resolved_namespace".to_string(),
                serde_json::Value::String(ns.clone()),
            );
            let dedup_key = format!("scope.namespace_fallback:{}", ns);
            self.logger.log_warn_dedup(event, &dedup_key, 10);
        }
        ns
    }

    pub(crate) async fn find_episode_record(
        &self,
        episode_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        self.find_record_by_id(episode_id).await
    }

    async fn project_for_source_episode(
        &self,
        source_episode: &str,
    ) -> Result<Option<String>, MemoryError> {
        let (record, _) = self.find_episode_record(source_episode).await?;
        Ok(record
            .as_ref()
            .and_then(|map| map.get("project"))
            .and_then(string_from_value))
    }

    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        self.find_record_by_id(fact_id).await
    }

    /// Scans all namespaces for a record by its ID, returning the payload and owning namespace.
    async fn find_record_by_id(
        &self,
        record_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(record_id, namespace).await?;
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

    async fn find_entity_record_by_id(
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

    pub(crate) fn enforce_rate_limit(
        &self,
        access: Option<&AccessPayload>,
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

    /// Computes graph insights once for a batch of entity links (reduced explain budget).
    async fn build_graph_insights_batched(
        &self,
        entity_links: &[String],
        namespace: &str,
    ) -> Result<Option<GraphInsights>, MemoryError> {
        let mut seen_linked_entities = HashSet::new();
        let linked_entities = entity_links
            .iter()
            .filter(|entity_id| entity_id.starts_with("entity:"))
            .filter(|entity_id| seen_linked_entities.insert((**entity_id).clone()))
            .take(MAX_GRAPH_INSIGHT_LINKED_ENTITIES)
            .cloned()
            .collect::<Vec<_>>();
        if linked_entities.is_empty() {
            self.logger.log(
                log_event(
                    "explain.graph_insights.skipped",
                    json!({"namespace": namespace}),
                    json!({"reason": "no_linked_entities"}),
                    None,
                    None,
                    None,
                ),
                LogLevel::Trace,
            );
            return Ok(None);
        }

        self.logger.log(
            log_event(
                "explain.graph_insights.start",
                json!({
                    "namespace": namespace,
                    "linked_entity_count": linked_entities.len(),
                }),
                json!({}),
                None,
                None,
                None,
            ),
            LogLevel::Debug,
        );

        let budget = super::apps::graph::GraphTraversalBudget::EXPLAIN;
        let cutoff = super::query::now();
        let hub_entities = super::apps::graph::find_hub_entities(
            self,
            namespace,
            cutoff,
            MAX_GRAPH_INSIGHT_HUBS,
            budget,
        )
        .await?
        .into_iter()
        .map(|hub| GraphHubEntity {
            entity_id: hub.entity_id,
            canonical_name: hub.canonical_name,
            degree: hub.degree,
        })
        .collect::<Vec<_>>();

        let mut surprising_connections = Vec::new();
        let mut seen_connections = HashSet::new();

        for entity_id in linked_entities {
            for connection in super::apps::graph::find_surprising_connections(
                self, namespace, &entity_id, 3, budget,
            )
            .await?
            {
                let key = format!(
                    "{}->{}",
                    connection.source_entity_id, connection.target_entity_id
                );
                if seen_connections.insert(key) {
                    surprising_connections.push(connection);
                }
                if surprising_connections.len() >= MAX_GRAPH_INSIGHT_CONNECTIONS {
                    break;
                }
            }

            if surprising_connections.len() >= MAX_GRAPH_INSIGHT_CONNECTIONS {
                break;
            }
        }

        surprising_connections.sort_by(|left, right| {
            left.hop_count
                .cmp(&right.hop_count)
                .then_with(|| left.target_entity_name.cmp(&right.target_entity_name))
                .then_with(|| left.target_entity_id.cmp(&right.target_entity_id))
        });

        self.logger.log(
            log_event(
                "explain.graph_insights.done",
                json!({"namespace": namespace}),
                json!({
                    "hub_entities": hub_entities.len(),
                    "surprising_connections": surprising_connections.len(),
                }),
                None,
                None,
                None,
            ),
            LogLevel::Trace,
        );

        Ok(Some(GraphInsights {
            hub_entities,
            surprising_connections,
        }))
    }

    /// Collects provenance sources for an explain item, using an episode-via-entity cache
    /// to avoid redundant `find_episodes_via_entity` calls for the same entity across items.
    async fn collect_provenance_sources_cached(
        &self,
        primary_episode: &crate::models::Episode,
        entity_links: &[String],
        namespace: &str,
        cache: &mut HashMap<String, Vec<crate::models::Episode>>,
    ) -> Result<Vec<ProvenanceSource>, MemoryError> {
        let mut sources = Vec::new();

        // 1. Add direct source episode
        sources.push(ProvenanceSource {
            episode_id: primary_episode.episode_id.clone(),
            episode_content: primary_episode.content.clone(),
            episode_t_ref: crate::service::normalize_dt(primary_episode.t_ref),
            relationship: "direct".to_string(),
            entity_path: None,
        });

        // 2. Traverse entity_links to find connected episodes (cache-aware)
        for entity_id in entity_links {
            let linked_episodes = if let Some(cached) = cache.get(entity_id) {
                cached.clone()
            } else {
                let episodes = self.find_episodes_via_entity(entity_id, namespace).await?;
                cache.insert(entity_id.clone(), episodes.clone());
                episodes
            };

            for ep in linked_episodes {
                // Skip if this is the primary source (already added)
                if ep.episode_id == primary_episode.episode_id {
                    continue;
                }

                sources.push(ProvenanceSource {
                    episode_id: ep.episode_id.clone(),
                    episode_content: ep.content.clone(),
                    episode_t_ref: crate::service::normalize_dt(ep.t_ref),
                    relationship: "linked".to_string(),
                    entity_path: Some(format!("{} -> {}", primary_episode.episode_id, entity_id)),
                });
            }
        }

        // Sort: direct first, then by t_ref descending
        sources.sort_by(|a, b| {
            if a.relationship == "direct" {
                std::cmp::Ordering::Less
            } else if b.relationship == "direct" {
                std::cmp::Ordering::Greater
            } else {
                b.episode_t_ref.cmp(&a.episode_t_ref)
            }
        });

        Ok(sources)
    }

    /// Finds all episodes that mention or are linked to an entity.
    async fn find_episodes_via_entity(
        &self,
        entity_id: &str,
        namespace: &str,
    ) -> Result<Vec<crate::models::Episode>, MemoryError> {
        // Traverse: entity -> involved_in edge -> fact -> source_episode -> episode
        // Note: edge.out is a RecordId, so we cast to string for fact_id comparison.
        // source_episode stores episode_id values, matched against episode.episode_id.
        let sql = "SELECT * FROM episode WHERE episode_id IN (SELECT VALUE source_episode FROM fact WHERE fact_id IN (SELECT VALUE type::string(out) FROM edge WHERE in = <record> $entity_id AND relation = 'involved_in')) ORDER BY t_ref DESC LIMIT 10";
        let result = self
            .db_client
            .query(sql, Some(json!({"entity_id": entity_id})), namespace)
            .await?;

        let episodes: Vec<crate::models::Episode> = result
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let obj = v.as_object()?;
                        super::episode::episode_from_record(obj)
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(episodes)
    }
}

fn provenance_source_id(provenance: &Value) -> Option<String> {
    provenance
        .as_object()
        .and_then(|map| map.get("source_id"))
        .and_then(string_from_value)
}

fn reference_index_terms(raw: &str) -> Vec<String> {
    let query_terms = crate::service::query::search_query_terms(raw);
    let mut keys = crate::service::query::query_hard_anchor_terms(&query_terms)
        .into_iter()
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

/// Resolves a scope string to a namespace, using prefix matching against
/// available namespaces. Returns `(namespace, fell_back)` where `fell_back`
/// is true when the default was used for an unknown scope.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_EMBEDDING_DIMENSION;
    use crate::models::EntityCandidate;
    use crate::models::{AccessPayload, AccessScopeAllow};
    use crate::service::EmbeddingProvider;
    use crate::service::startup::{apply_startup_migrations, build_startup_versions_event};
    use crate::service::util::rate_limit::SafeMutex;
    use crate::storage::{DbClient, SurrealDbClient};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn log_event_creates_expected_structure() {
        let event = log_event(
            "test_op",
            json!({"key": "value"}),
            json!({"result": "ok"}),
            None,
            None,
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
        let access = AccessPayload {
            caller_id: Some("test-caller".to_string()),
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        let event = log_event("test_op", json!({}), json!({}), Some(&access), None, None);
        let access_event = event.get("access").unwrap();
        assert_eq!(
            access_event.get("caller_id").unwrap().as_str(),
            Some("test-caller")
        );
    }

    #[test]
    fn serialize_access_includes_all_fields() {
        let access = AccessPayload {
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
    fn string_from_value_handles_record_id() {
        let value = json!({"RecordId": {"table": "entity", "key": "alice"}});
        assert_eq!(string_from_value(&value), Some("entity:alice".to_string()));
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
    fn build_fact_embedding_input_formats_correctly() {
        let result = MemoryService::build_fact_embedding_input("note", "Hello world", "Hello!");
        assert_eq!(result, "note\nHello world\nHello!");
    }

    #[test]
    fn build_fact_embedding_input_handles_empty_parts() {
        let result = MemoryService::build_fact_embedding_input("", "", "");
        assert_eq!(result, "\n\n");
    }

    /// Verifies that truncation is inside `generate_embedding` by checking
    /// that build_fact_embedding_input + generate_embedding together won't
    /// pass a 60k+ input to the provider. The truncation limit is 8,000 chars.
    /// This test is deliberately lightweight — the full reembed pipeline
    /// with long content is exercised in `reembed_long_fact_content_does_not_fail`.
    #[test]
    fn generate_embedding_input_builder_respects_truncation_limit() {
        // Simulate what build_fact_embedding_input produces for a very long
        // fact, then verify the truncation would apply.
        let long_content = "x".repeat(60_000);
        let full_input =
            MemoryService::build_fact_embedding_input("note", &long_content, &long_content);
        // The input + overhead is > 8,000, so generate_embedding will truncate
        assert!(full_input.len() > 8_000);
        // Truncation should produce at most 8,000 chars
        let truncated: String = full_input.chars().take(8_000).collect();
        assert_eq!(truncated.chars().count(), 8_000);
    }

    #[tokio::test]
    async fn apply_startup_migrations_runs_for_every_namespace() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StartupMigrationDbClient {
            calls: Arc<Mutex<Vec<String>>>,
            apply_count: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl DbClient for StartupMigrationDbClient {
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

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
                self.apply_count.fetch_add(1, Ordering::SeqCst);
                self.calls.safe_lock().push(namespace.to_string());
                Ok(())
            }
        }

        let db_client = Arc::new(StartupMigrationDbClient {
            calls: Arc::new(Mutex::new(Vec::new())),
            apply_count: AtomicUsize::new(0),
        });
        let db_client_dyn: Arc<dyn DbClient> = db_client.clone();
        let namespaces = vec![
            "org".to_string(),
            "personal".to_string(),
            "private".to_string(),
        ];

        apply_startup_migrations(&db_client_dyn, &namespaces)
            .await
            .expect("startup migrations");

        assert_eq!(db_client.apply_count.load(Ordering::SeqCst), 3);
        assert_eq!(
            db_client.calls.safe_lock().clone(),
            vec![
                "org".to_string(),
                "personal".to_string(),
                "private".to_string(),
            ]
        );
    }

    #[test]
    fn bfs_path_returns_single_element_for_same_node() {
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), vec![]);

        let path = bfs_path(&graph, "A", "A", 5);
        assert_eq!(path, Some(vec!["A".to_string()]));
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

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

    fn create_test_service_with_rate_limit(rps: i32, burst: i32) -> MemoryService {
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

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        MemoryService::new(
            Arc::new(MockDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            rps,
            burst,
        )
        .unwrap()
    }

    #[test]
    fn is_scope_allowed_returns_true_when_no_restrictions() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload::default();
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_returns_true_for_allowed_scope() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_returns_false_for_disallowed_scope() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec!["personal".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        assert!(!access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_allows_with_cross_scope_wildcard() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload {
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
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn enforce_rate_limit_allows_without_caller_id() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload::default();
        assert!(service.enforce_rate_limit(Some(&access)).is_ok());
    }

    #[test]
    fn enforce_rate_limit_allows_within_limit() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload {
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
                        vec![json!({"in": "entity:bob", "out": "entity:openai"})]
                    }
                    "entity:bob" => vec![json!({"in": "entity:alice", "out": "entity:bob"})],
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

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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
                        vec![json!({"in": "entity:bob", "out": "entity:openai"})]
                    }
                    "entity:bob" => vec![json!({"in": "entity:alice", "out": "entity:bob"})],
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

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

    #[tokio::test]
    async fn find_intro_chain_prefers_shortest_path_over_lexicographic_candidate() {
        use std::sync::Arc;

        struct ShortestPathDbClient;

        #[async_trait::async_trait]
        impl DbClient for ShortestPathDbClient {
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
                    "entity:openai" => vec![
                        json!({"in": "entity:bob", "out": "entity:openai"}),
                        json!({"in": "entity:carol", "out": "entity:openai"}),
                    ],
                    "entity:bob" => vec![json!({"in": "entity:alice", "out": "entity:bob"})],
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

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = MemoryService::new(
            Arc::new(ShortestPathDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let chain = service.find_intro_chain("OpenAI", 3, None).await.unwrap();

        assert_eq!(
            chain,
            vec!["entity:carol".to_string(), "entity:openai".to_string()],
            "the shortest discovered introduction path should win even if a longer path starts with a lexicographically earlier id"
        );
    }

    #[tokio::test]
    async fn find_intro_chain_prefers_shortest_path_in_multi_hop_diamond() {
        use std::sync::Arc;

        struct DiamondTraversalDbClient;

        #[async_trait::async_trait]
        impl DbClient for DiamondTraversalDbClient {
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
                    "entity:openai" => vec![
                        json!({"in": "entity:bob", "out": "entity:openai"}),
                        json!({"in": "entity:carol", "out": "entity:openai"}),
                    ],
                    "entity:bob" => vec![json!({"in": "entity:alice", "out": "entity:bob"})],
                    "entity:carol" => vec![json!({"in": "entity:diana", "out": "entity:carol"})],
                    "entity:alice" => vec![json!({"in": "entity:erin", "out": "entity:alice"})],
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

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
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

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = MemoryService::new(
            Arc::new(DiamondTraversalDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let chain = service.find_intro_chain("OpenAI", 4, None).await.unwrap();

        assert_eq!(
            chain,
            vec![
                "entity:diana".to_string(),
                "entity:carol".to_string(),
                "entity:openai".to_string(),
            ],
            "the traversal should keep the shorter diamond branch instead of returning the deeper alternative"
        );
    }

    struct StaticTestEmbeddingProvider {
        salary_vector: Vec<f64>,
        neutral_vector: Vec<f64>,
    }

    impl StaticTestEmbeddingProvider {
        fn new() -> Self {
            let mut salary_vector = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
            salary_vector[0] = 1.0;
            let mut neutral_vector = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
            neutral_vector[1] = 1.0;
            Self {
                salary_vector,
                neutral_vector,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for StaticTestEmbeddingProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "test"
        }

        fn dimension(&self) -> usize {
            DEFAULT_EMBEDDING_DIMENSION
        }

        async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
            let normalized = input.to_ascii_lowercase();
            if normalized.contains("salary raise") || normalized.contains("compensation increase") {
                return Ok(self.salary_vector.clone());
            }

            Ok(self.neutral_vector.clone())
        }
    }

    struct FlakyRemoteTestEmbeddingProvider {
        remaining_failures: AtomicUsize,
        embedding: Vec<f64>,
    }

    impl FlakyRemoteTestEmbeddingProvider {
        fn new(failures_before_success: usize) -> Self {
            let mut embedding = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
            embedding[0] = 1.0;
            Self {
                remaining_failures: AtomicUsize::new(failures_before_success),
                embedding,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FlakyRemoteTestEmbeddingProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "openai-compatible"
        }

        fn dimension(&self) -> usize {
            DEFAULT_EMBEDDING_DIMENSION
        }

        async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
            let remaining = self.remaining_failures.load(Ordering::SeqCst);
            if remaining > 0 {
                self.remaining_failures.fetch_sub(1, Ordering::SeqCst);
                return Err(MemoryError::Transient(
                    "synthetic remote embedding outage".to_string(),
                ));
            }

            Ok(self.embedding.clone())
        }
    }

    #[tokio::test]
    async fn add_fact_persists_embedding_when_provider_enabled() {
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "testdb",
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db_client.apply_migrations("org").await.expect("migrations");

        let service = MemoryService::new_with_embedding_provider(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
            Arc::new(StaticTestEmbeddingProvider::new()),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        )
        .expect("service");

        let fact_id = service
            .add_fact(
                "note",
                "Compensation increase approved for engineering",
                "Compensation increase approved",
                "episode:test",
                Utc::now(),
                "org",
                0.9,
                vec![],
                vec![],
                json!({"source_episode": "episode:test"}),
            )
            .await
            .expect("add fact");

        let fact = db_client
            .select_one(&fact_id, "org")
            .await
            .expect("select fact")
            .expect("stored fact");

        assert_eq!(
            fact.get("embedding")
                .and_then(|value| value.as_array())
                .map(Vec::len),
            Some(DEFAULT_EMBEDDING_DIMENSION)
        );
    }

    #[tokio::test]
    async fn add_fact_defers_background_embedding_after_transient_remote_failure() {
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "testdb_background_fact_embedding",
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db_client.apply_migrations("org").await.expect("migrations");

        let mut service = MemoryService::new_with_embedding_provider(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
            Arc::new(FlakyRemoteTestEmbeddingProvider::new(1)),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        )
        .expect("service");
        service.current_embedding_signature = Some("embsig:background-test".to_string());
        service.current_embedding_model = Some("test-model".to_string());
        service.current_embedding_dimension = Some(DEFAULT_EMBEDDING_DIMENSION);

        let fact_id = service
            .add_fact(
                "note",
                "Provider outage should not block fact creation",
                "Provider outage should not block fact creation",
                "episode:test",
                Utc::now(),
                "org",
                0.9,
                vec![],
                vec![],
                json!({"source_episode": "episode:test"}),
            )
            .await
            .expect("add fact");

        let initial = db_client
            .select_one(&fact_id, "org")
            .await
            .expect("select fact")
            .expect("stored fact");
        assert!(initial.get("embedding").is_none());

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let fact = db_client
                    .select_one(&fact_id, "org")
                    .await
                    .expect("select fact")
                    .expect("stored fact");
                if fact.get("embedding_signature").and_then(Value::as_str)
                    == Some("embsig:background-test")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("background embedding should complete");
    }

    #[tokio::test]
    async fn generate_query_embedding_uses_background_cache_after_transient_remote_failure() {
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "testdb_background_query_embedding",
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db_client.apply_migrations("org").await.expect("migrations");

        let mut service = MemoryService::new_with_embedding_provider(
            db_client,
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
            Arc::new(FlakyRemoteTestEmbeddingProvider::new(1)),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        )
        .expect("service");
        service.current_embedding_signature = Some("embsig:background-query-test".to_string());
        service.current_embedding_model = Some("test-model".to_string());
        service.current_embedding_dimension = Some(DEFAULT_EMBEDDING_DIMENSION);

        let first = service
            .generate_query_embedding_with_background("salary raise")
            .await
            .expect("transient failure should degrade to background mode");
        assert!(first.is_none());

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if service
                    .cached_query_embedding("salary raise")
                    .await
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("background query embedding should populate cache");

        let second = service
            .generate_query_embedding_with_background("salary raise")
            .await
            .expect("cached embedding");
        assert_eq!(
            second.as_ref().map(Vec::len),
            Some(DEFAULT_EMBEDDING_DIMENSION)
        );
    }

    #[test]
    fn log_event_with_full_access_context() {
        let access = AccessPayload {
            caller_id: Some("test-user".to_string()),
            allowed_scopes: Some(vec!["personal".to_string(), "org".to_string()]),
            allowed_tags: Some(vec!["tag1".to_string()]),
            session_vars: Some(json!({"session": "value"})),
            transport: Some("grpc".to_string()),
            content_type: Some("application/grpc".to_string()),
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "personal".to_string(),
                to: "org".to_string(),
            }]),
        };
        let event = log_event("test_op", json!({}), json!({}), Some(&access), None, None);
        let access_val = event.get("access").unwrap();
        assert_eq!(
            access_val.get("caller_id").unwrap().as_str(),
            Some("test-user")
        );
        assert_eq!(
            access_val
                .get("allowed_scopes")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(access_val.get("transport").unwrap().as_str(), Some("grpc"));
        assert_eq!(
            access_val.get("content_type").unwrap().as_str(),
            Some("application/grpc")
        );
    }

    #[test]
    fn log_event_without_access_context_omits_access_field() {
        let event = log_event("test_op", json!({}), json!({}), None, None, None);
        assert!(!event.contains_key("access"));
    }

    #[test]
    fn log_args_with_duration_adds_duration_ms_field() {
        let args = log_args_with_duration(
            json!({"scope": "org"}),
            std::time::Duration::from_millis(42),
        );

        assert_eq!(args.get("scope").and_then(Value::as_str), Some("org"));
        assert_eq!(args.get("duration_ms").and_then(Value::as_u64), Some(42));
    }

    #[test]
    fn build_embedding_log_result_reports_generated_count_and_dimension() {
        let result = build_embedding_log_result(1, Some(384));

        assert_eq!(
            result.get("generated_embeddings").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(result.get("dimension").and_then(Value::as_u64), Some(384));
    }

    #[test]
    fn serialize_access_with_all_none_fields() {
        let access = AccessPayload {
            caller_id: None,
            allowed_scopes: None,
            allowed_tags: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
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
    fn string_from_value_handles_object_without_expected_keys() {
        let value = json!({"Other": "value"});
        assert_eq!(string_from_value(&value), None);
    }

    #[test]
    fn string_from_value_handles_record_id_missing_fields() {
        let value = json!({"RecordId": {"table": "entity"}});
        assert_eq!(string_from_value(&value), None);
    }

    #[test]
    fn namespace_for_scope_handles_various_inputs() {
        let service = create_test_service(vec!["org", "personal", "private"]);

        assert_eq!(service.namespace_for_scope("org"), "org");
        assert_eq!(service.namespace_for_scope("personal"), "personal");
        assert_eq!(service.namespace_for_scope("private"), "private");
        assert_eq!(service.namespace_for_scope("unknown"), "org");
        assert_eq!(service.namespace_for_scope(""), "org");
        assert_eq!(service.namespace_for_scope("ORG"), "org");
    }

    #[test]
    fn is_scope_allowed_with_empty_allowed_scopes() {
        let service = create_test_service(vec!["org"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec![]),
            ..Default::default()
        };
        assert!(!access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_with_multiple_allowed_scopes() {
        let service = create_test_service(vec!["org", "personal"]);
        let access = AccessPayload {
            allowed_scopes: Some(vec!["org".to_string(), "personal".to_string()]),
            ..Default::default()
        };
        assert!(access.is_scope_allowed("org"));
        assert!(access.is_scope_allowed("personal"));
        assert!(!access.is_scope_allowed("private"));
    }

    #[test]
    fn enforce_rate_limit_with_burst_capacity() {
        let service = create_test_service_with_rate_limit(10, 5);
        let access = AccessPayload {
            caller_id: Some("burst-test".to_string()),
            ..Default::default()
        };

        for _ in 0..5 {
            assert!(service.enforce_rate_limit(Some(&access)).is_ok());
        }
    }

    #[test]
    fn enforce_rate_limit_multiple_users_isolated() {
        let service = create_test_service_with_rate_limit(10, 1);

        let user1 = AccessPayload {
            caller_id: Some("user-1".to_string()),
            ..Default::default()
        };
        let user2 = AccessPayload {
            caller_id: Some("user-2".to_string()),
            ..Default::default()
        };

        assert!(service.enforce_rate_limit(Some(&user1)).is_ok());
        assert!(service.enforce_rate_limit(Some(&user1)).is_err());

        assert!(service.enforce_rate_limit(Some(&user2)).is_ok());
    }

    #[test]
    fn build_intro_chain_from_start_builds_correct_path() {
        let mut next_hop = HashMap::new();
        next_hop.insert("A".to_string(), "B".to_string());
        next_hop.insert("B".to_string(), "C".to_string());
        next_hop.insert("C".to_string(), "D".to_string());

        let path = build_intro_chain_from_start("A", "D", &next_hop);
        assert_eq!(
            path,
            Some(vec![
                "A".to_string(),
                "B".to_string(),
                "C".to_string(),
                "D".to_string()
            ])
        );
    }

    #[test]
    fn build_intro_chain_from_start_returns_none_for_unreachable() {
        let mut next_hop = HashMap::new();
        next_hop.insert("A".to_string(), "B".to_string());
        next_hop.insert("B".to_string(), "C".to_string());

        let path = build_intro_chain_from_start("A", "Z", &next_hop);
        assert_eq!(path, None);
    }

    #[test]
    fn build_intro_chain_from_start_handles_direct_connection() {
        let mut next_hop = HashMap::new();
        next_hop.insert("A".to_string(), "B".to_string());

        let path = build_intro_chain_from_start("A", "B", &next_hop);
        assert_eq!(path, Some(vec!["A".to_string(), "B".to_string()]));
    }

    #[test]
    fn bfs_path_handles_empty_graph() {
        let graph: HashMap<String, Vec<String>> = HashMap::new();
        let path = bfs_path(&graph, "A", "B", 5);
        assert_eq!(path, None);
    }

    #[test]
    fn bfs_path_finds_shortest_path_in_complex_graph() {
        let mut graph = HashMap::new();
        graph.insert("A".to_string(), vec!["B".to_string(), "C".to_string()]);
        graph.insert("B".to_string(), vec!["D".to_string()]);
        graph.insert("C".to_string(), vec!["D".to_string(), "E".to_string()]);
        graph.insert("D".to_string(), vec!["F".to_string()]);
        graph.insert("E".to_string(), vec!["F".to_string()]);
        graph.insert("F".to_string(), vec![]);

        let path = bfs_path(&graph, "A", "F", 10);
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.len() <= 4);
    }

    #[tokio::test]
    async fn resolve_entity_by_type_delegates_to_resolve() {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "resolve_entity_test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in-memory test db"),
        );
        let service = MemoryService::new(db_client, namespaces, "warn".to_string(), 50, 100)
            .expect("create test service");

        // Resolve the same entity via different typed methods
        let id1 = service
            .resolve_person("Alice Smith")
            .await
            .expect("resolve person");
        let id2 = service
            .resolve_person("Alice Smith")
            .await
            .expect("resolve person again");
        assert_eq!(id1, id2);

        let id3 = service
            .resolve_company("Acme Corp")
            .await
            .expect("resolve company");
        assert_ne!(id1, id3);
    }

    #[tokio::test]
    async fn relate_creates_edge_between_entities() {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces("relate_test", &namespaces, "warn")
                .await
                .expect("connect in-memory test db"),
        );
        let service = MemoryService::new(db_client, namespaces, "warn".to_string(), 50, 100)
            .expect("create test service");

        let from_id = service
            .resolve_person("Alice Relate")
            .await
            .expect("resolve alice");
        let to_id = service
            .resolve_company("Acme Relate")
            .await
            .expect("resolve acme");

        service
            .relate(&from_id, "works_at", &to_id)
            .await
            .expect("relate entities");
    }

    #[tokio::test]
    async fn get_surrealdb_config_returns_namespaces() {
        let namespaces = vec!["org".to_string(), "personal".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces("config_test", &namespaces, "warn")
                .await
                .expect("connect in-memory test db"),
        );
        let service =
            MemoryService::new(db_client, namespaces.clone(), "warn".to_string(), 50, 100)
                .expect("create test service");

        let config = service.get_surrealdb_config().await.expect("get config");
        let config_namespaces = config["namespaces"].as_array().expect("namespaces array");
        assert_eq!(config_namespaces.len(), 2);
    }

    #[tokio::test]
    async fn episode_count_returns_zero_for_empty_db() {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "episode_count_test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in-memory test db"),
        );
        for ns in &namespaces {
            db_client
                .apply_migrations(ns)
                .await
                .expect("apply migrations");
        }
        let service = MemoryService::new(db_client, namespaces, "warn".to_string(), 50, 100)
            .expect("create test service");

        let count = service.episode_count().await.expect("count episodes");
        assert_eq!(count, 0);
    }
}
