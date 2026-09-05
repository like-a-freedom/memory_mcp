//! Durable Tenant Task records.
//!
//! The `tenant_task` table holds extraction tasks with
//! optimistic versioning and a fenced lease. The state
//! module owns the projection (`TaskState`,
//! `TenantTaskRecord`, `TaskHandle`) and the `TaskStore`
//! seam; the worker module owns the fenced claim and
//! commit; the scheduler module owns the retry /
//! reconcile / retention pass.

pub mod scheduler;
pub mod state;
pub mod worker;

/// Black-box test driver for one tenant's durable task store.
///
/// Used by the HTTP crash-recovery suite (Task 6) and the
/// durable-tasks integration suite (Task 7). The driver is the
/// only surface those tests touch; it never exposes raw
/// SurrealDB queries or the underlying store type, and the
/// store itself stays `pub(crate)` so the test contract is the
/// public one.
#[cfg(any(test, feature = "test-fixtures"))]
pub struct DurableTaskTestDriver {
    store: worker::DurableTaskStore,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl DurableTaskTestDriver {
    pub fn new(
        db: std::sync::Arc<crate::storage::client::BoundDbClient>,
        tenant_id: String,
    ) -> Self {
        Self {
            store: worker::DurableTaskStore::new(db, tenant_id),
        }
    }

    pub fn new_with_options(
        db: std::sync::Arc<crate::storage::client::BoundDbClient>,
        tenant_id: String,
        retention_secs: i64,
        queue_capacity: usize,
    ) -> Self {
        Self {
            store: worker::DurableTaskStore::new_with_options(
                db,
                tenant_id,
                retention_secs,
                queue_capacity,
            ),
        }
    }

    fn as_store(&self) -> &dyn state::TaskStore {
        &self.store
    }

    pub async fn enqueue(
        &self,
        fingerprint: &str,
        params: serde_json::Value,
    ) -> Result<String, crate::error::MemoryError> {
        self.as_store().enqueue(fingerprint, params).await
    }

    pub async fn load(
        &self,
        task_id: &str,
    ) -> Result<Option<state::TenantTaskRecord>, crate::error::MemoryError> {
        self.as_store().load(task_id).await
    }

    pub async fn set_cancellation_intent(
        &self,
        task_id: &str,
    ) -> Result<(), crate::error::MemoryError> {
        self.as_store().set_cancellation_intent(task_id).await
    }

    pub async fn claim_next_due(
        &self,
        replica_id: &str,
    ) -> Result<Option<state::TaskHandle>, crate::error::MemoryError> {
        self.as_store().claim_next_due(replica_id).await
    }

    pub async fn complete_fenced(
        &self,
        handle: &state::TaskHandle,
        result: serde_json::Value,
        completed_before_cancel: bool,
    ) -> Result<(), crate::error::MemoryError> {
        self.as_store()
            .complete_fenced(handle, result, completed_before_cancel)
            .await
    }

    pub async fn cancel_before_commit_fenced(
        &self,
        handle: &state::TaskHandle,
    ) -> Result<(), crate::error::MemoryError> {
        self.as_store().cancel_before_commit_fenced(handle).await
    }

    pub async fn fail_fenced(
        &self,
        handle: &state::TaskHandle,
        error: serde_json::Value,
    ) -> Result<(), crate::error::MemoryError> {
        self.as_store().fail_fenced(handle, error).await
    }

    pub async fn requeue_expired_running(&self) -> Result<u64, crate::error::MemoryError> {
        self.as_store().requeue_expired_running().await
    }

    pub async fn reconcile_artifacts(&self) -> Result<u64, crate::error::MemoryError> {
        self.as_store().reconcile_artifacts().await
    }

    pub async fn delete_expired(&self) -> Result<u64, crate::error::MemoryError> {
        self.as_store().delete_expired().await
    }

    pub async fn count_completed_tasks(&self) -> Result<u64, crate::error::MemoryError> {
        self.store.count_completed_tasks().await
    }

    pub async fn count_committed_artifacts(&self) -> Result<u64, crate::error::MemoryError> {
        self.store.count_committed_artifacts().await
    }

    pub async fn force_requeue_all_for_test(&self) -> Result<u64, crate::error::MemoryError> {
        self.store.force_requeue_all_for_test().await
    }

    /// Test-only helper: apply the durable task store's table
    /// schema to the bound namespace. Required before the first
    /// enqueue in a fresh namespace so the test can target a
    /// clean DB without going through the full production
    /// provisioning path.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub async fn apply_migrations_for_test(
        &self,
        namespace: &str,
    ) -> Result<(), crate::error::MemoryError> {
        crate::storage::client::DbClient::apply_migrations(&*self.store.db.db, namespace).await?;
        // The `tenant_task` table lives in the HTTP tenant
        // migrations (040–044), not the storage migrations
        // (which `DbClient::apply_migrations` runs). Apply the
        // HTTP tenant migration script that creates it so the
        // driver can enqueue without the table-missing error.
        const TENANT_TASK_DDL: &str = include_str!("../../migrations/041_tenant_tasks.surql");
        crate::storage::client::DbClient::query(
            &*self.store.db.db,
            TENANT_TASK_DDL,
            None,
            namespace,
        )
        .await
        .map_err(|err| {
            crate::error::MemoryError::Storage(format!("apply_http_migrations_for_test: {err}"))
        })?;
        Ok(())
    }

    /// Test-only helper: force a task's `retention_expiry` into
    /// the past so the cleanup sweep picks it up without
    /// requiring the test to wait the full retention window.
    /// Used by the Task 7 integration suite to exercise
    /// `delete_expired` deterministically.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub async fn force_expire_for_test(
        &self,
        task_id: &str,
        namespace: &str,
    ) -> Result<(), crate::error::MemoryError> {
        crate::storage::client::DbClient::query(
            &*self.store.db.db,
            "UPDATE type::record('tenant_task', $id) SET retention_expiry = type::datetime('1970-01-01T00:00:00Z')",
            Some(serde_json::json!({"id": task_id})),
            namespace,
        )
        .await
        .map_err(|err| crate::error::MemoryError::Storage(format!("force_expire_for_test: {err}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn tenant_task_table_is_present_in_migration() {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "migrations",
            "041_tenant_tasks.surql",
        ]
        .iter()
        .collect();
        let body = std::fs::read_to_string(&path).expect("migration file exists");
        assert!(
            body.contains("DEFINE TABLE IF NOT EXISTS tenant_task"),
            "041_tenant_tasks.surql must define the tenant_task table"
        );
        assert!(body.contains("state"), "table must carry a state column");
        assert!(
            body.contains("lease_generation"),
            "table must carry a lease_generation column for the fence"
        );
        assert!(
            body.contains("retention_expiry"),
            "table must carry a retention_expiry column for the TTL"
        );
    }
}
