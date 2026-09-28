//! Container-shaped entry points moved out of the memory context.
//!
//! The `From<&MemoryService>` impls and the `fn(&MemoryService, ..)`
//! wrappers for this module. They are adapters, so they live on the
//! side allowed to know the container; the memory context keeps the
//! port-based implementation they delegate to.

pub use crate::memory::capabilities::invalidate::InvalidateCapability;
use crate::memory::capabilities::invalidate::InvalidationPort;

use crate::error::MemoryError;
use crate::memory::capabilities::deps::InvalidateDeps;
use crate::models::{AccessPayload, InvalidateRequest};

impl InvalidateCapability {
    /// Invalidates a fact by closing both bi-temporal fields.
    ///
    /// The fact is looked up to verify existence, then closed through the
    /// storage close owner: `t_invalid` takes the caller-supplied valid time,
    /// `t_invalid_ingested` defaults to server-side now, and `request.reason`
    /// is persisted to `invalidation_reason`. Derived claims are closed when
    /// the claim pipeline is wired.
    pub async fn invalidate_from_service(
        service: &crate::service::MemoryService,
        request: InvalidateRequest,
        access: Option<AccessPayload>,
    ) -> Result<(), MemoryError> {
        let deps = InvalidateDeps::from(service);
        crate::memory::api::invalidate_fact(
            &InvalidationPort { deps: &deps },
            &crate::memory::retrieval_deps::RateLimitDeps {
                rate_limiter: &deps.rate_limiter,
            },
            &crate::memory::api::InvalidationRequest {
                fact_id: request.fact_id,
                t_invalid: request.t_invalid,
                reason: request.reason,
                caller_id: access.and_then(|payload| payload.caller_id),
            },
        )
        .await
    }
}
