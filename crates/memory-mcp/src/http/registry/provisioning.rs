//! Provisioning state machine (ADR-0052, plan §5.1).
//!
//! The `can_transition` table is the durable seam: every status
//! change goes through it. Two callers exist:
//! - `transition_fenced` for the scheduler: it carries a
//!   `ProvisioningLease` and the CAS predicate includes
//!   `(version, owner, lease, generation)`.
//! - `transition` (unfenced) for operator state changes that do
//!   not perform provisioning work (e.g. `Suspend`, `Delete`).
//!
//! The state machine NEVER deletes. `Deleting -> Purged` is the
//! only path that destroys records, and it requires a
//! fenced-CAS update of the Tenant's `version`.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::ProvisioningLease;
use crate::http::registry::RegistryStore;
use crate::http::registry::storage::LeaseFence;

/// The `ProvisioningStage` is the same enum as `TenantStatus`
/// (re-exported so the state machine file is the one place
/// readers look for stage names).
pub use crate::http::registry::models::TenantStatus as ProvisioningStage;

/// CAS-update the tenant's status without a fencing
/// predicate. Use only for operator state changes (Task 6.5
/// / Phase 10 control plane).
pub async fn transition(
    store: &dyn RegistryStore,
    tenant_id: &str,
    expected_version: u64,
    from: ProvisioningStage,
    to: ProvisioningStage,
) -> Result<u64, MemoryError> {
    if !can_transition(from, to) {
        return Err(MemoryError::Validation(format!(
            "provisioning transition {from:?}->{to:?}"
        )));
    }
    store
        .update_tenant_state(tenant_id, expected_version, from, to)
        .await
}

/// Fenced CAS-update the tenant's status. The predicate
/// includes `(version, status, owner, lease, generation)` so a
/// stale worker cannot advance a tenant whose lease has been
/// reassigned.
pub async fn transition_fenced(
    store: &dyn RegistryStore,
    tenant_id: &str,
    expected_version: u64,
    from: ProvisioningStage,
    to: ProvisioningStage,
    lease: &ProvisioningLease,
) -> Result<u64, MemoryError> {
    if !can_transition(from, to) {
        return Err(MemoryError::Validation(format!(
            "provisioning transition {from:?}->{to:?}"
        )));
    }
    store
        .update_tenant_state_fenced(
            tenant_id,
            expected_version,
            from,
            to,
            &LeaseFence::from_lease(lease),
        )
        .await
}

/// The legal transition table (plan §5.1 Step 1). Anything
/// outside this set is a programmer error and surfaces as
/// `MemoryError::Validation`.
pub fn can_transition(from: ProvisioningStage, to: ProvisioningStage) -> bool {
    use ProvisioningStage::*;
    match (from, to) {
        (Reserved, NamespaceCreating) => true,
        (NamespaceCreating, Migrating) => true,
        (NamespaceCreating, Failed) => true,
        (Migrating, Ready) => true,
        (Migrating, Failed) => true,
        (Ready, Suspended) => true,
        (Suspended, Ready) => true,
        // `Deleting` is reachable from every non-terminal
        // state. `Purged` is reachable only from `Deleting`.
        (_, Deleting)
            if matches!(
                from,
                Reserved | NamespaceCreating | Migrating | Ready | Suspended | Failed
            ) =>
        {
            true
        }
        (Deleting, Purged) => true,
        // Retry from Failed: a worker may re-enter
        // NamespaceCreating or Migrating.
        (Failed, NamespaceCreating) => true,
        (Failed, Migrating) => true,
        _ => false,
    }
}

