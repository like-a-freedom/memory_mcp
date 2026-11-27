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
async fn losing_lexical_pool_retains_source_without_native_fts_provenance() {
    use memory_mcp::storage::DbClient;
    let (service, db) = common::make_service_with_client().await;
    let controls = "\t[10] button 'Export archive', clickable, visible\n\t[11] button 'Validate archive', clickable, visible\n\t[12] button 'Publish archive', clickable, visible\n\tStaticText 'Archive details'\n";
    let time = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for n in 0..8 {
        let episode = format!("episode:competing_toolbar{n}");
        let text = if n == 7 {
            format!(
                "{controls}{}",
                "Unrelated archived observations. ".repeat(100)
            )
        } else {
            format!("{}\n\t[1] button 'Open settings', visible\n\t[2] button 'Close settings', visible\n\tStaticText 'Settings'\n", "I am using our archive-based administration dashboard. On archive detail pages, how many actions are there in the top toolbar, including the settings buttons? Your final answer should be in English one two three and wrapped. Export reports. Validate backups. ".repeat(50))
        };
        db.create(&episode, serde_json::json!({"episode_id":episode,"source_type":"document","source_id":episode,"content":text,"t_ref":time,"t_ingested":time,"scope":"org","policy_tags":[]}), "org", &["t_ref","t_ingested"]).await.unwrap();
        let fact = format!("fact:competing_toolbar{n}");
        db.create(&fact, serde_json::json!({"fact_id":fact,"fact_type":"note","content":text,"quote":text,"source_episode":episode,"t_valid":time,"t_ingested":time,"confidence":0.9,"index_keys":[],"entity_links":[],"provenance":{"source":"manual"},"scope":"org","policy_tags":[]}), "org", &["t_valid","t_ingested"]).await.unwrap();
    }
    let request = serde_json::from_value(serde_json::json!({"query":"I am using our archive-based administration dashboard. On archive detail pages, how many actions are there in the top toolbar, including the \"Export archive\" and \"Validate archive\" buttons? Your final answer should be in English one two three and wrapped.","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"})).unwrap();
    let observer = memory_mcp::eval_support::retrieval::ContextEvidenceCapture::new(
        "episode:competing_toolbar7",
    );
    let capture = memory_mcp::eval_support::retrieval::assemble_context_recording_evidence(
        &service, request, &observer,
    )
    .await
    .unwrap();
    println!(
        "COMPETING_POOLS {}",
        serde_json::to_string(&observer.candidates().unwrap()).unwrap()
    );
    println!(
        "COMPETING_SOURCE_STAGES {}",
        serde_json::to_string(&capture.stages).unwrap()
    );
    let pools = observer.candidates().unwrap();
    let fetched = pools
        .iter()
        .find(|pool| pool.stage == "facts.term_groups_fetched")
        .unwrap();
    assert_eq!(
        serde_json::to_value(fetched).unwrap()["tracked_source_fact_ids"],
        serde_json::json!(["fact:competing_toolbar7"]),
        "fetch receipts must bind observed candidates to the tracked source, not infer it from aggregate counts"
    );
    assert_eq!(
        pools
            .iter()
            .find(|p| p.stage == "facts.initial_fetched")
            .unwrap()
            .unique_sources,
        7
    );
    assert_eq!(
        pools
            .iter()
            .find(|p| p.stage == "facts.term_groups_fetched")
            .unwrap()
            .unique_sources,
        8
    );
    let ranked = &capture
        .stages
        .iter()
        .find(|s| s.stage == "ranked_fact_candidates")
        .unwrap()
        .items;
    assert_eq!(
        ranked.len(),
        1,
        "the independently retrieved source must survive fusion even when its pool loses"
    );
    assert_eq!(ranked[0].source_episode, "episode:competing_toolbar7");
    assert_eq!(
        ranked[0].provenance["eval_candidate_position"]["candidate_rank"],
        0
    );
    assert_eq!(
        ranked[0].provenance["eval_candidate_position"]["first_source_rank"],
        0
    );
    assert_eq!(
        ranked[0].retrieval_tier.as_deref(),
        Some("fallback"),
        "union membership must not manufacture a native full-text hit"
    );
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
async fn nearby_control_group_survives_a_stronger_narrative_window() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let service = common::make_service().await;
        let controls = "\t[10] button 'Export archive', clickable, visible\n\t\tStaticText 'Export archive'\n\t[11] button 'Validate archive', clickable, visible\n\t\tStaticText 'Validate archive'\n\t[12] button 'Publish archive', clickable, visible\n\t\tStaticText 'Publish archive'\n\tStaticText 'Archive metadata'\n";
        let query = "Which buttons include Export archive and Validate archive?";
        let mut content = format!("{}\n{query}\n", "Unrelated navigation. ".repeat(40));
        content.push_str(&" ".repeat(3100-content.len()));
        content.push_str(controls);
        content.push_str(&"Unrelated notes. ".repeat(200));
        let episode = common::ingest_episode(&service, "adjacent-archive-toolbar", &content).await;
        let request: AssembleContextRequest = serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false})).unwrap();
        let items = AssembleContextCapability::assemble_context_from_service(&service, request).await.unwrap();
        let item = items.iter().find(|item| item.source_episode == episode).expect("source must be returned");
        assert!(item.content.contains(controls), "a query-bearing nearby complete group must survive narrative-window selection");
        assert!(item.content.len() <= 2048);
        assert_eq!(item.content,item.quote);
        let range=&item.provenance["episode_excerpt"];
        let start=range["start_byte"].as_u64().unwrap() as usize;
        let end=range["end_byte"].as_u64().unwrap() as usize;
        assert_eq!(&content[start..end],item.content);
    }).await.expect("bounded neighboring group scenario");
}

#[tokio::test]
async fn narrative_query_does_not_jump_to_a_distant_control_group() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let service=common::make_service().await;
        let query="Explain Export archive and Validate archive.";
        let controls="\t[10] button 'Export archive', clickable, visible\n\t[11] button 'Validate archive', clickable, visible\n\tStaticText 'Archive metadata'\n";
        let content=format!("{query} {}\n{controls}", "Unrelated notes. ".repeat(250));
        let episode=common::ingest_episode(&service,"narrative-controls",&content).await;
        let request=serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false})).unwrap();
        let items=AssembleContextCapability::assemble_context_from_service(&service,request).await.unwrap();
        let item=items.iter().find(|item|item.source_episode==episode).expect("source returned");
        assert!(item.content.contains(query));
        assert!(!item.content.contains(controls));
        assert!(item.content.len()<=2048);
    }).await.expect("bounded narrative scenario");
}

