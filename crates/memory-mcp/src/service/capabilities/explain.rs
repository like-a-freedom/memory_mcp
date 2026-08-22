//! Capability for explaining context items with provenance citations.

use crate::error::MemoryError;
use crate::models::{AccessPayload, ExplainItem, ExplainRequest};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::capabilities::test_support::make_context_base;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn explain_returns_empty_for_empty_context_items() {
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
        let request = ExplainRequest {
            context_pack: vec![],
            compact: crate::tools::parsers::default_compact(),
        };
        let result = ExplainCapability::explain(&ctx, request, None).await;
        assert!(result.is_ok(), "explain must succeed with empty items");
        let items = result.unwrap();
        assert!(
            items.is_empty(),
            "empty context_pack must produce empty explain"
        );
    }
}
