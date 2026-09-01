//! Account → Tenant resolver.
//!
//! A missing tenant is a RESOLUTION OUTCOME (NotFound → 404),
//! not an `Auth` error — the caller was already authenticated.
//! Do not map it to `MemoryError::Auth`.

use std::sync::Arc;

use super::models::{Tenant, TenantStatus};
use super::storage::RegistryStore;
use crate::error::MemoryError;

pub struct AccountResolver {
    store: Arc<dyn RegistryStore>,
}

/// Outcome of resolving an account to a tenant. The Tenant
/// Runtime consumes the `Ready` arm; the others become specific
/// 4xx/5xx responses at the auth-pipeline boundary.
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
    use crate::http::registry::storage::{InMemoryStore, SurrealRegistryStore};
    use std::sync::Arc;

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

    async fn store_with(tenant: Option<Tenant>) -> Arc<dyn RegistryStore> {
        let store: Arc<dyn RegistryStore> = Arc::new(InMemoryStore::default());
        store
            .write_account(&Account {
                id: "acct_1".into(),
                status: AccountStatus::Active,
                tenant_id: "ten_1".into(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        if let Some(t) = tenant {
            store.write_tenant(&t).await.unwrap();
        }
        store
    }

    #[tokio::test]
    async fn returns_ready_when_tenant_state_is_ready() {
        let store = store_with(Some(tenant(TenantStatus::Ready))).await;
        let r = AccountResolver::new(store);
        match r.resolve_ready_tenant("acct_1").await.unwrap() {
            ResolvedTenant::Ready(t) => assert_eq!(t.id, "ten_1"),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_provisioning_when_state_is_migrating() {
        let store = store_with(Some(tenant(TenantStatus::Migrating))).await;
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
        let store = store_with(None).await;
        let r = AccountResolver::new(store);
        match r.resolve_ready_tenant("acct_1").await.unwrap() {
            ResolvedTenant::NotFound => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_suspended_when_tenant_state_is_suspended() {
        let store = store_with(Some(tenant(TenantStatus::Suspended))).await;
        let r = AccountResolver::new(store);
        match r.resolve_ready_tenant("acct_1").await.unwrap() {
            ResolvedTenant::Suspended => {}
            other => panic!("expected Suspended, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn production_store_returns_unavailable_for_find_tenant() {
        // The production placeholder returns
        // MemoryError::Unavailable from every read; the resolver
        // surfaces that as a typed Err.
        let s: Arc<dyn RegistryStore> = Arc::new(SurrealRegistryStore::new());
        let r = AccountResolver::new(s);
        let res = r.resolve_ready_tenant("acct_1").await;
        assert!(matches!(res, Err(MemoryError::Unavailable(_))));
    }
}