#[tokio::test]
async fn enumeration_keeps_the_control_group_near_its_matching_narrative() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let service = common::make_service().await;
        let query = "When the archive is sorted by Rank, what period options appear in the dropdown?";
        let decoy = "\t[1] button 'Sort by Rank', clickable, visible\n\t[2] list '', visible\n\t\t[3] listitem 'Rank', visible\n\tStaticText 'Old state'\n";
        let controls = "\t[10] button 'Period', clickable, visible\n\t[11] list '', visible\n\t\t[12] listitem 'Recent', visible\n\t\t[13] listitem 'Entire history', visible\n\tStaticText 'Archive content'\n";
        let content = format!("{decoy}{}\n{query}\n{}\n{controls}{}", "Unrelated notes. ".repeat(250), " ".repeat(1300), "More notes. ".repeat(250));
        let episode = common::ingest_episode(&service, "narrative-group-coherence", &content).await;
        let request = serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false})).unwrap();
        let items = AssembleContextCapability::assemble_context_from_service(&service, request).await.unwrap();
        let item = items.iter().find(|item| item.source_episode == episode).expect("source returned");
        assert!(item.content.contains(controls), "a stronger sort-control match in another state must not displace the nearby period group: {:?}", item);
        assert!(!item.content.contains(decoy));
        assert_eq!(item.content, item.quote);
        assert!(item.content.len() <= 2048);
    }).await.expect("bounded narrative/group coherence scenario");
}

#[tokio::test]
async fn generic_options_group_does_not_replace_a_topical_narrative_answer() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let service = common::make_service().await;
        let query = "Which options are available for Falcon deployment?";
        let answer = "Falcon deployment options are blue-green and canary.";
        let controls = "\t[10] button 'More options', clickable, visible\n\t[11] button 'Close', clickable, visible\n\tStaticText 'Settings'\n";
        let content = format!("{query}\n{answer} {}\n{controls}", "Unrelated notes. ".repeat(250));
        let episode = common::ingest_episode(&service, "generic-options-narrative", &content).await;
        let request = serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false})).unwrap();
        let items = AssembleContextCapability::assemble_context_from_service(&service, request).await.unwrap();
        let item = items.iter().find(|item| item.source_episode == episode).expect("source returned");
        assert!(item.content.contains(answer), "generic UI terminology must not displace a topical answer");
        assert!(!item.content.contains(controls));
        assert_eq!(item.content, item.quote);
        assert!(item.content.len() <= 2048);
    }).await.expect("bounded generic options scenario");
}

#[tokio::test]
async fn complete_group_topic_support_beats_a_nearer_generic_control_group() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let service=common::make_service().await;
        let query="Which buttons include Export archive and Validate archive?";
        let decoy="\t[1] button 'Open settings', clickable, visible\n\t[2] button 'Close settings', clickable, visible\n\tStaticText 'Settings'\n";
        let controls="\t[10] button 'Export archive', clickable, visible\n\t[11] button 'Validate archive', clickable, visible\n\tStaticText 'Archive metadata'\n";
        let content=format!("{query}\n{decoy}{}\n{controls}{}", "Unrelated notes. ".repeat(250),"More notes. ".repeat(200));
        let episode=common::ingest_episode(&service,"competing-controls",&content).await;
        let request=serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false})).unwrap();
        let items=AssembleContextCapability::assemble_context_from_service(&service,request).await.unwrap();
        let item=items.iter().find(|item|item.source_episode==episode).expect("source returned");
        assert!(item.content.contains(controls));
        assert!(!item.content.contains(decoy));
        assert_eq!(item.content,item.quote);
        assert!(item.content.len()<=2048);
    }).await.expect("bounded competing group scenario");
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

#[tokio::test]
async fn response_format_instructions_do_not_dilute_the_same_question_evidence() {
    use memory_mcp::storage::DbClient;
    let (service, db) = common::make_service_with_client().await;
    let episode = "episode:format_independent_source";
    let content = "Which archive controls are available? Export, Validate and Publish.";
    let time = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    db.create(episode, serde_json::json!({"episode_id":episode,"source_type":"document","source_id":episode,"content":content,"t_ref":time,"t_ingested":time,"scope":"org","policy_tags":[]}), "org", &["t_ref","t_ingested"]).await.unwrap();
    db.create("fact:format_independent_source", serde_json::json!({
        "fact_id":"fact:format_independent_source", "fact_type":"observation", "content":content,"quote":content,
        "source_episode":episode,"t_valid":time,"t_ingested":time,"confidence":0.9,
        "index_keys":[],"scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}
    }),"org",&["t_valid","t_ingested"]).await.unwrap();
    let plain = serde_json::from_value(serde_json::json!({"query":"Which archive controls are available?","budget":1,"compact":false,"as_of":"2026-05-01T00:00:00Z"})).unwrap();
    let reference = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
        &service, plain, episode,
    )
    .await
    .unwrap();
    assert_eq!(
        reference.items.len(),
        1,
        "the ordinary question must return its source"
    );
    let formatted = serde_json::from_value(serde_json::json!({"query":"Which archive controls are available? Return the result as a comma-separated list in a JSON string with a short explanation in English.","budget":1,"compact":false,"as_of":"2026-05-01T00:00:00Z"})).unwrap();
    let actual = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
        &service, formatted, episode,
    )
    .await
    .unwrap();
    assert!(
        actual
            .stages
            .iter()
            .find(|stage| stage.stage == "admitted_fact_candidates")
            .unwrap()
            .items
            .iter()
            .any(|item| item.source_episode == episode),
        "answer formatting must not reject a source admitted for the same question"
    );
    let reference = &reference
        .stages
        .iter()
        .find(|stage| stage.stage == "ranked_fact_candidates")
        .unwrap()
        .items[0];
    let actual = &actual
        .stages
        .iter()
        .find(|stage| stage.stage == "ranked_fact_candidates")
        .unwrap()
        .items[0];
    assert!(reference.grounding.unwrap() > 0.0);
    assert_eq!(actual.source_episode, episode);
    assert_eq!(
        actual.grounding, reference.grounding,
        "formatting the same answer must not change source grounding during fusion"
    );
}

