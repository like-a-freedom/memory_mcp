use eval_harness::test_support::{TestMemory, ingest_probe};
use memory_mcp::service::MemoryService;
use memory_mcp::tools::{assemble_context, params::AssembleContextParams};
use serde_json::{Value, json};

async fn read_evidence(service: &MemoryService, query: &str, compact: bool) -> Value {
    assemble_context(
        service,
        serde_json::from_value::<AssembleContextParams>(json!({
            "query": query,
            "as_of": "2099-01-01T00:00:00Z",
            "budget": 1,
            "compact": compact
        }))
        .unwrap(),
    )
    .await
    .unwrap()
    .result
}

#[tokio::test]
async fn oversized_episode_returns_a_bounded_verbatim_witness_late_in_the_source() {
    let memory = TestMemory::new().await;
    let witness = "The Zephyrgate unlock phrase is copper crescent.";
    let source = format!("{}{witness}", "Unrelated archive material 🗂.\n".repeat(950));
    let episode = ingest_probe(&memory.service, "zephyrgate-manual", &source).await;
    let evidence = read_evidence(&memory.service, "Zephyrgate unlock phrase", false).await;

    let item = &evidence[0];
    assert_eq!(item["source_episode"], episode);
    let content = item["content"].as_str().unwrap();
    assert!(content.contains(witness));
    assert!(source.contains(content));
    assert_eq!(item["quote"], content);
    assert!(content.len() <= 2048, "returned {} bytes", content.len());
    let excerpt = &item["provenance"]["episode_excerpt"];
    let start = excerpt["start_byte"].as_u64().unwrap() as usize;
    let end = excerpt["end_byte"].as_u64().unwrap() as usize;
    assert_eq!(&source[start..end], content);
    assert_eq!(excerpt["source_bytes"], source.len());
}

#[tokio::test]
async fn independent_readers_return_identical_evidence_from_one_store() {
    let memory = TestMemory::new().await;
    let witness = "The Oakbridge restoration code is amber oak.";
    let source = format!("{}{witness}", "Unrelated catalogue entries.\n".repeat(300));
    let episode = ingest_probe(&memory.service, "oakbridge-manual", &source).await;
    let second_reader = MemoryService::new(
        memory.db_client.clone(),
        eval_harness::test_support::ACTIVE_NAMESPACE.to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .unwrap()
    .with_claim_rollout_stage("evidence")
    .unwrap();
    let first = read_evidence(&memory.service, "Oakbridge restoration code", true).await;
    let second = read_evidence(&second_reader, "Oakbridge restoration code", true).await;

    assert_eq!(first[0]["source_episode"], episode);
    assert!(first[0]["content"].as_str().unwrap().contains(witness));
    assert_eq!(first, second);
    assert!(serde_json::to_vec(&first).unwrap().len() < 4096);
}

#[tokio::test]
async fn excerpt_keeps_the_answer_after_query_terms_at_a_window_edge() {
    let memory = TestMemory::new().await;
    let witness = "The Lanternbay locker phrase is silver birch.";
    let prefix = "Unrelated catalogue entries.\n".repeat(67);
    let source = format!(
        "{prefix}{}{witness}\n{prefix}",
        " ".repeat(2019 - prefix.len())
    );
    ingest_probe(&memory.service, "lanternbay-manual", &source).await;
    let evidence = read_evidence(&memory.service, "Lanternbay locker phrase", true).await;

    assert!(evidence[0]["content"].as_str().unwrap().contains(witness));
}

#[tokio::test]
async fn short_episode_preserves_its_full_content_and_quote() {
    let memory = TestMemory::new().await;
    let source = "The Millbridge door code is blue ash. It applies to the front door.";
    let episode = ingest_probe(&memory.service, "millbridge-manual", source).await;
    let evidence = read_evidence(&memory.service, "Millbridge door code", false).await;

    let item = &evidence[0];
    assert_eq!(item["source_episode"], episode);
    assert_eq!(item["content"], source);
    assert_eq!(item["quote"], source);
    assert!(item["provenance"].get("episode_excerpt").is_none());
}

#[tokio::test]
async fn oversized_extracted_fact_returns_bounded_content_and_source_quote() {
    let memory = TestMemory::new().await;
    let witness = "The Fernbridge passphrase is violet cedar.";
    let source = format!(
        "# Operations summary\n\nFact: {}{witness}",
        "archived catalogue entry; ".repeat(300)
    );
    let episode = ingest_probe(&memory.service, "fernbridge-summary", &source).await;
    memory_mcp::tools::extract(
        &memory.service,
        serde_json::from_value(json!({"episode_id": episode})).unwrap(),
    )
    .await
    .unwrap();

    let evidence = read_evidence(&memory.service, "Fernbridge passphrase", false).await;
    let item = &evidence[0];
    assert!(item["fact_id"].as_str().unwrap().starts_with("fact:"));
    assert_eq!(item["source_episode"], episode);
    let content = item["content"].as_str().unwrap();
    let quote = item["quote"].as_str().unwrap();
    assert!(content.contains(witness));
    assert!(quote.contains(witness));
    assert!(source.contains(quote));
    assert!(content.len() <= 2048, "returned {} bytes", content.len());
    assert!(quote.len() <= 2048);
}
