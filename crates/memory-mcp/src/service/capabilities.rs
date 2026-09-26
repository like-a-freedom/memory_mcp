pub mod invalidate;

pub mod assemble_context;
pub mod explain;
pub mod extract;
pub mod ingest;
pub mod resolve;

/// Implements the transport-facing tool port for the legacy
/// service container. This is the single adapter between
/// `ServiceContext` and the tool handlers.
mod tool_context_impl;

/// The per-capability dependency structs, so a capability declares
/// what it uses instead of receiving the shared container.
pub(crate) mod deps;

use std::sync::Arc;

use crate::error::MemoryError;
use crate::service::util::RateLimiter;

/// The rate-limit policy a capability charges, as a dependency
/// rather than a field reached through the shared container.
///
/// Capabilities reach the limiter only through this adapter, so the
/// access policy is charged in exactly one place.
pub(crate) struct RateLimitDeps<'a> {
    pub(crate) rate_limiter: &'a Arc<RateLimiter>,
}

impl crate::memory::api::RateLimitPort for RateLimitDeps<'_> {
    fn check(&self, caller: Option<&str>) -> Result<(), MemoryError> {
        let access = caller.map(|caller_id| crate::models::AccessPayload {
            caller_id: Some(caller_id.to_owned()),
            ..Default::default()
        });
        // `check_access` is the single enforcement point, exactly as
        // the container's `enforce_rate_limit` was, so the bucket is
        // charged once and a `None` caller still costs one call.
        self.rate_limiter.check_access(access.as_ref())
    }
}

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
