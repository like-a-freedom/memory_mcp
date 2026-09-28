use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::MemoryError;
use crate::http::registry::RegistryHandle;
use crate::http::registry::models::TenantStatus;
use crate::http::registry::storage::RegistryStore;
use crate::operations::api::{AccountDeletionPort, DeletionRecoveryPort, RecoveryOutcome};
use crate::platform::fault_injection::FaultInjector;
use crate::platform::persistence::control::AccountDeletionTx;

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

const DELETION_LEASE_TTL_SECS: i64 = 60;
const APP_SESSION_CLEANUP_SQL: &str =
    "DELETE FROM app_session WHERE idle_expiry <= time::now() OR absolute_expiry <= time::now();";
const TASK_CLEANUP_SQL: &str = "DELETE FROM tenant_task WHERE retention_expiry <= time::now() AND state IN ['completed', 'completed_before_cancel', 'cancelled', 'cancelled_before_commit', 'failed'];";

pub(crate) struct LegacyDeletionRecoveryAdapter {
    registry: RegistryHandle,
    tx: Arc<dyn AccountDeletionTx>,
    fault_injector: Arc<dyn FaultInjector>,
    owner_id: String,
}

impl LegacyDeletionRecoveryAdapter {
    pub(crate) fn new(registry: RegistryHandle, fault_injector: Arc<dyn FaultInjector>) -> Self {
        let tx: Arc<dyn AccountDeletionTx> =
            Arc::new(registry.store_clone()) as Arc<dyn AccountDeletionTx>;
        Self {
            registry,
            tx,
            fault_injector,
            owner_id: crate::http::leases::scheduler::replica_id(),
        }
    }
}

#[async_trait::async_trait]
impl DeletionRecoveryPort for LegacyDeletionRecoveryAdapter {
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

    async fn recover_tenant(
        &self,
        tenant_id: &str,
        _now: DateTime<Utc>,
    ) -> Result<RecoveryOutcome, MemoryError> {
        let store = self.registry.store_clone();
        let Some(tenant) = store.find_tenant_by_id(tenant_id).await? else {
            return Ok(RecoveryOutcome::Purged);
        };
        if tenant.status == TenantStatus::Purged {
            return Ok(RecoveryOutcome::Purged);
        }
        let engine = self.registry.tenant_engine()?;
        let lease_id = uuid::Uuid::new_v4().to_string();
        let Some(lease) = store
            .claim_provisioning(
                &tenant.id,
                &self.owner_id,
                &lease_id,
                DELETION_LEASE_TTL_SECS,
            )
            .await?
        else {
            return Ok(RecoveryOutcome::Finalized);
        };
        let namespace = tenant.namespace_binding.namespace.clone();
        let tx_for_work = Arc::clone(&self.tx);
        let engine_for_work = engine.clone();
        let lease_for_work = lease.clone();
        let tenant_id = tenant_id.to_string();
        let tenant_id_for_work = tenant_id.clone();
        let injector = Arc::clone(&self.fault_injector);
        let result = lease
            .run_with_heartbeat(self.registry.clone(), &tenant_id, async move {
                let client = engine_for_work.bind(&tenant).await?;
                match client
                    .execute_migration_script(APP_SESSION_CLEANUP_SQL, &namespace)
                    .await
                {
                    Ok(()) => {}
                    Err(error) if missing_table(&error, "app_session") => {}
                    Err(error) => return Err(error),
                }
                match client
                    .execute_migration_script(TASK_CLEANUP_SQL, &namespace)
                    .await
                {
                    Ok(()) => {}
                    Err(error) if missing_table(&error, "tenant_task") => {}
                    Err(error) => return Err(error),
                }
                tx_for_work
                    .finalize_account_deletion(
                        &tenant_id_for_work,
                        &lease_for_work.owner_id,
                        &lease_for_work.lease_id,
                        lease_for_work.fencing_generation,
                        Utc::now(),
                    )
                    .await?;
                injector.hit(crate::platform::fault_injection::FaultPoint::AccountDeletionFinalized)
            })
            .await;
        match result {
            Ok(()) => Ok(RecoveryOutcome::Finalized),
            Err(error) => {
                if deletion_is_purged(store.as_ref(), &tenant_id).await? {
                    Ok(RecoveryOutcome::Purged)
                } else {
                    let _ = lease.release(store.as_ref(), &tenant_id).await;
                    Err(error)
                }
            }
        }
    }
}

async fn deletion_is_purged(
    store: &dyn RegistryStore,
    tenant_id: &str,
) -> Result<bool, MemoryError> {
    Ok(store
        .find_tenant_by_id(tenant_id)
        .await?
        .is_some_and(|tenant| tenant.status == TenantStatus::Purged))
}

fn missing_table(error: &MemoryError, table: &str) -> bool {
    let MemoryError::Storage(message) = error else {
        return false;
    };
    let lower = message.to_ascii_lowercase();
    lower.contains(table)
        && ((lower.contains("does not exist") && lower.contains("table"))
            || lower.contains("unknown table")
            || lower.contains("table not found"))
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
        Account, AccountStatus, DeletionChallengeRecord, NamespaceBinding, Tenant, TenantStatus,
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
