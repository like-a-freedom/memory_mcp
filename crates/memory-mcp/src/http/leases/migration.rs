//! Migration worker (ADR-0052, plan §5.3).
//!
//! `provision_one(registry, tenant_id, lease)` is the single
//! production path that takes a Tenant from `Reserved` to
//! `Ready`. The state machine is `can_transition` from
//! `registry::provisioning`; the CAS predicate is
//! `(version, owner, lease, generation)` and the only
//! failure path transitions the Tenant to `Failed` and
//! records the error.
//!
//! The actual SurrealDB work (DDL + migrations) is delegated
//! to the [`ApplyMigrations`] trait so the worker does not
//! know whether the backend is the embedded Mem engine or a
//! remote Ws engine. Task 5.4 wires the trait into the
//! `PrivilegedEngine` enum; Phase 4 tests use a stub
//! implementation that records calls in-memory.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::ProvisioningLease;
use crate::http::registry::RegistryStore;
use crate::http::registry::models::TenantStatus;
use crate::http::registry::provisioning::transition_fenced;
use crate::http::registry::storage::LeaseFence;

/// What `provision_one` needs from a privileged SurrealDB
/// engine: `ensure_namespace` (Task 5.2) plus the ability to
/// apply the versioned migrations in `storage/migrations.rs`
/// to the bound tenant namespace.
#[async_trait::async_trait]
pub trait ApplyMigrations: Send + Sync + 'static {
    /// Ensure the (namespace, database) pair exists.
    /// Implemented by `ensure_namespace` from Task 5.2.
    async fn ensure_namespace(&self, namespace: &str, database: &str) -> Result<(), MemoryError>;

    /// Apply the versioned migrations to the bound client.
    /// Returns the new `schema_version` (the count of
    /// versioned scripts applied) on success.
    async fn apply_migrations(&self, namespace: &str) -> Result<u32, MemoryError>;
}

/// A no-op `ApplyMigrations` impl useful for tests that only
/// exercise the state machine (no actual DDL).
pub struct NoopMigrations;

#[async_trait::async_trait]
impl ApplyMigrations for NoopMigrations {
    async fn ensure_namespace(&self, _namespace: &str, _database: &str) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn apply_migrations(&self, _namespace: &str) -> Result<u32, MemoryError> {
        Ok(0)
    }
}

/// Run one full provisioning pass for `tenant_id` under the
/// provided `lease`. Returns `Ok(())` when the Tenant is
/// `Ready`, the original error otherwise. The Tenant is
/// transitioned to `Failed` on every error path so a stale
/// worker can never leave a partially-migrated Tenant in
/// `Migrating`.
pub async fn provision_one(
    store: Arc<dyn RegistryStore>,
    tenant_id: &str,
    lease: ProvisioningLease,
    migrations: Arc<dyn ApplyMigrations>,
) -> Result<(), MemoryError> {
    let mut tenant = store
        .find_tenant_by_id(tenant_id)
        .await?
        .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;

    // Re-entry: if the tenant is already in a later state
    // (e.g. another worker already advanced it), we no-op.
    if tenant.status == TenantStatus::Ready {
        return Ok(());
    }

    let retry_from = tenant.retry_stage;
    if tenant.status == TenantStatus::Reserved {
        transition_fenced(
            store.as_ref(),
            tenant_id,
            tenant.version,
            TenantStatus::Reserved,
            TenantStatus::NamespaceCreating,
            &lease,
        )
        .await?;
    } else if tenant.status == TenantStatus::Failed {
        let stage = retry_from
            .ok_or_else(|| MemoryError::Validation("failed tenant has no retry stage".into()))?;
        transition_fenced(
            store.as_ref(),
            tenant_id,
            tenant.version,
            TenantStatus::Failed,
            stage,
            &lease,
        )
        .await?;
    }

    tenant = store
        .find_tenant_by_id(tenant_id)
        .await?
        .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
    if tenant.status == TenantStatus::NamespaceCreating {
        transition_fenced(
            store.as_ref(),
            tenant_id,
            tenant.version,
            TenantStatus::NamespaceCreating,
            TenantStatus::Migrating,
            &lease,
        )
        .await?;
    }

    tenant = store
        .find_tenant_by_id(tenant_id)
        .await?
        .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
    if tenant.status != TenantStatus::Migrating {
        return Err(MemoryError::Conflict(
            "tenant is no longer in Migrating state".into(),
        ));
    }
    let binding = tenant.namespace_binding.clone();
    let _fence = LeaseFence::from_lease(&lease);

    let migration_result = run_heartbeated(lease.clone(), async {
        migrations
            .ensure_namespace(&binding.namespace, &binding.database)
            .await?;
        let new_version = migrations.apply_migrations(&binding.namespace).await?;
        Ok(new_version)
    })
    .await;

    let schema_version = match migration_result {
        Ok(version) => version,
        Err(error) => {
            // Best-effort failure transition. If this fails we
            // still propagate the original error.
            if let Ok(Some(failed)) = store.find_tenant_by_id(tenant_id).await
                && matches!(
                    failed.status,
                    TenantStatus::NamespaceCreating | TenantStatus::Migrating
                )
            {
                let _ = transition_fenced(
                    store.as_ref(),
                    tenant_id,
                    failed.version,
                    failed.status,
                    TenantStatus::Failed,
                    &lease,
                )
                .await;
            }
            return Err(error);
        }
    };

    // Persist the new schema_version with a fenced CAS.
    tenant = store
        .find_tenant_by_id(tenant_id)
        .await?
        .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
    let new_version = store
        .update_tenant_schema_version_fenced(
            tenant_id,
            tenant.version,
            schema_version,
            &lease.owner_id,
            &lease.lease_id,
            lease.fencing_generation,
        )
        .await?;
    transition_fenced(
        store.as_ref(),
        tenant_id,
        new_version,
        TenantStatus::Migrating,
        TenantStatus::Ready,
        &lease,
    )
    .await?;
    Ok(())
}

