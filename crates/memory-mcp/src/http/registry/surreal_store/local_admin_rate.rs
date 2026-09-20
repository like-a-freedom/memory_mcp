//! Bounded maintenance for the durable local-admin rate buckets.
//!
//! The bucket *reservation* logic (window arithmetic, saturation, the
//! denied aggregate, and credential/challenge domain separation) lives in
//! `super::local_admin::reserve_attempt`, which owns the atomic
//! transaction. This module implements the remaining obligation from the
//! approved design: expired counters are reclaimed by a **bounded**
//! delete so a long-running deployment cannot accumulate rows without
//! limit, and the delete is driven by database time rather than a
//! replica clock.
//!
//! `local_admin_rate_bucket` is `SCHEMAFULL`; the only field this module
//! touches is `expires_at`, which every bucket row sets.

use serde_json::json;
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::MemoryError;
use crate::http::registry::RegistryHandle;
use crate::service::local_admin::contracts::{LocalAdminError, LocalResult};

use super::SurrealRegistryStore;

/// Maximum rows a single cleanup pass may remove. Keeps the maintenance
/// statement bounded so it cannot hold a long write transaction.
pub const RATE_BUCKET_CLEANUP_BATCH: u64 = 512;

/// Minimum spacing between cleanup passes in one process. Expiry is enforced
/// by database time on every reservation, so this delay cannot admit an
/// extra attempt; it only bounds how often the maintenance pass runs.
const RATE_BUCKET_CLEANUP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// Convert a `MemoryError` to `LocalAdminError::Infrastructure`.
fn infra(e: MemoryError) -> LocalAdminError {
    LocalAdminError::Infrastructure(e)
}

impl SurrealRegistryStore {
    /// Delete at most [`RATE_BUCKET_CLEANUP_BATCH`] rate buckets whose
    /// `expires_at` has passed according to database time.
    ///
    /// Returns the number of rows removed. Callers that want to fully
    /// drain the table call this repeatedly until it returns zero, which
    /// bounds each individual transaction while still converging.
    ///
    /// The [`crate::service::local_admin::contracts::LocalAdminStore`] trait
    /// exposes this as `cleanup_rate_buckets` so the maintenance scheduler
    /// can reach it through the store handle.
    pub async fn cleanup_expired_rate_buckets(&self) -> LocalResult<u64> {
        // `expires_at` is a real field on the schemafull table, and
        // `time::now()` is database time, so a replica with a skewed
        // clock cannot delete a bucket that is still inside its window.
        //
        // Statement order is LET, FOR, RETURN, so the result index is 2.
        // A transaction cannot be used here: a `LET` bound inside a
        // transaction is gone once it commits, so the count would be
        // unavailable at `RETURN`. The batch `LIMIT` is what keeps the
        // write bounded instead.
        let sql = "
            LET $doomed = (
                SELECT id FROM local_admin_rate_bucket
                WHERE expires_at < time::now()
                LIMIT $batch
            );
            FOR $row IN $doomed {
                DELETE $row.id;
            };
            RETURN array::len($doomed);
        ";

        let result = self
            .admin_query_at(sql, Some(json!({ "batch": RATE_BUCKET_CLEANUP_BATCH })), 2)
            .await
            .map_err(infra)?;

        // The `RETURN` yields a single number; anything else means the
        // statement shape changed and the count below would be a lie.
        let removed = result
            .last()
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                LocalAdminError::Infrastructure(MemoryError::Storage(
                    "rate bucket cleanup returned no count".into(),
                ))
            })?;

        Ok(removed)
    }
}

/// Scheduler job: reclaim expired throttle buckets.
///
/// The row count is bounded by construction (buckets are keyed into 4096
/// slots per dimension), so this is hygiene rather than a correctness
/// requirement: every reservation already resets an expired window in place,
/// and expiry is compared against database time. The job does nothing when
/// the deployment has no durable local-admin store (data-plane-only builds).
pub fn rate_bucket_cleanup_scheduler_job() -> crate::http::leases::scheduler::SchedulerJob {
    Arc::new(|registry| Box::pin(cleanup_expired(registry)))
}

async fn cleanup_expired(registry: RegistryHandle) -> Result<(), MemoryError> {
    if !due_now() {
        return Ok(());
    }
    cleanup_expired_now(registry).await
}

