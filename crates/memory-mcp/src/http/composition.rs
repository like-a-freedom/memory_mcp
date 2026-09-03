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
use super::fault_injection::{FaultInjector, NoFaults};
use super::leases::migration::{ApplyMigrations, SurrealTenantMigrations};
use super::registry::{RegistryHandle, SurrealRegistryStore};

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
    /// from config. Embedded RocksDB permits one process handle per
    /// path, so both targets pointing at the same endpoint share one
    /// connection; remote deployments may use independent connections.
    /// The fault injector defaults to [`NoFaults`]; callers wanting a
    /// test injector should construct the composition manually.
    pub async fn connect(config: &HttpConfig) -> Result<Self, MemoryError> {
        let store = SurrealRegistryStore::connect(&config.control_db)
            .await
            .map_err(|err| {
                MemoryError::Storage(format!("control registry storage connect failed: {err}"))
            })?;
        let engine = if config.control_db.url == config.tenant_db.url
            && config.control_db.username == config.tenant_db.username
            && config.control_db.password == config.tenant_db.password
        {
            store.privileged_engine()
        } else {
            SurrealRegistryStore::connect_engine(&config.tenant_db)
                .await
                .map_err(|err| {
                    MemoryError::Storage(format!("tenant engine storage connect failed: {err}"))
                })?
        };
        let migrations = Arc::new(SurrealTenantMigrations::new(engine.clone()));
        Ok(Self {
            registry: RegistryHandle::from_durable(Arc::new(store), engine),
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
