//! Explicit HTTP composition values (ADR-0053).
//!
//! Cargo features gate compile-time capabilities; they never select
//! storage or migration adapters. Production composition always
//! connects the durable Registry and the tenant migration adapter
//! from validated `HttpConfig`. Test composition names the in-memory
//! adapters explicitly so a test states which adapters it uses.

use std::sync::Arc;

use crate::error::MemoryError;

use super::config::HttpConfig;
use super::leases::migration::{ApplyMigrations, SurrealTenantMigrations};
use super::registry::{RegistryBackend, RegistryHandle, SurrealRegistryStore};
use crate::platform::fault_injection::{FaultInjector, NoFaults};

/// The production adapter bundle: durable control Registry plus the
/// tenant migration worker. Selected once at startup; request
/// handling never replaces either member.
pub struct HttpProductionComposition {
    pub registry: RegistryHandle,
    pub tenant_migrations: Arc<dyn ApplyMigrations>,
    /// The fault injector threaded into the scheduler and the deletion
    /// worker. Production always installs [`NoFaults`]; tests may
    /// substitute `FailOnceAt` via the `test-fixtures` feature.
    pub fault_injector: Arc<dyn FaultInjector>,
}

impl HttpProductionComposition {
    /// Connect the control Registry and the privileged tenant engine
    /// from config. The physical local-store inventory runs before either
    /// engine opens; equivalent local paths with matching credentials share
    /// one process handle. Remote deployments may use independent connections.
    /// The fault injector defaults to [`NoFaults`]; callers wanting a
    /// test injector should construct the composition manually.
    pub async fn connect(config: &HttpConfig) -> Result<Self, MemoryError> {
        let inventory = super::storage_inventory::preflight(&config.control_db, &config.tenant_db)?;
        let store = SurrealRegistryStore::connect(&config.control_db)
            .await
            .map_err(|err| {
                MemoryError::Storage(format!("control registry storage connect failed: {err}"))
            })?;
        let engine = if inventory.share_control_engine_with_tenants {
            store.privileged_engine()
        } else {
            SurrealRegistryStore::connect_engine(&config.tenant_db)
                .await
                .map_err(|err| {
                    MemoryError::Storage(format!("tenant engine storage connect failed: {err}"))
                })?
        };
        let migrations = Arc::new(SurrealTenantMigrations::new(engine.clone()));
        // The concrete durable store is shared by two trait objects: the
        // registry surface used by request handling and (under
        // `control-plane`) the local-admin surface used by the browser
        // authentication routes. Trait objects cannot be downcast, so
        // both clones are taken here, once, at composition time.
        let store = Arc::new(store);
        let registry_backend: Arc<dyn RegistryBackend> = store.clone();
        #[cfg(feature = "control-plane")]
        let local_admin_store = store;
        Ok(Self {
            registry: RegistryHandle::from_durable(
                registry_backend,
                #[cfg(feature = "control-plane")]
                Some(local_admin_store),
                engine,
            ),
            tenant_migrations: migrations,
            fault_injector: Arc::new(NoFaults),
        })
    }

    /// Connect with an explicit fault injector. Production code uses
    /// [`Self::connect`]; tests construct the composition manually so
    /// the scheduler + deletion worker run with a deterministic
    /// injector.
    pub async fn connect_with_injector(
        config: &HttpConfig,
        fault_injector: Arc<dyn FaultInjector>,
    ) -> Result<Self, MemoryError> {
        let mut composition = Self::connect(config).await?;
        composition.fault_injector = fault_injector;
        Ok(composition)
    }
}

