use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::MemoryError;
use crate::http::registry::RegistryHandle;
use crate::http::registry::models::TenantStatus;
use crate::http::registry::storage::RegistryStore;
use crate::operations::api::{
    AccountDeletionPort, DeletionLease, DeletionRecoveryPort, RetainedTenantWork,
    TenantUnderDeletion,
};
use crate::platform::fault_injection::FaultInjector;
use crate::platform::persistence::control::AccountDeletionTx;
use crate::storage::client::BoundDbClient;

pub(crate) struct RegistryAccountDeletionAdapter {
    tx: Arc<dyn AccountDeletionTx>,
}

impl RegistryAccountDeletionAdapter {
    /// Composition still holds the omnibus registry handle, so this wraps
    /// it as the deletion port. The adapter itself is typed against the
    /// port only and cannot reach the wider trait — that is the point.
    pub(crate) fn from_registry(store: Arc<dyn RegistryStore>) -> Self {
        Self {
            tx: Arc::new(store) as Arc<dyn AccountDeletionTx>,
        }
    }
}

#[async_trait::async_trait]
impl AccountDeletionPort for RegistryAccountDeletionAdapter {
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.tx
            .begin_account_deletion(verifier, account_id, session_id, now)
            .await
    }
}

/// Durable steps of the deletion-recovery workflow.
///
/// The workflow itself lives in [`crate::operations::api::recover_tenant`]: the
/// order of these calls, the fencing rules and the failure policy are that
/// module's, not this file's. What is here is the SQL and the handle each step
/// needs, so the workflow can be read without a registry in the room.
pub(crate) struct RegistryDeletionRecoveryAdapter {
    registry: RegistryHandle,
    tx: Arc<dyn AccountDeletionTx>,
    fault_injector: Arc<dyn FaultInjector>,
}

impl RegistryDeletionRecoveryAdapter {
    pub(crate) fn new(registry: RegistryHandle, fault_injector: Arc<dyn FaultInjector>) -> Self {
        let tx: Arc<dyn AccountDeletionTx> =
            Arc::new(registry.store_clone()) as Arc<dyn AccountDeletionTx>;
        Self {
            registry,
            tx,
            fault_injector,
        }
    }
}

#[async_trait::async_trait]
impl DeletionRecoveryPort for RegistryDeletionRecoveryAdapter {
    async fn list_deleting_tenants(
        &self,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, MemoryError> {
        Ok(self
            .registry
            .store_clone()
            .list_deleting_tenants(limit, now)
            .await?
            .into_iter()
            .map(|tenant| tenant.id)
            .collect())
    }

    async fn find_tenant_for_recovery(
        &self,
        tenant_id: &str,
    ) -> Result<Option<TenantUnderDeletion>, MemoryError> {
        let Some(tenant) = self
            .registry
            .store_clone()
            .find_tenant_by_id(tenant_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(TenantUnderDeletion {
            tenant_id: tenant.id,
            namespace: tenant.namespace_binding.namespace,
            database: tenant.namespace_binding.database,
            purged: tenant.status == TenantStatus::Purged,
        }))
    }

    async fn claim_deletion_lease(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        ttl_secs: i64,
    ) -> Result<Option<DeletionLease>, MemoryError> {
        let Some(lease) = self
            .registry
            .store_clone()
            .claim_provisioning(tenant_id, owner_id, lease_id, ttl_secs)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(DeletionLease {
            owner_id: lease.owner_id,
            lease_id: lease.lease_id,
            fencing_generation: lease.fencing_generation,
        }))
    }

    async fn bind_tenant_namespace(
        &self,
        tenant: &TenantUnderDeletion,
    ) -> Result<Arc<BoundDbClient>, MemoryError> {
        // A maintenance bind, not the request path. The tenant is being
        // deleted, so the authenticated resolution pipeline deliberately
        // refuses it; this is the privileged maintenance binding the other
        // schedulers use. Wrapping the raw client in `BoundDbClient` pins the
        // namespace as an invariant of the handle, so the sweep that follows
        // cannot be re-pointed at a different tenant.
        let engine = self.registry.tenant_engine()?;
        let handle = engine
            .bind_namespace(&tenant.namespace, &tenant.database)
            .await?;
        Ok(Arc::new(BoundDbClient::new(
            handle,
            tenant.namespace.clone(),
        )))
    }

    async fn write_deletion_tombstone(
        &self,
        tenant_id: &str,
        lease: &DeletionLease,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        self.tx
            .finalize_account_deletion(
                tenant_id,
                &lease.owner_id,
                &lease.lease_id,
                lease.fencing_generation,
                now,
            )
            .await?;
        self.fault_injector
            .hit(crate::platform::fault_injection::FaultPoint::AccountDeletionFinalized)
    }

    async fn is_tenant_purged(&self, tenant_id: &str) -> Result<bool, MemoryError> {
        Ok(self
            .registry
            .store_clone()
            .find_tenant_by_id(tenant_id)
            .await?
            .is_some_and(|tenant| tenant.status == TenantStatus::Purged))
    }