#[tokio::test]
async fn diagnostic_observes_admission_between_fusion_and_selection() {
    let service = common::make_service().await;
    let episode = common::ingest_episode(
        &service,
        "admission-source",
        "The archive supports Export and Validate operations.",
    )
    .await;
    let request = serde_json::from_value(serde_json::json!({"query":"Which archive operations are supported?","budget":5,"compact":false})).unwrap();
    let capture = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
        &service, request, &episode,
    )
    .await
    .unwrap();
    let stages = capture
        .stages
        .iter()
        .map(|s| s.stage.as_str())
        .collect::<Vec<_>>();
    let fusion = stages
        .iter()
        .position(|s| *s == "ranked_fact_candidates")
        .unwrap();
    let admitted = stages
        .iter()
        .position(|s| *s == "admitted_fact_candidates")
        .expect("completed admission must be separately observable");
    let selected = stages
        .iter()
        .position(|s| *s == "selected_pre_packaging")
        .unwrap();
    assert!(fusion < admitted && admitted < selected);
}

#[tokio::test]
async fn substantive_qualifiers_after_a_question_remain_admission_constraints() {
    use memory_mcp::storage::DbClient;
    let (service, db) = common::make_service_with_client().await;
    let episode = "episode:unqualified_controls";
    let content = "The archive controls available are Export and Validate.";
    let time: chrono::DateTime<chrono::Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
    db.create(episode, serde_json::json!({"episode_id":episode,"source_type":"document","source_id":episode,"content":content,"t_ref":time,"t_ingested":time,"scope":"org","policy_tags":[]}), "org", &["t_ref","t_ingested"]).await.unwrap();
    db.create("fact:unqualified_controls", serde_json::json!({"fact_id":"fact:unqualified_controls","fact_type":"observation","content":content,"quote":content,"source_episode":episode,"t_valid":time,"t_ingested":time,"confidence":0.9,"index_keys":[],"scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}}), "org", &["t_valid","t_ingested"]).await.unwrap();
    let request = serde_json::from_value(serde_json::json!({"query":"Which archive controls are available? Only encrypted deployments approved after independent security review across isolated tenant environments using audited key rotation.","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"})).unwrap();
    let capture = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
        &service, request, episode,
    )
    .await
    .unwrap();
    println!(
        "SUBSTANTIVE_STAGES {}",
        serde_json::to_string(&capture.stages).unwrap()
    );
    assert!(
        capture
            .stages
            .iter()
            .find(|s| s.stage == "admitted_fact_candidates")
            .unwrap()
            .items
            .is_empty(),
        "a non-format clause must retain its substantive topical requirements"
    );
    assert!(capture.items.is_empty());
}

#[tokio::test]
async fn number_words_in_a_format_clause_do_not_hide_substantive_requirements() {
    let service = common::make_service().await;
    let episode = common::ingest_episode(
        &service,
        "format-with-qualifiers",
        "The archive controls available are Export and Validate.",
    )
    .await;
    let request = serde_json::from_value(serde_json::json!({
        "query":"Which archive controls are available? Your final answer should be in English one two three and include independently audited encrypted Canadian deployments approved after security review.",
        "budget":5,"compact":false
    })).unwrap();
    let capture = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
        &service, request, &episode,
    )
    .await
    .unwrap();
    assert!(
        capture.items.is_empty(),
        "a mixed clause must retain its substantive requirements"
    );
}

#[tokio::test]
async fn compound_when_question_requires_event_time_from_the_source() {
    let service = common::make_service().await;
    common::ingest_episode(
        &service,
        "undated-archive-event",
        "Alice saved a record and then exported the archive.",
    )
    .await;
    let request = serde_json::from_value(serde_json::json!({"query":"When did Alice save a record, what happened next?","budget":5,"compact":false})).unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    assert!(
        items.is_empty(),
        "an undated source cannot supply the date requested in the first clause: {items:?}"
    );
}

#[tokio::test]
async fn named_source_phrases_outrank_scattered_words_and_index_only_labels() {
    use memory_mcp::storage::DbClient;
    let (service, db) = common::make_service_with_client().await;
    let controls = "Archive toolbar actions:\n\t[10] button 'Export archive', visible\n\t[11] button 'Validate archive', visible\n\t[12] button 'Publish archive', visible\n\tStaticText 'Archive details'\n";
    let time: chrono::DateTime<chrono::Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
    for n in 0..8 {
        let episode = format!("episode:literal_toolbar{n}");
        let fact = format!("fact:literal_toolbar{n}");
        let text = if n == 7 {
            controls.to_owned()
        } else if n == 6 {
            "An unrelated archive toolbar contains buttons Export archiveCopy and Validate archiveCopy.".to_owned()
        } else {
            format!(
                "Archive toolbar contains Export reports and Validate backups. {} The archive contains reference number {n}.",
                "General archive toolbar information. ".repeat(40)
            )
        };
        let keys = if n == 6 {
            vec!["Export archive", "Validate archive"]
        } else {
            vec![]
        };
        db.create(&episode, serde_json::json!({"episode_id":episode,"source_type":"document","source_id":episode,"content":text,"t_ref":time,"t_ingested":time,"scope":"org","policy_tags":[]}), "org", &["t_ref","t_ingested"]).await.unwrap();
        db.create(&fact, serde_json::json!({"fact_id":fact,"fact_type":"note","content":text,"quote":text,"source_episode":episode,"t_valid":time,"t_ingested":time,"confidence":if n == 6 { 1.0 } else { 0.9 },"index_keys":keys,"entity_links":[],"provenance":{"source":"manual"},"scope":"org","policy_tags":[]}), "org", &["t_valid","t_ingested"]).await.unwrap();
    }
    let request = serde_json::from_value(serde_json::json!({"query":"Which archive toolbar contains \"Export archive\" and \"Validate archive\"?","budget":1,"compact":false,"as_of":"2026-05-01T00:00:00Z"})).unwrap();
    let capture = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
        &service,
        request,
        "episode:literal_toolbar7",
    )
    .await
    .unwrap();
    println!(
        "LITERAL_SELECTION {}",
        serde_json::json!({"stages":capture.stages.iter().map(|s|serde_json::json!({"stage":s.stage,"items":s.items.iter().map(|i|serde_json::json!({"id":i.fact_id,"tier":i.retrieval_tier,"provenance":i.provenance})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"final":capture.items.iter().map(|i|(&i.fact_id,&i.source_episode)).collect::<Vec<_>>()})
    );
    assert!(
        capture
            .stages
            .iter()
            .find(|s| s.stage == "admitted_fact_candidates")
            .unwrap()
            .items
            .iter()
            .any(|i| i.source_episode == "episode:literal_toolbar7"),
        "the fixture must reach actual admission before testing selection"
    );
    assert_eq!(capture.items.len(), 1);
    assert_eq!(
        capture.items[0].source_episode, "episode:literal_toolbar7",
        "literal source phrases must outrank distributed words and index-only labels"
    );
    assert!(capture.items[0].content.contains(controls));
}

#[tokio::test]
async fn replacement_quote_is_not_reported_as_a_requested_control_label() {
    let service = common::make_service().await;
    let episode = common::ingest_episode(
        &service,
        "control-migration",
        "The archive toolbar migration replaces Remove archive with Archive safely.",
    )
    .await;
    let request = serde_json::from_value(serde_json::json!({"query":"Which archive toolbar button replaces \"Remove archive\" during migration?","budget":1,"compact":false})).unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("the migration evidence must remain available");
    assert!(
        item.provenance.get("control_label_coverage").is_none(),
        "a quoted former control is not a requested membership identity"
    );
}

const POSITIONAL_TOOLBAR_SOURCE: &str = "Project Orchid operations dashboard toolbar actions:\n\t[10] button 'Pause dispatch', clickable, visible\n\t[11] button 'Synchronize routes', clickable, visible\n\t[12] button 'Resume dispatch', clickable, visible\n\tStaticText 'Operations dashboard'\n";

#[tokio::test]
async fn quoted_positional_toolbar_anchors_survive_fact_candidate_bounding() {
    use memory_mcp::storage::DbClient;

    let (service, db) = common::make_service_with_client().await;
    let cutoff: chrono::DateTime<chrono::Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
    let target = "episode:orchid_position_32";
    for number in 0..=32 {
        let episode = format!("episode:orchid_position_{number}");
        let fact = format!("fact:orchid_position_{number}");
        let (content, keys, confidence) = if episode == target {
            (POSITIONAL_TOOLBAR_SOURCE.to_owned(), Vec::new(), 0.8)
        } else {
            (
                format!(
                    "{}\nProject Orchid operations dashboard reference {number}.",
                    "Project Orchid operations dashboard toolbar action appears between shifts. Pause dispatchCopy and Resume dispatchCopy appear in the toolbar actions. ".repeat(24)
                ),
                vec!["Pause dispatch", "Resume dispatch"],
                1.0,
            )
        };
        db.create(
            &episode,
            serde_json::json!({"episode_id":episode,"source_type":"document","source_id":episode,"content":content,"t_ref":cutoff,"t_ingested":cutoff,"scope":"org","policy_tags":[]}),
            "org",
            &["t_ref", "t_ingested"],
        )
        .await
        .unwrap();
        db.create(
            &fact,
            serde_json::json!({"fact_id":fact,"fact_type":"note","content":content,"quote":content,"source_episode":episode,"t_valid":cutoff,"t_ingested":cutoff,"confidence":confidence,"index_keys":keys,"entity_links":[],"provenance":{"source":"manual"},"scope":"org","policy_tags":[]}),
            "org",
            &["t_valid", "t_ingested"],
        )
        .await
        .unwrap();
    }

    let request = serde_json::from_value(serde_json::json!({
        "query": "On the Project Orchid operations dashboard, what action appears between \"Pause dispatch\" and \"Resume dispatch\" in the toolbar?",
        "budget": 1,
        "compact": false,
        "as_of": "2026-05-01T00:00:00Z"
    }))
    .unwrap();
    let result = memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
        &service, request, target,
    )
    .await
    .unwrap();
    let stage = |name| {
        result
            .stages
            .iter()
            .find(|stage| stage.stage == name)
            .unwrap()
    };
    let admitted = stage("admitted_fact_candidates")
        .items
        .iter()
        .find(|item| item.source_episode == target)
        .expect("the common source must reach admission before testing the rank bound");
    let admitted_rank = &admitted.provenance["eval_candidate_position"]["candidate_rank"];
    assert!(
        stage("bounded_fact_candidates")
            .items
            .iter()
            .any(|item| item.fact_id == admitted.fact_id),
        "the sole carrier with both whole control labels disappeared at the candidate bound, admitted rank {admitted_rank}"
    );
    assert_eq!(result.items.len(), 1);
    let item = &result.items[0];
    assert_eq!(item.source_episode, target);
    assert!(item.content.contains(POSITIONAL_TOOLBAR_SOURCE));
    assert!(item.content.len() <= 2048);
}

async fn assert_positional_control_priority_absent(query: &str) {
    let service = common::make_service().await;
    let episode = common::ingest_episode(
        &service,
        "orchid-reference",
        "Project Orchid operations dashboard toolbar action between shifts. During migration the Pause dispatch button was replaced by Resume dispatch. The Pause dispatch and Resume dispatch buttons are described in the dispatch notes.",
    )
    .await;
    let request = serde_json::from_value(serde_json::json!({
        "query": query,
        "budget": 1,
        "compact": false
    }))
    .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("the ordinary-relevance source must remain available");
    assert!(item.provenance.get("control_label_coverage").is_none());
}

#[tokio::test]
async fn explicit_exclusion_does_not_gain_positional_control_priority() {
    assert_positional_control_priority_absent(
        "Which Project Orchid toolbar action does not appear between \"Pause dispatch\" and \"Resume dispatch\"?",
    )
    .await;
}

#[tokio::test]
async fn unrelated_between_phrase_does_not_turn_migration_quotes_into_control_anchors() {
    assert_positional_control_priority_absent(
        "Between shifts, which Project Orchid toolbar action replaced \"Pause dispatch\" or \"Resume dispatch\" during migration?",
    )
    .await;
}

#[tokio::test]
async fn quoted_dispute_participants_do_not_gain_positional_control_priority() {
    assert_positional_control_priority_absent(
        "What action settled the dispute between \"Pause dispatch\" and \"Resume dispatch\" during the Project Orchid toolbar review?",
    )
    .await;
}

#[tokio::test]
async fn compound_when_question_preserves_a_source_with_the_event_date() {
    let service = common::make_service().await;
    let episode = common::ingest_episode(
        &service,
        "dated-archive-event",
        "Alice saved a record on January 2, 2026 and then exported the archive.",
    )
    .await;
    let request = serde_json::from_value(serde_json::json!({"query":"When did Alice save a record, what happened next?","budget":5,"compact":false})).unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    assert!(
        items
            .iter()
            .any(|item| item.source_episode == episode && item.content.contains("January 2, 2026"))
    );
}

async fn assert_excluded_label_has_no_positive_membership_priority(query: &str) {
    let service = common::make_service().await;
    let episode = common::ingest_episode(&service, "excluded-control", "The archive toolbar does not contain Delete account. The archive toolbar contains Export archive and Validate archive.").await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":1,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("the explicit exclusion source must remain available");
    assert!(
        item.provenance.get("control_label_coverage").is_none(),
        "an excluded label cannot acquire positive-membership priority"
    );
}

#[tokio::test]
async fn excluded_label_is_not_reported_as_requested_control_membership() {
    assert_excluded_label_has_no_positive_membership_priority(
        "Which archive toolbar does not contain \"Delete account\"?",
    )
    .await;
}

#[tokio::test]
async fn no_longer_included_label_has_no_positive_membership_priority() {
    assert_excluded_label_has_no_positive_membership_priority(
        "Which archive toolbar no longer includes \"Delete account\"?",
    )
    .await;
}

#[tokio::test]
async fn excepted_label_has_no_positive_membership_priority() {
    assert_excluded_label_has_no_positive_membership_priority(
        "Which archive toolbar contains all controls except \"Delete account\"?",
    )
    .await;
}

#[tokio::test]
async fn excludes_label_has_no_positive_membership_priority() {
    assert_excluded_label_has_no_positive_membership_priority(
        "Which archive toolbar excludes the button named \"Delete account\"?",
    )
    .await;
}

async fn toolbar_copy_context(
    query: &str,
    controls: &str,
    hidden: &str,
) -> memory_mcp::models::AssembledContextItem {
    let service = common::make_service().await;
    let content = format!(
        "Archive details\n{controls}{}\n{}\n{hidden}{}",
        "Unrelated archived observations. ".repeat(180),
        query.repeat(8),
        "Unrelated archived observations. ".repeat(180)
    );
    let episode =
        common::ingest_episode(&service, "visible-versus-hidden-controls", &content).await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":1,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    items
        .into_iter()
        .find(|item| item.source_episode == episode)
        .expect("source must remain available")
}

const VISIBLE_ARCHIVE_CONTROLS: &str = "\t[10] button 'Export archive', clickable, visible\n\t[11] button 'Validate archive', clickable, visible\n\t[12] button 'Publish archive', clickable, visible\n\tStaticText 'Archive details'\n";
const HIDDEN_ARCHIVE_CONTROLS: &str = "\t[20] button 'Export archive', clickable\n\t[21] button 'Validate archive', clickable\n\t[22] button 'Publish archive', clickable\n\t[23] button 'Search', clickable, visible\n\t[24] button 'Reset Filter', clickable, visible\n\tStaticText 'Search results'\n";

#[tokio::test]
async fn visible_controls_outrank_a_hidden_copy_near_the_lexical_window() {
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        VISIBLE_ARCHIVE_CONTROLS,
        HIDDEN_ARCHIVE_CONTROLS,
    )
    .await;
    assert!(
        item.content.contains(VISIBLE_ARCHIVE_CONTROLS),
        "visible named controls must survive ahead of their hidden historical copy"
    );
    assert!(!item.content.contains(HIDDEN_ARCHIVE_CONTROLS));
    assert!(item.content.len() <= 2048);
    assert_eq!(item.content, item.quote);
}

#[tokio::test]
async fn explicitly_hidden_control_question_keeps_its_matching_hidden_group() {
    let item = toolbar_copy_context(
        "Which hidden archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        VISIBLE_ARCHIVE_CONTROLS,
        HIDDEN_ARCHIVE_CONTROLS,
    )
    .await;
    assert!(item.content.contains(HIDDEN_ARCHIVE_CONTROLS));
    assert!(!item.content.contains(VISIBLE_ARCHIVE_CONTROLS));
    assert!(item.content.len() <= 2048);
}

#[tokio::test]
async fn unknown_visibility_keeps_the_prior_nearest_group_ordering() {
    let unknown_controls = VISIBLE_ARCHIVE_CONTROLS.replace(", visible", "");
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        &unknown_controls,
        HIDDEN_ARCHIVE_CONTROLS,
    )
    .await;
    assert!(item.content.contains(HIDDEN_ARCHIVE_CONTROLS));
    assert!(!item.content.contains(&unknown_controls));
}

#[tokio::test]
async fn visible_word_in_hidden_control_name_is_not_a_visibility_attribute() {
    let visible_controls =
        VISIBLE_ARCHIVE_CONTROLS.replace("Export archive", "Export visible archive");
    let hidden_controls =
        HIDDEN_ARCHIVE_CONTROLS.replace("Export archive", "Export visible archive");
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export visible archive\" and \"Validate archive\"?",
        &visible_controls,
        &hidden_controls,
    )
    .await;
    assert!(item.content.contains(&visible_controls));
    assert!(!item.content.contains(&hidden_controls));
}

