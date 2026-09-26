//! Each capability declares the dependencies it actually uses.
//!
//! Before this, every capability took `&ServiceContext`, a struct
//! holding nineteen fields spanning storage, retrieval, extraction,
//! claims and lifecycle. A capability that only needed one service
//! still depended on all the others, so the dependency graph could
//! not be read off the type, and nothing prevented a capability from
//! reaching for a field it had no business touching.
//!
//! Each struct here is the complete set of fields that capability
//! uses and nothing more. They are assembled once in
//! `service/capabilities/tool_context_impl.rs`, so the wiring stays
//! in one place and the god-struct is no longer part of any
//! capability's signature.

use std::sync::Arc;

use tokio::sync::{RwLock, Semaphore};

use lru::LruCache;

use crate::logging::StdoutLogger;
use crate::service::claims::projection::ClaimService;
use crate::service::entity::EntityService;
use crate::service::entity_resolution::EntityResolver;
use crate::service::explanation::ExplanationService;
use crate::service::ingestion::IngestionService;
use crate::service::triple_extractor::TripleExtractor;
use crate::service::util::RateLimiter;
use crate::storage::claims::ClaimStore;

impl From<&crate::service::MemoryService> for IngestDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            ingestion_service: ctx.ingestion_service.clone(),
        }
    }
}

impl From<&crate::service::MemoryService> for ResolveDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            entity_resolver: ctx.entity_resolver.clone(),
            entity_service: ctx.entity_service.clone(),
            rate_limiter: ctx.rate_limiter.clone(),
        }
    }
}

impl From<&crate::service::MemoryService> for ExplainDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            explanation_service: ctx.explanation_service.clone(),
            rate_limiter: ctx.rate_limiter.clone(),
        }
    }
}

impl From<&crate::service::MemoryService> for InvalidateDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            db_client: ctx.db_client.clone(),
            active_namespace: ctx.active_namespace.clone(),
            rate_limiter: ctx.rate_limiter.clone(),
            claim_store: ctx.claim_store(),
            context_cache: ctx.context_cache.clone(),
            #[cfg(feature = "streamable-http")]
            outbox_enabled: ctx.outbox_enabled,
        }
    }
}

impl From<&crate::service::MemoryService> for ExtractDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            db_client: ctx.db_client.clone(),
            active_namespace: ctx.active_namespace.clone(),
            logger: ctx.logger.clone(),
            entity_extractor: ctx.entity_extractor.clone(),
            entity_service: ctx.entity_service.clone(),
            entity_resolver: ctx.entity_resolver.clone(),
            triple_extractor: ctx.triple_extractor.clone(),
            triple_extraction_semaphore: ctx.triple_extraction_semaphore.clone(),
            embedding_service: ctx.embedding_service(),
            fact_service: ctx.fact_service.clone(),
            claim_service: ctx.claim_service.clone(),
            context_cache: ctx.context_cache.clone(),
            rate_limiter: ctx.rate_limiter.clone(),
            #[cfg(feature = "streamable-http")]
            outbox_enabled: ctx.outbox_enabled,
        }
    }
}

