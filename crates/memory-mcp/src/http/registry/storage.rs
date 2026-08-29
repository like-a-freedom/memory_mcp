//! Control-namespace storage trait (ADR-0052, plan §4.1).
//!
//! The trait is the boundary the rest of the HTTP profile (Tasks
//! 4.3–4.7, 5.6, 6.2) depends on. The privileged
//! `SurrealRegistryStore` lives in the same module because the
//! SQL surface is the only thing that varies by backend; the
//! in-memory store for tests is in a sibling module.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::models::*;
use crate::error::MemoryError;

/// Abstract control store. Backed by a privileged SurrealDB
/// credential in production; an in-memory implementation is used
/// in unit tests and embedded conformance runs.
///
/// Methods are named after the records they touch; the SQL
/// implementation does not use the `DbClient` trait because the
/// `DbClient` trait is per-namespace and the registry is
/// multi-record across many tables.
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait RegistryStore: Send + Sync + 'static {
    async fn ping(&self) -> bool;

    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError>;
    /// `subject_verifier` is a keyed blind index; raw OIDC `sub`
    /// is never persisted.
    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &[u8; 32],
    ) -> Result<Option<Account>, MemoryError>;

    async fn find_tenant_by_account(&self, account_id: &str) -> Result<Option<Tenant>, MemoryError>;
    async fn find_tenant_by_id(&self, tenant_id: &str) -> Result<Option<Tenant>, MemoryError>;

    async fn find_api_key(&self, key_id: &str) -> Result<Option<ApiKey>, MemoryError>;
    async fn write_api_key(&self, key: &ApiKey) -> Result<(), MemoryError>;
    async fn list_api_keys(&self, account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError>;
    async fn revoke_api_key(&self, account_id: &str, key_id: &str) -> Result<(), MemoryError>;
    async fn touch_api_key(&self, key_id: &str, used_at: DateTime<Utc>) -> Result<(), MemoryError>;

    async fn write_account(&self, account: &Account) -> Result<(), MemoryError>;
    async fn write_tenant(&self, tenant: &Tenant) -> Result<(), MemoryError>;

    /// CAS on both `version` and `status`; returns the new version.
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        expected_state: TenantStatus,
        new_state: TenantStatus,
    ) -> Result<u64, MemoryError>;

    async fn update_tenant_schema_version(
        &self,
        tenant_id: &str,
        expected_version: u64,
        schema_version: u32,
    ) -> Result<u64, MemoryError>;

    /// The fenced variants are the only methods provisioning may
    /// use after a lease is claimed. They CAS tenant
    /// version/status and the exact lease owner/id/generation in
    /// one durable update.
    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        expected_state: TenantStatus,
        new_state: TenantStatus,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<u64, MemoryError>;

    async fn update_tenant_schema_version_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        schema_version: u32,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<u64, MemoryError>;

    /// Append a provisioning event (durable seam consumed by the
    /// Task 6.2 scheduler; written by `enqueue_provisioning`,
    /// Task 4.7).
    async fn append_provisioning_event(
        &self,
        tenant_id: &str,
        stage: &str,
    ) -> Result<(), MemoryError>;

    async fn load_plan(&self, plan_id: &str) -> Result<Plan, MemoryError>;

    async fn increment_usage(
        &self,
        tenant_id: &str,
        counter: UsageCounter,
        delta: u64,
    ) -> Result<u64, MemoryError>;

    async fn list_due_provisioning(
        &self,
        limit: u32,
        now: DateTime<Utc>,
    ) -> Result<Vec<Tenant>, MemoryError>;

    /// Atomic claim: status/retry eligibility, lease expiry, and
    /// generation are checked in one UPDATE ... RETURN AFTER.
    /// `None` means another worker won.
    async fn claim_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        now: DateTime<Utc>,
        lease_expiry: DateTime<Utc>,
    ) -> Result<Option<crate::http::leases::ProvisioningLease>, MemoryError>;

    async fn heartbeat_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        now: DateTime<Utc>,
        lease_expiry: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    async fn release_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<(), MemoryError>;
}

/// Production store: a SurrealDB connection bound to the control
/// namespace. The actual SQL is added in Task 5.x; for Phase 4
/// this struct holds the connection and exposes an
/// `Arc<dyn RegistryStore>` so callers can be tested against the
/// in-memory store. Phase 4 also exposes `in_memory()` so unit
/// tests don't need a real SurrealDB.
pub struct SurrealRegistryStore {
    /// Privileged client; unused while SQL is still TODO.
    _client: Arc<()>,
}

impl SurrealRegistryStore {
    /// Build an unconnected store. The store becomes usable
    /// only after the SurrealDB control store ships in Task 5.x.
    pub fn new_unconnected() -> Self {
        Self {
            _client: Arc::new(()),
        }
    }
}

