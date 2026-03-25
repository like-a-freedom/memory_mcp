mod embedded_support;

use chrono::{Duration, Utc};
use memory_mcp::models::AssembleContextRequest;

/// Integration test: verifies that multi-word queries work through the full
/// SurrealDB stack (embedded) with the FTS index and per-word fallback.
#[tokio::test]
async fn embedded_multiword_fts_search() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;
    let t = Utc::now() - Duration::days(1);

    service
        .add_fact(
            "note",
            "Survey: Delta site includes enrollment workflow and gateway component on host alpha",
            "Delta Survey",
            "episode:fts_test_1",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:fts_test_1"}),
        )
        .await?;

    service
        .add_fact(
            "note",
            "Checklist entry: cert rotation scheduled, token refresh in progress, ports 5223 and 443 open",
            "cert checklist",
            "episode:fts_test_2",
            t,
            "org",
            0.85,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:fts_test_2"}),
        )
        .await?;

    let ctx = service
        .assemble_context(AssembleContextRequest {
            query: "Delta Enrollment".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await?;

    assert!(
        !ctx.is_empty(),
        "Multi-word FTS query 'Delta Enrollment' should find facts (got empty)"
    );
    let content = &ctx[0].content;
    assert!(
        content.contains("enrollment"),
        "Result content should contain 'enrollment', got: {content}"
    );

    let ctx2 = service
        .assemble_context(AssembleContextRequest {
            query: "mobile certs tokens ports episode:fts_test_2".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await?;

    assert!(
        !ctx2.is_empty(),
        "Query with episode ref should find facts after preprocessing (got empty)"
    );

    let ctx3 = service
        .assemble_context(AssembleContextRequest {
            query: "cert".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await?;

    assert!(
        !ctx3.is_empty(),
        "Single-word query 'cert' should still find facts (regression)"
    );

    Ok(())
}
