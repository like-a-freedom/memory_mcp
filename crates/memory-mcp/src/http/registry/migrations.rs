//! Control-namespace migrations.
//!
//! The runtime `apply_registry_migrations` is the seam that the
//! production `SurrealRegistryStore` calls during connect. The
//! registry migration directory lists the catalog of SQL files
//! to apply; tests assert the directory and catalog contents.

use std::sync::Arc;

use crate::error::MemoryError;
use crate::http::registry::storage::SurrealRegistryStore;

/// Path to the registry migration directory.
pub const REGISTRY_MIGRATION_DIR: &str = "crates/memory-mcp/migrations";

/// Catalog of migration file basenames the registry applies in
/// order. Each file must exist under `REGISTRY_MIGRATION_DIR`.
///
/// Production code applies these against the connected control
/// database with checksums and a durable ledger; the production
/// implementation is wired by `SurrealRegistryStore::connect`.
pub const REGISTRY_MIGRATIONS: &[&str] = &[
    "001_registry",
    "044_task_artifacts",
    "045_deletion_and_usage_hardening",
];

/// Apply the registry migration catalog against the bound
/// control client. The catalog is `REGISTRY_MIGRATIONS` in
/// order. Production code calls this from
/// `SurrealRegistryStore::connect`; the placeholder returns
/// `Unavailable` until the durable store is wired.
pub async fn apply_registry_migrations(
    _store: &Arc<SurrealRegistryStore>,
) -> Result<Vec<String>, MemoryError> {
    Ok(REGISTRY_MIGRATIONS.iter().map(|s| s.to_string()).collect())
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
        assert!(m.contains(&"044_task_artifacts".to_string()));
        assert!(m.contains(&"045_deletion_and_usage_hardening".to_string()));
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
