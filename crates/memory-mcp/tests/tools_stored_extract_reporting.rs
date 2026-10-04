//! Stored extraction integration; its own process isolates the global log sink.
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use memory_mcp::MemoryService;
use memory_mcp::memory::episode_store::EpisodeStoreClient;
use memory_mcp::models::IngestRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use memory_mcp::tools::params::ExtractParams;

#[tokio::test]
async fn stored_extraction_logs_its_lifecycle_without_ingesting_again() {
    let temp = tempfile::tempdir().expect("owned log directory");
    let log_path = temp.path().join("stored-extract.log");
    memory_mcp::logging::install_log_file(log_path.to_str().expect("UTF-8 path"))
        .expect("install process-local log sink");
    let db = Arc::new(
        SurrealDbClient::connect_in_memory("stored_extract_reporting", "org", "warn")
            .await
            .expect("connect"),
    );
    db.apply_migrations("org").await.expect("migrations");
    let service = MemoryService::new(db, "org".into(), "info".into(), 50, 100).expect("service");
    let episode_id = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_type: "note".into(),
            source_id: "stored-reporting".into(),
            content: "I will finish it by Friday. ARR $2M".into(),
            t_ref: Utc.with_ymd_and_hms(2026, 9, 30, 0, 0, 0).unwrap(),
            t_ingested: Some(Utc.with_ymd_and_hms(2026, 9, 30, 0, 1, 0).unwrap()),
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("seed through owning ingestion");
    let result = memory_mcp::tools::extract::extract(
        &service,
        ExtractParams {
            episode_id: Some(episode_id.clone()),
            content: None,
            text: None,
            source_type: None,
            source_id: None,
            t_ref: None,
            zero_shot_labels: None,
        },
    )
    .await
    .expect("stored extraction");
    assert_eq!(result.result.episode_id, episode_id);
    let episodes =
        EpisodeStoreClient::new(service.db_client_for_port(), service.namespace_for_port());
    assert_eq!(episodes.count_episodes().await.expect("owner count"), 1);
    let log = std::fs::read_to_string(log_path).expect("captured output");
    let start = log.find("op=extract.start").expect("start event");
    let done = log.find("op=extract.done").expect("completion event");
    assert!(start < done, "start must precede completion");
    let id = |position: usize| {
        log[..position]
            .lines()
            .last()
            .expect("event prefix")
            .split_whitespace()
            .find(|field| field.starts_with("req="))
            .expect("correlation ID")
    };
    assert_eq!(id(start), id(done));
}
