//! Concrete fact store: owns fact persistence queries without exposing the
//! full `DbClient` surface.
//!
//! Replaces the formerly implicit `DbClient` consumption in
//! `service/fact.rs`.

use std::sync::Arc;

use serde_json::Value;

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Narrow store for fact CRUD.
///
/// Only the constructor and the owner-scoped [`Self::select_fact`]
/// are `pub`, because an external integration test
/// (`tests/typed_record_accessors.rs`) needs them and cannot reach
/// `pub(crate)`. Every other method stays crate-internal, so the
/// write surface is not published at two paths on a library crate.
#[derive(Clone)]
pub struct FactStoreClient {
    db: BoundDbClient,
}

impl FactStoreClient {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub(crate) fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
    }

    /// Returns the persisted record for `fact_id`, or `None` if absent.
    pub(crate) async fn select_one(&self, fact_id: &str) -> Result<Option<Value>, MemoryError> {
        self.db.select_one(fact_id).await
    }

    /// Returns a fact record, refusing any id that does not name a
    /// fact.
    ///
    /// [`Self::select_one`] is retained for the callers inside
    /// this module that already hold a validated id; this is the
    /// owner-scoped entry point for everything else.
    pub async fn select_fact(&self, fact_id: &str) -> Result<Option<Value>, MemoryError> {
        crate::storage::require_record_kind(fact_id, "fact")?;
        self.db.select_one(fact_id).await
    }

    /// Persists a new fact record. Returns `Value::Null` on success.
    pub(crate) async fn create(&self, fact_id: &str, content: Value) -> Result<Value, MemoryError> {
        self.db.create(fact_id, content).await
    }

    #[cfg(feature = "streamable-http")]
    pub(crate) async fn create_with_event(
        &self,
        fact_id: &str,
        content: Value,
    ) -> Result<(), MemoryError> {
        crate::storage::validate_record_id(fact_id)?;
        let (sql, vars) = crate::storage::build_create_query(fact_id, content);
        let mutation = crate::platform::persistence::outbox::TenantMutation::new(sql, vars)?;
        crate::platform::persistence::outbox::commit_tenant_mutation_with_event(
            &self.db,
            mutation,
            crate::platform::persistence::outbox::TenantChangeEvent {
                sequence: 0,
                resource_id: "ui://memory/apps/inspector".into(),
                revision: 1,
                change_kind: "fact_created".into(),
                created_at: chrono::Utc::now(),
            },
            &self.db.fault_injector,
        )
        .await
    }
}
