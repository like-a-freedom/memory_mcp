mod embedded_support;

use chrono::{Duration, Utc};
use memory_mcp::models::{AssembleContextRequest, InvalidateRequest};

#[tokio::test]
async fn embedded_invalidate_removes_fact_from_context() -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, service) = embedded_support::setup_embedded_service().await?;
    let now = Utc::now();

    let fact_id = service
        .add_fact(
            "metric",
            "ARR is $1M",
            "ARR is $1M",
            "episode:1",
            now - Duration::days(1),
            "org",
            0.9,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:1"}),
        )
        .await?;

    let as_of_before = Utc::now() + Duration::seconds(1);

    let context_before = service
        .assemble_context(AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(as_of_before),
            budget: 5,
            access: None,
        })
        .await?;
    assert!(!context_before.is_empty());

    service
        .invalidate(
            InvalidateRequest {
                fact_id,
                reason: "Superseded".to_string(),
                t_invalid: now - Duration::seconds(1),
            },
            None,
        )
        .await?;

    let as_of_after = Utc::now() + Duration::seconds(2);
    let context_after = service
        .assemble_context(AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(as_of_after),
            budget: 5,
            access: None,
        })
        .await?;
    assert!(context_after.is_empty());

    Ok(())
}
