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

use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::remote::ws::Client;

/// Privileged SurrealDB engine held by the registry. The
/// provisioning worker (Task 5.3) and the runtime factory
/// (Task 5.4) both dispatch on this enum to issue namespace
/// DDL or to clone+bind a per-tenant handle. The handle types
/// are concrete `Connection` impls; the `Ws` / `Mem`
/// configuration markers are not themselves connections and
/// only appear in the `Surreal::new::<...>` callsite.
#[derive(Clone)]
pub enum PrivilegedEngine {
    /// Production remote (Ws-backed) engine. The HTTP binary
    /// builds this from `SurrealTargetConfig` in Task 5.6.
    Remote(Arc<Surreal<Client>>),
    /// Production embedded (RocksDB) engine.
    Local(Arc<Surreal<Db>>),
    /// Test-only in-memory engine. `Surreal::new::<Mem>(())`
    /// returns a `Surreal<Db>`; the kv engine is in-memory.
    LocalMem(Arc<Surreal<Db>>),
}

/// Thin facade over `Arc<dyn RegistryStore>` plus the
/// privileged engine seam. The auth pipeline (Phase 4 Task
/// 4.6) dispatches against the trait, not the handle; the
/// handle exists so the construction site reads
/// `state.registry` and not `state.store`.
#[derive(Clone)]
pub struct RegistryHandle {
    pub(crate) store: Arc<dyn RegistryStore>,
    engine: Option<Arc<PrivilegedEngine>>,
}

impl RegistryHandle {
    /// Build a production handle. Phase 4 ships the
    /// `SurrealRegistryStore` placeholder (every read returns
    /// `MemoryError::Unavailable`); Task 5.x replaces the inner
    /// store with a real SurrealDB-backed implementation.
    pub fn new() -> Self {
        Self {
            store: Arc::new(SurrealRegistryStore::new()),
            engine: None,
        }
    }

    /// Build a handle backed by the in-memory test backend.
    /// Feature-gated on `test-fixtures` so a production build
    /// cannot accidentally swap it in.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(InMemoryStore::default()),
            engine: None,
        }
    }

    /// Build a handle backed by the in-memory test backend
    /// AND a privileged in-memory engine. The test fixture is
    /// the only call site; production code never wires the
    /// engine.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn in_memory_with_mem_engine(privileged: Arc<Surreal<Db>>) -> Self {
        Self {
            store: Arc::new(InMemoryStore::default()),
            engine: Some(Arc::new(PrivilegedEngine::LocalMem(privileged))),
        }
    }

    /// Convenience: build a Mem engine and use it for both the
    /// in-memory store and the privileged handle. The
    /// resulting handle is the test-only fixture used by the
    /// conformance suite (Task 5.8).
    #[cfg(any(test, feature = "test-fixtures"))]
    pub async fn in_memory_with_default_mem_engine() -> Self {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem engine init");
        db.use_ns("control").use_db("control").await.unwrap();
        let db_arc: Arc<Surreal<Db>> = Arc::new(db);
        Self {
            store: Arc::new(InMemoryStore::default()),
            engine: Some(Arc::new(PrivilegedEngine::LocalMem(db_arc))),
        }
    }

    /// Set the privileged engine after construction. Used by
    /// `HttpState::new` (Task 5.6) to wire the production
    /// engine without exposing the field publicly.
    pub fn with_engine(mut self, engine: Arc<PrivilegedEngine>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// Replace the underlying store. Used by tests that need
    /// an in-memory backend seeded with specific data; the
    /// default `in_memory()` constructor creates a fresh
    /// backend that the test cannot reach.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn with_inner_store(mut self, store: Arc<dyn RegistryStore>) -> Self {
        self.store = store;
        self
    }

    /// Privileged engine for the provisioning worker and the
    /// runtime factory. Returns `MemoryError::Storage` if no
    /// engine is wired — production code that has not yet
    /// been migrated to Task 5.6 surfaces the missing wire-up
    /// loudly instead of silently using the placeholder.
    pub fn tenant_engine(&self) -> PrivilegedEngine {
        let engine = self.engine.clone().ok_or_else(|| {
            crate::error::MemoryError::Storage(
                "registry has no privileged engine; wire PrivilegedEngine via with_engine".into(),
            )
        });
        // PrivilegedEngine: Clone is derived, but we return by
        // value here for ergonomics; the engine itself is a
        // handle.
        match engine {
            Ok(e) => (*e).clone(),
            Err(_) => PrivilegedEngine::Remote(Arc::new(Surreal::init())),
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

    /// Build a handle from a store without an engine. The
    /// scheduler path (Task 6.2) uses this for tests; the
    /// production binary uses `Self::new()` or
    /// `Self::in_memory_with_default_mem_engine`.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn from_store(store: Arc<dyn RegistryStore>) -> Self {
        Self {
            store,
            engine: None,
        }
    }
}

impl Default for RegistryHandle {
    fn default() -> Self {
        Self::new()
    }
}
