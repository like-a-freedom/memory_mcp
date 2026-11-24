use std::sync::Arc;

use memory_mcp::MemoryService;
use memory_mcp::models::MAX_EXTRACT_FACT_EVIDENCE_WIRE_BYTES;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use memory_mcp::tools::params::ExtractParams;

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

fn params(content: String, include_fact_evidence: bool) -> ExtractParams {
    ExtractParams {
        episode_id: None,
        content: Some(content),
        text: None,
        source_type: Some("agent_note".to_string()),
        source_id: Some("eval-extract-evidence-contract".to_string()),
        t_ref: Some("2026-09-30T00:00:00Z".to_string()),
        zero_shot_labels: None,
        include_fact_evidence,
    }
}

#[tokio::test]
async fn requested_fact_evidence_has_a_deterministic_wire_size_ceiling() {
    let service = make_service("eval_extract_evidence_within_budget").await;
    let content = "## Decisions\n- Regional launch approved for September 30.\n\n## Facts\n- Beta launch scheduled for Friday.".to_string();
    let with_evidence =
        memory_mcp::tools::extract::extract(&service, params(content.clone(), true))
            .await
            .expect("evidence extraction succeeds");
    let compact = memory_mcp::tools::extract::extract(&service, params(content, false))
        .await
        .expect("compact extraction succeeds");

    assert!(!with_evidence.result.facts.is_empty());
    assert_eq!(with_evidence.result.fact_evidence_omitted, Some(false));
    let evidence_wire_bytes = serde_json::to_vec(&with_evidence)
        .expect("evidence response serializes")
        .len();
    let compact_wire_bytes = serde_json::to_vec(&compact)
        .expect("compact response serializes")
        .len();
    assert!(
        evidence_wire_bytes.saturating_sub(compact_wire_bytes)
            <= MAX_EXTRACT_FACT_EVIDENCE_WIRE_BYTES,
        "evidence added {} bytes, above the {} byte contract",
        evidence_wire_bytes.saturating_sub(compact_wire_bytes),
        MAX_EXTRACT_FACT_EVIDENCE_WIRE_BYTES
    );
}

#[tokio::test]
async fn oversized_fact_evidence_is_omitted_without_returning_a_partial_pair() {
    let service = make_service("eval_extract_evidence_over_budget").await;
    let content = format!(
        "The verified project decision remains active for future work.\n{}",
        "The verified project decision remains active for future work.\n".repeat(180)
    );
    let response = memory_mcp::tools::extract::extract(&service, params(content, true))
        .await
        .expect("oversized source extraction succeeds");

    assert!(!response.result.facts.is_empty());
    assert_eq!(response.result.fact_evidence_omitted, Some(true));
    assert!(
        response
            .result
            .facts
            .iter()
            .all(|fact| fact.statement.is_none() && fact.source_quote.is_none())
    );
}
