//! Tenant Registry and its durable SurrealDB composition seams.
//!
//! `InMemoryStore` is available only to tests and test-fixture builds. Production
//! startup must construct `SurrealRegistryStore` and provide a privileged engine
//! explicitly; there is no silent in-memory fallback.

pub mod account;
pub mod migrations;
pub mod models;
pub mod plan;
pub mod provisioning;
pub mod storage;

pub mod surreal_store;

pub use storage::{RegistryStore, SurrealRegistryStore};

#[cfg(any(test, feature = "test-fixtures"))]
pub use storage::InMemoryStore;

use std::sync::{Arc, OnceLock};

use serde_json::Value;
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
    /// Test-only convenience: bind the engine to a fresh
    /// `use_ns(<namespace>).use_db("memory")` and return a
    /// `SurrealDbClient` ready to wrap in a `BoundDbClient`.
    /// Mirrors what `RegistryHandle::bind` does in production.
    /// The Task 7 integration suite uses this to construct
    /// two independent `BoundDbClient` handles against the same
    /// engine.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub async fn bind_to_test_namespace(
        self,
        namespace: &str,
    ) -> Arc<crate::storage::client::SurrealDbClient> {
        use crate::storage::client::SurrealDbClient;
        match self {
            Self::LocalMem(db) => {
                let session = Arc::clone(&db).as_ref().clone();
                session
                    .use_ns(namespace)
                    .use_db("memory")
                    .await
                    .expect("mem bind");
                Arc::new(SurrealDbClient::from_prebound_mem(
                    session, namespace, "warn",
                ))
            }
            Self::Local(db) => {
                let session = Arc::clone(&db).as_ref().clone();
                session
                    .use_ns(namespace)
                    .use_db("memory")
                    .await
                    .expect("rocksdb bind");
                Arc::new(SurrealDbClient::from_prebound(session, namespace, "warn"))
            }
            Self::Remote(_db) => {
                // bind_to_test_namespace is for embedded engines only;
                // the integration suites that need a remote session
                // should build it themselves.
                panic!("bind_to_test_namespace is for embedded engines only")
            }
        }
    }

    /// Bind the engine to a tenant namespace and return a
    /// `SurrealDbClient` ready for queries. The cleanup
    /// scheduler uses this to issue per-tenant DELETEs
    /// against `app_session`.
    pub async fn list_namespaces(&self) -> Result<Vec<String>, crate::error::MemoryError> {
        async fn root_info<C: surrealdb::Connection>(
            db: &Surreal<C>,
        ) -> Result<Value, crate::error::MemoryError> {
            let mut response = db.query("INFO FOR ROOT").await.map_err(|error| {
                crate::error::MemoryError::Storage(format!("root namespace probe failed: {error}"))
            })?;
            let errors = response.take_errors();
            if !errors.is_empty() {
                return Err(crate::error::MemoryError::Storage(
                    "root namespace probe returned a database error".into(),
                ));
            }
            response
                .take::<Option<Value>>(0)
                .map_err(|error| {
                    crate::error::MemoryError::Storage(format!(
                        "root namespace probe decode failed: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    crate::error::MemoryError::Storage(
                        "root namespace probe returned no data".into(),
                    )
                })
        }
        let info = match self {
            Self::Remote(db) => root_info(db).await?,
            Self::Local(db) | Self::LocalMem(db) => root_info(db).await?,
        };
        let namespaces = info
            .get("namespaces")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                crate::error::MemoryError::Storage(
                    "root namespace probe returned no namespace catalog".into(),
                )
            })?;
        Ok(namespaces.keys().cloned().collect())
    }

    pub async fn bind(
        &self,
        tenant: &super::registry::models::Tenant,
    ) -> Result<Arc<crate::storage::client::SurrealDbClient>, crate::error::MemoryError> {
        use crate::storage::client::SurrealDbClient;
        // `use_ns/use_db` is a connection-session mutation. Surreal clones
        // isolate the resulting bound adapter, but concurrent binds on the
        // same underlying local/remote engine can conflict while the session
        // command is being applied. Serialize only that short bind operation;
        // tenant queries remain concurrent after each clone is bound.
        static BIND_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        let _bind_guard = BIND_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
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
    pub fn tenant_engine(&self) -> Result<PrivilegedEngine, crate::error::MemoryError> {
        let engine = self.engine.clone().ok_or_else(|| {
            crate::error::MemoryError::Storage(
                "registry has no privileged engine; wire PrivilegedEngine via with_engine".into(),
            )
        })?;
        Ok((*engine).clone())
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

    /// Ensure the deployment's version-1 signup plan exists without
    /// overwriting an operator-managed durable plan.
    pub async fn ensure_plan(&self, plan: &models::Plan) -> Result<(), crate::error::MemoryError> {
        self.store.ensure_plan(plan).await
    }

    /// Build a handle from a store without an engine.
    /// This is intentionally available only to tests and fixture
    /// builds: production tenant activation requires an explicit
    /// privileged engine.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn from_store(store: Arc<dyn RegistryStore>) -> Self {
        Self {
            store,
            engine: None,
        }
    }

    /// Build the production handle from the durable registry store and
    /// the separately configured privileged tenant engine.
    pub fn from_durable(store: Arc<dyn RegistryStore>, engine: PrivilegedEngine) -> Self {
        Self {
            store,
            engine: Some(Arc::new(engine)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn privileged_engine_lists_server_namespaces() {
        let db = Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem engine");
        storage::ensure_namespace(&db, "control", "registry")
            .await
            .expect("control namespace");
        storage::ensure_namespace(&db, "tns_probe", "memory")
            .await
            .expect("tenant namespace");
        let engine = PrivilegedEngine::LocalMem(Arc::new(db));
        let namespaces = engine.list_namespaces().await.expect("namespace catalog");
        assert!(namespaces.iter().any(|name| name == "tns_probe"));
    }
}