/// Whether this process should run a pass now. Keeps a multi-replica
/// deployment from issuing the maintenance statement on every scheduler
/// tick; correctness never depends on it.
fn due_now() -> bool {
    static LAST_RUN: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    let mut last = LAST_RUN
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if last.is_some_and(|instant| instant.elapsed() < RATE_BUCKET_CLEANUP_INTERVAL) {
        return false;
    }
    *last = Some(std::time::Instant::now());
    true
}

/// One bounded pass, without the interval gate.
async fn cleanup_expired_now(registry: RegistryHandle) -> Result<(), MemoryError> {
    let Some(store) = registry.local_admin_store_clone() else {
        return Ok(());
    };
    store
        .cleanup_rate_buckets()
        .await
        .map(|_removed| ())
        .map_err(|error| MemoryError::Storage(format!("rate bucket cleanup failed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn store() -> SurrealRegistryStore {
        let namespace = format!("rate_bucket_{}", uuid::Uuid::new_v4().simple());
        SurrealRegistryStore::connect_in_memory(&namespace, "registry")
            .await
            .expect("migrated in-memory registry")
    }

    /// A registry handle carrying both trait surfaces for the same durable
    /// store, exactly as the production composition builds it.
    fn handle(store: Arc<SurrealRegistryStore>) -> crate::http::registry::RegistryHandle {
        let registry: Arc<dyn crate::http::registry::RegistryStore> = store.clone();
        crate::http::registry::RegistryHandle::from_store(registry).with_local_admin_store(store)
    }

    async fn insert_bucket(store: &SurrealRegistryStore, id: &str, expires_offset_secs: i64) {
        // Bind an absolute timestamp rather than a SurrealDB duration
        // literal: `type::duration` rejects a negative duration, and a
        // bucket that already expired is exactly what this test needs.
        let expires_at =
            (chrono::Utc::now() + chrono::Duration::seconds(expires_offset_secs)).to_rfc3339();
        let sql = "
            CREATE type::record('local_admin_rate_bucket', $id) SET
                bucket_id = $id,
                source_bucket = 1,
                username_bucket = NONE,
                action = 'credentials',
                reason = NONE,
                denied_count = 0,
                window_start = time::now(),
                expires_at = type::datetime($expires_at);
        ";
        store
            .handle()
            .query_json(sql, Some(json!({ "id": id, "expires_at": expires_at })))
            .await
            .expect("insert bucket");
    }

    async fn count_buckets(store: &SurrealRegistryStore) -> u64 {
        let result = store
            .handle()
            .query_json("RETURN count(SELECT * FROM local_admin_rate_bucket);", None)
            .await
            .expect("count buckets");
        result
            .last()
            .and_then(|value| value.as_u64())
            .expect("count is a number")
    }

    #[tokio::test]
    async fn cleanup_removes_only_expired_buckets() {
        let store = store().await;
        insert_bucket(&store, "expired_a", -120).await;
        insert_bucket(&store, "expired_b", -1).await;
        insert_bucket(&store, "live", 600).await;
        assert_eq!(count_buckets(&store).await, 3);

        let removed = store.cleanup_expired_rate_buckets().await.expect("cleanup");
        assert_eq!(removed, 2, "both expired buckets are reclaimed");
        assert_eq!(count_buckets(&store).await, 1, "the live bucket survives");
    }

    #[tokio::test]
    async fn cleanup_on_an_empty_table_is_a_no_op() {
        let store = store().await;
        assert_eq!(
            store.cleanup_expired_rate_buckets().await.expect("cleanup"),
            0
        );
    }

    #[tokio::test]
    async fn scheduler_job_reclaims_expired_buckets_through_the_store_handle() {
        let store = Arc::new(store().await);
        insert_bucket(&store, "expired", -60).await;
        insert_bucket(&store, "live", 600).await;

        cleanup_expired_now(handle(store.clone()))
            .await
            .expect("cleanup pass");
        assert_eq!(
            count_buckets(&store).await,
            1,
            "the maintenance pass reaches the durable bucket table"
        );
    }

    #[tokio::test]
    async fn scheduler_job_is_a_no_op_without_a_local_admin_store() {
        let store = Arc::new(store().await);
        let registry: Arc<dyn crate::http::registry::RegistryStore> = store.clone();
        let handle = crate::http::registry::RegistryHandle::from_store(registry);
        cleanup_expired_now(handle)
            .await
            .expect("a data-plane-only handle must not fail the job");
        assert_eq!(count_buckets(&store).await, 0);
    }
}
