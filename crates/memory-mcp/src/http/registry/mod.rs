//! Tenant Registry seam (ADR-0052). Phase 4 production
//! placeholder + InMemoryStore backend (test-only). The
//! privileged SurrealDB store is added in Task 5.x.

pub mod account;
pub mod migrations;
pub mod models;
pub mod provisioning;
pub mod storage;

pub use storage::{RegistryStore, SurrealRegistryStore};

#[cfg(any(test, feature = "test-fixtures"))]
pub use storage::InMemoryStore;

use std::sync::Arc;

/// Thin facade over `Arc<dyn RegistryStore>`. The auth pipeline
/// (Phase 4 Task 4.6) dispatches against the trait, not the
/// handle; the handle exists so the construction site reads
/// `state.registry` and not `state.store`.
#[derive(Clone)]
pub struct RegistryHandle {
    pub(crate) store: Arc<dyn RegistryStore>,
}

impl RegistryHandle {
    /// Build a production handle. Phase 4 ships the
    /// `SurrealRegistryStore` placeholder (every read returns
    /// `MemoryError::Unavailable`); Task 5.x replaces the inner
    /// store with a real SurrealDB-backed implementation.
    pub fn new() -> Self {
        Self {
            store: Arc::new(SurrealRegistryStore::new()),
        }
    }

    /// Build a handle backed by the in-memory test backend.
    /// Feature-gated on `test-fixtures` so a production build
    /// cannot accidentally swap it in.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(InMemoryStore::default()),
        }
    }

    pub async fn ping(&self) -> bool {
        self.store.ping().await
    }

    /// Clone the inner `Arc<dyn RegistryStore>`. The authenticator
    /// (Task 4.4) takes the store by trait-object, not by handle,
    /// so the handle is a thin facade over the store.
    pub fn store_clone(&self) -> Arc<dyn RegistryStore> {
        Arc::clone(&self.store)
    }
}

impl Default for RegistryHandle {
    fn default() -> Self {
        Self::new()
    }
}
