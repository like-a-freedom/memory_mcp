//! Fact-creation orchestration.
//!
//! `add_fact` is the pipeline entry point: it validates, builds index
//! keys, generates embeddings, persists the fact, invalidates the
//! context cache, spawns triple extraction and projects claims. The
//! persistence core lives in [`crate::knowledge::fact_service`]; what
//! remains here is the orchestration that needs the embedding service,
//! the claim service, the cache and the logger — a consumer's wiring,
//! not knowledge's own write path.
//!
//! It takes `&ExtractDeps`, the narrow port that bundles exactly those
//! handles, rather than the service container.

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::error::MemoryError;
use crate::knowledge::fact_service::{EmbeddingPayload, FactService};
use crate::logging::LogLevel;
use crate::memory::context_cache::invalidate_cache;
use crate::models::FactId;
use crate::models::Provenance;
#[cfg(test)]
use crate::shared::ids::deterministic_fact_id;
use crate::shared::temporal::{normalize_dt, now};
use crate::shared::validation::validate_fact_input;

impl FactService {
    /// Adds a new fact, orchestrating embedding generation, triple extraction,
    /// and claim projection.
    ///
    /// This is the full fact-creation entry point. The persistence core is
    /// delegated to [`FactService::create_fact`]; this method handles the
    /// surrounding orchestration previously held on the container.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn add_fact(
        &self,
        ctx: &crate::memory::capabilities::deps::ExtractDeps,
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

        let (index_keys, episode_source_lineage) = self
            .prepare_index_keys(
                ctx,
                content,
                source_episode,
                &provenance,
                &entity_links,
                t_valid,
            )
            .await?;

        let (embedding_fields, deferred_embedding_input) = self
            .generate_embedding_fields(ctx, fact_type, content, quote)
            .await?;

        let outcome = self
            .persist_fact(
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

        self.run_post_persist(
            ctx,
            &namespace,
            &outcome.fact_id,
            outcome.created,
            source_episode,
            fact_type,
            content,
            &policy_tags,
            &entity_links,
            t_valid,
            episode_source_lineage.as_deref(),
            deferred_embedding_input,
        )
        .await;

        Ok(outcome.fact_id)
    }

    /// Pre-fetch linked entity records and the episode's source lineage,
    /// then build the deterministic index keys.
    ///
    /// The entity map is fetched in one batch per namespace, avoiding O(N)
    /// round-trips. Returns None for missing IDs. The episode's `source_id`
    /// also serves as the ADR-0008 source lineage for claim projection (the
    /// connector's stable record identifier); filesystem episodes carry an
    /// explicit `source_lineage` preferred over the versioned `source_id`.
    async fn prepare_index_keys(
        &self,
        ctx: &crate::memory::capabilities::deps::ExtractDeps,
        content: &str,
        source_episode: &str,
        provenance: &Provenance,
        entity_links: &[String],
        t_valid: DateTime<Utc>,
    ) -> Result<(Vec<String>, Option<String>), MemoryError> {
        let entity_map: std::collections::HashMap<String, (String, Vec<String>)> =
            ctx.find_entity_records_by_ids(entity_links).await?;

        let episode_record = ctx.find_episode_record(source_episode).await?;
        let episode_source_id = episode_record
            .as_ref()
            .and_then(|map| map.get("source_id"))
            .and_then(crate::storage::value_helpers::string_from_value);
        let episode_source_lineage = episode_record
            .as_ref()
            .and_then(|map| map.get("source_lineage"))
            .and_then(crate::storage::value_helpers::string_from_value)
            .or_else(|| episode_source_id.clone());

        let entity_lookup = |entity_id: &str| Ok(entity_map.get(entity_id).cloned());
        let source_reference_lookup = |_episode_id: &str| Ok(episode_source_id.clone());

        let index_keys = self
            .build_index_keys(
                content,
                source_episode,
                provenance,
                entity_links,
                t_valid,
                entity_lookup,
                source_reference_lookup,
            )
            .await?;

        Ok((index_keys, episode_source_lineage))
    }

