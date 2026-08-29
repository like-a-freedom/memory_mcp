//! Control-namespace schema migrations (ADR-0052, plan §4.1).
//!
//! The migration SQL is added in Task 5.x alongside the rest of
//! the `SurrealRegistryStore` implementation. The function
//! signature is fixed here so the wiring lands early.

use crate::error::MemoryError;

/// Idempotent: creates the control tables if they do not exist.
/// Returns the list of applied migration ids (so the registry
/// can record them in a `migration_log` table).
pub async fn run(_client: &()) -> Result<Vec<String>, MemoryError> {
    // SQL deferred to Task 5.x. The function exists today so the
    // boot path in the composition root can wire it.
    Ok(Vec::new())
}
