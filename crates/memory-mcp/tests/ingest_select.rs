mod embedded_support;
use chrono::Utc;
use memory_mcp::models::IngestRequest;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;

#[tokio::test]
async fn ingest_then_extract_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let svc = embedded_support::setup_embedded_service().await?;

    let req = IngestRequest {
        source_type: "meeting".to_string(),
        source_id: "test-1".to_string(),
        content: "Meeting with Alice Inc and Bob Corp. Budget $100k".to_string(),
        t_ref: Utc::now(),
        t_ingested: None,
        policy_tags: vec![],
    };

    let episode_id = IngestCapability::ingest(&svc.build_context(), req.clone(), None).await?;
    let episode_id_2 = IngestCapability::ingest(&svc.build_context(), req, None).await?;
    assert_eq!(episode_id, episode_id_2);

    let payload = ExtractCapability::extract(&svc.build_context(), &episode_id, None, None).await?;
    assert_eq!(payload.episode_id, episode_id);
    assert!(!payload.entities.is_empty());
    assert!(!payload.facts.is_empty());

    let count = svc.episode_count().await?;
    assert!(count >= 1, "expected at least one episode in DB");

    Ok(())
}
