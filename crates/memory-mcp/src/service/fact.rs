//! Fact persistence service — handles fact record creation, validation, and index keys.
//!
//! Reduces the God Object.
//! Embedding generation, triple extraction, and claim projection remain
//! orchestrated by `MemoryService` — this service handles only the core
//! fact record lifecycle.

use std::collections::HashSet;
#[cfg(test)]
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{FactId, Provenance};
use crate::service::cache::invalidate_cache;
use crate::service::error::MemoryError;
use crate::service::util::{deterministic_fact_id, validate_fact_input};
#[cfg(test)]
use crate::storage::FactStoreClient;

use super::{normalize_dt, normalize_text, now};

/// Embedding fields to persist with a fact record.
/// Built by `MemoryService` from its embedding provider state and passed
/// to `FactService::create_fact`.
pub(crate) struct EmbeddingPayload {
    pub embedding: Vec<f64>,
    pub provider: String,
    pub model: Option<String>,
    pub dimension: usize,
    pub signature: Option<String>,
    pub updated_at: String,
}

/// Handles fact record CRUD: validation, ID generation, index key building, and persistence.
#[derive(Clone)]
pub struct FactService {
    db: crate::storage::FactStoreClient,
}

impl FactService {
    pub fn new(db_client: crate::storage::FactStoreClient) -> Self {
        Self { db: db_client }
    }

    /// Creates a new fact record if it does not already exist.
    ///
    /// Returns the fact ID. If the fact already exists (same deterministic ID),
    /// returns the existing ID without re-writing.
    ///
    /// The caller is responsible for embedding generation, triple extraction,
    /// claim projection, and cache invalidation — this method handles only
    /// the core persistence path.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_fact(
        &self,
        fact_type: &str,
        content: &str,
        quote: &str,
        source_episode: &str,
        t_valid: DateTime<Utc>,
        confidence: f64,
        entity_links: &[String],
        policy_tags: &[String],
        provenance: &Provenance,
        embedding_fields: Option<EmbeddingPayload>,
        index_keys: Vec<String>,
    ) -> Result<String, MemoryError> {
        validate_fact_input(fact_type, content, quote, source_episode, "")?;

        let fact_id = deterministic_fact_id(fact_type, content, source_episode, t_valid);
        let existing = self.db.select_one(&fact_id).await?;
        if existing.is_some() {
            return Ok(fact_id);
        }

        let t_ingested = now();
        let mut payload = serde_json::Map::from_iter([
            ("fact_id".to_string(), json!(fact_id.clone())),
            ("fact_type".to_string(), json!(fact_type)),
            ("content".to_string(), json!(content)),
            ("quote".to_string(), json!(quote)),
            ("source_episode".to_string(), json!(source_episode)),
            ("t_valid".to_string(), json!(normalize_dt(t_valid))),
            ("t_ingested".to_string(), json!(normalize_dt(t_ingested))),
            ("confidence".to_string(), json!(confidence)),
            ("index_keys".to_string(), json!(index_keys)),
            ("access_count".to_string(), json!(0)),
            ("entity_links".to_string(), json!(entity_links)),
            ("policy_tags".to_string(), json!(policy_tags)),
            ("provenance".to_string(), provenance.to_json_value()),
        ]);
        if let Some(ep) = embedding_fields {
            payload.insert("embedding".to_string(), json!(ep.embedding));
            payload.insert("embedding_provider".to_string(), json!(ep.provider));
            if let Some(model) = ep.model {
                payload.insert("embedding_model".to_string(), json!(model));
            }
            payload.insert("embedding_dimension".to_string(), json!(ep.dimension));
            if let Some(signature) = ep.signature {
                payload.insert("embedding_signature".to_string(), json!(signature));
            }
            payload.insert("embedding_updated_at".to_string(), json!(ep.updated_at));
        }

        let created = self.db.create(&fact_id, Value::Object(payload)).await?;
        if created.is_null() {
            return Err(MemoryError::Storage(
                "failed to persist fact record".to_string(),
            ));
        }
        Ok(fact_id)
    }

    /// Builds the search index keys for a fact from entity links, temporal markers,
    /// and source references.
    ///
    /// `entity_lookup` is a closure that resolves an entity_id to its canonical
    /// name and aliases. This avoids a hard dependency on EntityService.
    #[allow(clippy::too_many_arguments)]
    pub async fn build_index_keys(
        &self,
        content: &str,
        source_episode: &str,
        provenance: &Provenance,
        entity_links: &[String],
        t_valid: DateTime<Utc>,
        entity_lookup: impl Fn(&str) -> Result<Option<(String, Vec<String>)>, MemoryError>,
        source_reference_lookup: impl Fn(&str) -> Result<Option<String>, MemoryError>,
    ) -> Result<Vec<String>, MemoryError> {
        let mut keys = HashSet::new();

        for entity_id in entity_links {
            if let Some((canonical, aliases)) = entity_lookup(entity_id)? {
                let normalized = normalize_text(&canonical);
                if !normalized.is_empty() {
                    keys.insert(normalized);
                }
                for alias in &aliases {
                    let normalized = normalize_text(alias);
                    if !normalized.is_empty() {
                        keys.insert(normalized);
                    }
                }
            }
        }

        keys.extend(crate::service::core::extract_temporal_index_keys(
            content, t_valid,
        ));
        keys.extend(reference_index_terms(content));

        // Collect source references
        let mut seen = HashSet::new();
        if let Some(source_id) = &provenance.source_id {
            let normalized = normalize_text(source_id);
            if !normalized.is_empty() && seen.insert(normalized.clone()) {
                keys.extend(reference_index_terms(source_id));
            }
        }
        if let Some(episode_source_id) = source_reference_lookup(source_episode)? {
            let normalized = normalize_text(&episode_source_id);
            if !normalized.is_empty() && seen.insert(normalized) {
                keys.extend(reference_index_terms(&episode_source_id));
            }
        }

        let mut keys: Vec<_> = keys.into_iter().collect();
        keys.sort();
        Ok(keys)
    }
}