#[async_trait]
impl RegistryStore for SurrealRegistryStore {
    async fn ping(&self) -> bool {
        true
    }
    // The remaining methods are unimplemented until Task 5.x
    // lands the SQL. They are gated by `#[cfg(feature =
    // "control-plane")]` calls from real code (the registry is
    // only reached from the auth pipeline, which is itself
    // behind a feature flag).
    async fn find_account_by_id(
        &self,
        _account_id: &str,
    ) -> Result<Option<Account>, MemoryError> {
        unimplemented!("SurrealRegistryStore::find_account_by_id lands in Task 5.x")
    }
    async fn find_account_by_identity(
        &self,
        _issuer: &str,
        _subject_verifier: &[u8; 32],
    ) -> Result<Option<Account>, MemoryError> {
        unimplemented!("SurrealRegistryStore::find_account_by_identity lands in Task 5.x")
    }
    async fn find_tenant_by_account(
        &self,
        _account_id: &str,
    ) -> Result<Option<Tenant>, MemoryError> {
        unimplemented!("SurrealRegistryStore::find_tenant_by_account lands in Task 5.x")
    }
    async fn find_tenant_by_id(&self, _tenant_id: &str) -> Result<Option<Tenant>, MemoryError> {
        unimplemented!("SurrealRegistryStore::find_tenant_by_id lands in Task 5.x")
    }
    async fn find_api_key(&self, _key_id: &str) -> Result<Option<ApiKey>, MemoryError> {
        unimplemented!("SurrealRegistryStore::find_api_key lands in Task 5.x")
    }
    async fn write_api_key(&self, _key: &ApiKey) -> Result<(), MemoryError> {
        unimplemented!("SurrealRegistryStore::write_api_key lands in Task 5.x")
    }
    async fn list_api_keys(&self, _account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError> {
        unimplemented!("SurrealRegistryStore::list_api_keys lands in Task 5.x")
    }
    async fn revoke_api_key(
        &self,
        _account_id: &str,
        _key_id: &str,
    ) -> Result<(), MemoryError> {
        unimplemented!("SurrealRegistryStore::revoke_api_key lands in Task 5.x")
    }
    async fn touch_api_key(
        &self,
        _key_id: &str,
        _used_at: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        unimplemented!("SurrealRegistryStore::touch_api_key lands in Task 5.x")
    }
    async fn write_account(&self, _account: &Account) -> Result<(), MemoryError> {
        unimplemented!("SurrealRegistryStore::write_account lands in Task 5.x")
    }
    async fn write_tenant(&self, _tenant: &Tenant) -> Result<(), MemoryError> {
        unimplemented!("SurrealRegistryStore::write_tenant lands in Task 5.x")
    }
    async fn update_tenant_state(
        &self,
        _tenant_id: &str,
        _expected_version: u64,
        _expected_state: TenantStatus,
        _new_state: TenantStatus,
    ) -> Result<u64, MemoryError> {
        unimplemented!("SurrealRegistryStore::update_tenant_state lands in Task 5.x")
    }
    async fn update_tenant_schema_version(
        &self,
        _tenant_id: &str,
        _expected_version: u64,
        _schema_version: u32,
    ) -> Result<u64, MemoryError> {
        unimplemented!("SurrealRegistryStore::update_tenant_schema_version lands in Task 5.x")
    }
    async fn update_tenant_state_fenced(
        &self,
        _tenant_id: &str,
        _expected_version: u64,
        _expected_state: TenantStatus,
        _new_state: TenantStatus,
        _owner_id: &str,
        _lease_id: &str,
        _fencing_generation: u64,
    ) -> Result<u64, MemoryError> {
        unimplemented!("SurrealRegistryStore::update_tenant_state_fenced lands in Task 5.x")
    }
    async fn update_tenant_schema_version_fenced(
        &self,
        _tenant_id: &str,
        _expected_version: u64,
        _schema_version: u32,
        _owner_id: &str,
        _lease_id: &str,
        _fencing_generation: u64,
    ) -> Result<u64, MemoryError> {
        unimplemented!(
            "SurrealRegistryStore::update_tenant_schema_version_fenced lands in Task 5.x"
        )
    }
    async fn append_provisioning_event(
        &self,
        _tenant_id: &str,
        _stage: &str,
    ) -> Result<(), MemoryError> {
        unimplemented!("SurrealRegistryStore::append_provisioning_event lands in Task 5.x")
    }
    async fn load_plan(&self, _plan_id: &str) -> Result<Plan, MemoryError> {
        unimplemented!("SurrealRegistryStore::load_plan lands in Task 5.x")
    }
    async fn increment_usage(
        &self,
        _tenant_id: &str,
        _counter: UsageCounter,
        _delta: u64,
    ) -> Result<u64, MemoryError> {
        unimplemented!("SurrealRegistryStore::increment_usage lands in Task 5.x")
    }
    async fn list_due_provisioning(
        &self,
        _limit: u32,
        _now: DateTime<Utc>,
    ) -> Result<Vec<Tenant>, MemoryError> {
        unimplemented!("SurrealRegistryStore::list_due_provisioning lands in Task 5.x")
    }
    async fn claim_provisioning(
        &self,
        _tenant_id: &str,
        _owner_id: &str,
        _lease_id: &str,
        _now: DateTime<Utc>,
        _lease_expiry: DateTime<Utc>,
    ) -> Result<Option<crate::http::leases::ProvisioningLease>, MemoryError> {
        unimplemented!("SurrealRegistryStore::claim_provisioning lands in Task 5.x")
    }
    async fn heartbeat_provisioning(
        &self,
        _tenant_id: &str,
        _owner_id: &str,
        _lease_id: &str,
        _fencing_generation: u64,
        _now: DateTime<Utc>,
        _lease_expiry: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        unimplemented!("SurrealRegistryStore::heartbeat_provisioning lands in Task 5.x")
    }
    async fn release_provisioning(
        &self,
        _tenant_id: &str,
        _owner_id: &str,
        _lease_id: &str,
        _fencing_generation: u64,
    ) -> Result<(), MemoryError> {
        unimplemented!("SurrealRegistryStore::release_provisioning lands in Task 5.x")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_object_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn RegistryStore>>();
    }
}
