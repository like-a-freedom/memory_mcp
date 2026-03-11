mod embedded_support;
use chrono::Utc;
use memory_mcp::models::IngestRequest;

#[tokio::test]
async fn ingest_then_extract_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let svc = embedded_support::setup_embedded_service().await?;

    let req = IngestRequest {
        source_type: "meeting".to_string(),
        source_id: "test-1".to_string(),
        content: "Meeting with Alice Inc and Bob Corp. Budget $100k".to_string(),
        t_ref: Utc::now(),
        scope: "org".to_string(),
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };

    let episode_id = svc.ingest(req.clone(), None).await?;
    // ingest again should be idempotent (same id)
    let episode_id_2 = svc.ingest(req, None).await?;
    assert_eq!(episode_id, episode_id_2);

    // Basic extract should work after persistence
    let payload = svc.extract(&episode_id, None).await?;
    assert_eq!(payload.episode_id, episode_id);
    assert!(!payload.entities.is_empty());
    assert!(!payload.facts.is_empty());

    // Count should reflect at least one episode
    let count = svc.episode_count().await?;
    assert!(count >= 1, "expected at least one episode in DB");

    // Idempotency: ingesting same source twice returns same episode id
    Ok(())
}
