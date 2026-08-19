use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use lru::LruCache;

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::AssembledContextItem;
use crate::service::cache::CacheKey;
use crate::service::embedding_service::EmbeddingService;
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
///
/// This struct is intentionally a narrow seam: it holds shared fields and
/// cross-cutting lookup helpers. Domain logic lives on the owning services:
/// fact creation orchestration on [`FactService`], embedding generation on
/// [`EmbeddingService`], and triple extraction in `episode::triples`.
pub struct ServiceContext {
    pub(crate) db_client: Arc<dyn DbClient>,
    pub(crate) active_namespace: String,
    pub(crate) logger: StdoutLogger,
    pub(crate) rate_limiter: Arc<RateLimiter>,
    pub(crate) ingestion_service: IngestionService,
    pub(crate) explanation_service: ExplanationService,
    pub(crate) entity_resolver: EntityResolver,
    pub(crate) entity_service: EntityService,
    pub(crate) entity_extractor: Arc<dyn EntityExtractor>,
    pub(crate) embedding_service: EmbeddingService,
    pub(crate) fact_service: FactService,
    pub(crate) triple_extractor: Arc<dyn TripleExtractor>,
    pub(crate) context_cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    pub(crate) claim_store: Option<Arc<dyn crate::storage::claims::ClaimStore>>,
    pub(crate) query_logging_enabled: bool,
    pub(crate) query_log_retention_days: u32,
    /// Claim service reference for extract reconciliation.
    pub(crate) claim_service: crate::service::claims::projection::ClaimService,
    /// Bounded-concurrency semaphore for fire-and-forget triple extraction.
    pub(crate) triple_extraction_semaphore: Arc<tokio::sync::Semaphore>,
}

impl ServiceContext {
    /// Looks up a record in the process-bound Active Namespace.
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
        self.app_store().find_record_by_id(record_id).await
    }

    /// Enforces rate limit based on the caller ID in the access payload.
    ///
    /// Delegates to [`RateLimiter::check_access`], the single enforcement
    /// point for the token-bucket policy.
    pub(crate) fn enforce_rate_limit(
        &self,
        access: Option<&crate::models::AccessPayload>,
    ) -> Result<(), MemoryError> {
        self.rate_limiter.check_access(access)
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
    pub(crate) fn context_store(&self) -> crate::storage::ContextStoreClient {
        crate::storage::ContextStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// Returns the context access log handle bound to the Active Namespace.
    pub(crate) fn context_access_log(&self) -> crate::storage::ContextAccessLogClient {
        crate::storage::ContextAccessLogClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// Returns the app store handle bound to the Active Namespace.
    pub(crate) fn app_store(&self) -> crate::storage::AppStoreClient {
        crate::storage::AppStoreClient::new(self.db_client.clone(), self.active_namespace.clone())
    }

    /// Returns the bi-temporal close owner bound to the Active Namespace
    /// (ADR-0039: the only place that composes close operations).
    pub(crate) fn close_store(&self) -> crate::storage::CloseStoreClient {
        crate::storage::CloseStoreClient::new(self.db_client.clone(), self.active_namespace.clone())
    }

    /// Returns the triple store handle bound to the Active Namespace — the
    /// single owner of every read/write on the `triple` table.
    pub(crate) fn triple_store(&self) -> crate::storage::TripleStoreClient {
        crate::storage::TripleStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// Returns the episode store handle (the db client).
    pub(crate) fn episode_store(&self) -> crate::storage::EpisodeStoreClient {
        crate::storage::EpisodeStoreClient::new(
            self.db_client.clone(),
            self.active_namespace.clone(),
        )
    }

    /// Batch-fetch entity records by IDs in the Active Namespace.
    ///
    /// Missing IDs are omitted from the result (caller checks `map.get`).
    pub(crate) async fn find_entity_records_by_ids(
        &self,
        entity_ids: &[String],
    ) -> Result<std::collections::HashMap<String, (String, Vec<String>)>, MemoryError> {
        use std::collections::HashMap;
        let mut result: HashMap<String, (String, Vec<String>)> = HashMap::new();
        if entity_ids.is_empty() {
            return Ok(result);
        }
        let rows = self.app_store().select_entities_by_ids(entity_ids).await?;
        {
            for row in rows {
                let serde_json::Value::Object(map) = row else {
                    continue;
                };
                let entity_id = map
                    .get("entity_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
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
                result.entry(entity_id).or_insert((canonical, aliases));
            }
        }
        Ok(result)
    }
}

impl crate::service::apps::graph::GraphContext for ServiceContext {
    fn app_store(&self) -> crate::storage::AppStoreClient {
        crate::storage::AppStoreClient::new(self.db_client.clone(), self.active_namespace.clone())
    }
    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}

#[cfg(test)]
mod tests {
    use crate::service::capabilities::test_support::make_context_base;
    use crate::service::error::MemoryError;
    use crate::service::mock_db::MockDbClient;

    // These tests drive the wiring of `validate_record_id` into
    // `ServiceContext::find_record_by_id`.

    #[tokio::test]
    async fn find_record_by_id_rejects_bare_hex_with_validation_error() {
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
        let result = ctx.find_record_by_id("474b2d8b81b3feabf832ef08").await;
        match result {
            Err(MemoryError::Validation(msg)) => {
                assert!(msg.contains("'<table>:<id>'"), "{msg}");
                assert!(msg.contains("474b2d8b81b3feabf832ef08"), "{msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn find_record_by_id_rejects_empty_id_part() {
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
        let result = ctx.find_record_by_id("episode:").await;
        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    #[tokio::test]
    async fn find_record_by_id_rejects_empty_input() {
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
        let result = ctx.find_record_by_id("").await;
        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    #[tokio::test]
    async fn find_record_by_id_accepts_wellformed_episode_id() {
        // Sanity: fully-formed ID must not be rejected by pre-validation.
        // DB layer may still return Ok(None, None) — that's an honest "not found".
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
        let result = ctx.find_record_by_id("episode:doesnotexist").await;
        assert!(
            result.is_ok(),
            "well-formed id must pass validation: {result:?}"
        );
    }

    #[test]
    fn enforce_rate_limit_delegates_to_rate_limiter() {
        use std::sync::Arc;

        use crate::models::AccessPayload;
        use crate::service::util::RateLimiter;

        let db = MockDbClient::new();
        let mut ctx = make_context_base(db);
        ctx.rate_limiter = Arc::new(RateLimiter::new(1, 1));

        let access = AccessPayload {
            caller_id: Some("ctx-user".into()),
            ..Default::default()
        };
        assert!(ctx.enforce_rate_limit(Some(&access)).is_ok());
        let err = ctx.enforce_rate_limit(Some(&access)).unwrap_err();
        assert!(matches!(err, MemoryError::Validation(ref msg) if msg == "rate limit exceeded"));
        // No caller → always allowed.
        assert!(ctx.enforce_rate_limit(None).is_ok());
    }
}
