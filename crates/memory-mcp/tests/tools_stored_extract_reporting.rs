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
    let service =
        MemoryService::new(db.clone(), "org".into(), "info".into(), 50, 100).expect("service");
    let episode_id = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
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
            include_fact_evidence: true,
        },
    )
    .await
    .expect("stored extraction");
    assert_eq!(result.result.episode_id, episode_id);
    assert!(!result.result.facts.is_empty());
    assert!(result.result.facts.iter().all(|fact| {
        fact.statement
            .as_deref()
            .is_some_and(|statement| !statement.is_empty())
            && fact
                .source_quote
                .as_deref()
                .is_some_and(|quote| !quote.is_empty())
    }));
    let fact_id = result.result.facts[0].fact_id.clone();
    let persisted_quote = "I will finish it by Friday.";
    db.query(
        "UPDATE fact SET quote = $quote WHERE fact_id = $fact_id",
        Some(serde_json::json!({
            "fact_id": fact_id,
            "quote": persisted_quote,
        })),
        "org",
    )
    .await
    .expect("change only the persisted quote for the idempotency fixture");
    let retried = memory_mcp::tools::extract::extract(
        &service,
        ExtractParams {
            episode_id: Some(episode_id.clone()),
            content: None,
            text: None,
            source_type: None,
            source_id: None,
            t_ref: None,
            zero_shot_labels: None,
            include_fact_evidence: true,
        },
    )
    .await
    .expect("repeat extraction returns persisted evidence");
    assert_eq!(
        retried.result.facts[0].source_quote.as_deref(),
        Some(persisted_quote),
        "idempotent extraction must report the quote stored with that fact ID"
    );
    let episodes =
        EpisodeStoreClient::new(service.db_client_for_port(), service.namespace_for_port());
    assert_eq!(episodes.count_episodes().await.expect("owner count"), 1);
    let log = std::fs::read_to_string(log_path).expect("captured output");
    let start = log.find("op=extract.start").expect("start event");
    let done = log.find("op=extract.done").expect("completion event");
    assert!(start < done, "start must precede completion");
    assert!(
        !log.contains("I will finish it by Friday") && !log.contains("ARR $2M"),
        "opt-in response evidence is never written to logs"
    );
    let id = |position: usize| {
        // The whole line that contains `position`: the human format now puts
        // `op=` before `req=` (the operation is the primary tag), so reading
        // only the prefix before the match would miss the correlation id.
        let line_start = log[..position].rfind('\n').map_or(0, |index| index + 1);
        let line_end = log[position..]
            .find('\n')
            .map_or(log.len(), |index| position + index);
        log[line_start..line_end]
            .split_whitespace()
            .find(|field| field.starts_with("req="))
            .expect("correlation ID")
    };
    assert_eq!(id(start), id(done));
}
