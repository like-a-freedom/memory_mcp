pub(crate) mod invalidate;

pub(crate) mod assemble_context;
pub(crate) mod explain;
pub(crate) mod extract;
pub(crate) mod ingest;
pub(crate) mod resolve;

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared test helpers for capability unit tests.
    use std::num::NonZeroUsize;
    use std::sync::Arc;

    use lru::LruCache;
    use tokio::sync::{Mutex, RwLock};

    use crate::logging::StdoutLogger;
    use crate::service::embedding::DisabledEmbeddingProvider;
    use crate::service::entity::EntityService;
    use crate::service::entity_resolution::EntityResolver;
    use crate::service::explanation::ExplanationService;
    use crate::service::ingestion::IngestionService;
    use crate::service::mock_db::MockDbClient;
    use crate::service::service_context::ServiceContext;
    use crate::service::triple_extractor::RuleBasedTripleExtractor;
    use crate::service::util::RateLimiter;
    use crate::storage::claims::SurrealClaimStore;

    /// Builds a `ServiceContext` wired to a `MockDbClient` and no-op
    /// providers, suitable for capability unit tests.
    pub(crate) fn make_context_base(db: MockDbClient) -> ServiceContext {
        let db_client: Arc<dyn crate::storage::DbClient> = Arc::new(db);
        let namespaces = vec!["org".to_string()];
        ServiceContext {
            db_client: db_client.clone(),
            namespaces: namespaces.clone(),
            default_namespace: "org".to_string(),
            logger: StdoutLogger::new("warn"),
            rate_limiter: Arc::new(RateLimiter::new(100, 100)),
            ingestion_service: IngestionService::new(
                db_client.clone(),
                namespaces.clone(),
                StdoutLogger::new("warn"),
                Arc::new(RateLimiter::new(100, 100)),
            ),
            explanation_service: ExplanationService::new(
                db_client.clone(),
                StdoutLogger::new("warn"),
                namespaces.clone(),
            ),
            entity_resolver: EntityResolver::new(0.85),
            entity_service: EntityService::new(db_client.clone()),
            entity_extractor: Arc::new(
                crate::service::entity_extraction::RegexEntityExtractor::new()
                    .expect("regex extractor"),
            )
                as Arc<dyn crate::service::entity_extraction::EntityExtractor>,
            embedding_service: crate::service::embedding_service::EmbeddingService::new(
                db_client.clone(),
                StdoutLogger::new("warn"),
                Arc::new(DisabledEmbeddingProvider::new(0))
                    as Arc<dyn crate::service::embedding::EmbeddingProvider>,
                0.0,
                None,
                None,
                None,
                Arc::new(RwLock::new(LruCache::new(
                    NonZeroUsize::new(64).expect("valid size"),
                ))),
                Arc::new(Mutex::new(LruCache::new(
                    NonZeroUsize::new(64).expect("valid size"),
                ))),
                Arc::new(crate::service::embedding::task_runner::BackgroundTaskRunner::new()),
            ),
            fact_service: crate::service::fact::FactService::new(db_client.clone()),
            triple_extractor: Arc::new(RuleBasedTripleExtractor::new())
                as Arc<dyn crate::service::triple_extractor::TripleExtractor>,
            context_cache: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(64).expect("valid size"),
            ))),
            claim_store: None,
            query_logging_enabled: false,
            query_log_retention_days: 7,
            claim_service: crate::service::claims::project::ClaimService::new(Arc::new(
                SurrealClaimStore::new(db_client.clone()),
            )),
            triple_extraction_semaphore: Arc::new(tokio::sync::Semaphore::new(
                crate::service::TRIPLE_EXTRACTION_MAX_CONCURRENCY,
            )),
        }
    }
}
