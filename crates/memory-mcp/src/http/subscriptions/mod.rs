//! Durable subscriptions and transactional outbox (spec §11).
//!
//! The outbox ensures every canonical mutation atomically
//! increments a tenant-local sequence and emits a
//! `TenantChangeEvent`. Subscriptions/listen reads from
//! this log. The cross-replica wake uses SurrealDB LIVE
//! queries with outbox-based polling fallback.

pub mod outbox;
pub mod scheduler;
pub mod stream;

use crate::error::MemoryError;

use crate::http::subscriptions::outbox::TenantChangeEvent;
use crate::storage::client::BoundDbClient;

/// Tenant-bound subscription store. The handler queries
/// this for new events to deliver to active listeners.
#[async_trait::async_trait]
pub trait SubscriptionStore: Send + Sync + 'static {
    /// Current durable sequence number.
    async fn current_sequence(&self) -> Result<u64, MemoryError>;
    /// Fetch events after `after_sequence`. Returns at
    /// most 256 events.
    async fn next_batch(&self, after_sequence: u64) -> Result<Vec<TenantChangeEvent>, MemoryError>;
}

/// Durable subscription store over the tenant namespace's
/// outbox tables.
pub struct DurableSubscriptionStore {
    db: std::sync::Arc<BoundDbClient>,
}

impl DurableSubscriptionStore {
    pub fn new(db: std::sync::Arc<BoundDbClient>) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl SubscriptionStore for DurableSubscriptionStore {
    async fn current_sequence(&self) -> Result<u64, MemoryError> {
        let result = self
            .db
            .query("SELECT value FROM `tenant_change_sequence` LIMIT 1;", None)
            .await?;
        let val = result
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("value"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Ok(val)
    }

    async fn next_batch(&self, after_sequence: u64) -> Result<Vec<TenantChangeEvent>, MemoryError> {
        use serde_json::json;

        let result = self
            .db
            .query(
                "SELECT * FROM tenant_change_event \
                 WHERE event_seq > $after \
                 ORDER BY event_seq ASC \
                 LIMIT 256;",
                Some(json!({ "after": after_sequence })),
            )
            .await?;
        let rows: Vec<TenantChangeEvent> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("subscription batch parse: {e}")))?;
        Ok(rows)
    }
}
