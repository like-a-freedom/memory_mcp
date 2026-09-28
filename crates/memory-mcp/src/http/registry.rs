//! Tenant Registry and its durable SurrealDB composition seams.
//!
//! `InMemoryStore` is available only to tests and test-fixture builds. Production
//! startup must construct `SurrealRegistryStore` and provide a privileged engine
//! explicitly; there is no silent in-memory fallback.

pub mod account;
pub mod migrations;
pub use crate::models::registry as models;
pub mod control_impl;
pub mod plan;
pub mod provisioning;
pub mod storage;

pub mod surreal_store;

pub use storage::{
    AccountStore, ApiKeyStore, BrowserPolicyStore, IdentityStore, ProvisioningStore, SessionStore,
    StoreHealth, SurrealRegistryStore, TenantStore, UsageStore,
};

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
        self.bind_namespace(
            &tenant.namespace_binding.namespace,
            &tenant.namespace_binding.database,
        )
        .await
    }

    /// Bind a namespace by name, for a caller that holds the binding rather
    /// than the whole tenant record.
    ///
    /// The maintenance workers — the deletion-recovery pass, the App Session
    /// sweeper — are given a tenant's namespace binding without the rest of
    /// the record, because that is all their work needs. Reconstructing a
    /// `Tenant` to call [`Self::bind`] would mean fabricating fields the
    /// caller does not have, and for a deleting tenant the status it would
    /// have to invent is the one field that must not be guessed.
    pub async fn bind_namespace(
        &self,
        namespace: &str,
        database: &str,
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
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|err| {
                        crate::error::MemoryError::Storage(format!("tenant bind failed: {err}"))
                    })?;
                Ok(Arc::new(SurrealDbClient::from_prebound_remote(
                    ns_client, namespace, "info",
                )))
            }
            PrivilegedEngine::Local(privileged) => {
                let ns_client = (**privileged).clone();
                ns_client
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|err| {
                        crate::error::MemoryError::Storage(format!("tenant bind failed: {err}"))
                    })?;
                Ok(Arc::new(SurrealDbClient::from_prebound(
                    ns_client, namespace, "info",
                )))
            }
            PrivilegedEngine::LocalMem(privileged) => {
                let ns_client = (**privileged).clone();
                ns_client
                    .use_ns(namespace)
                    .use_db(database)
                    .await
                    .map_err(|err| {
                        crate::error::MemoryError::Storage(format!("tenant bind failed: {err}"))
                    })?;
                Ok(Arc::new(SurrealDbClient::from_prebound_mem(
                    ns_client, namespace, "info",
                )))
            }
        }
    }
}

/// The registry's stores, grouped by canonical table owner.
///
/// Replaces the single omnibus trait that every caller used to be handed. A
/// consumer that needs one owner's tables takes that owner's trait; a consumer
/// that genuinely spans two — provisioning, which reads a tenant's plan and
/// its account to decide whether it may issue a key — holds this and names the
/// two capabilities it actually uses.
///
/// The two adapters, `InMemoryStore` and `SurrealRegistryStore`, satisfy every
/// field from one object, so constructing this is a fan-out rather than a
/// second set of implementations to keep in step.
#[derive(Clone)]
pub struct RegistryStores {
    pub accounts: Arc<dyn AccountStore>,
    pub identities: Arc<dyn IdentityStore>,
    pub tenants: Arc<dyn TenantStore>,
    pub api_keys: Arc<dyn ApiKeyStore>,
    pub provisioning: Arc<dyn ProvisioningStore>,
    pub usage: Arc<dyn UsageStore>,
    pub sessions: Arc<dyn SessionStore>,
    pub browser_policy: Arc<dyn BrowserPolicyStore>,
}

impl RegistryStores {
    /// The account store. A caller that needs this and one other owner holds
    /// the struct; a caller that needs only this takes the field.
    pub fn accounts(&self) -> Arc<dyn AccountStore> {
        Arc::clone(&self.accounts)
    }

    pub fn identities(&self) -> Arc<dyn IdentityStore> {
        Arc::clone(&self.identities)
    }

    pub fn tenants(&self) -> Arc<dyn TenantStore> {
        Arc::clone(&self.tenants)
    }

    pub fn api_keys(&self) -> Arc<dyn ApiKeyStore> {
        Arc::clone(&self.api_keys)
    }

    pub fn provisioning(&self) -> Arc<dyn ProvisioningStore> {
        Arc::clone(&self.provisioning)
    }

