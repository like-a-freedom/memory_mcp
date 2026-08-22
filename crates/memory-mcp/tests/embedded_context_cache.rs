mod common;

use chrono::{Duration, Utc};
use memory_mcp::models::{AccessPayload, AssembleContextRequest, Provenance};
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;

#[tokio::test]
async fn embedded_context_cache_returns_same_results() -> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let now = Utc::now();

    service
        .add_fact(
            "metric",
            "ARR $5M",
            "ARR $5M",
            "episode:cache",
            now - Duration::days(1),
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:cache"),
        )
        .await?;

    let request = AssembleContextRequest {
        query: "ARR".to_string(),
        as_of: Some(now),
        budget: 5,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: Some(AccessPayload {
            allowed_tags: None,
            caller_id: Some("cache-user".to_string()),
            session_vars: None,
            transport: None,
            content_type: None,
        }),
        compact: false,
    };

    let first =
        AssembleContextCapability::assemble_context(&service.build_context(), request.clone())
            .await?;
    let second =
        AssembleContextCapability::assemble_context(&service.build_context(), request).await?;

    assert_eq!(first, second);
    Ok(())
}
