#![cfg(feature = "eval-support")]
mod common;

use memory_mcp::models::AssembleContextRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;

fn source_case(case_id: &str) -> serde_json::Value {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../evals/longmemeval_v2/fixtures/reviewed-state-cases-2026-11-26.json"
    ))
    .unwrap();
    cases
        .into_iter()
        .find(|case| case["case_id"] == case_id)
        .unwrap()
}

#[tokio::test]
async fn reviewed_dropdown_survives_public_context_packaging() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let case = source_case("0c6b0c60");
        let service = common::make_service().await;
        let started = std::time::Instant::now();
        let episode = common::ingest_episode(&service, case["source_id"].as_str().unwrap(), case["content"].as_str().unwrap()).await;
        let formation_ms = started.elapsed().as_secs_f64() * 1000.0;
        let traced_request: AssembleContextRequest = serde_json::from_value(serde_json::json!({"query":case["query"],"budget":5,"compact":false})).unwrap();
        let receipt = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(&service, traced_request, &episode).await.unwrap();
        println!("STAGE_RECEIPT {}", serde_json::json!({"case_id":case["case_id"],"source_episode_id":episode,"formation_ingest_extract_ms":formation_ms,"observations":receipt.stages}));
        let items = AssembleContextCapability::assemble_context_from_service(&service, AssembleContextRequest {
            query: case["query"].as_str().unwrap().to_owned(), budget: 5, fact_types: vec![], as_of: None, view_mode: None, window_start: None, window_end: None, access: None, compact: false
        }).await.unwrap();
        assert!(items.iter().any(|item| item.content.contains(case["required_serialized_span"].as_str().unwrap())), "closed dropdown source block was split in context packaging");
    }).await.expect("bounded reviewed dropdown scenario");
}

#[tokio::test]
async fn diagnostic_observes_the_selected_source_before_and_after_packaging() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let case = source_case("0c6b0c60");
        let service = common::make_service().await;
        let episode = common::ingest_episode(
            &service,
            case["source_id"].as_str().unwrap(),
            case["content"].as_str().unwrap(),
        )
        .await;
        let request: AssembleContextRequest = serde_json::from_value(
            serde_json::json!({"query":case["query"],"budget":5,"compact":false}),
        )
        .unwrap();
        let observed = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
            &service, request, &episode,
        )
        .await
        .unwrap();
        assert!(
            observed
                .stages
                .iter()
                .any(|stage| stage.stage == "selected_pre_packaging"
                    && stage.items.iter().any(|item| item
                        .content
                        .contains(case["required_serialized_span"].as_str().unwrap()))),
            "prepackaging evidence must reveal whether the complete reviewed source was selected"
        );
        assert!(
            observed
                .stages
                .iter()
                .any(|stage| stage.stage == "packed_context")
        );
        assert!(
            observed
                .stages
                .iter()
                .any(|stage| stage.stage == "formation"
                    && stage
                        .items
                        .iter()
                        .any(|item| item.content == case["content"].as_str().unwrap())),
            "formation must be an actual source read, not inferred from returned answer"
        );
    })
    .await
    .expect("bounded stage observation");
}

#[tokio::test]
async fn reviewed_toolbar_survives_public_context_packaging() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let case = source_case("f61a096f");
        let service = common::make_service().await;
        let started = std::time::Instant::now();
        let episode = common::ingest_episode(&service, case["source_id"].as_str().unwrap(), case["content"].as_str().unwrap()).await;
        let formation_ms = started.elapsed().as_secs_f64() * 1000.0;
        let traced_request: AssembleContextRequest = serde_json::from_value(serde_json::json!({"query":case["query"],"budget":5,"compact":false})).unwrap();
        let receipt = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(&service, traced_request, &episode).await.unwrap();
        println!("STAGE_RECEIPT {}", serde_json::json!({"case_id":case["case_id"],"source_episode_id":episode,"formation_ingest_extract_ms":formation_ms,"observations":receipt.stages}));
        let request: AssembleContextRequest = serde_json::from_value(serde_json::json!({"query":case["query"],"budget":5,"compact":false})).unwrap();
        let items = AssembleContextCapability::assemble_context_from_service(&service, request).await.unwrap();
        assert!(items.iter().any(|item| item.content.contains(case["required_serialized_span"].as_str().unwrap())), "complete nine-action source toolbar missing from returned context");
    }).await.expect("bounded reviewed toolbar scenario");
}

