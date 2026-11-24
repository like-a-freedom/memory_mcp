//! Container-shaped entry points moved out of the memory context.
//!
//! The `From<&MemoryService>` impls and the `fn(&MemoryService, ..)`
//! wrappers for this module. They are adapters, so they live on the
//! side allowed to know the container; the memory context keeps the
//! port-based implementation they delegate to.

pub use crate::memory::capabilities::assemble_context::AssembleContextCapability;

use crate::error::MemoryError;
use crate::memory::retrieval_deps::AssembleContextDeps;
use crate::models::{AssembleContextRequest, AssembledContextItem};

impl AssembleContextCapability {
    /// Assembles context for a query.
    ///
    /// Orchestrates: parameter preparation → cache check → view-mode dispatch
    /// (facets / wake_up / map / default multi-tier) → experience append →
    /// cache store → query log. All logic is delegated to `context::pipeline`
    /// and `context::views`.
    pub async fn assemble_context_from_service(
        service: &crate::service::MemoryService,
        request: AssembleContextRequest,
    ) -> Result<Vec<AssembledContextItem>, MemoryError> {
        let deps = AssembleContextDeps::from(service);
        crate::memory::retrieval::assemble_context(&deps, request).await
    }

    /// Runs the bounded historical read through the same owner dependencies.
    pub async fn assemble_context_temporal_from_service(
        service: &crate::service::MemoryService,
        request: AssembleContextRequest,
        temporal: crate::models::TemporalContextQuery,
    ) -> Result<crate::models::ContextAssemblyResult, MemoryError> {
        let deps = AssembleContextDeps::from(service);
        crate::memory::retrieval::assemble_context_temporal(&deps, request, temporal).await
    }
}