    async fn release_deletion_lease(
        &self,
        tenant_id: &str,
        lease: &DeletionLease,
    ) -> Result<(), MemoryError> {
        let store = self.registry.store_clone();
        let _ = store
            .release_provisioning_lease(
                tenant_id,
                &lease.owner_id,
                &lease.lease_id,
                lease.fencing_generation,
            )
            .await;
        Ok(())
    }
}

/// Sweeps the tenant-namespace work a deletion must not leave behind.
///
/// The two deletes belong to the stores that own those tables —
/// [`crate::http::app_sessions::store::AppSessionStore`] for `app_session` and
/// [`crate::http::tasks::worker::DurableTaskStore`] for `tenant_task`. This
/// path used to carry hand-written copies of both statements, which is how they
/// came to disagree with the stores about tenant scoping and missing-table
/// handling. It now calls the owners.
pub(crate) struct TenantRetainedWork;

#[async_trait::async_trait]
impl RetainedTenantWork for TenantRetainedWork {
    async fn purge_retained_work(
        &self,
        tenant_id: &str,
        bound_db: Arc<BoundDbClient>,
    ) -> Result<(), MemoryError> {
        use crate::http::tasks::state::TaskStore;

        // `BoundDbClient::query_rows` degrades a missing table to an empty
        // result, so a tenant that never opened an App Session and has no
        // `app_session` table sweeps cleanly.
        #[cfg(feature = "mcp-apps")]
        crate::http::app_sessions::store::AppSessionStore::new(Arc::clone(&bound_db))
            .delete_expired()
            .await?;

        // The store that owns `tenant_task`, with its own retention rule. The
        // copy this path used to carry omitted the `tenant_id` guard, so it
        // deleted every tenant's rows in the namespace rather than this
        // tenant's.
        crate::http::tasks::worker::DurableTaskStore::new(bound_db, tenant_id.to_string())
            .delete_expired()
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! `RegistryAccountDeletionAdapter` holds only `AccountDeletionTx`.
    //! These tests exercise the adapter: a verified start must land the
    //! store's tombstone, and a bad verifier must surface the store's
    //! refusal rather than being swallowed. They demonstrate the effect
    //! reaching the store; because the port implementation is a
    //! pass-through they cannot distinguish a trait-object call from a
    //! direct one, so the type is what carries that guarantee.
    use super::*;
    use crate::http::registry::models::{
        Account, AccountStatus, DeletionChallengeRecord, NamespaceBinding, Tenant,
    };
    use crate::http::registry::storage::{
        AccountStore, InMemoryStore, RegistryStore, SessionStore, TenantStore,
    };

    async fn seeded_store() -> Arc<InMemoryStore> {
        let store = Arc::new(InMemoryStore::default());
        let now = Utc::now();
        store
            .create_account_bundle(
                &Account {
                    id: "acct_1".into(),
                    status: AccountStatus::Active,
                    tenant_id: "ten_1".into(),
                    created_at: now,
                },
                &Tenant {
                    id: "ten_1".into(),
                    status: TenantStatus::Ready,
                    namespace_binding: NamespaceBinding {
                        namespace: "tns_1".into(),
                        database: "memory".into(),
                    },
                    plan_version: 1,
                    schema_version: 0,
                    retry_stage: None,
                    provisioning_lease: None,
                    created_at: now,
                    version: 0,
                },
                None,
            )
            .await
            .expect("seed account");
        store
    }

    #[tokio::test]
    async fn a_verified_start_lands_the_tombstone() {
        let store = seeded_store().await;
        let adapter =
            RegistryAccountDeletionAdapter::from_registry(store.clone() as Arc<dyn RegistryStore>);

        // `verifier` is the single-use challenge token, not an actor id:
        // the store matches it, then checks the (account, session)
        // tuple it was minted for.
        let now = Utc::now();
        store
            .create_deletion_challenge(&DeletionChallengeRecord {
                id: "chl_1".into(),
                verifier: "vrf_1".into(),
                account_id: "acct_1".into(),
                session_id: "sess_1".into(),
                expires_at: now + chrono::Duration::minutes(10),
                consumed_at: None,
            })
            .await
            .expect("mint challenge");

        adapter
            .begin_account_deletion("vrf_1", "acct_1", "sess_1", now)
            .await
            .expect("verified start");

        assert!(
            store
                .list_deleting_tenants(64, Utc::now())
                .await
                .expect("list")
                .iter()
                .any(|tenant| tenant.id == "ten_1"),
            "the tombstone reached the store through the port"
        );
    }

    #[tokio::test]
    async fn an_unminted_verifier_is_refused() {
        let store = seeded_store().await;
        let adapter =
            RegistryAccountDeletionAdapter::from_registry(store.clone() as Arc<dyn RegistryStore>);

        assert!(
            adapter
                .begin_account_deletion("op_1", "acct_absent", "sess_1", Utc::now())
                .await
                .is_err(),
            "the adapter must not swallow the store's refusal"
        );
    }
}