#[tokio::test]
async fn separate_visible_button_names_do_not_form_one_requested_label() {
    let split_names = "\t[30] button 'Export', visible\n\t[31] button 'archive', visible\n\t[32] button 'Validate', visible\n\t[33] button 'archive', visible\n\tStaticText 'Unrelated controls'\n";
    let unknown_controls = VISIBLE_ARCHIVE_CONTROLS.replace(", visible", "");
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        split_names,
        &unknown_controls,
    )
    .await;
    assert!(
        item.content.contains(&unknown_controls),
        "a label must occur within one actual control name"
    );
    assert!(!item.content.contains(split_names));
}

#[tokio::test]
async fn quoted_popup_attributes_do_not_hide_visible_control_labels() {
    let visible_controls =
        VISIBLE_ARCHIVE_CONTROLS.replace(", visible", ", visible, hasPopup='menu'");
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        &visible_controls,
        HIDDEN_ARCHIVE_CONTROLS,
    )
    .await;
    assert!(
        item.content.contains(&visible_controls),
        "visibility belongs to the role attributes after the control name, including quoted attributes"
    );
    assert!(!item.content.contains(HIDDEN_ARCHIVE_CONTROLS));
}

#[tokio::test]
async fn quoted_visibility_word_inside_popup_value_is_not_an_attribute() {
    let quoted_controls =
        VISIBLE_ARCHIVE_CONTROLS.replace(", visible", ", hasPopup='menu, visible, actions'");
    let unknown_controls = VISIBLE_ARCHIVE_CONTROLS.replace(", visible", "");
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        &quoted_controls,
        &unknown_controls,
    )
    .await;
    assert!(
        item.content.contains(&unknown_controls),
        "quoted attribute content must not establish visibility"
    );
    assert!(!item.content.contains(&quoted_controls));
}

