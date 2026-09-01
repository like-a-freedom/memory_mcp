//! Control-namespace migration catalog.
//!
//! The durable runner lives on `SurrealRegistryStore` so it can use the same
//! bound SurrealDB connection as the registry. This module owns only the
//! append-only catalog and the public adapter used by startup.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::registry::storage::SurrealRegistryStore;

/// Path to the registry migration directory.
pub const REGISTRY_MIGRATION_DIR: &str = "crates/memory-mcp/migrations";

/// Catalog of migration file basenames for the control namespace. Tenant-only
/// schemas (App Sessions, Tasks, outbox, and task artifacts) are applied by the
/// tenant migration runner and must not be installed in the registry.
pub const REGISTRY_MIGRATIONS: &[&str] = &[
    "001_registry",
    "045_deletion_and_usage_hardening",
    "046_registry_correctness",
];

/// Apply the registry migration catalog through the durable store. The store
/// performs checksum validation, lease-based claiming, recovery of expired
/// `applying` rows, and postcondition checks.
pub async fn apply_registry_migrations(
    store: &Arc<SurrealRegistryStore>,
) -> Result<Vec<String>, MemoryError> {
    store.apply_migrations().await
}

/// Migration ids the registry needs. The actual SQL is in the
/// migration directory referenced by `REGISTRY_MIGRATION_DIR`.
#[allow(dead_code)]
fn list_migrations() -> Vec<String> {
    REGISTRY_MIGRATIONS.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_dir_is_defined() {
        assert!(REGISTRY_MIGRATION_DIR.ends_with("migrations"));
    }

    #[test]
    fn list_migrations_contains_required_files() {
        let m = list_migrations();
        assert!(m.contains(&"001_registry".to_string()));
        assert!(!m.contains(&"044_task_artifacts".to_string()));
        assert!(m.contains(&"045_deletion_and_usage_hardening".to_string()));
        assert!(m.contains(&"046_registry_correctness".to_string()));
    }

    #[test]
    fn list_migrations_is_sorted_and_unique() {
        let m = list_migrations();
        let mut sorted = m.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(m, sorted);
    }

    #[tokio::test]
    async fn registry_migration_files_exist_on_disk() {
        // Resolve relative to the crate manifest so the test is
        // independent of the cwd used when running `cargo test`.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        assert!(dir.is_dir(), "registry migration dir missing: {dir:?}");
        for name in REGISTRY_MIGRATIONS {
            let path = dir.join(format!("{name}.surql"));
            assert!(
                path.is_file(),
                "registry migration file missing: {}",
                path.display()
            );
        }
    }
}
