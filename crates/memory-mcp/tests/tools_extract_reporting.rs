use std::sync::Arc;

use memory_mcp::MemoryService;
use memory_mcp::memory::episode_store::EpisodeStoreClient;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use memory_mcp::tools::params::ExtractParams;
use tempfile::TempDir;

async fn make_service(name: &str) -> MemoryService {
    let namespaces = vec!["org".to_string()];
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(name, &namespaces, "warn")
            .await
            .expect("in-memory database"),
    );
    db.apply_migrations("org")
        .await
        .expect("apply memory schema");
    MemoryService::new(db, "org".to_string(), "warn".to_string(), 50, 100).expect("memory service")
}

/// Regression: free-form prose ingested as `ad-hoc` extracts no durable facts.
/// The tool must not stay silent — it must name the `episode-only` outcome and
/// point at the structured-capture path, so an agent that followed the capture
/// SOP learns why nothing was stored and how to fix it.
#[tokio::test]
async fn unstructured_prose_reports_episode_only_guidance() {
    let service = make_service("tools_extract_episode_only").await;
    let extracted = memory_mcp::tools::extract::extract(
        &service,
        ExtractParams {
            episode_id: None,
            content: Some(
                "Verified facts for the session: the release moved and we should tell the customer."
                    .to_string(),
            ),
            text: None,
            source_type: Some("ad-hoc".to_string()),
            source_id: Some("episode-only-source".to_string()),
            t_ref: Some("2026-10-06T00:00:00Z".to_string()),
            zero_shot_labels: None,
            include_fact_evidence: false,
        },
    )
    .await
    .expect("extract succeeds");

    assert_eq!(extracted.status, "success");
    assert!(
        extracted.result.facts.is_empty(),
        "prose without structure yields no durable facts"
    );
    let guidance = extracted
        .guidance
        .expect("episode-only guidance is present");
    assert!(
        guidance.contains("episode-only"),
        "guidance must name the episode-only outcome: {guidance}"
    );
    assert!(
        guidance.contains("## ") || guidance.contains("Decision:"),
        "guidance must point at the structured-capture path: {guidance}"
    );
}

/// A structured summary is the documented way to get facts from `ad-hoc`
/// content; it must still extract and keep the entity-resolution guidance.
#[tokio::test]
async fn structured_summary_returns_facts_with_entity_guidance() {
    let service = make_service("tools_extract_structured").await;
    let extracted = memory_mcp::tools::extract::extract(
        &service,
        ExtractParams {
            episode_id: None,
            content: Some(
                "## Decisions\n- Regional launch approved for September 30.\n\n## Facts\n- Beta launch scheduled for Friday."
                    .to_string(),
            ),
            text: None,
            source_type: Some("ad-hoc".to_string()),
            source_id: Some("structured-source".to_string()),
            t_ref: Some("2026-10-06T00:00:00Z".to_string()),
            zero_shot_labels: None,
            include_fact_evidence: false,
        },
    )
    .await
    .expect("extract succeeds");

    assert!(
        !extracted.result.facts.is_empty(),
        "a structured summary must yield facts"
    );
    assert!(
        extracted
            .result
            .facts
            .iter()
            .all(|fact| { fact.statement.is_none() && fact.source_quote.is_none() })
    );
    let compact = serde_json::to_value(&extracted.result).expect("serialize compact result");
    assert!(compact["facts"][0].get("statement").is_none());
    assert!(compact["facts"][0].get("source_quote").is_none());
    let guidance = extracted.guidance.expect("guidance is present");
    assert!(
        guidance.contains("Resolve canonical entities"),
        "the facts path keeps the entity-resolution guidance: {guidance}"
    );
}

