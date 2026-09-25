use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::MemoryError;
use crate::http::fault_injection::FaultInjector;
use crate::http::registry::RegistryHandle;
use crate::http::registry::models::TenantStatus;
use crate::http::registry::storage::RegistryStore;
use crate::operations::api::{AccountDeletionPort, DeletionRecoveryPort, RecoveryOutcome};

pub(crate) struct RegistryAccountDeletionAdapter {
    store: Arc<dyn RegistryStore>,
}

impl RegistryAccountDeletionAdapter {
    pub(crate) fn new(store: Arc<dyn RegistryStore>) -> Self {
        Self { store }
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
        self.store
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
    fault_injector: Arc<dyn FaultInjector>,
    owner_id: String,
}

impl LegacyDeletionRecoveryAdapter {
    pub(crate) fn new(registry: RegistryHandle, fault_injector: Arc<dyn FaultInjector>) -> Self {
        Self {
            registry,
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
        let store_for_work = Arc::clone(&store);
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
                store_for_work
                    .finalize_account_deletion(
                        &tenant_id_for_work,
                        &lease_for_work.owner_id,
                        &lease_for_work.lease_id,
                        lease_for_work.fencing_generation,
                        Utc::now(),
                    )
                    .await?;
                injector.hit(crate::http::fault_injection::FaultPoint::AccountDeletionFinalized)
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
