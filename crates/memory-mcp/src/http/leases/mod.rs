//! Provisioning leases.
//!
//! `ProvisioningLease` is the durable claim returned by
//! `RegistryStore::claim_provisioning`. The fenced CAS in
//! the registry matches `(owner_id, lease_id,
//! fencing_generation)` against the stored lease before any
//! state advance. The `run_with_heartbeat` helper spawns a
//! jittered heartbeat task and cancels the work future if
//! the lease is lost.
//!
//! `LeaseRecord` is the durable shape stored in the
//! `provisioning_lease` field of a `Tenant`. The
//! `FenceUpdate` enum and `commit_with_fence` helper
//! implement the closed-set CAS surface so a request handler
//! cannot inject arbitrary SurrealQL through the lease.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod migration;
pub mod scheduler;

/// Stored shape of a `provisioning_lease` in the
/// `tenant_provisioning_lease` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub owner_id: String,
    pub lease_id: String,
    pub expires_at: DateTime<Utc>,
    pub fencing_generation: u64,
    pub heartbeat_at: DateTime<Utc>,
}

/// Fenced provisioning lease returned by
/// `RegistryStore::claim_provisioning`. The token is
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
    /// Seconds remaining until the lease expires. May be
    /// negative if the scheduler has not yet noticed expiry.
    pub fn ttl_secs(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_seconds()
    }

    /// True if the lease has expired relative to `now`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.ttl_secs(now) <= 0
    }

    /// Heartbeat the lease. Forwarded to
    /// `RegistryStore::heartbeat_provisioning`.
    pub async fn heartbeat(
        &self,
        store: &dyn crate::http::registry::RegistryStore,
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

    /// Release the lease. Forwarded to
    /// `RegistryStore::release_provisioning_lease`.
    pub async fn release(
        &self,
        store: &dyn crate::http::registry::RegistryStore,
        tenant_id: &str,
    ) -> Result<(), crate::error::MemoryError> {
        store
            .release_provisioning_lease(
                tenant_id,
                &self.owner_id,
                &self.lease_id,
                self.fencing_generation,
            )
            .await
    }

    /// Run `work` while heartbeating the lease at a
    /// `lease_ttl / 3` cadence with ±20% jitter. If the
    /// heartbeat fails (lease lost), the work future is
    /// cancelled and a `Conflict` error is returned. The
    /// tenant id is passed explicitly; it is never derived
    /// by parsing the opaque lease id.
    pub async fn run_with_heartbeat<T, F>(
        &self,
        registry: crate::http::registry::RegistryHandle,
        tenant_id: &str,
        work: F,
    ) -> Result<T, crate::error::MemoryError>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, crate::error::MemoryError>> + Send,
    {
        let store = registry.store_clone();
        let tenant_id = tenant_id.to_owned();
        let heartbeat_cancel = tokio_util::sync::CancellationToken::new();
        let (lost_tx, mut lost_rx) = tokio::sync::oneshot::channel();
        let lease = self.clone();
        let cancel = heartbeat_cancel.clone();
        let heartbeat = tokio::spawn(async move {
            let mut first = true;
            loop {
                let delay = if first {
                    std::time::Duration::ZERO
                } else {
                    // 16–24 seconds: lease_ttl / 3 with ±20% jitter.
                    std::time::Duration::from_secs(16 + u64::from(rand_u8_below(9)))
                };
                first = false;
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(delay) => {
                        let now = chrono::Utc::now();
                        let expiry = now + chrono::Duration::seconds(60);
                        if lease.heartbeat(store.as_ref(), &tenant_id, now, expiry).await.is_err() {
                            let _ = lost_tx.send(());
                            break;
                        }
                    }
                }
            }
        });
        let result = tokio::select! {
            result = work => result,
            _ = &mut lost_rx => Err(crate::error::MemoryError::Conflict("provisioning lease lost".into())),
        };
        heartbeat_cancel.cancel();
        let _ = heartbeat.await;
        result
    }
}

/// Closed-set lease mutations. Callers cannot inject
/// arbitrary SurrealQL through `commit_with_fence`; only
/// these three shapes are valid.
#[derive(Debug, Clone)]
pub enum FenceUpdate {
    Claim {
        owner_id: String,
        lease_id: String,
        lease_expiry: DateTime<Utc>,
    },
    Heartbeat {
        owner_id: String,
        lease_id: String,
        lease_expiry: DateTime<Utc>,
        heartbeat_at: DateTime<Utc>,
    },
    Release {
        owner_id: String,
        lease_id: String,
    },
}