    pub fn usage(&self) -> Arc<dyn UsageStore> {
        Arc::clone(&self.usage)
    }

    pub fn sessions(&self) -> Arc<dyn SessionStore> {
        Arc::clone(&self.sessions)
    }

    pub fn browser_policy(&self) -> Arc<dyn BrowserPolicyStore> {
        Arc::clone(&self.browser_policy)
    }

    /// Fan a backend out into the owner stores.
    ///
    /// Public so a test that seeds an in-memory backend can hand the same
    /// adapter constructors production composition uses, rather than a
    /// parallel wiring path that only exists under `cfg(test)`.
    pub fn from_backend(backend: Arc<dyn RegistryBackend>) -> Self {
        // Trait upcasting in the direction that actually works: the concrete
        // adapter is converted to each owner trait in turn, so each field is a
        // `dyn OwnerStore` view of one shared object. The compiler checks
        // every coercion here, which is the point — an owner trait that gained
        // a method its impl does not have stops compiling at this line rather
        // than at whichever context later asked for it.
        Self {
            accounts: backend.clone(),
            identities: backend.clone(),
            tenants: backend.clone(),
            api_keys: backend.clone(),
            provisioning: backend.clone(),
            usage: backend.clone(),
            sessions: backend.clone(),
            browser_policy: backend.clone(),
        }
    }
}

/// A store that satisfies every registry table-owner trait.
///
/// Both adapters implement the full set, so production composition has one
/// implementation of this. It exists so `RegistryStores::from_backend` can
/// upcast once per field rather than needing a blanket impl per adapter; a
/// new adapter that implements only part of the registry is a compile error
/// here rather than a runtime surprise in whichever context asked for the part
/// it does not have.
pub trait RegistryBackend:
    StoreHealth
    + AccountStore
    + IdentityStore
    + TenantStore
    + ApiKeyStore
    + ProvisioningStore
    + UsageStore
    + SessionStore
    + BrowserPolicyStore
{
}

impl<T> RegistryBackend for T where
    T: StoreHealth
        + AccountStore
        + IdentityStore
        + TenantStore
        + ApiKeyStore
        + ProvisioningStore
        + UsageStore
        + SessionStore
        + BrowserPolicyStore
{
}

/// Thin facade over the registry's stores plus the privileged engine seam.
/// The auth pipeline dispatches against the owner traits, not the handle; the
/// handle exists so the construction site reads `state.registry` and not
/// `state.registry_stores`.
#[derive(Clone)]
pub struct RegistryHandle {
    pub(crate) stores: RegistryStores,
    health: Arc<dyn StoreHealth>,
    #[cfg(feature = "control-plane")]
    pub(crate) local_admin_store:
        Option<Arc<dyn crate::service::local_admin::contracts::LocalAdminStore>>,
    engine: Option<Arc<PrivilegedEngine>>,
}

