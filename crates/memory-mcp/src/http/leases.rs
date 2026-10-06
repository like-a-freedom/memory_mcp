//! Provisioning leases.
//!
//! `ProvisioningLease` is the durable claim returned by
//! `ProvisioningStore::claim_provisioning`. The fenced CAS in
//! the registry matches `(owner_id, lease_id,
//! fencing_generation)` against the stored lease before any
//! state advance, and `heartbeat` forwards the periodic renewal.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub(crate) async fn await_body_or_lease_loss<T, F>(
    body: F,
    lost_rx: &mut tokio::sync::oneshot::Receiver<()>,
) -> Result<T, crate::error::MemoryError>
where
    F: std::future::Future<Output = Result<T, crate::error::MemoryError>>,
{
    tokio::pin!(body);
    tokio::select! {
        // Once the fenced operation has completed, its result is authoritative.
        // This prevents a simultaneous stale-heartbeat signal from turning a
        // committed operation into an ambiguous Conflict for the caller.
        biased;
        result = &mut body => result,
        _ = &mut *lost_rx => Err(crate::error::MemoryError::Conflict("provisioning lease lost".into())),
    }
}

pub mod migration;
pub mod scheduler;

/// Fenced provisioning lease returned by
/// `ProvisioningStore::claim_provisioning`. The token is
/// intentionally not constructible by a request handler; only
/// the atomic registry claim returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningLease {
    pub owner_id: String,
    pub lease_id: String,
    pub fencing_generation: u64,
    pub expires_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
}

impl ProvisioningLease {
    /// Heartbeat the lease. Forwarded to
    /// `ProvisioningStore::heartbeat_provisioning`.
    pub async fn heartbeat(
        &self,
        store: &dyn crate::http::registry::ProvisioningStore,
        tenant_id: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), crate::error::MemoryError> {
        store
            .heartbeat_provisioning(
                tenant_id,
                &self.owner_id,
                &self.lease_id,
                self.fencing_generation,
                now,
                expires_at,
            )
            .await
    }
}

#[cfg(test)]
mod heartbeat_tests {
    use super::*;

    use crate::error::MemoryError;

    #[tokio::test]
    async fn completed_body_wins_over_simultaneous_lease_loss() {
        let (lost_tx, mut lost_rx) = tokio::sync::oneshot::channel();
        lost_tx.send(()).expect("receiver is still alive");

        let result =
            await_body_or_lease_loss(async { Ok::<_, MemoryError>(42) }, &mut lost_rx).await;

        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn a_completed_body_error_is_returned_unchanged() {
        let (lost_tx, mut lost_rx) = tokio::sync::oneshot::channel();
        lost_tx.send(()).expect("receiver is still alive");

        let result = await_body_or_lease_loss(
            async { Err::<u32, _>(MemoryError::Storage("index down".into())) },
            &mut lost_rx,
        )
        .await;

        assert!(matches!(result, Err(MemoryError::Storage(_))));
    }

    #[tokio::test]
    async fn a_lease_loss_before_the_body_completes_is_a_conflict() {
        let (_lost_tx, mut lost_rx) = tokio::sync::oneshot::channel::<()>();
        // The sender is dropped, so the receiver resolves immediately and the
        // lease-loss arm wins before the (never-ready) body can complete.
        drop(_lost_tx);

        let result = await_body_or_lease_loss(
            async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok::<_, MemoryError>(42)
            },
            &mut lost_rx,
        )
        .await;

        assert!(matches!(result, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    async fn a_pending_lease_does_not_block_the_body() {
        // The sender is held, so the lease-loss arm can never fire and the body
        // is the only way this select can resolve.
        let (lost_tx, mut lost_rx) = tokio::sync::oneshot::channel::<()>();

        let result =
            await_body_or_lease_loss(async { Ok::<_, MemoryError>(7) }, &mut lost_rx).await;

        assert_eq!(result.unwrap(), 7);
        drop(lost_tx);
    }
}

#[cfg(test)]
mod tests {
    use crate::error::MemoryError;

    use crate::http::registry::models::{Account, AccountStatus, Tenant, TenantStatus};
    use crate::http::registry::storage::{
        AccountStore, InMemoryStore, ProvisioningStore, TenantStore,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn stale_fenced_worker_cannot_commit() {
        let store: Arc<InMemoryStore> = Arc::new(InMemoryStore::default());
        let now = chrono::Utc::now();
        store
            .write_account(&Account {
                id: "acct_a".into(),
                status: AccountStatus::Active,
                tenant_id: "ten_a".into(),
                created_at: now,
                display_name: None,
            })
            .await
            .unwrap();
        store
            .write_tenant(&Tenant {
                id: "ten_a".into(),
                status: TenantStatus::Reserved,
                namespace_binding: crate::http::registry::models::NamespaceBinding {
                    namespace: "tns_a".into(),
                    database: "memory".into(),
                },
                plan_version: 1,
                schema_version: 0,
                retry_stage: None,
                provisioning_lease: None,
                created_at: now,
                version: 0,
            })
            .await
            .unwrap();
        // 1. Worker A claims a lease at gen 0.
        let lease_a = store
            .claim_provisioning("ten_a", "replica_a", "lease_a", 60)
            .await
            .expect("claim")
            .expect("tenant due");
        assert_eq!(lease_a.fencing_generation, 1);
        assert!(matches!(
            store
                .claim_provisioning("ten_a", "replica_b", "lease_b", 60)
                .await,
            Err(MemoryError::Conflict(_))
        ));
        // 2. The lease expires according to datastore time.
        // Only then may worker B take it over with a higher
        // fencing generation.
        store
            .heartbeat_provisioning(
                "ten_a",
                &lease_a.owner_id,
                &lease_a.lease_id,
                lease_a.fencing_generation,
                now - chrono::Duration::seconds(2),
                now - chrono::Duration::seconds(1),
            )
            .await
            .expect("expire lease");
        let lease_b = store
            .claim_provisioning("ten_a", "replica_b", "lease_b", 60)
            .await
            .expect("claim")
            .expect("still due (lease expired-from-the-POV-of-claim)");
        assert_eq!(lease_b.fencing_generation, 2);
        // 3. Worker A tries to heartbeat with the stale
        // generation. The registry must reject.
        let result = store
            .heartbeat_provisioning(
                "ten_a",
                &lease_a.owner_id,
                &lease_a.lease_id,
                lease_a.fencing_generation,
                now,
                now + chrono::Duration::seconds(60),
            )
            .await;
        assert!(matches!(result, Err(MemoryError::Conflict(_))));
    }
}