#[tokio::test]
async fn quoted_visibility_word_inside_escaped_label_is_not_an_attribute() {
    let quoted_controls = VISIBLE_ARCHIVE_CONTROLS
        .replace("', clickable, visible", "\\', visible, unnamed', clickable");
    let unknown_controls = VISIBLE_ARCHIVE_CONTROLS.replace(", visible", "");
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        &quoted_controls,
        &unknown_controls,
    )
    .await;
    assert!(
        item.content.contains(&unknown_controls),
        "escaped label text must not establish visibility"
    );
    assert!(!item.content.contains(&quoted_controls));
}

#[tokio::test]
async fn visible_control_container_confirms_an_unqualified_toolbar_question() {
    let confirmed = format!("[1] main '', visible\n{VISIBLE_ARCHIVE_CONTROLS}");
    let unknown = format!(
        "[2] main ''\n{}",
        HIDDEN_ARCHIVE_CONTROLS.replace(", clickable\n", ", clickable, visible\n")
    );
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        &confirmed,
        &unknown,
    )
    .await;
    assert!(
        item.content.contains(VISIBLE_ARCHIVE_CONTROLS),
        "an explicitly visible control container provides stronger observation evidence than an unknown one"
    );
    assert!(!item.content.contains(&unknown));
}

#[tokio::test]
async fn later_toolbar_request_does_not_prefer_the_old_visible_container() {
    let old = format!("[1] main '', visible\n{VISIBLE_ARCHIVE_CONTROLS}");
    let changed = HIDDEN_ARCHIVE_CONTROLS
        .replace(", clickable\n", ", clickable, visible\n")
        .replace("Publish archive", "Inspect archive");
    let updated = format!("[2] main ''\n{changed}");
    let item = toolbar_copy_context(
        "After the archive update, which toolbar contains \"Export archive\" and \"Validate archive\"?",
        &old, &updated,
    ).await;
    assert!(
        item.content.contains(&changed),
        "observation annotations must not override the requested later state"
    );
    assert!(!item.content.contains(VISIBLE_ARCHIVE_CONTROLS));
}

