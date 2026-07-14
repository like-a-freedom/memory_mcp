use std::sync::Arc;

use lru::LruCache;
use tokio::sync::RwLock;

use crate::service::cache::CacheKey;
use crate::service::util::RateLimiter;
use crate::storage::DbClient;

use crate::models::AssembledContextItem;

/// Shared context passed to capability modules.
///
/// Contains the infrastructure dependencies that all capabilities need,
/// without exposing the full `MemoryService` surface.
pub struct ServiceContext {
    pub db_client: Arc<dyn DbClient>,
    pub namespaces: Vec<String>,
    pub rate_limiter: Arc<RateLimiter>,
    pub context_cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
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
        super::MemoryError,
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
    ) -> Result<(), super::MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(super::MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
    }
}