impl RegistryHandle {
    /// Build a handle backed by the in-memory test backend.
    /// Feature-gated on `test-fixtures` so a production build
    /// cannot accidentally swap it in.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn in_memory() -> Self {
        Self::from_backend(Arc::new(InMemoryStore::default()), None)
    }

    /// Build a handle backed by the in-memory test backend
    /// AND a privileged in-memory engine. The test fixture is
    /// the only call site; production code never wires the
    /// engine.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn in_memory_with_mem_engine(privileged: Arc<Surreal<Db>>) -> Self {
        Self::from_backend_with_engine(
            Arc::new(InMemoryStore::default()),
            PrivilegedEngine::LocalMem(privileged),
        )
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
        Self::from_backend_with_engine(
            Arc::new(InMemoryStore::default()),
            PrivilegedEngine::LocalMem(db_arc),
        )
    }

    /// The single construction path every backend goes through.
    ///
    /// Test-only: production composes with [`Self::from_durable`], which
    /// also supplies the privileged engine. Keeping the engine-free path
    /// behind the same gate as the in-memory backend means a production
    /// build cannot construct a handle with no tenant engine.
    #[cfg(any(test, feature = "test-fixtures"))]
    fn from_backend(
        backend: Arc<dyn RegistryBackend>,
        #[cfg(feature = "control-plane")] local_admin_store: Option<
            Arc<dyn crate::service::local_admin::contracts::LocalAdminStore>,
        >,
    ) -> Self {
        Self {
            stores: RegistryStores::from_backend(backend.clone()),
            health: backend,
            #[cfg(feature = "control-plane")]
            local_admin_store,
            engine: None,
        }
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    fn from_backend_with_engine(
        backend: Arc<dyn RegistryBackend>,
        engine: PrivilegedEngine,
    ) -> Self {
        Self {
            stores: RegistryStores::from_backend(backend.clone()),
            health: backend,
            #[cfg(feature = "control-plane")]
            local_admin_store: None,
            engine: Some(Arc::new(engine)),
        }
    }

    /// The owner stores, for a caller that genuinely spans two of them.
    pub fn stores(&self) -> &RegistryStores {
        &self.stores
    }

    pub fn accounts(&self) -> Arc<dyn AccountStore> {
        Arc::clone(&self.stores.accounts)
    }

    pub fn identities(&self) -> Arc<dyn IdentityStore> {
        Arc::clone(&self.stores.identities)
    }

    pub fn tenants(&self) -> Arc<dyn TenantStore> {
        Arc::clone(&self.stores.tenants)
    }

    pub fn api_keys(&self) -> Arc<dyn ApiKeyStore> {
        Arc::clone(&self.stores.api_keys)
    }

    pub fn provisioning(&self) -> Arc<dyn ProvisioningStore> {
        Arc::clone(&self.stores.provisioning)
    }

    pub fn usage(&self) -> Arc<dyn UsageStore> {
        Arc::clone(&self.stores.usage)
    }

    pub fn sessions(&self) -> Arc<dyn SessionStore> {
        Arc::clone(&self.stores.sessions)
    }

    pub fn browser_policy(&self) -> Arc<dyn BrowserPolicyStore> {
        Arc::clone(&self.stores.browser_policy)
    }

    /// Set the privileged engine after construction. Used by
    /// `HttpState::new` to wire the production engine without
    /// exposing the field publicly.
    pub fn with_engine(mut self, engine: Arc<PrivilegedEngine>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// Replace the underlying backend. Used by tests that need
    /// an in-memory backend seeded with specific data; the
    /// default `in_memory()` constructor creates a fresh
    /// backend that the test cannot reach.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn with_inner_store(mut self, store: Arc<dyn RegistryBackend>) -> Self {
        self.stores = RegistryStores::from_backend(store.clone());
        self.health = store;
        self
    }

    /// Clone the inner local-admin store if available.
    #[cfg(feature = "control-plane")]
    pub fn local_admin_store_clone(
        &self,
    ) -> Option<Arc<dyn crate::service::local_admin::contracts::LocalAdminStore>> {
        self.local_admin_store.as_ref().map(Arc::clone)
    }

    /// Attach a local-admin store to a handle that was built without
    /// one. Used by the feature-gated test builder to assemble an
    /// explicitly durable local composition; production composition
    /// always supplies the store through [`Self::from_durable`].
    #[cfg(all(any(test, feature = "test-fixtures"), feature = "control-plane"))]
    pub fn with_local_admin_store(
        mut self,
        store: Arc<dyn crate::service::local_admin::contracts::LocalAdminStore>,
    ) -> Self {
        self.local_admin_store = Some(store);
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
        self.health.ping().await
    }

    /// Ensure the deployment's version-1 signup plan exists without
    /// overwriting an operator-managed durable plan.
    pub async fn ensure_plan(&self, plan: &models::Plan) -> Result<(), crate::error::MemoryError> {
        self.stores.usage.ensure_plan(plan).await
    }

    /// Ensure the local browser-authentication plan exists and has not
    /// drifted from the configured limits. Returns the durable plan.
    pub async fn ensure_local_plan(
        &self,
        plan: &models::Plan,
    ) -> Result<models::Plan, crate::error::MemoryError> {
        self.stores.usage.ensure_local_plan(plan).await
    }

    /// Build a handle from a store without an engine.
    /// This is intentionally available only to tests and fixture
    /// builds: production tenant activation requires an explicit
    /// privileged engine.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn from_store(store: Arc<dyn RegistryBackend>) -> Self {
        Self::from_backend(
            store,
            #[cfg(feature = "control-plane")]
            None,
        )
    }

    /// Build the production handle from the durable registry store and
    /// the separately configured privileged tenant engine.
    pub fn from_durable(
        store: Arc<dyn RegistryBackend>,
        #[cfg(feature = "control-plane")] local_admin_store: Option<
            Arc<dyn crate::service::local_admin::contracts::LocalAdminStore>,
        >,
        engine: PrivilegedEngine,
    ) -> Self {
        Self {
            stores: RegistryStores::from_backend(store.clone()),
            health: store,
            #[cfg(feature = "control-plane")]
            local_admin_store,
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
