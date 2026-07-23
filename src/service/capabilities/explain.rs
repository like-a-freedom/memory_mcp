//! Capability for explaining context items with provenance citations.

use crate::models::{AccessPayload, ExplainItem, ExplainRequest};
use crate::service::error::MemoryError;
use crate::service::service_context::ServiceContext;

/// Capability for explaining context items.
pub struct ExplainCapability;

impl ExplainCapability {
    /// Provides explanations for context items with batched graph insights.
    ///
    /// Delegates to `ExplanationService` — see `src/service/explanation.rs`
    /// for the three-phase pipeline (episode/fact resolution → shared graph
    /// insights → cached provenance assembly).
    pub async fn explain(
        ctx: &ServiceContext,
        request: ExplainRequest,
        access: Option<AccessPayload>,
    ) -> Result<Vec<ExplainItem>, MemoryError> {
        ctx.enforce_rate_limit(access.as_ref())?;
        ctx.explanation_service.explain(request, access).await
    }
}