#[tokio::test]
async fn unknown_nearest_container_does_not_inherit_outer_visibility() {
    let nested_controls = VISIBLE_ARCHIVE_CONTROLS.replace('\n', "\n\t");
    let unknown_inner = format!("[1] main '', visible\n\t[2] group ''\n\t{nested_controls}");
    let near_controls = HIDDEN_ARCHIVE_CONTROLS.replace(", clickable\n", ", clickable, visible\n");
    let near = format!("[3] main ''\n{near_controls}");
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        &unknown_inner,
        &near,
    )
    .await;
    assert!(
        item.content.contains(&near_controls),
        "unknown nearest container cannot borrow stronger evidence from an outer ancestor"
    );
    assert!(!item.content.contains(&nested_controls));
}

#[tokio::test]
async fn unnamed_container_without_attributes_still_blocks_outer_visibility() {
    let nested_controls = VISIBLE_ARCHIVE_CONTROLS.replace('\n', "\n\t");
    let unknown_inner = format!("[1] main '', visible\n\t[2] group\n\t{nested_controls}");
    let near_controls = HIDDEN_ARCHIVE_CONTROLS.replace(", clickable\n", ", clickable, visible\n");
    let near = format!("[3] main ''\n{near_controls}");
    let item = toolbar_copy_context(
        "Which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        &unknown_inner,
        &near,
    )
    .await;
    assert!(
        item.content.contains(&near_controls),
        "a bare semantic role provides unknown visibility, not permission to inherit an outer value"
    );
    assert!(!item.content.contains(&nested_controls));
}

#[tokio::test]
async fn relative_date_question_keeps_its_matching_unknown_container() {
    let old = format!("[1] main '', visible\n{VISIBLE_ARCHIVE_CONTROLS}");
    let changed = HIDDEN_ARCHIVE_CONTROLS
        .replace(", clickable\n", ", clickable, visible\n")
        .replace("Publish archive", "Inspect archive");
    let updated = format!("[2] main ''\n{changed}");
    let item = toolbar_copy_context(
        "Today, which archive toolbar contains \"Export archive\" and \"Validate archive\"?",
        &old,
        &updated,
    )
    .await;
    assert!(
        item.content.contains(&changed),
        "an explicit date scope must retain its narrative match"
    );
    assert!(!item.content.contains(VISIBLE_ARCHIVE_CONTROLS));
}

#[tokio::test]
async fn directory_tabs_keep_the_complete_short_link_list() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let service = common::make_service().await;
        let query = "On the Archive index directory page, which browse tabs appear above the archive list?";
        let controls = "\t[40] main '', visible\n\t\t[41] list '', visible\n\t\t\t[42] listitem '', visible\n\t\t\t\t[43] link 'Collections', clickable, visible\n\t\t\t[44] listitem '', visible\n\t\t\t\t[45] link 'Labels', clickable, visible\n\t\t\t[46] listitem '', visible\n\t\t\t\t[47] link 'Recent activity', clickable, visible\n\t\t[48] heading 'Archive index', visible\n";
        let content = format!("{query}\n{}\n{controls}{}", "Unrelated navigation. ".repeat(130), "Unrelated archived details. ".repeat(150));
        let episode = common::ingest_episode(&service, "independent-directory-tabs", &content).await;
        let request = serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false})).unwrap();
        let items = AssembleContextCapability::assemble_context_from_service(&service, request).await.unwrap();
        let item = items.iter().find(|item| item.source_episode == episode).expect("directory source returned");
        assert!(item.content.contains(controls), "short link list must remain complete rather than preserve only its surrounding narrative");
        assert!(item.content.len() <= 2048);
        assert_eq!(item.content, item.quote);
        let range = &item.provenance["episode_excerpt"];
        let start = range["start_byte"].as_u64().unwrap() as usize;
        let end = range["end_byte"].as_u64().unwrap() as usize;
        assert_eq!(&content[start..end], item.content);
    }).await.expect("bounded directory tabs scenario");
}

