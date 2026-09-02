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
use super::registry::{RegistryHandle, SurrealRegistryStore};

/// The production adapter bundle: durable control Registry plus the
/// tenant migration worker. Selected once at startup; request
/// handling never replaces either member.
pub struct HttpProductionComposition {
    pub registry: RegistryHandle,
    pub tenant_migrations: Arc<dyn ApplyMigrations>,
}

impl HttpProductionComposition {
    /// Connect the control Registry and the privileged tenant engine
    /// from config. Embedded RocksDB permits one process handle per
    /// path, so both targets pointing at the same endpoint share one
    /// connection; remote deployments may use independent connections.
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
        })
    }
}

/// The test-only adapter bundle. Named explicitly by tests; enabling
/// `test-fixtures` never substitutes these values for production
/// composition (ADR-0053).
#[cfg(any(test, feature = "test-fixtures"))]
pub struct HttpTestComposition {
    pub registry: RegistryHandle,
    pub tenant_migrations: Arc<dyn ApplyMigrations>,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpTestComposition {
    pub async fn in_memory() -> Self {
        Self {
            registry: RegistryHandle::in_memory_with_default_mem_engine().await,
            tenant_migrations: Arc::new(super::leases::migration::NoopMigrations),
        }
    }
}