/// The test-only adapter bundle. Named explicitly by tests; enabling
/// `test-fixtures` never substitutes these values for production
/// composition (ADR-0053).
#[cfg(any(test, feature = "test-fixtures"))]
pub struct HttpTestComposition {
    pub registry: RegistryHandle,
    pub tenant_migrations: Arc<dyn ApplyMigrations>,
    pub fault_injector: Arc<dyn FaultInjector>,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpTestComposition {
    pub async fn in_memory() -> Self {
        Self {
            registry: RegistryHandle::in_memory_with_default_mem_engine().await,
            tenant_migrations: Arc::new(super::leases::migration::NoopMigrations),
            fault_injector: Arc::new(NoFaults),
        }
    }

    pub fn with_fault_injector(mut self, injector: Arc<dyn FaultInjector>) -> Self {
        self.fault_injector = injector;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_test_composition_uses_an_in_memory_registry() {
        let composition = HttpTestComposition::in_memory().await;

        // A ready-tenant listing against an in-memory registry is a cheap
        // proof the engine is actually wired, not just constructed.
        let observed = composition
            .registry
            .tenants()
            .list_ready_tenants(None, 10)
            .await;

        assert!(observed.is_ok());
    }

    #[tokio::test]
    async fn the_test_composition_starts_with_no_faults_injected() {
        let composition = HttpTestComposition::in_memory().await;

        let observed = composition
            .fault_injector
            .hit(crate::platform::fault_injection::FaultPoint::TaskClaimed);

        assert!(observed.is_ok(), "the default injector must never fire");
    }

    #[tokio::test]
    async fn a_substituted_fault_injector_is_installed() {
        let composition = HttpTestComposition::in_memory()
            .await
            .with_fault_injector(Arc::new(crate::platform::fault_injection::FailOnceAt::new(
                crate::platform::fault_injection::FaultPoint::TaskClaimed,
            )));

        let observed = composition
            .fault_injector
            .hit(crate::platform::fault_injection::FaultPoint::TaskClaimed);

        assert!(observed.is_err(), "the substituted injector must fire");
    }

    #[tokio::test]
    async fn the_test_composition_applies_its_tenant_migrations() {
        let composition = HttpTestComposition::in_memory().await;
        let namespace = format!("composition_{}", uuid::Uuid::new_v4().simple());

        let observed = composition
            .tenant_migrations
            .apply_migrations(&namespace, "memory")
            .await;

        assert!(observed.is_ok());
    }

    #[tokio::test]
    async fn local_control_and_tenant_aliases_share_one_surreal_kv_engine() {
        let directory = tempfile::tempdir().expect("temporary storage parent");
        let store_path = directory.path().join("shared");
        let mut config = HttpConfig::default_for_test();
        config.control_db.url = format!("surrealkv://{}", store_path.display());
        config.tenant_db.url = format!("rocksdb://{}", store_path.display());
        config.tenant_db.database = "tenant".into();
        config.tenant_db.namespace = "tenant".into();

        let composition = HttpProductionComposition::connect(&config).await;

        assert!(
            composition.is_ok(),
            "equivalent local aliases must reuse the open engine"
        );
    }

    #[tokio::test]
    async fn overlapping_local_store_paths_are_rejected_before_any_engine_opens() {
        let directory = tempfile::tempdir().expect("temporary storage parent");
        let control_path = directory.path().join("control");
        let tenant_path = control_path.join("tenant");
        let mut config = HttpConfig::default_for_test();
        config.control_db.url = format!("surrealkv://{}", control_path.display());
        config.tenant_db.url = format!("surrealkv://{}", tenant_path.display());
        config.tenant_db.database = "tenant".into();
        config.tenant_db.namespace = "tenant".into();

        let result = HttpProductionComposition::connect(&config).await;

        assert!(
            matches!(result, Err(MemoryError::ConfigInvalid(_))),
            "overlapping physical stores must be rejected as invalid configuration"
        );
        assert!(
            !control_path.exists(),
            "preflight must reject overlapping stores before creating the control path"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_control_and_tenant_symlink_aliases_share_one_engine() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary storage parent");
        let store_path = directory.path().join("shared");
        std::fs::create_dir(&store_path).expect("create physical store path");
        let alias_path = directory.path().join("alias");
        symlink(&store_path, &alias_path).expect("create path alias");
        let mut config = HttpConfig::default_for_test();
        config.control_db.url = format!("surrealkv://{}", store_path.display());
        config.tenant_db.url = format!("rocksdb://{}", alias_path.display());
        config.tenant_db.database = "tenant".into();
        config.tenant_db.namespace = "tenant".into();

        let composition = HttpProductionComposition::connect(&config).await;

        assert!(
            composition.is_ok(),
            "symlink-equivalent local targets must reuse the open engine"
        );
    }

    #[tokio::test]
    async fn identical_control_and_tenant_bindings_are_rejected_before_opening_storage() {
        let directory = tempfile::tempdir().expect("temporary storage parent");
        let store_path = directory.path().join("shared");
        let mut config = HttpConfig::default_for_test();
        config.control_db.url = format!("surrealkv://{}", store_path.display());
        config.tenant_db.url = config.control_db.url.clone();
        config.tenant_db.database = config.control_db.database.clone();
        config.tenant_db.namespace = config.control_db.namespace.clone();

        let result = HttpProductionComposition::connect(&config).await;

        assert!(
            matches!(result, Err(MemoryError::ConfigInvalid(_))),
            "control and tenant cannot bind the same database"
        );
        assert!(
            !store_path.exists(),
            "invalid bindings must be rejected before any engine creates its path"
        );
    }

    #[tokio::test]
    async fn one_physical_store_cannot_have_conflicting_credentials() {
        let directory = tempfile::tempdir().expect("temporary storage parent");
        let store_path = directory.path().join("shared");
        let mut config = HttpConfig::default_for_test();
        config.control_db.url = format!("surrealkv://{}", store_path.display());
        config.tenant_db.url = config.control_db.url.clone();
        config.tenant_db.username = "another-user".into();
        config.tenant_db.password = "another-password".into();
        config.tenant_db.database = "tenant".into();
        config.tenant_db.namespace = "tenant".into();

        let result = HttpProductionComposition::connect(&config).await;

        assert!(
            matches!(result, Err(MemoryError::ConfigInvalid(_))),
            "conflicting credentials for one physical path must fail preflight"
        );
        assert!(
            !store_path.exists(),
            "credential conflicts must be rejected before the engine creates its path"
        );
    }
}