#[tokio::test]
async fn directory_narrative_does_not_jump_to_unrequested_link_tabs() {
    let service = common::make_service().await;
    let query = "Explain Archive index directory navigation.";
    let links = "\t[40] list '', visible\n\t\t[41] listitem '', visible\n\t\t\t[42] link 'Collections', visible\n\t\t[43] listitem '', visible\n\t\t\t[44] link 'Labels', visible\n";
    let content = format!(
        "{query} {}\n{links}",
        "Unrelated archived details. ".repeat(200)
    );
    let episode = common::ingest_episode(&service, "directory-narrative-only", &content).await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("narrative source returned");
    assert!(item.content.contains(query));
    assert!(!item.content.contains(links));
}

#[tokio::test]
async fn short_tab_list_inside_an_oversized_list_remains_complete() {
    let service = common::make_service().await;
    let query =
        "On the Archive index directory page, which browse tabs appear above the archive list?";
    let controls = "\t\t\t[50] list '', visible\n\t\t\t\t[51] listitem '', visible\n\t\t\t\t\t[52] link 'Collections', clickable, visible\n\t\t\t\t[53] listitem '', visible\n\t\t\t\t\t[54] link 'Labels', clickable, visible\n\t\t\t\t[55] listitem '', visible\n\t\t\t\t\t[56] link 'Recent activity', clickable, visible\n\t\t\t[57] heading 'Archive index', visible\n";
    let content = format!(
        "{query}\n\t[40] list '', visible\n\t\t[41] listitem '', visible\n\t\t\tStaticText '{}'\n{controls}\t[58] heading 'Unrelated page', visible\n{}",
        "Unrelated navigation. ".repeat(140),
        "Unrelated archived details. ".repeat(150)
    );
    let episode = common::ingest_episode(&service, "nested-directory-tabs", &content).await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("directory source returned");
    assert!(
        item.content.contains(controls),
        "oversized enclosing list must not hide the short nested tab group"
    );
    assert!(item.content.len() <= 2048);
    let range = &item.provenance["episode_excerpt"];
    assert_eq!(
        &content[range["start_byte"].as_u64().unwrap() as usize
            ..range["end_byte"].as_u64().unwrap() as usize],
        item.content
    );
}

#[tokio::test]
async fn tab_list_after_a_long_button_group_remains_complete() {
    let service = common::make_service().await;
    let query =
        "On the Archive index directory page, which browse tabs appear above the archive list?";
    let controls = "\t[50] list '', visible\n\t\t[51] listitem '', visible\n\t\t\t[52] link 'Collections', clickable, visible\n\t\t[53] listitem '', visible\n\t\t\t[54] link 'Labels', clickable, visible\n\t\t[55] listitem '', visible\n\t\t\t[56] link 'Recent activity', clickable, visible\n\t[57] heading 'Archive index', visible\n";
    let content = format!(
        "{query}\n\t[40] button 'Open account', visible\n\t\tStaticText '{}'\n{controls}{}",
        "Unrelated navigation. ".repeat(140),
        "Unrelated archived details. ".repeat(150)
    );
    let episode = common::ingest_episode(&service, "tabs-after-long-button", &content).await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("directory source returned");
    assert!(
        item.content.contains(controls),
        "unrelated preceding button group must not swallow the requested link-list unit"
    );
    assert!(item.content.len() <= 2048);
}

#[tokio::test]
async fn button_request_with_tab_context_keeps_the_requested_buttons() {
    let service = common::make_service().await;
    let query = "Which archive toolbar buttons are available below the tabs?";
    let controls = "\t[10] button 'Export archive', visible\n\t[11] button 'Validate archive', visible\n\t[12] button 'Publish archive', visible\n\t[13] heading 'Archive details', visible\n";
    let links = "\t[40] list '', visible\n\t\t[41] listitem '', visible\n\t\t\t[42] link 'Archived records', visible\n\t\t[43] listitem '', visible\n\t\t\t[44] link 'Current records', visible\n\t[45] heading 'Archive navigation', visible\n";
    let content = format!(
        "{query}\n{}\n{controls}{}\n{links}{}",
        "Unrelated notes. ".repeat(180),
        "Unrelated notes. ".repeat(160),
        "Unrelated notes. ".repeat(100)
    );
    let episode = common::ingest_episode(&service, "button-request-tab-context", &content).await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("archive source returned");
    assert!(
        item.content.contains(controls),
        "context word tabs must not replace the explicitly requested toolbar buttons"
    );
    assert!(!item.content.contains(links));
}

#[tokio::test]
async fn neighboring_link_groups_crossing_the_window_edge_remain_complete() {
    let service = common::make_service().await;
    let query = "On the Archive directory page, which browse tabs are above the archive list?";
    let first = "\t[10] list '', visible\n\t\t[11] listitem '', visible\n\t\t\t[12] link 'Archive', clickable, visible\n\t\t[13] listitem '', visible\n\t\t\t[14] link 'Help', clickable, visible\n\t[15] heading 'Archive directory', visible\n";
    let second = "\t[20] list '', visible\n\t\t[21] listitem '', visible\n\t\t\t[22] link 'Collections', clickable, visible\n\t\t[23] listitem '', visible\n\t\t\t[24] link 'Labels', clickable, visible\n\t\t[25] listitem '', visible\n\t\t\t[26] link 'Recent activity', clickable, visible\n\t[27] heading 'Archive directory', visible\n";
    // The lexical winner contains the first group and only the beginning of
    // the second. The two complete groups together fit the existing byte cap.
    let mut content = format!("{query}\n{}\n", " ".repeat(800));
    content.push_str(first);
    content.push_str(&" ".repeat(1799 - content.len()));
    content.push('\n');
    content.push_str(second);
    content.push_str(&"Unrelated details. ".repeat(200));
    let episode = common::ingest_episode(&service, "adjacent-directory-groups", &content).await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("directory source returned");
    assert!(item.content.contains(first));
    assert!(
        item.content.contains(second),
        "a complete neighboring list must not make a partially covered list disappear"
    );
    assert!(item.content.len() <= 2048);
    let range = &item.provenance["episode_excerpt"];
    assert_eq!(
        &content[range["start_byte"].as_u64().unwrap() as usize
            ..range["end_byte"].as_u64().unwrap() as usize],
        item.content
    );
}

