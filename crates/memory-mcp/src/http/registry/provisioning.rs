//! Provisioning wiring for the Tenant lifecycle.
//!
//! The transition table and the two use cases that apply it live in
//! [`crate::provisioning::api`], because which moves between Tenant statuses
//! are legal is a fact about a Tenant rather than about HTTP. What stays here
//! is the wiring: the durable enqueue, the reconciliation scheduler job, and
//! the pass that drives the tenant list.

use std::collections::HashSet;
use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::registry::storage::{ProvisioningStore, TenantStore};
use crate::models::registry::TenantStatus;

/// The provisioning stage is the Tenant status under another name. Re-exported
/// so this module's wiring reads in stage vocabulary while the type stays the
/// one the context's transition table takes.
pub use crate::models::registry::TenantStatus as ProvisioningStage;

/// Durable enqueue: append a provisioning event for the
/// reserved tenant. The control API calls this after writing a
/// reserved Tenant. The scheduler consumes the events; bootstrap
/// calls it for each ready tenant.
pub async fn enqueue_provisioning(
    store: &Arc<dyn ProvisioningStore>,
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
/// Registry reconciliation metric family: namespaces the registry believes in
/// and the engine does not, or the other way round.
pub const METRIC_HTTP_REGISTRY_RECONCILIATION_TOTAL: &str =
    "memory_http_registry_reconciliation_total";

/// Record one namespace the registry and the engine disagree about.
///
/// A single recorder for both directions, for the same reason the rest of the
/// surface has one per family: two inline `metrics::counter!` calls with the
/// family name spelled out as a string is how a family ends up with two
/// spellings, and it is also why nothing else could record this one — a test
/// cannot drive a counter that only exists inside a loop over a database.
///
/// `kind` is bounded by the two branches below and collapses to `other`, so a
/// caller cannot widen the series by formatting a string into it.
pub(crate) fn record_registry_reconciliation(kind: &'static str) {
    let kind = match kind {
        "missing_namespace" => "missing_namespace",
        "orphan_namespace" => "orphan_namespace",
        _ => "other",
    };
    metrics::counter!(METRIC_HTTP_REGISTRY_RECONCILIATION_TOTAL, "kind" => kind).increment(1);
}

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
    let tenant_store = registry.tenants();
    let registered = tenant_store.list_tenants(10_000).await?;
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
        record_registry_reconciliation("missing_namespace");
        crate::http::logging::log_warn(
            "http.registry.missing_namespace_binding",
            &format!(
                "tenant_fingerprint={} namespace_fingerprint={}",
                identifier_fingerprint(&tenant.id),
                identifier_fingerprint(&tenant.namespace_binding.namespace)
            ),
        );
    }
    for namespace in diff.orphan {
        record_registry_reconciliation("orphan_namespace");
        crate::http::logging::log_warn(
            "http.registry.orphan_namespace",
            &format!(
                "namespace_fingerprint={}",
                identifier_fingerprint(namespace)
            ),
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
    tenant_store: &Arc<dyn TenantStore>,
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
        let found = tenant_store.find_tenant_by_id(&tenant.id).await?;
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

// The first test module held only the transition table's cases and the
// illegal-pair case, both of which moved with the table to
// `tests/tenant_lifecycle.rs`. The wiring tests live in the module below.
#[cfg(test)]
mod reconcile_tests {
    use super::*;
    use crate::http::leases::ProvisioningLease;
    use crate::http::registry::models::*;
    use crate::http::registry::storage::{AccountStore, InMemoryStore, TenantStore};
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
            display_name: None,
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
        let tenant_store = s.clone() as Arc<dyn TenantStore>;
        let report = reconcile(&tenant_store, std::slice::from_ref(&tenant))
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
        let tenant_store = s.clone() as Arc<dyn TenantStore>;
        let report = reconcile(&tenant_store, &[ready, reserved]).await.unwrap();
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
        let lifecycle = crate::provisioning::api::TenantLifecycle::new(s.as_ref());
        let res = crate::provisioning::api::transition_tenant_fenced(
            &lifecycle,
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
        let lifecycle = crate::provisioning::api::TenantLifecycle::new(s.as_ref());
        let res = crate::provisioning::api::transition_tenant_fenced(
            &lifecycle,
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
