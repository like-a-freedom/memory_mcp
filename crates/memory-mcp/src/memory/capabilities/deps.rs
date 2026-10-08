//! Each capability declares the dependencies it actually uses.
//!
//! Before this, every capability took `&MemoryService` (the former
//! `ServiceContext`), a struct holding nineteen fields spanning
//! storage, retrieval, extraction, claims and lifecycle. A capability
//! that only needed one service still depended on all the others, so
//! the dependency graph could not be read off the type, and nothing
//! prevented a capability from reaching for a field it had no business
//! touching.
//!
//! Each struct here is the complete set of fields that capability
//! uses and nothing more. They are assembled once in
//! `service/capabilities/tool_context_impl.rs`, so the wiring stays
//! in one place and the god-struct is no longer part of any
//! capability's signature.

use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::knowledge::claims::ClaimStore;
use crate::knowledge::claims_policy::projection::ClaimService;
use crate::knowledge::entity_resolution::EntityResolver;
use crate::knowledge::entity_service::EntityService;
use crate::logging::StdoutLogger;
use crate::memory::context_cache::ContextCache;
use crate::memory::explanation::ExplanationService;
use crate::memory::ingestion::IngestionService;
use crate::platform::rate_limiter::RateLimiter;
use crate::shared::triple_extractor::TripleExtractor;

/// What `ingest` needs: the ingestion service itself.
///
/// The rate-limit charge lives inside the service, so the capability
/// does not also hold the limiter: taking it here would invite a
/// second charge against the same shared bucket.
#[derive(Clone)]
pub(crate) struct IngestDeps {
    pub(crate) ingestion_service: IngestionService,
}

/// What `extract` needs.
#[derive(Clone)]
pub(crate) struct ExtractDeps {
    pub(crate) db_client: Arc<dyn crate::storage::DbClient>,
    pub(crate) active_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) entity_extractor: Arc<dyn crate::knowledge::entity_extraction::EntityExtractor>,
    pub(crate) entity_service: EntityService,
    pub(crate) entity_resolver: EntityResolver,
    pub(crate) triple_extractor: Arc<dyn TripleExtractor>,
    pub(crate) triple_extraction_semaphore: Arc<Semaphore>,
    pub(crate) embedding_service: crate::embedding::service::EmbeddingService,
    pub(crate) fact_service: crate::knowledge::fact_service::FactService,
    pub(crate) claim_service: ClaimService,
    pub(crate) context_cache: ContextCache,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    #[cfg(feature = "streamable-http")]
    pub(crate) outbox_enabled: bool,
}

impl ExtractDeps {
    /// The rate limiter, borrowed for the capability's own charge.
    pub(crate) fn rate_limiter(&self) -> &Arc<RateLimiter> {
        &self.rate_limiter
    }

    /// Owner-scoped episode store.
    pub(crate) fn episode_store(&self) -> crate::memory::episode_store::EpisodeStoreClient {
        crate::memory::episode_store::EpisodeStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// Owner-scoped triple store.
    pub(crate) fn triple_store(&self) -> crate::knowledge::TripleStoreClient {
        crate::knowledge::TripleStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// Owner-scoped fact store.
    pub(crate) fn fact_store(&self) -> crate::knowledge::FactStoreClient {
        crate::knowledge::FactStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// The close owner, for retracting a fact with its claims.
    ///
    /// Mirrors the container's `close_store`: the outbox is attached
    /// only when this deployment enabled it, so a close and its
    /// invalidation event stay in one transaction exactly as before.
    pub(crate) fn close_store(&self) -> crate::knowledge::CloseStoreClient {
        let store = crate::knowledge::CloseStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        );
        #[cfg(feature = "streamable-http")]
        if self.outbox_enabled {
            return store.with_outbox();
        }
        store
    }

    /// Read one episode record, refusing an id that names another
    /// kind.
    pub(crate) async fn find_episode_record(
        &self,
        episode_id: &str,
    ) -> Result<crate::storage::RecordLookup, crate::error::MemoryError> {
        crate::storage::owner_scoped_read(self.episode_store().select_episode(episode_id).await)
    }

    /// Read one fact record, refusing an id that names another kind.
    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<crate::storage::RecordLookup, crate::error::MemoryError> {
        crate::storage::owner_scoped_read(self.fact_store().select_fact(fact_id).await)
    }

    /// Read an entity record for provenance assembly.
    ///
    /// One batched query per call site, as before: the entity
    /// projection is `(canonical_name, aliases)` keyed by entity id,
    /// and a missing id is simply absent from the map.
    pub(crate) async fn find_entity_records_by_ids(
        &self,
        entity_ids: &[String],
    ) -> Result<std::collections::HashMap<String, (String, Vec<String>)>, crate::error::MemoryError>
    {
        use std::collections::HashMap;
        let mut result: HashMap<String, (String, Vec<String>)> = HashMap::new();
        if entity_ids.is_empty() {
            return Ok(result);
        }
        let rows = self
            .knowledge_graph_store()
            .select_entities_by_ids(entity_ids)
            .await?;
        for row in rows {
            let serde_json::Value::Object(map) = row else {
                continue;
            };
            let entity_id = map
                .get("entity_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let canonical = map
                .get("canonical_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(entity_id.as_str())
                .to_string();
            let aliases = map
                .get("aliases")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            result.entry(entity_id).or_insert((canonical, aliases));
        }
        Ok(result)
    }

    /// The app store the community and entity reads use.
    pub(crate) fn knowledge_graph_store(
        &self,
    ) -> crate::knowledge::graph_store::KnowledgeGraphStore {
        crate::knowledge::graph_store::KnowledgeGraphStore::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }
}

/// What `resolve` needs.
#[derive(Clone)]
pub(crate) struct ResolveDeps {
    pub(crate) entity_resolver: EntityResolver,
    pub(crate) entity_service: EntityService,
    pub(crate) rate_limiter: Arc<RateLimiter>,
}

/// What `explain` needs.
#[derive(Clone)]
pub(crate) struct ExplainDeps {
    pub(crate) explanation_service: ExplanationService,
    pub(crate) rate_limiter: Arc<RateLimiter>,
}

/// What `invalidate` needs: the owner-scoped record read, the close
/// owner, the cache, and the rate-limit policy.
#[derive(Clone)]
pub(crate) struct InvalidateDeps {
    pub(crate) db_client: Arc<dyn crate::storage::DbClient>,
    pub(crate) active_namespace: String,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    pub(crate) claim_store: Option<Arc<dyn ClaimStore>>,
    pub(crate) context_cache: ContextCache,
    #[cfg(feature = "streamable-http")]
    pub(crate) outbox_enabled: bool,
}
