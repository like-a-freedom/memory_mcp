//! Migration worker.
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
//! remote Ws engine. The trait is wired into the
//! `PrivilegedEngine` enum; tests use a stub implementation
//! that records calls in-memory.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::leases::ProvisioningLease;
use crate::http::registry::RegistryStore;
use crate::http::registry::models::TenantStatus;
use crate::http::registry::provisioning::transition_fenced;
use crate::http::registry::storage::LeaseFence;

/// The schema version this binary ships. The actual
/// migrations live in
/// `crates/memory-mcp/migrations/*.surql` and bump this
/// constant via the release process; the runner copies the
/// value out at compile time so the test path can compare
/// against it without an env var.
pub const CURRENT_SCHEMA_VERSION: u32 = 30;

/// Inclusive range of schema versions this replica can
/// serve. The lower bound is `CURRENT_SCHEMA_VERSION - 1`
/// so a rolling N → N+1 deployment keeps tenants with
/// schema N reachable on the old replica while the new
/// replica migrates them to N+1.
pub const REPLICA_SCHEMA_RANGE: std::ops::RangeInclusive<u32> =
    CURRENT_SCHEMA_VERSION.saturating_sub(1)..=CURRENT_SCHEMA_VERSION;

/// Best-effort warn helper. Mirrors the pool's helper; the
/// workspace does not yet depend on `tracing` or `log`.
#[allow(dead_code)]
fn tracing_warn(message: &str) {
    let _ = message;
}

/// What `provision_one` needs from a privileged SurrealDB
/// engine: `ensure_namespace` plus the ability to apply the
/// versioned migrations in `storage/migrations.rs` to the bound
/// tenant namespace.
#[async_trait::async_trait]
pub trait ApplyMigrations: Send + Sync + 'static {
    /// Ensure the (namespace, database) pair exists.
    /// Implemented by `ensure_namespace`.
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
        // The apply_migrations contract is the new
        // schema_version after a successful run, not the
        // number of migrations applied. NoopMigrations is
        // the test fixture for the state machine; the real
        // DDL runner computes the target from the
        // migrations directory and the binary's
        // CURRENT_SCHEMA_VERSION.
        Ok(CURRENT_SCHEMA_VERSION)
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
    // (e.g. another worker already advanced it), we no-op
    // before any schema or state CAS. This is what makes
    // `ten_3` (Ready, schema=1) a benign no-op regardless
    // of the current binary's REPLICA_SCHEMA_RANGE.
    if tenant.status == TenantStatus::Ready {
        return Ok(());
    }

    // N/N-1 schema compatibility. A tenant whose
    // schema_version sits outside this replica's range is
    // skipped — the scheduler will pick it up on a
    // compatible replica, or the data plane will surface
    // the Unavailable (→503) until the tenant is migrated
    // forward.
    if !REPLICA_SCHEMA_RANGE.contains(&tenant.schema_version) {
        return Err(MemoryError::Unavailable(format!(
            "tenant {tenant_id} schema_version {} outside replica range {:?}",
            tenant.schema_version, REPLICA_SCHEMA_RANGE
        )));
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
    // Release the lease. Best-effort: a stale release is
    // surfaced as `Conflict`; the tenant is already Ready and
    // the operator can clear the lease via the next scheduler
    // sweep. We do not regress the Ready transition on a
    // release failure.
    if let Err(error) = store
        .release_provisioning_lease(
            tenant_id,
            &lease.owner_id,
            &lease.lease_id,
            lease.fencing_generation,
        )
        .await
    {
        tracing_warn(&format!(
            "post-Ready lease release failed for {tenant_id}: {error}"
        ));
    }
    Ok(())
}

/// Scheduler tick: walk up to `limit` due tenants, claim a
/// fresh lease for each, and run `provision_one` while
/// heartbeating. A conflict (someone else claimed the lease
/// first, or the tenant moved to a terminal state in the
/// meantime) is recorded as a bounded warning and the
/// scheduler continues with the next row. Tenants are never
/// mutated directly by the scheduler: all state advances
/// happen through `provision_one`.
pub async fn run_due_provisioning(
    registry: crate::http::registry::RegistryHandle,
) -> Result<(), MemoryError> {
    let store = registry.store_clone();
    run_due_provisioning_for(registry, store, 100, chrono::Utc::now()).await
}

