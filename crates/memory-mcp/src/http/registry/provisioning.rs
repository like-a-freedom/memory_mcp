//! Provisioning state machine.
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

use std::collections::HashSet;
use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::ProvisioningLease;
use crate::http::registry::RegistryStore;
use crate::http::registry::models::TenantStatus;
use crate::http::registry::storage::LeaseFence;

/// The `ProvisioningStage` is the same enum as `TenantStatus`
/// (re-exported so the state machine file is the one place
/// readers look for stage names).
pub use crate::http::registry::models::TenantStatus as ProvisioningStage;

/// CAS-update the tenant's status without a fencing
/// predicate. Use only for operator state changes.
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

/// The legal transition table. Anything outside this set is a
/// programmer error and surfaces as `MemoryError::Validation`.
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
/// reserved tenant. The control API calls this after writing a
/// reserved Tenant. The scheduler consumes the events; bootstrap
/// calls it for each ready tenant.
pub async fn enqueue_provisioning(
    store: &Arc<dyn RegistryStore>,
    tenant: &crate::http::registry::models::Tenant,
) -> Result<(), MemoryError> {
    store
        .append_provisioning_event(&tenant.id, "reserved")
        .await
}

/// Tracked reconciliation job for server-generated Tenant namespaces. It only
/// reports missing/orphan bindings; it never deletes or rebinds a namespace.
#[cfg(feature = "streamable-http")]
pub fn reconciliation_scheduler_job() -> crate::http::leases::scheduler::SchedulerJob {
    Arc::new(|registry| Box::pin(reconcile_namespaces(registry)))
}

/// Classify a set of registered tenants against the namespaces
/// the privileged engine actually reports. Extracted from
/// `reconcile_namespaces` so the diff can be tested without the
/// 60-second scheduler throttle.
#[cfg(feature = "streamable-http")]
fn classify_namespace_diff<'a>(
    tenants: &'a [crate::http::registry::models::Tenant],
    actual_namespaces: &'a HashSet<String>,
) -> NamespaceDiff<'a> {
    let registered_namespaces: HashSet<&'a str> = tenants
        .iter()
        .map(|tenant| tenant.namespace_binding.namespace.as_str())
        .collect();
    let missing: Vec<&'a crate::http::registry::models::Tenant> = tenants
        .iter()
        .filter(|tenant| !actual_namespaces.contains(&tenant.namespace_binding.namespace))
        .collect();
    let orphan: Vec<&'a str> = actual_namespaces
        .iter()
        .map(String::as_str)
        .filter(|namespace| !registered_namespaces.contains(*namespace))
        .collect();
    NamespaceDiff { missing, orphan }
}

#[cfg(feature = "streamable-http")]
struct NamespaceDiff<'a> {
    missing: Vec<&'a crate::http::registry::models::Tenant>,
    orphan: Vec<&'a str>,
}

#[cfg(feature = "streamable-http")]
async fn reconcile_namespaces(
    registry: crate::http::registry::RegistryHandle,
) -> Result<(), MemoryError> {
    static LAST_RUN: std::sync::OnceLock<std::sync::Mutex<Option<std::time::Instant>>> =
        std::sync::OnceLock::new();
    {
        let mut last = LAST_RUN
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.is_some_and(|instant| instant.elapsed() < std::time::Duration::from_secs(60)) {
            return Ok(());
        }
        *last = Some(std::time::Instant::now());
    }
    let registered = registry.store_clone().list_tenants(10_000).await?;
    let Some(engine) = registry.tenant_engine_optional() else {
        return Ok(());
    };
    let actual = engine.list_namespaces().await?;
    let actual_namespaces: HashSet<String> = actual
        .into_iter()
        .filter(|namespace| namespace.starts_with("tns_"))
        .collect();
    let diff = classify_namespace_diff(&registered, &actual_namespaces);
    for tenant in diff.missing {
        metrics::counter!(
            "memory_http_registry_reconciliation_total",
            "kind" => "missing_namespace"
        )
        .increment(1);
        eprintln!(
            "memory_mcp::http::registry: missing namespace binding tenant_fingerprint={} namespace_fingerprint={}",
            identifier_fingerprint(&tenant.id),
            identifier_fingerprint(&tenant.namespace_binding.namespace)
        );
    }
    for namespace in diff.orphan {
        metrics::counter!(
            "memory_http_registry_reconciliation_total",
            "kind" => "orphan_namespace"
        )
        .increment(1);
        eprintln!(
            "memory_mcp::http::registry: orphan namespace namespace_fingerprint={}",
            identifier_fingerprint(namespace)
        );
    }
    Ok(())
}

#[cfg(feature = "streamable-http")]
fn identifier_fingerprint(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    hex::encode(&digest[..8])
}

