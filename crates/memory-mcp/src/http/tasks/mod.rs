//! Durable Tenant Task records (spec §10).
//!
//! The `tenant_task` table holds extraction tasks with
//! optimistic versioning and a fenced lease. The state
//! module owns the projection (`TaskState`,
//! `TenantTaskRecord`, `TaskHandle`) and the `TaskStore`
//! seam; the worker module owns the fenced claim and
//! commit; the scheduler module owns the retry /
//! reconcile / retention pass.

pub mod state;
pub mod worker;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn tenant_task_table_is_present_in_migration() {
        let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "migrations", "041_tenant_tasks.surql"]
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