// ─── Fact creation orchestration ───────────────────────────────────────────
//
// `add_fact` orchestrates the full fact-creation pipeline: validation,
// index-key building, embedding generation (with transient-failure background
// retry), fact persistence, cache invalidation, triple extraction, and claim
// projection. It lives on `FactService` but takes `&ServiceContext` as the seam
// that bundles the infrastructure handles (embedding service, entity lookups,
// claim service, logger, and the bound Active Namespace) it needs.

impl FactService {
    /// Adds a new fact, orchestrating embedding generation, triple extraction,
    /// and claim projection.
    ///
    /// This is the full fact-creation entry point. The persistence core is
    /// delegated to [`FactService::create_fact`]; this method handles the
    /// surrounding orchestration previously held on `ServiceContext`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn add_fact(
        &self,
        ctx: &crate::service::service_context::ServiceContext,
        fact_type: &str,
        content: &str,
        quote: &str,
        source_episode: &str,
        t_valid: DateTime<Utc>,
        confidence: f64,
        entity_links: Vec<String>,
        policy_tags: Vec<String>,
        provenance: Provenance,
    ) -> Result<String, MemoryError> {
        validate_fact_input(fact_type, content, quote, source_episode, "")?;

        // Storage routing comes from the process-bound stores in the context;
        // this value is retained only for derived job/diagnostic metadata.
        let namespace = ctx.active_namespace.clone();

        // Pre-fetch linked entity records in a single batch query per namespace,
        // avoiding O(N) round-trips. Returns None for missing IDs.
        let entity_map: std::collections::HashMap<String, (String, Vec<String>)> =
            ctx.find_entity_records_by_ids(&entity_links).await?;

        // Pre-fetch episode source_id; also serves as the ADR-0008 source
        // lineage for claim projection (the connector's stable record identifier).
        let (episode_record, _) = ctx.find_episode_record(source_episode).await?;
        let episode_source_id = episode_record
            .as_ref()
            .and_then(|map| map.get("source_id"))
            .and_then(crate::service::value_helpers::string_from_value);

        let entity_lookup = |entity_id: &str| Ok(entity_map.get(entity_id).cloned());
        let source_reference_lookup = |_episode_id: &str| Ok(episode_source_id.clone());

        let index_keys = self
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
        let embedding_fields = match ctx
            .embedding_service
            .generate_embedding(&embedding_input)
            .await
        {
            Ok(Some(embedding)) => {
                ctx.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("embedding.generate.success")),
                        (
                            "provider".to_string(),
                            json!(ctx.embedding_service.embedding_provider().provider_name()),
                        ),
                        ("fact_type".to_string(), json!(fact_type)),
                    ]),
                    LogLevel::Info,
                );
                Some(self.build_embedding_payload(ctx, embedding)?)
            }
            Ok(None) => None,
            Err(err) => {
                ctx.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("embedding.write_skipped")),
                        (
                            "provider".to_string(),
                            json!(ctx.embedding_service.embedding_provider().provider_name()),
                        ),
                        ("error".to_string(), json!(err.to_string())),
                        ("fact_type".to_string(), json!(fact_type)),
                    ]),
                    LogLevel::Warn,
                );
                if ctx.embedding_service.should_defer_embedding_retry(&err) {
                    deferred_embedding_input = Some(embedding_input.clone());
                }
                None
            }
        };

        // Create fact record via FactService.
        let fact_id = self
            .create_fact(
                fact_type,
                content,
                quote,
                source_episode,
                t_valid,
                confidence,
                &entity_links,
                &policy_tags,
                &provenance,
                embedding_fields,
                index_keys,
            )
            .await?;

        // Invalidate caches immediately after ingestion to ensure isolation.
        invalidate_cache(&ctx.context_cache).await;

        // Background processes: triple extraction and pending embedding retries.
        crate::service::episode::triples::spawn_triple_extraction(ctx, &fact_id, content);

        // Synchronous claim projection for deterministic extract visibility.
        let claim_svc = ctx.claim_service.clone();
        let claim_fact_id = FactId::from(fact_id.clone());
        let claim_episode_id = crate::models::EpisodeId::from(source_episode.to_string());
        let claim_content = content.to_string();
        let claim_entity_links = entity_links.clone();
        let claim_t_valid = t_valid;
        let claim_params = crate::service::claims::project::FactPersistedParams {
            namespace: &namespace,
            fact_id: &claim_fact_id,
            source_episode_id: &claim_episode_id,
            fact_type,
            content: &claim_content,
            policy_tags: &policy_tags,
            entity_links: &claim_entity_links,
            t_valid: claim_t_valid,
            source_lineage: episode_source_id.as_deref(),
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
            ctx.embedding_service
                .enqueue_background_fact_embedding(namespace.clone(), fact_id.clone(), input)
                .await;
        }

        Ok(fact_id)
    }

    /// Builds the embedding payload for a fact from the embedding provider
    /// state held on the context.
    fn build_embedding_payload(
        &self,
        ctx: &crate::service::service_context::ServiceContext,
        embedding: Vec<f64>,
    ) -> Result<EmbeddingPayload, MemoryError> {
        let provider = ctx.embedding_service.embedding_provider();
        let expected_dim = ctx
            .embedding_service
            .current_embedding_dimension()
            .unwrap_or_else(|| provider.dimension());
        if embedding.len() != expected_dim {
            return Err(MemoryError::Validation(format!(
                "embedding dimension mismatch: provider returned {}, expected {expected_dim}",
                embedding.len()
            )));
        }
        Ok(EmbeddingPayload {
            embedding,
            provider: provider.provider_name().to_string(),
            model: ctx
                .embedding_service
                .current_embedding_model()
                .map(str::to_string),
            dimension: expected_dim,
            signature: ctx
                .embedding_service
                .current_embedding_signature()
                .map(str::to_string),
            updated_at: normalize_dt(now()),
        })
    }

    /// Builds the input string passed to the embedding provider for a fact.
    pub(crate) fn build_fact_embedding_input(
        fact_type: &str,
        content: &str,
        quote: &str,
    ) -> String {
        format!("{fact_type}\n{content}\n{quote}")
    }
}

