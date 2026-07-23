//! Capability for assembling context for a query.
//!
//! This is a thin entry point. The multi-tier retrieval pipeline lives in
//! [`super::super::context`]. After the capability-seam migration, the
//! pipeline reads from `&ServiceContext` exclusively.

use crate::models::{AssembleContextRequest, AssembledContextItem};
use crate::service::error::MemoryError;
use crate::service::service_context::ServiceContext;

/// Capability for assembling the most relevant active memory context.
pub struct AssembleContextCapability;

impl AssembleContextCapability {
    /// Assembles context for a query.
    ///
    /// Orchestrates: parameter preparation → cache check → view-mode dispatch
    /// (facets / wake_up / map / default multi-tier) → experience append →
    /// cache store → query log. All logic is delegated to `context::pipeline`
    /// and `context::views`, which read from `&ServiceContext`.
    pub async fn assemble_context(
        ctx: &ServiceContext,
        request: AssembleContextRequest,
    ) -> Result<Vec<AssembledContextItem>, MemoryError> {
        crate::service::context::assemble_context(ctx, request).await
    }
}
