//! Durable HTTP App Sessions (plan §7).
//!
//! App Sessions are short-lived interactive state opened
//! by an MCP client during a session. The HTTP SaaS
//! profile persists them in the tenant's SurrealDB
//! namespace; the stdio profile keeps the existing
//! in-process store. Both branches are wired through
//! `mcp::handlers::AppSessionBackend`.
//!
//! The store module owns the data path (open / command /
//! close with optimistic versioning and TTL expiry). The
//! scheduler module owns the process-level cleanup pass
//! that physically deletes expired rows.

pub mod scheduler;
pub mod store;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// The `040_app_sessions.surql` migration is loaded
    /// by the registry migration runner (Task 5.x). The
    /// loader's test asserts the file's existence; this
    /// test pins the schema for Phase 7 by reading the
    /// file and asserting the table is defined.
    #[test]
    fn app_session_table_is_present_in_migration() {
        let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "migrations", "040_app_sessions.surql"]
            .iter()
            .collect();
        let body = std::fs::read_to_string(&path).expect("migration file exists");
        assert!(
            body.contains("DEFINE TABLE IF NOT EXISTS app_session"),
            "040_app_sessions.surql must define the app_session table"
        );
        assert!(body.contains("handle"), "table must carry a handle column");
        assert!(
            body.contains("idle_expiry"),
            "table must carry an idle_expiry column for the TTL"
        );
        assert!(
            body.contains("absolute_expiry"),
            "table must carry an absolute_expiry column for the TTL"
        );
    }
}
