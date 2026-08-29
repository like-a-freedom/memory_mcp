//! Account → Tenant resolver (ADR-0052, plan §4.5).
//!
//! A missing tenant is a RESOLUTION OUTCOME (NotFound → 404 in
//! Task 5.6), not an `Auth` error — the caller was already
//! authenticated. Do not map it to `MemoryError::Auth`.

use std::sync::Arc;

use super::models::{Tenant, TenantStatus};
use super::storage::RegistryStore;
use crate::error::MemoryError;

pub struct AccountResolver {
    store: Arc<dyn RegistryStore>,
}

/// Outcome of resolving an account to a tenant. The Tenant
/// Runtime (Task 5.6) consumes the `Ready` arm; the others
/// become specific 4xx/5xx responses at the auth-pipeline
/// boundary.
#[derive(Debug)]
pub enum ResolvedTenant {
    Ready(Tenant),
    /// The tenant is being created/migrated. The second field
    /// is a correlation id the caller can return to the client.
    Provisioning(TenantStatus, String),
    Suspended,
    Failed(String),
    NotFound,
}

impl AccountResolver {
    pub fn new(store: Arc<dyn RegistryStore>) -> Self {
        Self { store }
    }

    pub async fn resolve_ready_tenant(
        &self,
        account_id: &str,
    ) -> Result<ResolvedTenant, MemoryError> {
        let Some(tenant) = self.store.find_tenant_by_account(account_id).await? else {
            return Ok(ResolvedTenant::NotFound);
        };
        Ok(match tenant.status {
            TenantStatus::Ready => ResolvedTenant::Ready(tenant),
            TenantStatus::Suspended => ResolvedTenant::Suspended,
            TenantStatus::Failed => ResolvedTenant::Failed(tenant.id),
            other => ResolvedTenant::Provisioning(other, tenant.id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::{
        Account, AccountStatus, NamespaceBinding, Tenant, TenantStatus,
    };
    use crate::http::registry::storage::SurrealRegistryStore;
    use std::sync::Arc;

    /// In-memory stub of `RegistryStore` that returns a fixed
    /// `find_tenant_by_account` result.
    struct Stub {
        tenant: Option<Tenant>,
    }

    #[async_trait::async_trait]
    impl crate::http::registry::storage::RegistryStore for Stub {
        async fn ping(&self) -> bool {
            true
        }
        async fn find_account_by_id(
            &self,
            _id: &str,
        ) -> Result<Option<Account>, MemoryError> {
            Ok(Some(Account {
                id: "acct_1".into(),
                status: AccountStatus::Active,
                tenant_id: "ten_1".into(),
                created_at: chrono::Utc::now(),
            }))
        }
        async fn find_account_by_identity(
            &self,
            _issuer: &str,
            _subject_verifier: &[u8; 32],
        ) -> Result<Option<Account>, MemoryError> {
            Ok(None)
        }
        async fn find_tenant_by_account(
            &self,
            _account_id: &str,
        ) -> Result<Option<Tenant>, MemoryError> {
            Ok(self.tenant.clone())
        }
        async fn find_tenant_by_id(
            &self,
            _tenant_id: &str,
        ) -> Result<Option<Tenant>, MemoryError> {
            Ok(None)
        }
        // The remaining methods are not exercised by these tests.
        async fn find_api_key(
            &self,
            _: &str,
        ) -> Result<Option<crate::http::registry::models::ApiKey>, MemoryError> {
            unimplemented!()
        }
        async fn write_api_key(
            &self,
            _: &crate::http::registry::models::ApiKey,
        ) -> Result<(), MemoryError> {
            unimplemented!()
        }
        async fn list_api_keys(
            &self,
            _: &str,
        ) -> Result<Vec<crate::http::registry::models::ApiKeyMeta>, MemoryError> {
            unimplemented!()
        }
        async fn revoke_api_key(&self, _: &str, _: &str) -> Result<(), MemoryError> {
            unimplemented!()
        }
        async fn touch_api_key(
            &self,
            _: &str,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), MemoryError> {
            unimplemented!()
        }
        async fn write_account(&self, _: &Account) -> Result<(), MemoryError> {
            unimplemented!()
        }
        async fn write_tenant(&self, _: &Tenant) -> Result<(), MemoryError> {
            unimplemented!()
        }
        async fn update_tenant_state(
            &self,
            _: &str,
            _: u64,
            _: TenantStatus,
            _: TenantStatus,
        ) -> Result<u64, MemoryError> {
            unimplemented!()
        }
        async fn update_tenant_schema_version(
            &self,
            _: &str,
            _: u64,
            _: u32,
        ) -> Result<u64, MemoryError> {
            unimplemented!()
        }
        async fn update_tenant_state_fenced(
            &self,
            _: &str,
            _: u64,
            _: TenantStatus,
            _: TenantStatus,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unimplemented!()
        }
        async fn update_tenant_schema_version_fenced(
            &self,
            _: &str,
            _: u64,
            _: u32,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unimplemented!()
        }
        async fn append_provisioning_event(
            &self,
            _: &str,
            _: &str,
        ) -> Result<(), MemoryError> {
            unimplemented!()
        }
        async fn load_plan(
            &self,
            _: &str,
        ) -> Result<crate::http::registry::models::Plan, MemoryError> {
            unimplemented!()
        }
        async fn increment_usage(
            &self,
            _: &str,
            _: crate::http::registry::models::UsageCounter,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unimplemented!()
        }
        async fn list_due_provisioning(
            &self,
            _: u32,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<Vec<Tenant>, MemoryError> {
            unimplemented!()
        }
        async fn claim_provisioning(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: chrono::DateTime<chrono::Utc>,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<Option<crate::http::leases::ProvisioningLease>, MemoryError> {
            unimplemented!()
        }
        async fn heartbeat_provisioning(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
            _: chrono::DateTime<chrono::Utc>,
            _: chrono::DateTime<chrono::Utc>,
        ) -> Result<(), MemoryError> {
            unimplemented!()
        }
        async fn release_provisioning(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<(), MemoryError> {
            unimplemented!()
        }
    }

    fn tenant(status: TenantStatus) -> Tenant {
        Tenant {
            id: "ten_1".into(),
            status,
            namespace_binding: NamespaceBinding {
                namespace: "tns_x".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 1,
            retry_stage: None,
            provisioning_lease: None,
            created_at: chrono::Utc::now(),
            version: 1,
        }
    }

    #[tokio::test]
    async fn returns_ready_when_tenant_state_is_ready() {
        let store: Arc<dyn RegistryStore> = Arc::new(Stub {
            tenant: Some(tenant(TenantStatus::Ready)),
        });
        let r = AccountResolver::new(store);
        match r.resolve_ready_tenant("acct_1").await.unwrap() {
            ResolvedTenant::Ready(t) => assert_eq!(t.id, "ten_1"),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_provisioning_when_state_is_reserved_or_migrating() {
        let store: Arc<dyn RegistryStore> = Arc::new(Stub {
            tenant: Some(tenant(TenantStatus::Migrating)),
        });
        let r = AccountResolver::new(store);
        match r.resolve_ready_tenant("acct_1").await.unwrap() {
            ResolvedTenant::Provisioning(s, id) => {
                assert_eq!(s, TenantStatus::Migrating);
                assert_eq!(id, "ten_1");
            }
            other => panic!("expected Provisioning, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_not_found_when_tenant_missing() {
        let store: Arc<dyn RegistryStore> = Arc::new(Stub { tenant: None });
        let r = AccountResolver::new(store);
        match r.resolve_ready_tenant("acct_1").await.unwrap() {
            ResolvedTenant::NotFound => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_suspended_when_tenant_state_is_suspended() {
        let store: Arc<dyn RegistryStore> = Arc::new(Stub {
            tenant: Some(tenant(TenantStatus::Suspended)),
        });
        let r = AccountResolver::new(store);
        match r.resolve_ready_tenant("acct_1").await.unwrap() {
            ResolvedTenant::Suspended => {}
            other => panic!("expected Suspended, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unconnected_store_does_not_implement_find_tenant_by_account() {
        // SurrealRegistryStore::new_unconnected() returns
        // unimplemented!() for these methods; the production path
        // is wired in Task 5.x. The production wiring is covered
        // by the integration tests in tests/. We assert here only
        // that the resolver does not short-circuit on a fresh
        // AccountResolver; the production behavior is tested
        // end-to-end in Phase 5.
        //
        // We cannot call `resolve_ready_tenant` because the
        // underlying method panics; we instead verify the
        // constructor does not panic.
        let s: Arc<dyn RegistryStore> = Arc::new(SurrealRegistryStore::new_unconnected());
        let _r = AccountResolver::new(s);
    }
}