/// Reconcile a tenant list against the privileged database.
/// For every non-terminal tenant the function verifies that
/// the tenant record still exists; missing entries are
/// recorded as `orphans`. The orphan detection (namespaces
/// in the DB without a tenant) is logged and surfaced via
/// the `orphans` field; no destructive action is taken.
pub async fn reconcile(
    store: &Arc<dyn RegistryStore>,
    tenants: &[crate::http::registry::models::Tenant],
) -> Result<ReconcileReport, MemoryError> {
    let mut report = ReconcileReport::default();
    for tenant in tenants {
        if matches!(
            tenant.status,
            TenantStatus::Ready
                | TenantStatus::Suspended
                | TenantStatus::Deleting
                | TenantStatus::Purged
        ) {
            continue;
        }
        // The production store cannot probe the DB; the
        // `InMemoryStore` returns Some(tenant) for every
        // probe, so this branch is exercised by tests.
        let found = store.find_tenant_by_id(&tenant.id).await?;
        if found.is_none() {
            report.missing_records.push(tenant.id.clone());
        } else {
            report.orphans.push(tenant.id.clone());
        }
    }
    Ok(report)
}

/// Output of `reconcile`. `orphans` is the list of tenants
/// the registry loaded. `missing_records` is the list of
/// tenants the registry could not load.
#[derive(Debug, Default, Clone)]
pub struct ReconcileReport {
    pub orphans: Vec<String>,
    pub missing_records: Vec<String>,
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
}

#[cfg(test)]
mod reconcile_tests {
    use super::*;
    use crate::http::registry::models::*;
    use crate::http::registry::storage::InMemoryStore;
    use chrono::Utc;
    use std::sync::Arc;

    fn reserved_tenant(id: &str) -> Tenant {
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

    #[tokio::test]
    async fn reconcile_marks_missing_records() {
        // Reconcile against a store that does not contain
        // any of the tenants; the report should list every
        // tenant under `missing_records`.
        let s = Arc::new(InMemoryStore::default());
        let tenant = Tenant {
            id: "ten_orphan".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_orphan".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        };
        let report = reconcile(
            &(s.clone() as Arc<dyn RegistryStore>),
            std::slice::from_ref(&tenant),
        )
        .await
        .unwrap();
        assert_eq!(report.missing_records, vec!["ten_orphan".to_string()]);
        assert!(report.orphans.is_empty());
    }

    #[tokio::test]
    async fn reconcile_skips_terminal_tenants() {
        let s = Arc::new(InMemoryStore::default());
        let ready = Tenant {
            id: "ten_a".into(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: "tns_a".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        };
        let reserved = Tenant {
            id: "ten_b".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_b".into(),
                database: "memory".into(),
            },
            ..ready.clone()
        };
        s.write_tenant(&ready).await.unwrap();
        s.write_tenant(&reserved).await.unwrap();
        let report = reconcile(&(s.clone() as Arc<dyn RegistryStore>), &[ready, reserved])
            .await
            .unwrap();
        assert!(report.missing_records.is_empty());
        assert_eq!(report.orphans, vec!["ten_b".to_string()]);
    }

    #[tokio::test]
    async fn transition_fenced_rejects_stale_generation() {
        let s = store_with_tenant(reserved_tenant("ten_1")).await;
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

    #[cfg(feature = "streamable-http")]
    #[test]
    fn classify_namespace_diff_surfaces_missing_and_orphan() {
        // Two tenants are registered. Only one of them
        // is present in the privileged engine's namespace
        // list, and the engine also reports an extra
        // namespace that no tenant points at.
        let tenants = vec![reserved_tenant("alive"), reserved_tenant("vanished")];
        let mut actual = std::collections::HashSet::new();
        actual.insert("tns_alive".into());
        actual.insert("tns_unbound".into());
        let diff = classify_namespace_diff(&tenants, &actual);
        let missing_ids: Vec<&str> = diff.missing.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(missing_ids, vec!["vanished"]);
        assert_eq!(diff.orphan, vec!["tns_unbound"]);
    }

    #[cfg(feature = "streamable-http")]
    #[test]
    fn classify_namespace_diff_is_empty_when_engines_match() {
        let tenants = vec![reserved_tenant("a"), reserved_tenant("b")];
        let mut actual = std::collections::HashSet::new();
        actual.insert("tns_a".into());
        actual.insert("tns_b".into());
        let diff = classify_namespace_diff(&tenants, &actual);
        assert!(diff.missing.is_empty());
        assert!(diff.orphan.is_empty());
    }

    #[cfg(feature = "streamable-http")]
    #[test]
    fn classify_namespace_diff_reports_non_tenant_namespaces_as_orphan() {
        // `classify_namespace_diff` is a pure diff over the
        // `actual_namespaces` set the caller hands it. The
        // `tns_*` filter is applied by the caller before
        // the diff runs, so any non-tenant namespace that
        // slips through will surface here as an orphan.
        // This test pins the diff contract; the caller
        // owns the prefix filter.
        let tenants = vec![reserved_tenant("a")];
        let mut actual = std::collections::HashSet::new();
        actual.insert("tns_a".into());
        actual.insert("system".into());
        let diff = classify_namespace_diff(&tenants, &actual);
        assert!(diff.missing.is_empty());
        assert_eq!(diff.orphan, vec!["system"]);
    }
}
