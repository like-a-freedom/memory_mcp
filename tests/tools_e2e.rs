use chrono::{TimeZone, Utc};
use rmcp::handler::server::wrapper::Parameters;

use memory_mcp::mcp::MemoryMcp;

mod common;

#[tokio::test]
async fn test_mcp_tools_flow() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "MSG-203",
        "content": "I will finish it by Friday. ARR $2M",
        "t_ref": "2026-01-10T00:00:00Z",
        "scope": "org"
    });
    let episode_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest")
        .0;
    assert_eq!(episode_id.status, "success");
    assert_eq!(
        episode_id.guidance.as_deref(),
        Some("Call extract next to derive entities and facts."),
    );
    let episode_id = episode_id.result;

    let extract_params = serde_json::json!({
        "episode_id": episode_id
    });
    let extraction = mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
        .expect("extract")
        .0;
    assert_eq!(extraction.status, "success");
    let extraction = extraction.result;
    assert!(extraction.facts.len() >= 2);

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
        .0;
    assert_eq!(context.status, "success");
    let context = context.result;
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
        .0;
    assert_eq!(explanation.status, "success");
    let explanation = explanation.result;
    assert_eq!(explanation[0].source_episode, episode_id);

    let ingest_params2 = serde_json::json!({
        "source_type": "email",
        "source_id": "MSG-204",
        "content": "Follow-up: ARR $500k",
        "t_ref": "2026-01-11T00:00:00Z",
        "scope": "org"
    });
    let episode_id2 = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params2).unwrap()))
        .await
        .expect("ingest2")
        .0
        .result;

    let context_items_ids =
        serde_json::to_string(&vec![episode_id.clone(), episode_id2.clone()]).unwrap();
    let explain_params_ids = serde_json::json!({"context_items": context_items_ids});
    let explanation_ids = mcp
        .explain(Parameters(
            serde_json::from_value(explain_params_ids).unwrap(),
        ))
        .await
        .expect("explain ids")
        .0
        .result;
    assert_eq!(explanation_ids[0].source_episode, episode_id);
    assert_eq!(explanation_ids[1].source_episode, episode_id2);
}

#[tokio::test]
async fn test_mcp_full_flow_end_to_end() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

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

    let extract_params = serde_json::json!({"episode_id": episode_id});
    let extraction = mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
        .expect("extract")
        .0
        .result;
    let facts = extraction.facts;
    assert!(facts.iter().any(|f| f.fact_type == "metric"));
    assert!(facts.iter().any(|f| f.fact_type == "promise"));

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

    let context_items = serde_json::to_string(&vec![serde_json::json!({"content": "ARR $1M","quote": "ARR $1M","source_episode": episode_id.clone()})]).unwrap();
    let explain_params = serde_json::json!({"context_items": context_items});
    let explanation = mcp
        .explain(Parameters(serde_json::from_value(explain_params).unwrap()))
        .await
        .expect("explain")
        .0
        .result;
    assert_eq!(explanation[0].source_episode, episode_id);

    let fact_id = context[0].fact_id.clone();
    let invalidate_params = serde_json::json!({"fact_id": fact_id, "reason": "superseded", "t_invalid": "2026-02-04T00:00:00Z"});
    let _ = mcp
        .invalidate(Parameters(
            serde_json::from_value(invalidate_params).unwrap(),
        ))
        .await
        .expect("invalidate");

    let assemble_params_after = serde_json::json!({"query": "ARR", "scope": "org", "as_of": Utc::now().to_rfc3339(), "budget": 5});
    let context_after = mcp
        .assemble_context(Parameters(
            serde_json::from_value(assemble_params_after).unwrap(),
        ))
        .await
        .expect("assemble")
        .0
        .result;
    assert!(
        !context_after
            .iter()
            .any(|c| c.fact_id == context[0].fact_id)
    );
}

#[tokio::test]
async fn test_mcp_extract_general_business_content_returns_structured_facts() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "E2E-GENERAL-1",
        "content": "DECISION: Product Release 9326394 is SIGNED. TASK: Elena Kondratyeva organizes alignment with regions. REQUIREMENT 6666474 must be updated for desktop integration.",
        "t_ref": "2026-02-05T00:00:00Z",
        "scope": "org"
    });
    let episode_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest")
        .0
        .result;

    let extract_params = serde_json::json!({"episode_id": episode_id});
    let extraction = mcp
        .extract(Parameters(serde_json::from_value(extract_params).unwrap()))
        .await
        .expect("extract")
        .0
        .result;

    assert!(
        !extraction.facts.is_empty(),
        "structured business content should produce extracted facts"
    );
}

#[tokio::test]
async fn test_mcp_ingest_validation_error() {
    let service = common::make_service().await;
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
    let service = common::make_service().await;
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
        .0;

    assert_eq!(extraction.status, "partial");
    assert!(extraction.result.entities.is_empty());
    assert!(extraction.result.facts.is_empty());
    assert_eq!(
        extraction.guidance.as_deref(),
        Some("Provide either `episode_id` or non-empty `content`/`text`, then retry."),
    );
}

