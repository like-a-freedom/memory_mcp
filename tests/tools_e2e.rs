use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;

use memory_mcp::mcp::MemoryMcp;

mod common;

#[tokio::test]
async fn test_mcp_tools_flow() {
    let service = common::make_service();
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "MSG-203",
        "content": "Сделаю до пятницы. ARR $2M",
        "t_ref": "2026-01-10T00:00:00Z",
        "scope": "org"
    });
    let episode_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest")
        .0
        .result;

    let extract_params = serde_json::json!({
        "episode_id": episode_id
    });
    let extraction = mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
        .expect("extract")
        .0
        .result;
    assert!(extraction["facts"].as_array().unwrap().len() >= 2);

    let assemble_params = serde_json::json!({
        "query": "ARR",
        "scope": "org",
        "as_of": Utc::now().to_rfc3339(),
        "budget": 5
    });
    let context = mcp
        .assemble_context(Parameters(serde_json::from_value(assemble_params).unwrap()))
        .await
        .expect("assemble")
        .0
        .result;
    assert!(!context.is_empty());

    let context_items = serde_json::to_string(&vec![serde_json::json!({
        "content": "ARR $2M",
        "quote": "ARR $2M",
        "source_episode": episode_id.clone()
    })])
    .unwrap();
    let explain_params = serde_json::json!({"context_items": context_items});
    let explanation = mcp
        .explain(Parameters(serde_json::from_value(explain_params).unwrap()))
        .await
        .expect("explain")
        .0
        .result;
    assert_eq!(explanation[0]["source_episode"], episode_id);
}

#[tokio::test]
async fn test_mcp_full_flow_end_to_end() {
    let service = common::make_service();
    let mcp = MemoryMcp::new(service);

    // Ingest content that contains ARR and a promise
    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "E2E-1",
        "content": "I will deliver ARR $1M by next week.",
        "t_ref": "2026-02-05T00:00:00Z",
        "scope": "org"
    });
    let episode_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest")
        .0
        .result;

    // Extract
    let extract_params = serde_json::json!({"episode_id": episode_id});
    let extraction = mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
        .expect("extract")
        .0
        .result;
    let facts = extraction["facts"].as_array().unwrap();
    assert!(facts.iter().any(|f| f["type"] == "metric"));
    assert!(facts.iter().any(|f| f["type"] == "promise"));

    // Assemble
    let assemble_params = serde_json::json!({"query": "ARR", "scope": "org", "as_of": Utc::now().to_rfc3339(), "budget": 5});
    let context = mcp
        .assemble_context(Parameters(
            serde_json::from_value(assemble_params.clone()).unwrap(),
        ))
        .await
        .expect("assemble")
        .0
        .result;
    assert!(!context.is_empty());

    // Explain
    let context_items = serde_json::to_string(&vec![serde_json::json!({"content": "ARR $1M","quote": "ARR $1M","source_episode": episode_id.clone()})]).unwrap();
    let explain_params = serde_json::json!({"context_items": context_items});
    let explanation = mcp
        .explain(Parameters(serde_json::from_value(explain_params).unwrap()))
        .await
        .expect("explain")
        .0
        .result;
    assert_eq!(explanation[0]["source_episode"], episode_id);

    // Invalidate the metric fact (use a past date so it's considered invalid)
    let fact_id = context[0]["fact_id"].as_str().unwrap().to_string();
    let invalidate_params = serde_json::json!({"fact_id": fact_id, "reason": "superseded", "t_invalid": "2026-02-04T00:00:00Z"});
    let _ = mcp
        .invalidate(Parameters(
            serde_json::from_value(invalidate_params).unwrap(),
        ))
        .await
        .expect("invalidate");

    // Assemble again, ensure invalidated fact is not returned
    let context_after = mcp
        .assemble_context(Parameters(
            serde_json::from_value(assemble_params.clone()).unwrap(),
        ))
        .await
        .expect("assemble")
        .0
        .result;
    assert!(
        !context_after
            .iter()
            .any(|c| c["fact_id"] == context[0]["fact_id"])
    );
}

#[tokio::test]
async fn test_mcp_ingest_validation_error() {
    let service = common::make_service();
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "",
        "source_id": "MSG-204",
        "content": "Missing source_type",
        "t_ref": "2026-01-10T00:00:00Z",
        "scope": "org"
    });

    let err = match mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
    {
        Ok(_) => panic!("expected ingest to fail validation"),
        Err(err) => err,
    };
    let message = err.to_string();
    assert!(message.contains("source_type"));
}

#[tokio::test]
async fn test_mcp_extract_no_input_returns_soft_result() {
    let service = common::make_service();
    let mcp = MemoryMcp::new(service);

    let extract_params = serde_json::json!({
        "episode_id": "",
        "content": "",
        "text": null
    });

    let extraction = mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
        .expect("extract")
        .0
        .result;

    assert_eq!(extraction["status"], "no_input");
    assert!(extraction["entities"].as_array().unwrap().is_empty());
    assert!(extraction["facts"].as_array().unwrap().is_empty());
}
