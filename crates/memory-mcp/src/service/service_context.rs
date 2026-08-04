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
    pub(crate) namespaces: Vec<String>,
    pub(crate) default_namespace: String,
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
    pub(crate) claim_service: crate::service::claims::project::ClaimService,
    /// Bounded-concurrency semaphore for fire-and-forget triple extraction.
    pub(crate) triple_extraction_semaphore: Arc<tokio::sync::Semaphore>,
}

impl ServiceContext {
    /// Scans all namespaces for a record by its ID, returning the payload and owning namespace.
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
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(record_id, namespace).await?;
            if let Some(serde_json::Value::Object(map)) = record {
                return Ok((Some(map), Some(namespace.clone())));
            }
        }
        Ok((None, None))
    }

    /// Enforces rate limit based on the caller ID in the access payload.
    pub(crate) fn enforce_rate_limit(
        &self,
        access: Option<&crate::models::AccessPayload>,
    ) -> Result<(), MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
    }

    /// Returns the namespace for a given scope.
    pub(crate) fn namespace_for_scope(&self, scope: &str) -> Result<String, MemoryError> {
        crate::service::MemoryScope::parse(scope)?.namespace(&self.namespaces)
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
        crate::storage::ContextStoreClient::new(self.db_client.clone())
    }

    /// Returns the context access log handle (the db client).
    pub(crate) fn context_access_log(&self) -> crate::storage::ContextAccessLogClient {
        crate::storage::ContextAccessLogClient::new(self.db_client.clone())
    }

    /// Returns the episode store handle (the db client).
    pub(crate) fn episode_store(&self) -> crate::storage::EpisodeStoreClient {
        crate::storage::EpisodeStoreClient::new(self.db_client.clone())
    }

    /// Batch-fetch entity records by IDs across all namespaces.
    ///
    /// Returns a map of entity ID to (canonical_name, aliases). Missing IDs are
    /// omitted from the result (caller checks `map.get`).Namespace precedence
    /// follows `self.namespaces` order: first namespace containing the ID wins.
    pub(crate) async fn find_entity_records_by_ids(
        &self,
        entity_ids: &[String],
    ) -> Result<std::collections::HashMap<String, (String, Vec<String>)>, MemoryError> {
        use std::collections::HashMap;
        let mut result: HashMap<String, (String, Vec<String>)> = HashMap::new();
        if entity_ids.is_empty() {
            return Ok(result);
        }
        let names = entity_ids.to_vec();
        for namespace in &self.namespaces {
            let sql =
                "SELECT entity_id, canonical_name, aliases FROM entity WHERE entity_id IN $ids";
            let rows = self
                .db_client
                .query(
                    sql,
                    Some(serde_json::json!({ "ids": names.clone() })),
                    namespace,
                )
                .await?;
            if let serde_json::Value::Array(rows) = rows {
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
        }
        Ok(result)
    }

    /// Returns the project associated with a source episode, if any.
    pub(crate) async fn project_for_source_episode(
        &self,
        source_episode: &str,
    ) -> Result<Option<String>, MemoryError> {
        let (record, _) = self.find_episode_record(source_episode).await?;
        Ok(record
            .as_ref()
            .and_then(|map| map.get("project"))
            .and_then(crate::service::value_helpers::string_from_value))
    }
}

impl crate::service::apps::graph::GraphContext for ServiceContext {
    fn app_store(&self) -> crate::storage::AppStoreClient {
        crate::storage::AppStoreClient::new(self.db_client.clone())
    }
    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}
