//! Capability for assembling context for a query.
//!
//! This is a thin entry point. The multi-tier retrieval pipeline lives in
//! [`crate::memory::retrieval`]. The capability takes the retrieval
//! pipeline's own dependencies, so no shared container is involved.

/// Capability for assembling the most relevant active memory context.
///
/// The context-facing operation takes the retrieval pipeline's own
/// dependencies. The container-shaped entry point lives in
/// `service::memory_container_shims`, which is the side allowed to know
/// the container.
pub struct AssembleContextCapability;

#[cfg(test)]
mod tests {
    use crate::memory::capabilities::test_support::make_service_base;
    use crate::models::AssembleContextRequest;
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
        let result = crate::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(&svc, request).await;
        assert!(result.is_ok(), "assemble_context must succeed on empty db");
        let items = result.unwrap();
        assert!(items.is_empty(), "empty db must return empty context");
    }
}
