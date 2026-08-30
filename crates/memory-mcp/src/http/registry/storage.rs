//! Control-namespace storage trait (ADR-0052, plan §4.1).
//!
//! Phase 4 ships the trait surface, the production placeholder
//! that returns `MemoryError::Unavailable` (so a misrouted
//! production request becomes a 503, not a panic), and an
//! in-memory test backend. The SurrealDB-backed production store
//! is added in Task 5.x against the migrations in `migrations.rs`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "test-fixtures"))]
use std::sync::Mutex;

use super::models::*;
use crate::error::MemoryError;

/// Compact view of the lease fields the registry uses for
/// fenced CAS predicates. The `&str` borrows let callers pass
/// `&ProvisioningLease` without copying; the struct stays
/// `Copy` so the trait method can take it by value.
#[derive(Debug, Clone, Copy)]
pub struct LeaseFence<'a> {
    pub owner_id: &'a str,
    pub lease_id: &'a str,
    pub fencing_generation: u64,
}

impl<'a> LeaseFence<'a> {
    pub fn from_lease(lease: &'a crate::http::leases::ProvisioningLease) -> Self {
        Self {
            owner_id: &lease.owner_id,
            lease_id: &lease.lease_id,
            fencing_generation: lease.fencing_generation,
        }
    }
}

/// Abstract control store. Backed by a privileged SurrealDB
/// credential in production; the `InMemoryStore` test backend is
/// the only non-test impl Phase 4 ships.
///
/// Methods are named after the records they touch; the SQL
/// implementation does not use the `DbClient` trait because the
/// `DbClient` trait is per-namespace and the registry is
/// multi-record across many tables.
#[async_trait]
pub trait RegistryStore: Send + Sync + 'static {
    async fn ping(&self) -> bool;

    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError>;
    /// `subject_verifier` is a keyed blind index; raw OIDC `sub`
    /// is never persisted.
    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError>;

    async fn find_tenant_by_account(&self, account_id: &str)
    -> Result<Option<Tenant>, MemoryError>;
    async fn find_tenant_by_id(&self, tenant_id: &str) -> Result<Option<Tenant>, MemoryError>;

    async fn find_api_key(&self, key_id: &str) -> Result<Option<ApiKey>, MemoryError>;
    async fn write_api_key(&self, key: &ApiKey) -> Result<(), MemoryError>;
    async fn list_api_keys(&self, account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError>;
    async fn revoke_api_key(&self, account_id: &str, key_id: &str) -> Result<(), MemoryError>;
    async fn touch_api_key(&self, key_id: &str, used_at: DateTime<Utc>) -> Result<(), MemoryError>;

    async fn write_account(&self, account: &Account) -> Result<(), MemoryError>;
    async fn write_tenant(&self, tenant: &Tenant) -> Result<(), MemoryError>;

    /// CAS-update the tenant's status. The predicate is
    /// `version = $expected_version AND status = $from`. Returns
    /// the new version on success, `MemoryError::Conflict` on
    /// stale read.
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
    ) -> Result<u64, MemoryError>;

    /// Fenced CAS-update the tenant's status. The predicate
    /// adds `provisioning_lease.owner_id = $owner_id AND
    /// provisioning_lease.lease_id = $lease_id AND
    /// provisioning_lease.fencing_generation = $generation`
    /// to the unfenced CAS, so a stale worker cannot advance
    /// a tenant whose lease has been reassigned.
    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
        lease: &LeaseFence<'_>,
    ) -> Result<u64, MemoryError>;

    /// Append a provisioning event (durable seam consumed by the
    /// Task 6.2 scheduler; written by `enqueue_provisioning`,
    /// Task 4.7).
    async fn append_provisioning_event(
        &self,
        tenant_id: &str,
        stage: &str,
    ) -> Result<(), MemoryError>;
}

/// Phase 4 production placeholder. Every method that would
/// require Task 5.x SQL returns `MemoryError::Unavailable`. The
/// struct exists so the type bound `Arc<dyn RegistryStore>` is
/// non-empty; `InMemoryStore` is what every test in Phase 4
/// actually uses.
pub struct SurrealRegistryStore {
    _private: (),
}