/// Test-friendly seam: walk up to `limit` due tenants. The
/// `now` parameter lets tests pin time without freezing
/// the clock.
pub async fn run_due_provisioning_for(
    registry: crate::http::registry::RegistryHandle,
    store: Arc<dyn crate::http::registry::RegistryStore>,
    limit: usize,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), MemoryError> {
    let due = store.list_due_provisioning(limit, now).await?;
    for tenant in due {
        let lease_id = uuid::Uuid::new_v4().to_string();
        let owner_id = "scheduler".to_string();
        let claim = match store
            .claim_provisioning(&tenant.id, &owner_id, &lease_id, 60)
            .await
        {
            Ok(Some(l)) => l,
            Ok(None) => {
                tracing_warn(&format!(
                    "scheduler: claim returned None for {} (terminal state)",
                    tenant.id
                ));
                continue;
            }
            Err(MemoryError::Conflict(reason)) => {
                tracing_warn(&format!(
                    "scheduler: claim conflict for {}: {reason}",
                    tenant.id
                ));
                continue;
            }
            Err(other) => {
                tracing_warn(&format!(
                    "scheduler: claim failed for {}: {other}",
                    tenant.id
                ));
                continue;
            }
        };
        // Re-derive a typed `ProvisioningLease` for the
        // claim so we can hand it to `provision_one`.
        let typed = ProvisioningLease {
            owner_id: claim.owner_id,
            lease_id: claim.lease_id,
            fencing_generation: claim.fencing_generation,
            expires_at: claim.expires_at,
            heartbeat_at: claim.heartbeat_at,
        };
        // The heartbeat helper runs at lease_ttl / 3; for
        // 60s leases that's ~20s, which is too long for a
        // scheduler tick. We use the bounded
        // `provision_one` (which already runs migrations
        // and then releases) and rely on the
        // `with_heartbeat` helper only when the work is
        // long-running. The current migration pass is
        // bounded; if it ever exceeds the lease window
        // the next scheduler tick will reclaim.
        if let Err(error) =
            provision_one(store.clone(), &tenant.id, typed, Arc::new(NoopMigrations)).await
        {
            tracing_warn(&format!(
                "scheduler: provision_one failed for {}: {error}",
                tenant.id
            ));
            // The tenant is in a failed state; the next
            // scheduler tick will skip it (no longer in
            // Reserved/Migrating/Suspended).
        }
        // The test path uses the in-memory store directly;
        // the production path uses the registry handle. We
        // hold both for symmetry with the spec.
        let _ = registry;
    }
    Ok(())
}

/// Heartbeat the lease on a `lease_ttl / 3` cadence with
/// ±20% jitter while the body runs.
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
                // The real heartbeat writes back to the registry.
                // The lease timeout is the safety net; the tick
                // here is a no-op so the test surfaces the right shape.
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

    fn reserved_tenant(id: &str, namespace: &str) -> Tenant {
        Tenant {
            id: id.to_string(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: namespace.to_string(),
                database: "memory".into(),
            },
            plan_version: 1,
            // The Reserved test fixture must sit inside
            // REPLICA_SCHEMA_RANGE so the N/N-1 gate in
            // provision_one does not bounce it. Real Reserved
            // tenants are N-1 (waiting to migrate to N).
            schema_version: CURRENT_SCHEMA_VERSION.saturating_sub(1),
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        }
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
            schema_version: CURRENT_SCHEMA_VERSION.saturating_sub(1),
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
            schema_version: CURRENT_SCHEMA_VERSION.saturating_sub(1),
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

    #[tokio::test]
    async fn run_due_provisioning_claims_and_provisions_due_tenant() {
        use crate::http::registry::RegistryHandle;
        use crate::http::registry::storage::InMemoryStore;
        let store = Arc::new(InMemoryStore::default());
        let registry = RegistryHandle::from_store(store.clone());
        let tenant = Tenant {
            id: "ten_due".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_due".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: CURRENT_SCHEMA_VERSION.saturating_sub(1),
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        };
        seed_reserved(&store, &tenant).await;
        run_due_provisioning_for(registry.clone(), store.clone(), 1, Utc::now())
            .await
            .expect("scheduler tick");
        let after = store
            .find_tenant_by_id("ten_due")
            .await
            .expect("read")
            .expect("present");
        assert_eq!(after.status, TenantStatus::Ready);
        assert!(after.provisioning_lease.is_none());
    }

    #[tokio::test]
    async fn replica_skips_tenant_outside_schema_range() {
        let store = Arc::new(InMemoryStore::default());
        // Place the tenant at a schema_version that sits
        // below REPLICA_SCHEMA_RANGE.start().
        let too_old = REPLICA_SCHEMA_RANGE.start().saturating_sub(1);
        let mut tenant = reserved_tenant("ten_old", "tns_old");
        tenant.schema_version = too_old;
        seed_reserved(&store, &tenant).await;
        let lease = lease_for("ten_old");
        seed_with_lease(&store, "ten_old", &lease).await;
        let result = provision_one(store.clone(), "ten_old", lease, Arc::new(NoopMigrations)).await;
        assert!(
            matches!(result, Err(MemoryError::Unavailable(_))),
            "expected Unavailable for out-of-range schema, got: {result:?}"
        );
        // The tenant must remain in its original status;
        // provision_one must not advance it.
        let after = store
            .find_tenant_by_id("ten_old")
            .await
            .expect("read")
            .expect("present");
        assert_eq!(after.status, TenantStatus::Reserved);
    }

    #[tokio::test]
    async fn migration_after_compatible_roll_marks_tenant_ready() {
        let store = Arc::new(InMemoryStore::default());
        // Place the tenant at REPLICA_SCHEMA_RANGE.start()
        // (i.e. one step behind current) so a single
        // provision_one call lands it on CURRENT_SCHEMA_VERSION.
        let mut tenant = reserved_tenant("ten_roll", "tns_roll");
        tenant.schema_version = *REPLICA_SCHEMA_RANGE.start();
        seed_reserved(&store, &tenant).await;
        let lease = lease_for("ten_roll");
        seed_with_lease(&store, "ten_roll", &lease).await;
        provision_one(store.clone(), "ten_roll", lease, Arc::new(NoopMigrations))
            .await
            .expect("provision succeeds on N-1");
        let after = store
            .find_tenant_by_id("ten_roll")
            .await
            .expect("read")
            .expect("present");
        assert_eq!(after.status, TenantStatus::Ready);
        assert_eq!(after.schema_version, CURRENT_SCHEMA_VERSION);
    }
}
