//! Capability for assembling context for a query.
//!
//! This is a thin entry point. The multi-tier retrieval pipeline lives in
//! [`super::super::context`]. After the capability-seam migration, the
//! pipeline reads from `&ServiceContext` exclusively.

use crate::error::MemoryError;
use crate::models::{AssembleContextRequest, AssembledContextItem};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::capabilities::test_support::make_context_base;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn assemble_context_returns_empty_for_empty_db() {
        let db = MockDbClient::new();
        let ctx = make_context_base(db);
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
        let result = AssembleContextCapability::assemble_context(&ctx, request).await;
        assert!(result.is_ok(), "assemble_context must succeed on empty db");
        let items = result.unwrap();
        assert!(items.is_empty(), "empty db must return empty context");
    }
}
