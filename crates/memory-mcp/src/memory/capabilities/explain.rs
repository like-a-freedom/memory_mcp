//! Capability for explaining context items with provenance citations.

use crate::error::MemoryError;
use crate::models::AccessPayload;

/// Capability for explaining context items.
///
/// The container-shaped entry point lives in
/// `service::memory_container_shims`, the side allowed to know the
/// container.
pub struct ExplainCapability;

/// Adapts the legacy `ExplanationService` to the memory-owned
/// explanation port.
///
/// Expiry removal: Phase 5, when the provenance pipeline becomes
/// memory-owned rather than a context-held service.
pub(crate) struct ExplanationPort<'a> {
    pub(crate) service: &'a crate::memory::explanation::ExplanationService,
    /// Caller identity, recorded on each explained item for
    /// provenance auditing.
    pub(crate) access: Option<AccessPayload>,
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
    use crate::memory::capabilities::test_support::make_service_base;
    use crate::models::ExplainRequest;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn explain_returns_empty_for_empty_context_items() {
        let db = MockDbClient::new();
        let svc = make_service_base(db);
        let request = ExplainRequest {
            context_pack: vec![],
            compact: crate::tools::parsers::default_compact(),
        };
        let result = crate::service::memory_container_shims::memory_capabilities_explain::ExplainCapability::explain_from_service(&svc, request, None).await;
        assert!(result.is_ok(), "explain must succeed with empty items");
        let items = result.unwrap();
        assert!(
            items.is_empty(),
            "empty context_pack must produce empty explain"
        );
    }
}
