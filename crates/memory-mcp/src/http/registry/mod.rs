//! Tenant Registry seam (ADR-0052). Phase 3 stub; real store in Phase 4.

pub mod account;
pub mod migrations;
pub mod models;
pub mod provisioning;
pub mod storage;

pub use storage::{RegistryStore, SurrealRegistryStore};

use std::sync::Arc;

/// Phase 3-compatible handle. Holds a typed `Arc<dyn RegistryStore>`
/// so the auth pipeline (Phase 4 Task 4.6) can dispatch against
/// a real backend without changing the router signature.
#[derive(Clone)]
pub struct RegistryHandle {
    pub(crate) store: Arc<dyn RegistryStore>,
}

impl RegistryHandle {
    /// Stub used in Phase 3 when the control store is not wired.
    pub fn stub() -> Self {
        Self {
            store: Arc::new(SurrealRegistryStore::new_unconnected()),
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