    /// Generate the embedding payload for the fact, or defer/skip it.
    ///
    /// Returns the optional embedding fields for persistence and the
    /// optional deferred input to enqueue as a background retry after
    /// claim projection.
    async fn generate_embedding_fields(
        &self,
        ctx: &crate::memory::capabilities::deps::ExtractDeps,
        fact_type: &str,
        content: &str,
        quote: &str,
    ) -> Result<(Option<EmbeddingPayload>, Option<String>), MemoryError> {
        let embedding_input = Self::build_fact_embedding_input(fact_type, content, quote);
        let mut deferred_embedding_input = None;
        // Through the port, so the input limit and the disabled check apply
        // here as they do everywhere else. `generate_embedding` was called
        // directly before, which left this one caller responsible for knowing
        // that the limit exists.
        let generated = crate::embedding::api::EmbeddingGeneration::generate(
            &ctx.embedding_service,
            &embedding_input,
        )
        .await;
        let embedding_fields = match generated {
            Ok(crate::embedding::api::GenerationOutcome::Generated(embedding)) => {
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
            // A disabled provider is a configuration, not a failure: the fact is
            // created without a vector and the pipeline moves on.
            Ok(crate::embedding::api::GenerationOutcome::Skipped(reason)) => {
                ctx.logger.log(
                    std::collections::HashMap::from([
                        ("op".to_string(), json!("embedding.write_skipped")),
                        ("reason".to_string(), json!(format!("{reason:?}"))),
                    ]),
                    LogLevel::Info,
                );
                None
            }
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
        Ok((embedding_fields, deferred_embedding_input))
    }

    /// Persist the fact record via the knowledge write path.
    ///
    /// `created == false` means the deterministic fact already existed
    /// (same id + same content): repeat ingestion is idempotent, so the
    /// post-persist phases below skip derived work for it.
    #[allow(clippy::too_many_arguments)]
    async fn persist_fact(
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
    ) -> Result<crate::knowledge::fact_service::CreateFactOutcome, MemoryError> {
        self.create_fact(
            fact_type,
            content,
            quote,
            source_episode,
            t_valid,
            confidence,
            entity_links,
            policy_tags,
            provenance,
            embedding_fields,
            index_keys,
        )
        .await
    }

    /// Post-persist phases in side-effect order: cache invalidation,
    /// background triple extraction, synchronous claim projection (fresh
    /// facts only), and the deferred embedding retry enqueue.
    #[allow(clippy::too_many_arguments)]
    async fn run_post_persist(
        &self,
        ctx: &crate::memory::capabilities::deps::ExtractDeps,
        namespace: &str,
        fact_id: &str,
        created: bool,
        source_episode: &str,
        fact_type: &str,
        content: &str,
        policy_tags: &[String],
        entity_links: &[String],
        t_valid: DateTime<Utc>,
        source_lineage: Option<&str>,
        deferred_embedding_input: Option<String>,
    ) {
        // Invalidate caches immediately after ingestion to ensure isolation.
        invalidate_cache(&ctx.context_cache).await;

        // Background processes: triple extraction and pending embedding retries.
        crate::memory::episode::triples::spawn_triple_extraction(ctx, fact_id, content);

        // Synchronous claim projection for deterministic extract visibility.
        // Runs only for freshly created facts: re-projecting an existing fact
        // would collide with its deterministic claim IDs (noisy "already
        // exists" errors) and must never overwrite claims of an invalidated
        // fact. Same-id/same-content is idempotent by contract.
        if created {
            crate::memory::context_cache::invalidation::invalidate_after(
                &ctx.context_cache,
                self.project_claims(
                    ctx,
                    namespace,
                    fact_id,
                    source_episode,
                    fact_type,
                    content,
                    policy_tags,
                    entity_links,
                    t_valid,
                    source_lineage,
                ),
            )
            .await;
        }

        // Enqueue background embedding after claim projection to ensure test invariants.
        if let Some(input) = deferred_embedding_input {
            ctx.embedding_service
                .enqueue_background_fact_embedding(
                    namespace.to_string(),
                    fact_id.to_string(),
                    input,
                )
                .await;
        }
    }

    /// Project the persisted fact onto claims and record the outcome.
    // Clippy: params are the fact's persisted attributes — one cohesive
    // projection step; a struct would just mirror the param list.
    #[allow(clippy::too_many_arguments)]
    async fn project_claims(
        &self,
        ctx: &crate::memory::capabilities::deps::ExtractDeps,
        namespace: &str,
        fact_id: &str,
        source_episode: &str,
        fact_type: &str,
        content: &str,
        policy_tags: &[String],
        entity_links: &[String],
        t_valid: DateTime<Utc>,
        source_lineage: Option<&str>,
    ) {
        let claim_svc = ctx.claim_service.clone();
        let claim_fact_id = FactId::from(fact_id.to_string());
        let claim_episode_id = crate::models::EpisodeId::from(source_episode.to_string());
        let claim_content = content.to_string();
        let claim_entity_links = entity_links.to_vec();
        let claim_params = crate::knowledge::claims_policy::projection::FactPersistedParams {
            namespace,
            fact_id: &claim_fact_id,
            source_episode_id: &claim_episode_id,
            fact_type,
            content: &claim_content,
            policy_tags,
            entity_links: &claim_entity_links,
            t_valid,
            source_lineage,
        };
        match claim_svc.after_fact_persisted(&claim_params).await {
            Ok(summary) => claim_svc.record_post_fact_success(
                namespace,
                &summary.fact_id,
                summary.claims_projected,
                summary.claims_skipped,
            ),
            Err(error) => claim_svc.record_post_fact_failure(namespace, &claim_fact_id, &error),
        }
    }

    /// The dimension check, before the payload is assembled.
    ///
    /// This is not a second copy of the check in
    /// `embedding::api::prepare_canonical_vector`, and the duplication is
    /// deliberate. That check guards a write to a fact that already exists;
    /// this one guards a fact that is about to be created, where the vector
    /// goes into the creation payload rather than through the write port —
    /// there is no stored record yet to compare a signature against, so
    /// `VectorWritePolicy` has nothing to decide and routing through
    /// `update_canonical_vector` would be a write to a row that does not
    /// exist.
    ///
    /// What this does share is the generation contract: the embedding arrives
    /// through `EmbeddingGeneration`, so the input limit and the
    /// disabled-provider check apply here exactly as they do in recovery.
    fn build_embedding_payload(
        &self,
        ctx: &crate::memory::capabilities::deps::ExtractDeps,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::FactStoreClient;
    use crate::service::mock_db::MockDbClient;
    use std::sync::Arc;

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

        let outcome = svc
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

        assert!(outcome.created, "expected a fresh fact record");
        assert!(outcome.fact_id.starts_with("fact:"));
    }

    #[tokio::test]
    async fn create_fact_returns_existing_id_on_duplicate() {
        let t = Utc::now();
        let fact_id = deterministic_fact_id("note", "dup", "episode:test", t);
        let db = MockDbClient::new()
            .expect_select_one(&fact_id, Some(json!({"fact_id": fact_id.clone()})));
        let svc = FactService::new(FactStoreClient::new(Arc::new(db), "org"));
        let provenance = Provenance::agent_observation("episode:test");

        let outcome = svc
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

        assert_eq!(outcome.fact_id, fact_id);
        assert!(!outcome.created, "duplicate must not re-write the fact");
    }
}
