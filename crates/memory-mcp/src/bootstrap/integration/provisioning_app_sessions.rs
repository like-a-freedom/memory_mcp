//! Temporary bootstrap adapter binding the durable
//! `AppSessionStore` to the public provisioning App
//! Session interface. The tenant binding is captured at
//! construction so the public port stays tenant-free.
//!
//! Expiry removal: Phase 4, when the MCP request path
//! depends on `provisioning::api` App Session use cases
//! instead of the store's typed surface.

use std::sync::Arc;

use crate::MemoryError;
use crate::http::app_sessions::store::{
    ABSOLUTE_EXPIRY_SECS, AppSessionStore, IDLE_EXPIRY_SECS, MAX_OPEN_PER_TENANT,
};
use crate::provisioning::api::{
    AppSessionPersistence, AppSessionRecord, OpenAppSessionCommand, open_app_session,
};

/// Durable App Session persistence for one tenant.
pub struct TenantAppSessionAdapter {
    store: Arc<AppSessionStore>,
    tenant_id: String,
}

impl TenantAppSessionAdapter {
    pub fn new(store: Arc<AppSessionStore>, tenant_id: impl Into<String>) -> Self {
        Self {
            store,
            tenant_id: tenant_id.into(),
        }
    }
}

#[async_trait::async_trait]
impl AppSessionPersistence for TenantAppSessionAdapter {
    async fn open(
        &self,
        app: &str,
        payload: serde_json::Value,
        max_open_per_tenant: u32,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<AppSessionRecord, MemoryError> {
        let max_open_per_tenant = if max_open_per_tenant == 0 {
            MAX_OPEN_PER_TENANT as u32
        } else {
            max_open_per_tenant
        };
        let (handle, version) = self
            .store
            .open_with_limit(&self.tenant_id, app, payload.clone(), max_open_per_tenant)
            .await?;
        Ok(AppSessionRecord {
            handle,
            app: app.to_owned(),
            version,
            payload,
            idle_expiry: now + chrono::Duration::seconds(IDLE_EXPIRY_SECS),
            absolute_expiry: now + chrono::Duration::seconds(ABSOLUTE_EXPIRY_SECS),
        })
    }

    async fn load(&self, handle: &str) -> Result<Option<AppSessionRecord>, MemoryError> {
        let Some(record) = self.store.load(&self.tenant_id, handle).await? else {
            return Ok(None);
        };
        Ok(Some(AppSessionRecord {
            handle: record.handle,
            app: record.app,
            version: record.version,
            payload: record.payload,
            idle_expiry: record.idle_expiry,
            absolute_expiry: record.absolute_expiry,
        }))
    }

    async fn command(
        &self,
        handle: &str,
        expected_version: u64,
        payload: serde_json::Value,
    ) -> Result<u64, MemoryError> {
        self.store
            .command(&self.tenant_id, handle, expected_version, payload)
            .await
    }

    async fn close(&self, handle: &str) -> Result<(), MemoryError> {
        self.store.close(&self.tenant_id, handle).await
    }
}

/// Open a session for a tenant through the public use case.
pub async fn open_tenant_app_session(
    store: &Arc<AppSessionStore>,
    tenant_id: &str,
    command: OpenAppSessionCommand,
) -> Result<AppSessionRecord, MemoryError> {
    let adapter = TenantAppSessionAdapter::new(Arc::clone(store), tenant_id);
    open_app_session(&adapter, command).await
}

/// Read a session for a tenant through the public use case.
pub async fn read_tenant_app_session(
    store: &Arc<AppSessionStore>,
    tenant_id: &str,
    handle: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<crate::provisioning::api::AppSessionView>, MemoryError> {
    let adapter = TenantAppSessionAdapter::new(Arc::clone(store), tenant_id);
    crate::provisioning::api::read_app_session(
        &adapter,
        &crate::provisioning::api::AppSessionCommand {
            handle: handle.to_owned(),
        },
        now,
    )
    .await
}

/// Apply a command for a tenant through the public use case.
pub async fn write_tenant_app_session(
    store: &Arc<AppSessionStore>,
    tenant_id: &str,
    handle: &str,
    expected_version: u64,
    payload: serde_json::Value,
) -> Result<u64, MemoryError> {
    let adapter = TenantAppSessionAdapter::new(Arc::clone(store), tenant_id);
    crate::provisioning::api::write_app_session(
        &adapter,
        &crate::provisioning::api::AppSessionCommand {
            handle: handle.to_owned(),
        },
        &crate::provisioning::api::AppSessionUsage {
            expected_version,
            payload,
        },
    )
    .await
}

/// Close a session for a tenant through the public use case.
pub async fn close_tenant_app_session(
    store: &Arc<AppSessionStore>,
    tenant_id: &str,
    handle: &str,
) -> Result<(), MemoryError> {
    let adapter = TenantAppSessionAdapter::new(Arc::clone(store), tenant_id);
    crate::provisioning::api::close_app_session(
        &adapter,
        &crate::provisioning::api::AppSessionCommand {
            handle: handle.to_owned(),
        },
    )
    .await
}