#[tokio::test]
async fn independent_plain_tree_group_keeps_quote_offsets_and_byte_cap() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let service = common::make_service().await;
        let controls = "\t[10] button 'Export archive', clickable, visible\n\t\tStaticText 'Export archive'\n\t[11] button 'Validate archive', clickable, visible\n\t\tStaticText 'Validate archive'\n\t[12] button 'Publish archive', clickable, visible\n\t\tStaticText 'Publish archive'\n\tStaticText 'Archive metadata'\n";
        let content = format!("{}\n{}{}", "Unrelated navigation. ".repeat(110), controls, "Unrelated notes. ".repeat(160));
        let episode = common::ingest_episode(&service, "independent-archive-toolbar", &content).await;
        let request: AssembleContextRequest = serde_json::from_value(serde_json::json!({"query":"Which buttons include Export archive and Validate archive?","budget":5,"compact":false})).unwrap();
        let items = AssembleContextCapability::assemble_context_from_service(&service, request).await.unwrap();
        let item = items.iter().find(|item| item.source_episode == episode).expect("source must be returned");
        assert!(item.content.contains(controls));
        assert!(item.content.len() <= 2048);
        assert_eq!(item.content, item.quote);
        let range = &item.provenance["episode_excerpt"];
        let start = range["start_byte"].as_u64().unwrap() as usize;
        let end = range["end_byte"].as_u64().unwrap() as usize;
        assert_eq!(&content[start..end], item.content);
    }).await.expect("bounded independent plain tree scenario");
}

#[tokio::test]
async fn oversized_control_group_retains_lexical_window_and_cap() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let service = common::make_service().await;
        let mut content = String::from("\t[1] button 'Export archive initial', clickable, visible\n");
        for id in 2..100 { content.push_str(&format!("\t[{id}] button 'Export archive', clickable, visible\n")); }
        content.push_str("\tStaticText 'Archive metadata'\n");
        let episode = common::ingest_episode(&service, "oversized-archive-group", &content).await;
        let request: AssembleContextRequest = serde_json::from_value(serde_json::json!({"query":"Which buttons include Export archive initial?","budget":5,"compact":false})).unwrap();
        let items = AssembleContextCapability::assemble_context_from_service(&service, request).await.unwrap();
        let item = items.iter().find(|item| item.source_episode == episode).expect("oversized source returned");
        assert!(item.content.contains("Export archive initial"), "an oversized group must not be reclassified as a complete short suffix");
        assert!(item.content.len() <= 2048);
    }).await.expect("bounded oversized group scenario");
}

#[tokio::test]
async fn diagnostic_source_observation_respects_request_policy() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
        let service = common::make_service().await;
        let ingest = serde_json::from_value(serde_json::json!({"source_type":"chat","source_id":"private-archive-controls","content":"Export archive and Validate archive controls", "t_ref":"2026-03-01T10:00:00Z", "policy_tags":["private"]})).unwrap();
        let episode = IngestCapability::ingest_from_service(&service, ingest, None).await.unwrap();
        let request: AssembleContextRequest = serde_json::from_value(serde_json::json!({"query":"Export archive and Validate archive controls","budget":5,"compact":false,"access":{"allowed_tags":[]}})).unwrap();
        let observed = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(&service, request, &episode).await.unwrap();
        assert!(observed.stages.iter().all(|stage| stage.items.is_empty()), "diagnostic must not reveal a source excluded by request policy");
    }).await.expect("bounded policy-isolated observation");
}
