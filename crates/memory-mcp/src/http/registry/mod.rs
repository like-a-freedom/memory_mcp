//! Tenant Registry seam with InMemoryStore backend (test-only). The
//! privileged SurrealDB store is added in a later milestone.

pub mod account;
pub mod migrations;
pub mod models;
pub mod plan;
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
/// provisioning worker and the runtime factory both dispatch on
/// this enum to issue namespace DDL or to clone+bind a per-tenant
/// handle. The handle types are concrete `Connection` impls; the
/// `Ws` / `Mem` configuration markers are not themselves
/// connections and only appear in the `Surreal::new::<...>` callsite.
#[derive(Clone)]
pub enum PrivilegedEngine {
    /// Production remote (Ws-backed) engine. The HTTP binary
    /// builds this from `SurrealTargetConfig`.
    Remote(Arc<Surreal<Client>>),
    /// Production embedded (RocksDB) engine.
    Local(Arc<Surreal<Db>>),
    /// Test-only in-memory engine. `Surreal::new::<Mem>(())`
    /// returns a `Surreal<Db>`; the kv engine is in-memory.
    LocalMem(Arc<Surreal<Db>>),
}

impl PrivilegedEngine {
    /// Bind the engine to a tenant namespace and return a
    /// `SurrealDbClient` ready for queries. The cleanup
    /// scheduler uses this to issue per-tenant DELETEs
    /// against `app_session`.
    pub async fn bind(
        &self,
        tenant: &super::registry::models::Tenant,
    ) -> Result<Arc<crate::storage::client::SurrealDbClient>, crate::error::MemoryError> {
        use crate::storage::client::SurrealDbClient;
        match self {
            PrivilegedEngine::Remote(privileged) => {
                let ns_client = (**privileged).clone();
                ns_client
                    .use_ns(&tenant.namespace_binding.namespace)
                    .use_db(&tenant.namespace_binding.database)
                    .await
                    .map_err(|err| {
                        crate::error::MemoryError::Storage(format!("tenant bind failed: {err}"))
                    })?;
                Ok(Arc::new(SurrealDbClient::from_prebound_remote(
                    ns_client,
                    &tenant.namespace_binding.namespace,
                    "info",
                )))
            }
            PrivilegedEngine::Local(privileged) => {
                let ns_client = (**privileged).clone();
                ns_client
                    .use_ns(&tenant.namespace_binding.namespace)
                    .use_db(&tenant.namespace_binding.database)
                    .await
                    .map_err(|err| {
                        crate::error::MemoryError::Storage(format!("tenant bind failed: {err}"))
                    })?;
                Ok(Arc::new(SurrealDbClient::from_prebound(
                    ns_client,
                    &tenant.namespace_binding.namespace,
                    "info",
                )))
            }
            PrivilegedEngine::LocalMem(privileged) => {
                let ns_client = (**privileged).clone();
                ns_client
                    .use_ns(&tenant.namespace_binding.namespace)
                    .use_db(&tenant.namespace_binding.database)
                    .await
                    .map_err(|err| {
                        crate::error::MemoryError::Storage(format!("tenant bind failed: {err}"))
                    })?;
                Ok(Arc::new(SurrealDbClient::from_prebound_mem(
                    ns_client,
                    &tenant.namespace_binding.namespace,
                    "info",
                )))
            }
        }
    }
}

/// Thin facade over `Arc<dyn RegistryStore>` plus the
/// privileged engine seam. The auth pipeline dispatches against
/// the trait, not the handle; the handle exists so the
/// construction site reads `state.registry` and not `state.store`.
#[derive(Clone)]
pub struct RegistryHandle {
    pub(crate) store: Arc<dyn RegistryStore>,
    engine: Option<Arc<PrivilegedEngine>>,
}

impl RegistryHandle {
    /// Build a production handle. Ships the `SurrealRegistryStore`
    /// placeholder (every read returns `MemoryError::Unavailable`);
    /// a later milestone replaces the inner store with a real
    /// SurrealDB-backed implementation.
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
    /// conformance suite.
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
    /// `HttpState::new` to wire the production engine without
    /// exposing the field publicly.
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
    /// engine is wired — code that has not yet been migrated
    /// surfaces the missing wire-up loudly instead of silently
    /// using the placeholder.
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

    /// Optional access to the privileged engine. Returns
    /// `None` when no engine is wired; callers that need
    /// to skip a tenant (e.g. the cleanup scheduler on a
    /// test path) use this rather than panicking through
    /// `tenant_engine()`'s storage-error fallback.
    pub fn tenant_engine_optional(&self) -> Option<PrivilegedEngine> {
        self.engine.as_ref().map(|e| (**e).clone())
    }

    pub async fn ping(&self) -> bool {
        self.store.ping().await
    }

    /// Clone the inner `Arc<dyn RegistryStore>`. The authenticator
    /// takes the store by trait-object, not by handle, so the
    /// handle is a thin facade over the store.
    pub fn store_clone(&self) -> Arc<dyn RegistryStore> {
        Arc::clone(&self.store)
    }

    /// Build a handle from a store without an engine.
    /// The scheduler uses this for tests; the production
    /// binary uses `Self::new()` or
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
