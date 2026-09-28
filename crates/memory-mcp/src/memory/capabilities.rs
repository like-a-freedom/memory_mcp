//! Capability assembly for the memory context.
//!
//! Each capability here is a thin operation over a set of ports the
//! capability itself declares. The container-shaped entry points that read
//! the legacy `MemoryService` live in
//! `service::memory_container_shims`, so the container is contained at the
//! transport edge rather than reached from inside a context.

pub mod assemble_context;
pub mod explain;
pub mod extract;
pub mod ingest;
pub mod invalidate;
pub mod resolve;

/// The per-capability dependency structs, so a capability declares
/// what it uses instead of receiving the shared container.
pub(crate) mod deps;

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared test helpers for capability unit tests.
    use std::sync::Arc;

    use crate::service::MemoryService;
    use crate::service::mock_db::MockDbClient;

    /// Builds a service wired to a `MockDbClient`, suitable for
    /// capability unit tests.
    ///
    /// This is the same builder production uses, so a test exercises
    /// the real composition rather than a hand-assembled stand-in.
    pub(crate) fn make_service_base(db: MockDbClient) -> MemoryService {
        make_service_with_rate_limit(db, 100, 100)
    }

    /// A service whose token bucket is `rps`/`burst`, for the
    /// rate-limit tests that need a bucket they can exhaust.
    pub(crate) fn make_service_with_rate_limit(
        db: MockDbClient,
        rps: i32,
        burst: i32,
    ) -> MemoryService {
        let db_client: Arc<dyn crate::storage::DbClient> = Arc::new(db);
        MemoryService::new(db_client, "org".to_string(), "warn".to_string(), rps, burst)
            .expect("test service builds from a mock db")
    }
}
