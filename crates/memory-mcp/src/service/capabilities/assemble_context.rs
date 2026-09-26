//! Capability for assembling context for a query.
//!
//! This is a thin entry point. The multi-tier retrieval pipeline lives in
//! [`crate::service::context`]. The capability takes the retrieval
//! pipeline's own dependencies, so no shared container is involved.

use crate::error::MemoryError;
use crate::models::{AssembleContextRequest, AssembledContextItem};
use crate::service::capabilities::deps::AssembleContextDeps;

/// Capability for assembling the most relevant active memory context.
pub struct AssembleContextCapability;

impl AssembleContextCapability {
    /// Assembles context for a query.
    ///
    /// Orchestrates: parameter preparation → cache check → view-mode dispatch
    /// (facets / wake_up / map / default multi-tier) → experience append →
    /// cache store → query log. All logic is delegated to `context::pipeline`
    /// and `context::views`.
    pub async fn assemble_context(
        service: &crate::service::MemoryService,
        request: AssembleContextRequest,
    ) -> Result<Vec<AssembledContextItem>, MemoryError> {
        let deps = AssembleContextDeps::from(service);
        crate::service::context::assemble_context(&deps, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::capabilities::test_support::make_service_base;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn assemble_context_returns_empty_for_empty_db() {
        let db = MockDbClient::new();
        let svc = make_service_base(db);
        let request = AssembleContextRequest {
            query: "nonexistent query".to_string(),
            fact_types: vec![],
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: crate::tools::parsers::default_compact(),
        };
        let result = AssembleContextCapability::assemble_context(&svc, request).await;
        assert!(result.is_ok(), "assemble_context must succeed on empty db");
        let items = result.unwrap();
        assert!(items.is_empty(), "empty db must return empty context");
    }
}
