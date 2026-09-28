use chrono::Utc;
use memory_mcp::models::IngestRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;

mod common;

#[tokio::test]
async fn test_promise_detection_extracts_promise_fact() {
    let service = common::make_service().await;
    let req = IngestRequest {
        source_type: "email".to_string(),
        source_id: "PROMISE-1".to_string(),
        content: "I will finish the integration by next Monday.".to_string(),
        t_ref: Utc::now(),
        t_ingested: None,
        policy_tags: vec![],
    };

    let episode_id = IngestCapability::ingest_from_service(&service, req, None)
        .await
        .expect("ingest");
    let extraction = ExtractCapability::extract_from_service(&service, &episode_id, None, None)
        .await
        .expect("extract");
    let facts = extraction.facts;
    assert!(
        facts.iter().any(|f| f.fact_type == "promise"),
        "expected a promise fact"
    );
}