#[tokio::test]
async fn neighboring_tab_groups_that_cannot_fit_keep_the_byte_cap() {
    let service = common::make_service().await;
    let query = "On the Archive directory page, which browse tabs are above the archive list?";
    let first = "\t[10] list '', visible\n\t\t[11] listitem '', visible\n\t\t\t[12] link 'Archive collections', visible\n\t\t[13] listitem '', visible\n\t\t\t[14] link 'Archive labels', visible\n\t[15] heading 'Archive directory', visible\n";
    let second = "\t[20] list '', visible\n\t\t[21] listitem '', visible\n\t\t\t[22] link 'Archive history', visible\n\t\t[23] listitem '', visible\n\t\t\t[24] link 'Archive revisions', visible\n\t[25] heading 'Archive directory', visible\n";
    let mut content = format!("{}\n{first}", " ".repeat(999));
    content.push_str(&" ".repeat(1799 - content.len()));
    content.push_str(&format!("\n{query}\n"));
    content.push_str(&" ".repeat(2999 - content.len()));
    content.push_str(&format!("\n{second}{}", "Unrelated details. ".repeat(200)));
    let episode = common::ingest_episode(&service, "unfittable-directory-groups", &content).await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("directory source returned");
    assert!(item.content.len() <= 2048);
    assert!(!(item.content.contains(first) && item.content.contains(second)));
    let range = &item.provenance["episode_excerpt"];
    assert_eq!(
        &content[range["start_byte"].as_u64().unwrap() as usize
            ..range["end_byte"].as_u64().unwrap() as usize],
        item.content
    );
}

#[tokio::test]
async fn role_only_list_does_not_displace_neighboring_directory_tabs() {
    let service = common::make_service().await;
    let query = "On the Archive directory page, which browse tabs are above the archive list?";
    let unrelated = "\t[1] list '', visible\n\t\t[2] listitem '', visible\n\t\t\t[3] link 'Cooking', visible\n\t\t[4] listitem '', visible\n\t\t\t[5] link 'Gardening', visible\n\t[6] heading 'Hobbies', visible\n";
    let first = "\t[10] list '', visible\n\t\t[11] listitem '', visible\n\t\t\t[12] link 'Archive', visible\n\t\t[13] listitem '', visible\n\t\t\t[14] link 'Help', visible\n\t[15] heading 'Archive directory', visible\n";
    let second = "\t[20] list '', visible\n\t\t[21] listitem '', visible\n\t\t\t[22] link 'Collections', visible\n\t\t[23] listitem '', visible\n\t\t\t[24] link 'Labels', visible\n\t\t[25] listitem '', visible\n\t\t\t[26] link 'Recent activity', visible\n\t[27] heading 'Archive directory', visible\n";
    // The unrelated list crosses the window's leading edge; the two relevant
    // groups fit together, but their union with that list exceeds the cap.
    let mut content = format!("{}\n", " ".repeat(899));
    content.push_str(unrelated);
    content.push_str(&" ".repeat(1399 - content.len()));
    content.push_str(&format!("\n{first}"));
    content.push_str(&" ".repeat(1799 - content.len()));
    content.push_str(&format!("\n{query}\n"));
    content.push_str(&" ".repeat(2999 - content.len()));
    content.push_str(&format!("\n{second}{}", "Unrelated details. ".repeat(200)));
    let episode = common::ingest_episode(&service, "role-only-neighbor", &content).await;
    let request =
        serde_json::from_value(serde_json::json!({"query":query,"budget":5,"compact":false}))
            .unwrap();
    let items = AssembleContextCapability::assemble_context_from_service(&service, request)
        .await
        .unwrap();
    let item = items
        .iter()
        .find(|item| item.source_episode == episode)
        .expect("directory source returned");
    assert!(item.content.contains(first));
    assert!(
        item.content.contains(second),
        "a structural role match must not crowd out the requested complete tab list"
    );
    assert!(item.content.len() <= 2048);
    let range = &item.provenance["episode_excerpt"];
    assert_eq!(
        &content[range["start_byte"].as_u64().unwrap() as usize
            ..range["end_byte"].as_u64().unwrap() as usize],
        item.content
    );
}

#[tokio::test]
async fn exploratory_32_source_control_fetch_preserves_exact_membership() {
    use memory_mcp::storage::DbClient;
    let input: serde_json::Value = serde_json::from_str(include_str!(
        "../../../evals/longmemeval_v2/fixtures/independent-control-source-fetch-v1.json"
    ))
    .unwrap();
    let (service, db) = common::make_service_with_client().await;
    for row in input["records"].as_array().unwrap() {
        let episode = row["source_id"].as_str().unwrap();
        let fact = row["fact_id"].as_str().unwrap();
        let content = row["content"].as_str().unwrap();
        db.create(episode, serde_json::json!({"episode_id":episode,"source_type":"document","source_id":episode,"content":content,"t_ref":input["t_ref"],"t_ingested":input["t_ref"],"scope":"org","policy_tags":[]}), "org", &["t_ref","t_ingested"]).await.unwrap();
        db.create(fact, serde_json::json!({"fact_id":fact,"fact_type":"note","content":content,"quote":content,"source_episode":episode,"t_valid":input["t_ref"],"t_ingested":input["t_ref"],"confidence":0.9,"index_keys":[],"entity_links":[],"provenance":{"source":"manual"},"scope":"org","policy_tags":[]}), "org", &["t_valid","t_ingested"]).await.unwrap();
    }
    let capture = memory_mcp::eval_support::retrieval::ContextEvidenceCapture::new(
        input["source_id"].as_str().unwrap(),
    );
    let request = serde_json::from_value(serde_json::json!({"query":input["query"],"budget":5,"compact":false,"as_of":input["as_of"]})).unwrap();
    let observed = memory_mcp::eval_support::retrieval::assemble_context_recording_evidence(
        &service, request, &capture,
    )
    .await
    .unwrap();
    println!(
        "EXPLORATORY32_POOLS {}",
        serde_json::to_string(&capture.candidates().unwrap()).unwrap()
    );
    println!(
        "EXPLORATORY32_STAGES {}",
        serde_json::to_string(&observed.stages).unwrap()
    );
    assert!(
        observed.items.iter().any(|item| item.source_episode
            == input["source_id"].as_str().unwrap()
            && item
                .content
                .contains(input["required_span"].as_str().unwrap())),
        "the complete exact source must survive; stage receipts distinguish fetch loss from later loss"
    );
}