impl SurrealRegistryStore {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for SurrealRegistryStore {
    fn default() -> Self {
        Self::new()
    }
}

fn unavailable(method: &str) -> MemoryError {
    MemoryError::Unavailable(format!(
        "SurrealRegistryStore::{method} is not yet wired; Task 5.x adds the SQL"
    ))
}

#[async_trait]
impl RegistryStore for SurrealRegistryStore {
    async fn ping(&self) -> bool {
        // The store is reachable as a type; we just don't have
        // any data behind it. `false` keeps `/health/ready` honest.
        false
    }
    async fn find_account_by_id(&self, _account_id: &str) -> Result<Option<Account>, MemoryError> {
        Err(unavailable("find_account_by_id"))
    }
    async fn find_account_by_identity(
        &self,
        _issuer: &str,
        _subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError> {
        Err(unavailable("find_account_by_identity"))
    }
    async fn find_tenant_by_account(
        &self,
        _account_id: &str,
    ) -> Result<Option<Tenant>, MemoryError> {
        Err(unavailable("find_tenant_by_account"))
    }
    async fn find_tenant_by_id(&self, _tenant_id: &str) -> Result<Option<Tenant>, MemoryError> {
        Err(unavailable("find_tenant_by_id"))
    }
    async fn find_api_key(&self, _key_id: &str) -> Result<Option<ApiKey>, MemoryError> {
        Err(unavailable("find_api_key"))
    }
    async fn write_api_key(&self, _key: &ApiKey) -> Result<(), MemoryError> {
        Err(unavailable("write_api_key"))
    }
    async fn list_api_keys(&self, _account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError> {
        Err(unavailable("list_api_keys"))
    }
    async fn revoke_api_key(&self, _account_id: &str, _key_id: &str) -> Result<(), MemoryError> {
        Err(unavailable("revoke_api_key"))
    }
    async fn touch_api_key(
        &self,
        _key_id: &str,
        _used_at: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        Err(unavailable("touch_api_key"))
    }
    async fn write_account(&self, _account: &Account) -> Result<(), MemoryError> {
        Err(unavailable("write_account"))
    }
    async fn write_tenant(&self, _tenant: &Tenant) -> Result<(), MemoryError> {
        Err(unavailable("write_tenant"))
    }
    async fn update_tenant_state(
        &self,
        _tenant_id: &str,
        _expected_version: u64,
        _from: TenantStatus,
        _to: TenantStatus,
    ) -> Result<u64, MemoryError> {
        Err(unavailable("update_tenant_state"))
    }
    async fn update_tenant_state_fenced(
        &self,
        _tenant_id: &str,
        _expected_version: u64,
        _from: TenantStatus,
        _to: TenantStatus,
        _lease: &LeaseFence<'_>,
    ) -> Result<u64, MemoryError> {
        Err(unavailable("update_tenant_state_fenced"))
    }
    async fn append_provisioning_event(
        &self,
        _tenant_id: &str,
        _stage: &str,
    ) -> Result<(), MemoryError> {
        Err(unavailable("append_provisioning_event"))
    }
}

/// In-memory `RegistryStore` for unit tests. The fields are
/// behind a single `Mutex`; the contention is acceptable for
/// unit-test traffic. The struct is feature-gated on
/// `test-fixtures` so a production build cannot accidentally
/// swap it in.
#[cfg(any(test, feature = "test-fixtures"))]
pub struct InMemoryStore {
    accounts: std::sync::Mutex<Vec<Account>>,
    tenants: std::sync::Mutex<Vec<Tenant>>,
    api_keys: std::sync::Mutex<Vec<ApiKey>>,
    events: std::sync::Mutex<Vec<(String, String)>>,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl Default for InMemoryStore {
    fn default() -> Self {
        Self {
            accounts: Mutex::new(Vec::new()),
            tenants: Mutex::new(Vec::new()),
            api_keys: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
#[async_trait]
impl RegistryStore for InMemoryStore {
    async fn ping(&self) -> bool {
        true
    }
    async fn find_account_by_id(&self, id: &str) -> Result<Option<Account>, MemoryError> {
        Ok(self
            .accounts
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .find(|a| a.id == id)
            .cloned())
    }
    async fn find_account_by_identity(
        &self,
        _issuer: &str,
        _subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError> {
        // Test backend: returns the first account if any.
        Ok(self
            .accounts
            .lock()
            .expect("in-memory store poisoned")
            .first()
            .cloned())
    }
    async fn find_tenant_by_account(
        &self,
        account_id: &str,
    ) -> Result<Option<Tenant>, MemoryError> {
        let account = self.find_account_by_id(account_id).await?;
        let Some(account) = account else {
            return Ok(None);
        };
        Ok(self
            .tenants
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .find(|t| t.id == account.tenant_id)
            .cloned())
    }
    async fn find_tenant_by_id(&self, id: &str) -> Result<Option<Tenant>, MemoryError> {
        Ok(self
            .tenants
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .find(|t| t.id == id)
            .cloned())
    }
    async fn find_api_key(&self, id: &str) -> Result<Option<ApiKey>, MemoryError> {
        Ok(self
            .api_keys
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .find(|k| k.id == id)
            .cloned())
    }
    async fn write_api_key(&self, key: &ApiKey) -> Result<(), MemoryError> {
        self.api_keys
            .lock()
            .expect("in-memory store poisoned")
            .push(key.clone());
        Ok(())
    }
    async fn list_api_keys(&self, account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError> {
        Ok(self
            .api_keys
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .filter(|k| k.account_id == account_id)
            .map(|k| ApiKeyMeta {
                id: k.id.clone(),
                name: k.name.clone(),
                status: k.status,
                created_at: k.created_at,
                expires_at: k.expires_at,
                last_used_at: k.last_used_at,
            })
            .collect())
    }
    async fn revoke_api_key(&self, account_id: &str, key_id: &str) -> Result<(), MemoryError> {
        let mut keys = self.api_keys.lock().expect("in-memory store poisoned");
        if let Some(k) = keys
            .iter_mut()
            .find(|k| k.id == key_id && k.account_id == account_id)
        {
            k.status = ApiKeyStatus::Revoked;
        }
        Ok(())
    }
    async fn touch_api_key(&self, key_id: &str, used_at: DateTime<Utc>) -> Result<(), MemoryError> {
        let mut keys = self.api_keys.lock().expect("in-memory store poisoned");
        if let Some(k) = keys.iter_mut().find(|k| k.id == key_id) {
            k.last_used_at = Some(used_at);
        }
        Ok(())
    }
    async fn write_account(&self, account: &Account) -> Result<(), MemoryError> {
        let mut accounts = self.accounts.lock().expect("in-memory store poisoned");
        if let Some(slot) = accounts.iter_mut().find(|a| a.id == account.id) {
            *slot = account.clone();
        } else {
            accounts.push(account.clone());
        }
        Ok(())
    }
    async fn write_tenant(&self, tenant: &Tenant) -> Result<(), MemoryError> {
        let mut tenants = self.tenants.lock().expect("in-memory store poisoned");
        if let Some(slot) = tenants.iter_mut().find(|t| t.id == tenant.id) {
            *slot = tenant.clone();
        } else {
            tenants.push(tenant.clone());
        }
        Ok(())
    }
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
    ) -> Result<u64, MemoryError> {
        let mut tenants = self.tenants.lock().expect("in-memory store poisoned");
        let t = tenants
            .iter_mut()
            .find(|t| t.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if t.version != expected_version || t.status != from {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} CAS failed: version {} (expected {}) status {:?} (expected {:?})",
                t.version, expected_version, t.status, from
            )));
        }
        t.status = to;
        t.version += 1;
        Ok(t.version)
    }
    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
        lease: &LeaseFence<'_>,
    ) -> Result<u64, MemoryError> {
        let mut tenants = self.tenants.lock().expect("in-memory store poisoned");
        let t = tenants
            .iter_mut()
            .find(|t| t.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if t.version != expected_version || t.status != from {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} CAS failed: version {} (expected {}) status {:?} (expected {:?})",
                t.version, expected_version, t.status, from
            )));
        }
        match &t.provisioning_lease {
            Some(stored)
                if stored.owner_id == lease.owner_id
                    && stored.lease_id == lease.lease_id
                    && stored.fencing_generation == lease.fencing_generation => {}
            Some(stored) => {
                return Err(MemoryError::Conflict(format!(
                    "tenant {tenant_id} fenced CAS failed: lease mismatch (got owner={} lease={} gen={}; expected owner={} lease={} gen={})",
                    stored.owner_id,
                    stored.lease_id,
                    stored.fencing_generation,
                    lease.owner_id,
                    lease.lease_id,
                    lease.fencing_generation,
                )));
            }
            None => {
                return Err(MemoryError::Conflict(format!(
                    "tenant {tenant_id} fenced CAS failed: no active lease"
                )));
            }
        }
        t.status = to;
        t.version += 1;
        Ok(t.version)
    }
    async fn append_provisioning_event(
        &self,
        tenant_id: &str,
        stage: &str,
    ) -> Result<(), MemoryError> {
        self.events
            .lock()
            .expect("in-memory store poisoned")
            .push((tenant_id.to_string(), stage.to_string()));
        Ok(())
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl InMemoryStore {
    pub fn provisioning_events(&self) -> Vec<(String, String)> {
        self.events
            .lock()
            .expect("in-memory store poisoned")
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_object_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn RegistryStore>>();
    }

    #[tokio::test]
    async fn surreal_store_unavailable() {
        let s = SurrealRegistryStore::new();
        let r = s.find_account_by_id("acct_x").await;
        assert!(matches!(r, Err(MemoryError::Unavailable(_))));
    }

    #[tokio::test]
    async fn surreal_store_ping_false() {
        let s = SurrealRegistryStore::new();
        assert!(!s.ping().await);
    }

    #[tokio::test]
    async fn in_memory_store_round_trips_account_and_tenant() {
        use super::super::models::{AccountStatus, NamespaceBinding, TenantStatus};
        let s = InMemoryStore::default();
        let account = Account {
            id: "acct_1".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_1".into(),
            created_at: chrono::Utc::now(),
        };
        let tenant = Tenant {
            id: "ten_1".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_x".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: chrono::Utc::now(),
            version: 0,
        };
        s.write_account(&account).await.unwrap();
        s.write_tenant(&tenant).await.unwrap();
        let got = s.find_account_by_id("acct_1").await.unwrap().unwrap();
        assert_eq!(got.id, "acct_1");
        let t = s.find_tenant_by_account("acct_1").await.unwrap().unwrap();
        assert_eq!(t.id, "ten_1");
    }
}