impl From<&crate::service::MemoryService> for AssembleContextDeps {
    fn from(ctx: &crate::service::MemoryService) -> Self {
        Self {
            active_namespace: ctx.active_namespace.clone(),
            logger: ctx.logger.clone(),
            context_store: crate::storage::ContextStoreClient::new(
                ctx.db_client.clone(),
                ctx.active_namespace.clone(),
            ),
            context_access_log: crate::storage::ContextAccessLogClient::new(
                ctx.db_client.clone(),
                ctx.active_namespace.clone(),
            ),
            app_store: crate::storage::AppStoreClient::new(
                ctx.db_client.clone(),
                ctx.active_namespace.clone(),
            ),
            embedding_service: ctx.embedding_service(),
            context_cache: ctx.context_cache.clone(),
            query_logging_enabled: ctx.query_logging_enabled,
            query_log_retention_days: ctx.query_log_retention_days,
            rate_limiter: ctx.rate_limiter.clone(),
        }
    }
}

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
    pub(crate) entity_extractor: Arc<dyn crate::service::entity_extraction::EntityExtractor>,
    pub(crate) entity_service: EntityService,
    pub(crate) entity_resolver: EntityResolver,
    pub(crate) triple_extractor: Arc<dyn TripleExtractor>,
    pub(crate) triple_extraction_semaphore: Arc<Semaphore>,
    pub(crate) embedding_service: crate::service::embedding_service::EmbeddingService,
    pub(crate) fact_service: crate::service::fact::FactService,
    pub(crate) claim_service: ClaimService,
    pub(crate) context_cache: Arc<
        RwLock<LruCache<crate::service::cache::CacheKey, Vec<crate::models::AssembledContextItem>>>,
    >,
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
    pub(crate) fn episode_store(&self) -> crate::storage::EpisodeStoreClient {
        crate::storage::EpisodeStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// Owner-scoped triple store.
    pub(crate) fn triple_store(&self) -> crate::storage::TripleStoreClient {
        crate::storage::TripleStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// Owner-scoped fact store.
    pub(crate) fn fact_store(&self) -> crate::storage::FactStoreClient {
        crate::storage::FactStoreClient::new(self.db_client.clone(), self.active_namespace.clone())
    }

    /// The close owner, for retracting a fact with its claims.
    ///
    /// Mirrors the container's `close_store`: the outbox is attached
    /// only when this deployment enabled it, so a close and its
    /// invalidation event stay in one transaction exactly as before.
    pub(crate) fn close_store(&self) -> crate::storage::CloseStoreClient {
        let store = crate::storage::CloseStoreClient::new(
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
        crate::storage::owner_scoped_read(
            self.episode_store().select_episode(episode_id).await,
        )
    }

    /// Read one fact record, refusing an id that names another kind.
    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<crate::storage::RecordLookup, crate::error::MemoryError> {
        crate::storage::owner_scoped_read(
            self.fact_store().select_fact(fact_id).await,
        )
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
        let rows = self.app_store().select_entities_by_ids(entity_ids).await?;
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
    pub(crate) fn app_store(&self) -> crate::storage::AppStoreClient {
        crate::storage::AppStoreClient::new(self.db_client.clone(), self.active_namespace.clone())
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
    pub(crate) context_cache: Arc<
        RwLock<LruCache<crate::service::cache::CacheKey, Vec<crate::models::AssembledContextItem>>>,
    >,
    #[cfg(feature = "streamable-http")]
    pub(crate) outbox_enabled: bool,
}

/// What the `assemble_context` pipeline needs.
///
/// This is the retrieval seam the pipeline already had, promoted to
/// a named owner-scoped dependency rather than a field bag on a
/// shared container. The pipeline and the capability that calls it
/// share this one type, so there is a single description of what
/// retrieval depends on.
#[derive(Clone)]
pub(crate) struct AssembleContextDeps {
    pub(crate) active_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) context_store: crate::storage::ContextStoreClient,
    pub(crate) context_access_log: crate::storage::ContextAccessLogClient,
    pub(crate) app_store: crate::storage::AppStoreClient,
    pub(crate) embedding_service: crate::service::embedding_service::EmbeddingService,
    pub(crate) context_cache: Arc<
        RwLock<LruCache<crate::service::cache::CacheKey, Vec<crate::models::AssembledContextItem>>>,
    >,
    pub(crate) query_logging_enabled: bool,
    pub(crate) query_log_retention_days: u32,
    pub(crate) rate_limiter: Arc<RateLimiter>,
}

impl AssembleContextDeps {
    /// The knowledge read owner, for canonical fact reads.
    pub(crate) fn context_store(&self) -> &crate::storage::ContextStoreClient {
        &self.context_store
    }

    /// The access-log owner, for query-log writes.
    pub(crate) fn context_access_log(&self) -> &crate::storage::ContextAccessLogClient {
        &self.context_access_log
    }

    /// Whether this deployment records a query log for each assembly.
    pub(crate) fn is_query_logging_enabled(&self) -> bool {
        self.query_logging_enabled
    }

    /// How long query-log rows are kept.
    pub(crate) fn query_log_retention_days(&self) -> u32 {
        self.query_log_retention_days
    }

    /// Record fact access for an assembled item.
    pub(crate) async fn record_fact_access(
        &self,
        fact_id: &str,
        boost: i64,
    ) -> Result<(), crate::error::MemoryError> {
        self.app_store.record_fact_access(fact_id, boost).await
    }
}