#[tokio::test]
async fn test_mcp_explain_loose_objects_without_quote_and_source_episode() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let context_items = serde_json::to_string(&vec![
        serde_json::json!({"content":"Follow up on ARR deal","id":"task:e8gsmlprfchnktf6js0p","source_type":"task"}),
        serde_json::json!({"content":"ASSIGNEE: Anton Solovey — Split requirements","id":"task:ha8caz3sb2fxr9ju2sbc","source_type":"task"}),
    ]).unwrap();
    let explain_params = serde_json::json!({"context_items": context_items});
    let explanation = mcp
        .explain(Parameters(serde_json::from_value(explain_params).unwrap()))
        .await
        .expect("explain with loose objects should not fail")
        .0
        .result;
    assert_eq!(explanation.len(), 2);
    assert_eq!(explanation[0].source_episode, "task:e8gsmlprfchnktf6js0p");
    assert_eq!(explanation[0].content, "Follow up on ARR deal");
    assert_eq!(explanation[0].quote, "");
    assert_eq!(explanation[1].source_episode, "task:ha8caz3sb2fxr9ju2sbc");
}

#[tokio::test]
async fn test_mcp_explain_objects_with_quote_and_id() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let context_items = serde_json::to_string(&vec![
        serde_json::json!({"content":"data","quote":"q","id":"task:abc","source_type":"task"}),
    ])
    .unwrap();
    let explain_params = serde_json::json!({"context_items": context_items});
    let explanation = mcp
        .explain(Parameters(serde_json::from_value(explain_params).unwrap()))
        .await
        .expect("explain with quote + id should not fail")
        .0
        .result;
    assert_eq!(explanation[0].source_episode, "task:abc");
    assert_eq!(explanation[0].quote, "q");
}

#[tokio::test]
async fn test_mcp_explain_mixed_array() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let context_items = serde_json::to_string(&vec![
        serde_json::json!("episode:plain-id"),
        serde_json::json!({"content":"info","id":"task:obj"}),
    ])
    .unwrap();
    let explain_params = serde_json::json!({"context_items": context_items});
    let explanation = mcp
        .explain(Parameters(serde_json::from_value(explain_params).unwrap()))
        .await
        .expect("explain with mixed array should not fail")
        .0
        .result;
    assert_eq!(explanation.len(), 2);
    assert_eq!(explanation[0].source_episode, "episode:plain-id");
    assert_eq!(explanation[1].source_episode, "task:obj");
    assert_eq!(explanation[1].content, "info");
}

#[tokio::test]
async fn test_mcp_explain_loads_episode_context() {
    let service = common::make_service().await;
    let mcp = MemoryMcp::new(service);

    let ingest_params = serde_json::json!({
        "source_type": "email",
        "source_id": "EXPLAIN-CTX-1",
        "content": "Customer confirmed ARR is now $3M and expects renewal next quarter.",
        "t_ref": "2026-02-15T08:30:00Z",
        "scope": "org"
    });
    let episode_id = mcp
        .ingest(Parameters(serde_json::from_value(ingest_params).unwrap()))
        .await
        .expect("ingest explain context")
        .0
        .result;

    let context_items = serde_json::to_string(&vec![serde_json::json!({
        "content": "ARR is now $3M",
        "quote": "ARR is now $3M",
        "source_episode": episode_id.clone()
    })])
    .unwrap();

    let explanation = mcp
        .explain(Parameters(
            serde_json::from_value(serde_json::json!({"context_items": context_items})).unwrap(),
        ))
        .await
        .expect("explain with loaded episode context")
        .0
        .result;

    assert_eq!(explanation.len(), 1);
    assert_eq!(explanation[0].source_episode, episode_id);
    assert_eq!(explanation[0].scope.as_deref(), Some("org"));
    assert_eq!(
        explanation[0].t_ref.map(|dt| dt.to_rfc3339()),
        Some("2026-02-15T08:30:00+00:00".to_string())
    );
    assert!(explanation[0].t_ingested.is_some());
    assert_eq!(
        explanation[0].citation_context.as_deref(),
        Some("Customer confirmed ARR is now $3M and expects renewal next quarter.")
    );
    assert_eq!(
        explanation[0].provenance.get("source_episode"),
        Some(&serde_json::json!(explanation[0].source_episode.clone()))
    );
    assert_eq!(
        explanation[0].provenance.get("source_type"),
        Some(&serde_json::json!("email"))
    );
    assert_eq!(
        explanation[0].provenance.get("source_id"),
        Some(&serde_json::json!("EXPLAIN-CTX-1"))
    );
}

#[tokio::test]
async fn test_mcp_assemble_context_timeline_mode_passes_optional_fields() {
    let service = common::make_service().await;

    common::seed_fact_at(
        &service,
        "personal",
        "Atlas planning started",
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas budget increased",
        Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap(),
    )
    .await;

    let mcp = MemoryMcp::new(service);
    let params = serde_json::json!({
        "query": "atlas",
        "scope": "personal",
        "as_of": Utc::now().to_rfc3339(),
        "budget": 10,
        "view_mode": "timeline",
        "window_start": "2026-02-01T00:00:00Z",
        "window_end": "2026-02-28T23:59:59Z"
    });

    let context = mcp
        .assemble_context(Parameters(serde_json::from_value(params).unwrap()))
        .await
        .expect("assemble timeline")
        .0
        .result;

    assert_eq!(context.len(), 1);
    assert_eq!(context[0].content, "Atlas budget increased");
}
