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
        crate::memory::api::explain_context(
            &ExplanationPort {
                service: &ctx.explanation_service,
                access: access.clone(),
            },
            &super::ServiceRateLimitPort { ctx },
            request,
            access.and_then(|payload| payload.caller_id),
        )
        .await
    }
}

/// Adapts the legacy `ExplanationService` to the memory-owned
/// explanation port.
///
/// Expiry removal: Phase 5, when the provenance pipeline becomes
/// memory-owned rather than a context-held service.
struct ExplanationPort<'a> {
    service: &'a crate::service::explanation::ExplanationService,
    /// Caller identity, recorded on each explained item for
    /// provenance auditing.
    access: Option<AccessPayload>,
}

#[async_trait::async_trait]
impl crate::memory::api::ExplanationPort for ExplanationPort<'_> {
    async fn explain(
        &self,
        request: crate::models::ExplainRequest,
    ) -> Result<Vec<crate::models::ExplainItem>, MemoryError> {
        self.service.explain(request, self.access.clone()).await
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
