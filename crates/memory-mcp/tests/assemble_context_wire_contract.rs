mod common;

use chrono::Utc;
use memory_mcp::tools::assemble_context::assemble_context;
use memory_mcp::tools::params::AssembleContextParams;

const EMPTY_GUIDANCE: &str =
    "No sufficient matching context was found. Refine the query or verify primary sources.";

fn params(query: &str, compact: bool) -> AssembleContextParams {
    AssembleContextParams {
        query: query.to_string(),
        fact_types: Vec::new(),
        as_of: String::new(),
        budget: 5,
        view_mode: None,
        window_start: None,
        window_end: None,
        compact,
    }
}

#[tokio::test]
async fn empty_context_returns_truthful_next_step_guidance() {
    let memory = common::TestMemory::new(false).await;

    let response = assemble_context(&memory.service, params("unmatched project detail", true))
        .await
        .expect("an empty retrieval is a successful result");

    assert!(response.result.as_array().is_some_and(Vec::is_empty));
    assert_eq!(response.guidance.as_deref(), Some(EMPTY_GUIDANCE));
}

#[tokio::test]
async fn compact_and_verbose_tool_json_preserve_ids_and_verbose_evidence() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = common::seed_fact_at(
        &memory.service,
        "wire-contract",
        "The Rust release uses cargo nextest for focused unit testing.",
        Utc::now(),
    )
    .await;
    let query = "Rust release cargo nextest focused unit testing";

    let compact = assemble_context(&memory.service, params(query, true))
        .await
        .expect("compact tool response");
    let verbose = assemble_context(&memory.service, params(query, false))
        .await
        .expect("verbose tool response");

    let compact_items = compact.result.as_array().expect("compact item array");
    let verbose_items = verbose.result.as_array().expect("verbose item array");
    let ids = |items: &[serde_json::Value]| {
        items
            .iter()
            .map(|item| item["fact_id"].as_str().expect("fact id").to_string())
            .collect::<Vec<_>>()
    };

    assert_eq!(ids(compact_items), ids(verbose_items));
    assert!(ids(verbose_items).contains(&fact_id));
    for item in verbose_items {
        let rationale = item["rationale"].as_str().expect("verbose rationale");
        let labeled_relevance = rationale
            .split_whitespace()
            .find_map(|part| part.strip_prefix("relevance="))
            .expect("rationale exposes a relevance score");
        let response_relevance = item["relevance"].as_f64().expect("response relevance");
        assert_eq!(
            labeled_relevance,
            format!("{response_relevance:.2}"),
            "the rationale must report the score returned in the final item"
        );
    }
    assert!(
        compact_items
            .iter()
            .all(|item| item.get("retrieval_evidence").is_none())
    );
    assert!(verbose_items.iter().any(|item| {
        item.get("retrieval_evidence")
            .is_some_and(serde_json::Value::is_object)
    }));
}