/// Durable enqueue: append a provisioning event for the
/// reserved tenant. The Phase 4 control API calls this after
/// writing a reserved Tenant. The Task 6.2 scheduler consumes
/// the events; the bootstrap (Task 5.8) calls it for each
/// ready tenant.
pub async fn enqueue_provisioning(
    store: &Arc<dyn RegistryStore>,
    tenant: &crate::http::registry::models::Tenant,
) -> Result<(), MemoryError> {
    store
        .append_provisioning_event(&tenant.id, "reserved")
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::*;
    use crate::http::registry::storage::InMemoryStore;
    use std::sync::Arc;

    fn reserved_tenant(id: &str) -> Tenant {
        use chrono::Utc;
        Tenant {
            id: id.to_string(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: format!("tns_{id}"),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        }
    }

    async fn store_with_tenant(tenant: Tenant) -> Arc<InMemoryStore> {
        use chrono::Utc;
        let s = Arc::new(InMemoryStore::default());
        let account = Account {
            id: "acct_1".into(),
            status: AccountStatus::Active,
            tenant_id: tenant.id.clone(),
            created_at: Utc::now(),
        };
        s.write_account(&account).await.unwrap();
        s.write_tenant(&tenant).await.unwrap();
        s
    }

    #[test]
    fn reserved_to_namespace_creating_legal() {
        assert!(can_transition(
            ProvisioningStage::Reserved,
            ProvisioningStage::NamespaceCreating
        ));
    }

    #[test]
    fn namespace_creating_to_migrating_legal() {
        assert!(can_transition(
            ProvisioningStage::NamespaceCreating,
            ProvisioningStage::Migrating
        ));
    }

    #[test]
    fn migrating_to_ready_legal() {
        assert!(can_transition(
            ProvisioningStage::Migrating,
            ProvisioningStage::Ready
        ));
    }

    #[test]
    fn ready_to_namespace_creating_illegal() {
        assert!(!can_transition(
            ProvisioningStage::Ready,
            ProvisioningStage::NamespaceCreating
        ));
    }

    #[test]
    fn migrating_to_purged_illegal() {
        assert!(!can_transition(
            ProvisioningStage::Migrating,
            ProvisioningStage::Purged
        ));
    }

    #[test]
    fn failed_to_namespace_creating_legal_retry() {
        assert!(can_transition(
            ProvisioningStage::Failed,
            ProvisioningStage::NamespaceCreating
        ));
    }

    #[test]
    fn deleting_to_purged_legal() {
        assert!(can_transition(
            ProvisioningStage::Deleting,
            ProvisioningStage::Purged
        ));
    }

    #[test]
    fn ready_to_deleting_legal() {
        assert!(can_transition(
            ProvisioningStage::Ready,
            ProvisioningStage::Deleting
        ));
    }

    #[tokio::test]
    async fn transition_cas_advances_version() {
        let s = store_with_tenant(reserved_tenant("ten_1")).await;
        let new_version = transition(
            s.as_ref(),
            "ten_1",
            0,
            ProvisioningStage::Reserved,
            ProvisioningStage::NamespaceCreating,
        )
        .await
        .expect("transition succeeds");
        assert_eq!(new_version, 1);
        let t = s.find_tenant_by_id("ten_1").await.unwrap().unwrap();
        assert_eq!(t.status, ProvisioningStage::NamespaceCreating);
        assert_eq!(t.version, 1);
    }

    #[tokio::test]
    async fn transition_rejects_stale_version() {
        let s = store_with_tenant(reserved_tenant("ten_1")).await;
        let res = transition(
            s.as_ref(),
            "ten_1",
            99,
            ProvisioningStage::Reserved,
            ProvisioningStage::NamespaceCreating,
        )
        .await;
        assert!(matches!(res, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    async fn transition_rejects_illegal_state_change() {
        let s = store_with_tenant(reserved_tenant("ten_1")).await;
        let res = transition(
            s.as_ref(),
            "ten_1",
            0,
            ProvisioningStage::Reserved,
            ProvisioningStage::Ready,
        )
        .await;
        assert!(matches!(res, Err(MemoryError::Validation(_))));
    }

    #[tokio::test]
    async fn transition_fenced_rejects_stale_generation() {
        use chrono::Utc;
        let s = store_with_tenant(reserved_tenant("ten_1")).await;
        // Seed a lease with generation 0.
        let mut tenant = s.find_tenant_by_id("ten_1").await.unwrap().unwrap();
        tenant.provisioning_lease = Some(ProvisioningLeaseState {
            owner_id: "replica_a".into(),
            lease_id: "lease_1".into(),
            expires_at: Utc::now() + chrono::Duration::seconds(60),
            fencing_generation: 0,
            heartbeat_at: Utc::now(),
        });
        tenant.version = 0;
        s.write_tenant(&tenant).await.unwrap();

        // First claim succeeds with gen=0.
        let lease = ProvisioningLease {
            owner_id: "replica_a".into(),
            lease_id: "lease_1".into(),
            fencing_generation: 0,
            expires_at: Utc::now() + chrono::Duration::seconds(60),
            heartbeat_at: Utc::now(),
        };
        let res = transition_fenced(
            s.as_ref(),
            "ten_1",
            0,
            ProvisioningStage::Reserved,
            ProvisioningStage::NamespaceCreating,
            &lease,
        )
        .await;
        assert!(res.is_ok(), "first fenced write with gen=0 succeeds");

        // Stale generation is rejected.
        let stale = ProvisioningLease {
            fencing_generation: 99,
            ..lease.clone()
        };
        let res = transition_fenced(
            s.as_ref(),
            "ten_1",
            1,
            ProvisioningStage::NamespaceCreating,
            ProvisioningStage::Migrating,
            &stale,
        )
        .await;
        assert!(
            matches!(res, Err(MemoryError::Conflict(_))),
            "stale generation must be rejected"
        );
    }
}