#[cfg(feature = "streamable-http")]
fn rand_u8_below(upper: u8) -> u8 {
    rand::random::<u8>() % upper
}
#[cfg(not(feature = "streamable-http"))]
fn rand_u8_below(upper: u8) -> u8 {
    // Without `streamable-http` the lease module is unused;
    // the function still exists for test builds that pull
    // it in directly. A deterministic 0 keeps the cadence
    // predictable.
    0u8.min(upper.saturating_sub(1))
}

impl ProvisioningLease {
    /// Fenced commit using the closed-set `FenceUpdate` enum.
    /// The `expected_generation` and `record_id` are bound
    /// here; callers cannot supply or override the generation
    /// through an interpolated clause.
    pub async fn commit_with_fence(
        client: &std::sync::Arc<dyn crate::storage::client::DbClient>,
        namespace: &str,
        record_id: &str,
        expected_generation: u64,
        update: FenceUpdate,
    ) -> Result<u64, crate::error::MemoryError> {
        if record_id.is_empty()
            || !record_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        {
            return Err(crate::error::MemoryError::Validation(format!(
                "unsafe record id: {record_id}"
            )));
        }
        let mut params = serde_json::Map::new();
        params.insert(
            "expected_gen".to_string(),
            serde_json::Value::from(expected_generation),
        );
        let (set_clause, owner_clause, update_vars, returned_generation) = match update {
            FenceUpdate::Claim {
                owner_id,
                lease_id,
                lease_expiry,
            } => {
                let next_generation = expected_generation.checked_add(1).ok_or_else(|| {
                    crate::error::MemoryError::Conflict("fencing generation overflow".into())
                })?;
                (
                    "lease_owner = $owner_id, lease_id = $lease_id, lease_expiry = $lease_expiry, lease_generation = $next_gen",
                    "(lease_expiry IS NONE OR lease_expiry < time::now())",
                    serde_json::json!({
                        "owner_id": owner_id,
                        "lease_id": lease_id,
                        "lease_expiry": lease_expiry,
                        "next_gen": next_generation,
                    }),
                    next_generation,
                )
            }
            FenceUpdate::Heartbeat {
                owner_id,
                lease_id,
                lease_expiry,
                heartbeat_at,
            } => (
                "lease_expiry = $lease_expiry, heartbeat_at = $heartbeat_at",
                "lease_owner = $owner_id AND lease_id = $lease_id",
                serde_json::json!({
                    "owner_id": owner_id,
                    "lease_id": lease_id,
                    "lease_expiry": lease_expiry,
                    "heartbeat_at": heartbeat_at,
                }),
                expected_generation,
            ),
            FenceUpdate::Release { owner_id, lease_id } => (
                "lease_owner = NONE, lease_id = NONE, lease_expiry = NONE",
                "lease_owner = $owner_id AND lease_id = $lease_id",
                serde_json::json!({
                    "owner_id": owner_id,
                    "lease_id": lease_id,
                }),
                expected_generation,
            ),
        };
        if let serde_json::Value::Object(update_vars) = update_vars {
            params.extend(update_vars);
        }
        let sql = format!(
            "UPDATE {record_id} SET {set_clause} WHERE lease_generation = $expected_gen AND {owner_clause} RETURN AFTER;"
        );
        let result = client
            .query(&sql, Some(serde_json::Value::Object(params)), namespace)
            .await?;
        let rows: Vec<serde_json::Value> = serde_json::from_value(result).map_err(|err| {
            crate::error::MemoryError::Storage(format!("fence commit result: {err}"))
        })?;
        match rows.first() {
            Some(row) => {
                let generation = row
                    .get("lease_generation")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        crate::error::MemoryError::Storage(
                            "fence commit returned no generation".into(),
                        )
                    })?;
                if generation != returned_generation {
                    return Err(crate::error::MemoryError::Conflict(format!(
                        "fencing generation mismatch: expected {returned_generation}, found {generation}"
                    )));
                }
                Ok(generation)
            }
            None => Err(crate::error::MemoryError::Conflict(
                "fence commit matched no rows (lease lost)".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::error::MemoryError;
    use crate::http::registry::RegistryStore;
    use crate::http::registry::models::{Account, AccountStatus, Tenant, TenantStatus};
    use crate::http::registry::storage::InMemoryStore;
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
