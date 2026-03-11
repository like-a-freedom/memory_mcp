mod embedded_support;

use chrono::{Duration, Utc};
use memory_mcp::models::{AccessPayload, AssembleContextRequest};

#[tokio::test]
async fn embedded_context_cache_returns_same_results() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;
    let now = Utc::now();

    service
        .add_fact(
            "metric",
            "ARR $5M",
            "ARR $5M",
            "episode:cache",
            now - Duration::days(1),
            "org",
            0.8,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:cache"}),
        )
        .await?;

    let request = AssembleContextRequest {
        query: "ARR".to_string(),
        scope: "org".to_string(),
        as_of: Some(now),
        budget: 5,
        access: Some(AccessPayload {
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            caller_id: Some("cache-user".to_string()),
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        }),
    };

    let first = service.assemble_context(request.clone()).await?;
    let second = service.assemble_context(request).await?;

    assert_eq!(first, second);
    Ok(())
}