// ─── Free helper functions ───────────────────────────────────────────────────

fn reference_index_terms(raw: &str) -> Vec<String> {
    let query_terms = crate::service::query::search_query_terms(raw);
    let mut keys = crate::service::query::query_hard_anchor_terms(&query_terms)
        .into_iter()
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::mock_db::MockDbClient;

    #[test]
    fn build_fact_embedding_input_formats_correctly() {
        let result = FactService::build_fact_embedding_input("note", "Hello world", "Hello!");
        assert_eq!(result, "note\nHello world\nHello!");
    }

    #[test]
    fn build_fact_embedding_input_handles_empty_parts() {
        let result = FactService::build_fact_embedding_input("", "", "");
        assert_eq!(result, "\n\n");
    }

    #[tokio::test]
    async fn create_fact_persists_record() {
        let t = Utc::now();
        let fact_id = deterministic_fact_id("note", "hello world", "episode:test", t);
        let db = MockDbClient::new().expect_create(
            &fact_id,
            json!({"fact_id": fact_id.clone(), "status": "ok"}),
        );
        let svc = FactService::new(FactStoreClient::new(Arc::new(db), "org"));
        let provenance = Provenance::agent_observation("episode:test");

        let fact_id = svc
            .create_fact(
                "note",
                "hello world",
                "hello",
                "episode:test",
                t,
                0.9,
                &[],
                &[],
                &provenance,
                None,
                vec![],
            )
            .await
            .expect("create fact");

        assert!(fact_id.starts_with("fact:"));
    }

    #[tokio::test]
    async fn create_fact_returns_existing_id_on_duplicate() {
        let t = Utc::now();
        let fact_id = deterministic_fact_id("note", "dup", "episode:test", t);
        let db = MockDbClient::new()
            .expect_select_one(&fact_id, Some(json!({"fact_id": fact_id.clone()})));
        let svc = FactService::new(FactStoreClient::new(Arc::new(db), "org"));
        let provenance = Provenance::agent_observation("episode:test");

        let result = svc
            .create_fact(
                "note",
                "dup",
                "dup",
                "episode:test",
                t,
                0.9,
                &[],
                &[],
                &provenance,
                None,
                vec![],
            )
            .await
            .expect("create fact dup");

        assert_eq!(result, fact_id);
    }

    #[test]
    fn extract_temporal_index_keys_includes_month() {
        let t = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 3, 15, 10, 0, 0).unwrap();
        let keys = crate::service::core::extract_temporal_index_keys("test", t);
        assert!(keys.contains(&"2026-03".to_string()));
        assert!(keys.contains(&"march 2026".to_string()));
    }
}