#[tokio::test]
async fn inline_extract_defaults_to_evidence_without_logging_source_text() {
    let temp_dir = TempDir::new().expect("owned log directory");
    let log_path = temp_dir.path().join("extract.log");
    memory_mcp::logging::install_log_file(log_path.to_str().expect("UTF-8 log path"))
        .expect("install isolated log sink");

    let namespaces = vec!["org".to_string()];
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(
            "tools_extract_reporting",
            &namespaces,
            "warn",
        )
        .await
        .expect("in-memory database"),
    );
    db.apply_migrations("org")
        .await
        .expect("apply memory schema");
    let service = MemoryService::new(db, "org".to_string(), "info".to_string(), 50, 100)
        .expect("memory service");
    let episodes =
        EpisodeStoreClient::new(service.db_client_for_port(), service.namespace_for_port());
    assert_eq!(episodes.count_episodes().await.expect("episode count"), 0);

    let content = "I will finish it by Friday. ARR $2M";
    let params = serde_json::from_value::<ExtractParams>(serde_json::json!({
        "content": content,
        "source_type": "note",
        "source_id": "inline-reporting-source",
        "t_ref": "2026-09-30T00:00:00Z"
    }))
    .expect("legacy-shaped request defaults to evidence");
    let extracted = memory_mcp::tools::extract::extract(&service, params)
        .await
        .expect("inline content is ingested and extracted");
    assert_eq!(extracted.status, "success");
    assert!(
        !extracted.result.facts.is_empty(),
        "the tool returns extracted facts"
    );
    assert!(extracted.result.facts.iter().all(|fact| {
        fact.statement
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            && fact
                .source_quote
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    }));
    assert_eq!(extracted.result.fact_evidence_omitted, Some(false));
    let source_quote = extracted.result.facts[0]
        .source_quote
        .as_deref()
        .expect("default result contains a source quote");
    assert!(content.contains(source_quote));
    let episode_id = extracted.result.episode_id.clone();
    let stored_episode = episodes
        .select_episode(&episode_id)
        .await
        .expect("owner episode read")
        .expect("inline content was persisted");
    assert_eq!(stored_episode["content"], content);
    assert_eq!(episodes.count_episodes().await.expect("episode count"), 1);

    let first_log = std::fs::read_to_string(&log_path).expect("read captured public log output");
    let start = first_log
        .lines()
        .find(|line| line.contains("op=extract.start") && line.contains("has_content"))
        .expect("inline extraction start is logged");
    let done = first_log
        .lines()
        .find(|line| line.contains("op=extract.done"))
        .expect("inline extraction completion is logged");
    assert!(
        !done.contains(&episode_id),
        "the completion log must not reflect the episode ID"
    );
    assert!(
        !first_log.contains("I will finish it by Friday") && !first_log.contains("ARR $2M"),
        "fact evidence must not be written to the log"
    );
    assert!(
        first_log.find(start).expect("start position")
            < first_log.find(done).expect("done position"),
        "extraction must start before completion"
    );
    assert_eq!(request_id(start), request_id(done));
}

#[tokio::test]
async fn oversized_default_fact_evidence_is_omitted_as_a_complete_pair() {
    let service = make_service("tools_extract_oversized_evidence").await;
    let content = format!(
        "The verified project decision remains active for future work.\n{}",
        "The verified project decision remains active for future work.\n".repeat(180)
    );
    let params = serde_json::from_value::<ExtractParams>(serde_json::json!({
        "content": content.clone(),
        "source_type": "agent_note",
        "source_id": "oversized-evidence-source",
        "t_ref": "2026-09-30T00:00:00Z"
    }))
    .expect("legacy-shaped request defaults to bounded evidence");
    let extracted = memory_mcp::tools::extract::extract(&service, params)
        .await
        .expect("inline extraction succeeds");

    assert!(!extracted.result.facts.is_empty());
    assert_eq!(extracted.result.facts[0].statement.as_deref(), None);
    assert_eq!(extracted.result.facts[0].source_quote.as_deref(), None);
    assert_eq!(extracted.result.fact_evidence_omitted, Some(true));

    let wire = serde_json::to_vec(&extracted.result).expect("result serializes");
    assert!(wire.len() < content.len());
}

fn request_id(line: &str) -> &str {
    line.split_whitespace()
        .find_map(|field| field.strip_prefix("req="))
        .filter(|request_id| request_id.starts_with("req_"))
        .expect("log line carries a request ID")
}
