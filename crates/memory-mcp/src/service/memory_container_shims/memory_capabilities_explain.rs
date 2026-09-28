//! Container-shaped entry points moved out of the memory context.
//!
//! The `From<&MemoryService>` impls and the `fn(&MemoryService, ..)`
//! wrappers for this module. They are adapters, so they live on the
//! side allowed to know the container; the memory context keeps the
//! port-based implementation they delegate to.

pub use crate::memory::capabilities::explain::ExplainCapability;
use crate::memory::capabilities::explain::ExplanationPort;

use crate::error::MemoryError;
use crate::memory::capabilities::deps::ExplainDeps;
use crate::models::{AccessPayload, ExplainItem, ExplainRequest};

impl ExplainCapability {
    /// Provides explanations for context items with batched graph insights.
    ///
    /// Delegates to `ExplanationService` — see `src/service/explanation.rs`
    /// for the three-phase pipeline (episode/fact resolution → shared graph
    /// insights → cached provenance assembly).
    pub async fn explain_from_service(
        service: &crate::service::MemoryService,
        request: ExplainRequest,
        access: Option<AccessPayload>,
    ) -> Result<Vec<ExplainItem>, MemoryError> {
        let deps = ExplainDeps::from(service);
        crate::memory::api::explain_context(
            &ExplanationPort {
                service: &deps.explanation_service,
                access: access.clone(),
            },
            &crate::memory::retrieval_deps::RateLimitDeps {
                rate_limiter: &deps.rate_limiter,
            },
            request,
            access.and_then(|payload| payload.caller_id),
        )
        .await
    }
}