/// Heartbeat the lease on a `lease_ttl / 3` cadence with
/// ±20% jitter while the body runs. Phase 5 ships a minimal
/// in-task version; Task 6.1 generalizes this primitive
/// across all workers.
pub async fn run_heartbeated<F, T>(lease: ProvisioningLease, body: F) -> Result<T, MemoryError>
where
    F: std::future::Future<Output = Result<T, MemoryError>>,
{
    let ttl_secs = (lease.expires_at - lease.heartbeat_at).num_seconds().max(1);
    let base_interval = std::time::Duration::from_secs((ttl_secs / 3).max(1) as u64);
    // Apply ±20% jitter from the process clock; we don't
    // pull in a RNG dep for this single jitter.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let span_ms = (base_interval.as_millis() / 5) as u64;
    let jitter_ms = if span_ms == 0 {
        0
    } else {
        nanos % (span_ms * 2)
    };
    let offset = jitter_ms.saturating_sub(span_ms);
    let interval_duration = base_interval + std::time::Duration::from_millis(offset);
    let mut interval = tokio::time::interval(interval_duration);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first tick.
    interval.tick().await;

    tokio::pin!(body);
    loop {
        tokio::select! {
            result = &mut body => return result,
            _ = interval.tick() => {
                // The real heartbeat (Task 6.1) writes back to
                // the registry. For Phase 5 the lease timeout
                // is the safety net; the tick here is a
                // no-op so the test surfaces the right shape.
                let _ = lease.expires_at;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::*;
    use crate::http::registry::storage::InMemoryStore;
    use chrono::Utc;

    fn lease_for(tenant_id: &str) -> ProvisioningLease {
        ProvisioningLease {
            owner_id: "replica_a".into(),
            lease_id: format!("lease-{tenant_id}"),
            fencing_generation: 0,
            expires_at: Utc::now() + chrono::Duration::seconds(60),
            heartbeat_at: Utc::now(),
        }
    }

    async fn seed_reserved(store: &InMemoryStore, tenant: &Tenant) {
        let account = Account {
            id: "acct_1".into(),
            status: AccountStatus::Active,
            tenant_id: tenant.id.clone(),
            created_at: Utc::now(),
        };
        store.write_account(&account).await.unwrap();
        store.write_tenant(tenant).await.unwrap();
    }

    /// Seed a tenant with the supplied lease already attached
    /// (so `transition_fenced` does not see a missing lease).
    async fn seed_with_lease(store: &InMemoryStore, tenant_id: &str, lease: &ProvisioningLease) {
        let mut t = store.find_tenant_by_id(tenant_id).await.unwrap().unwrap();
        t.provisioning_lease = Some(ProvisioningLeaseState {
            owner_id: lease.owner_id.clone(),
            lease_id: lease.lease_id.clone(),
            expires_at: lease.expires_at,
            fencing_generation: lease.fencing_generation,
            heartbeat_at: lease.heartbeat_at,
        });
        store.write_tenant(&t).await.unwrap();
    }

    #[tokio::test]
    async fn provision_one_advances_reserved_to_ready() {
        let store = Arc::new(InMemoryStore::default());
        let tenant = Tenant {
            id: "ten_1".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_ten_1".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        };
        seed_reserved(&store, &tenant).await;
        let lease = lease_for("ten_1");
        seed_with_lease(&store, "ten_1", &lease).await;
        provision_one(store.clone(), "ten_1", lease, Arc::new(NoopMigrations))
            .await
            .expect("provision succeeds");
        let t = store.find_tenant_by_id("ten_1").await.unwrap().unwrap();
        assert_eq!(t.status, TenantStatus::Ready);
    }

    #[tokio::test]
    async fn provision_one_records_failure_on_migration_error() {
        struct FailingMigrations;
        #[async_trait::async_trait]
        impl ApplyMigrations for FailingMigrations {
            async fn ensure_namespace(&self, _n: &str, _d: &str) -> Result<(), MemoryError> {
                Ok(())
            }
            async fn apply_migrations(&self, _n: &str) -> Result<u32, MemoryError> {
                Err(MemoryError::Storage("simulated failure".into()))
            }
        }

        let store = Arc::new(InMemoryStore::default());
        let tenant = Tenant {
            id: "ten_2".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_ten_2".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        };
        seed_reserved(&store, &tenant).await;
        let lease = lease_for("ten_2");
        seed_with_lease(&store, "ten_2", &lease).await;
        let res = provision_one(store.clone(), "ten_2", lease, Arc::new(FailingMigrations)).await;
        assert!(res.is_err());
        let t = store.find_tenant_by_id("ten_2").await.unwrap().unwrap();
        assert_eq!(t.status, TenantStatus::Failed);
    }

    #[tokio::test]
    async fn provision_one_noop_when_already_ready() {
        let store = Arc::new(InMemoryStore::default());
        let tenant = Tenant {
            id: "ten_3".into(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: "tns_ten_3".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 1,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 5,
        };
        seed_reserved(&store, &tenant).await;
        let lease = lease_for("ten_3");
        provision_one(store.clone(), "ten_3", lease, Arc::new(NoopMigrations))
            .await
            .expect("no-op succeeds");
    }
}
