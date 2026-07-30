use chrono::{TimeZone, Utc};
use memory_mcp::models::{
    AccessPayload, EntityCandidate, IngestRequest, InvalidateRequest, Provenance,
};
use memory_mcp::storage::DbClient;

mod common;

#[tokio::test]
async fn test_ingest_extract_and_assemble() {
    let service = common::make_service().await;
    let now = Utc::now();
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "email".to_string(),
                source_id: "MSG-201".to_string(),
                content: "ARR grew to $3M. I will send the update by Friday.".to_string(),
                t_ref: now - chrono::Duration::days(1),
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");

    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .expect("extract");
    let facts = extraction.facts;
    assert!(facts.iter().any(|fact| fact.fact_type == "metric"));
    assert!(facts.iter().any(|fact| fact.fact_type == "promise"));

    let context = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(now + chrono::Duration::seconds(1)),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble");
    assert!(!context.is_empty());
}

#[tokio::test]
async fn test_extract_skips_low_value_email_header_roster_note_fallback() {
    let service = common::make_service().await;
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "email".to_string(),
                source_id: "MSG-ROSTER-1".to_string(),
                content: "Subject: Weekly distro\nFrom: ops@example.com\nTo: alice@example.com; bob@example.com; carol@example.com\nCC: dave@example.com; erin@example.com".to_string(),
                t_ref: Utc.with_ymd_and_hms(2026, 4, 14, 9, 0, 0).unwrap(),
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest low-value email");

    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .expect("extract low-value email");

    assert!(
        extraction.facts.is_empty(),
        "raw email header/recipient roster should not become a fallback note fact: {:?}",
        extraction.facts
    );
}

#[tokio::test]
async fn test_resolve_aliases() {
    let service = common::make_service().await;
    let first = service
        .resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Dmitry Ivanov".to_string(),
                aliases: vec![],
            },
            None,
        )
        .await
        .expect("resolve");
    let alias = service
        .resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Dmitry Ivanov".to_string(),
                aliases: vec![],
            },
            None,
        )
        .await
        .expect("resolve alias");
    assert_eq!(first, alias);
}

#[tokio::test]
async fn test_invalidate_and_explain() {
    let service = common::make_service().await;
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "email".to_string(),
                source_id: "MSG-202".to_string(),
                content: "ARR is $1M".to_string(),
                t_ref: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");
    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .expect("extract");
    let fact_id = extraction.facts[0].fact_id.clone();

    service
        .invalidate(
            InvalidateRequest {
                fact_id: fact_id.to_string(),
                reason: "Superseded".to_string(),
                t_invalid: Utc.with_ymd_and_hms(2026, 1, 19, 0, 0, 0).unwrap(),
            },
            None,
        )
        .await
        .expect("invalidate");

    let context = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 1, 20, 0, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble");
    assert!(context.is_empty());

    let explanation = service
        .explain(
            memory_mcp::models::ExplainRequest {
                context_pack: vec![memory_mcp::models::ExplainItem {
                    fact_id: None,
                    content: "ARR is $1M".to_string(),
                    quote: "ARR is $1M".to_string(),
                    source_episode: episode_id.clone(),
                    scope: None,
                    t_ref: None,
                    t_ingested: None,
                    provenance: serde_json::Value::Null,
                    citation_context: None,
                    all_sources: vec![],
                    graph_insights: None,
                    fact_age_days: None,
                    decayed_confidence: None,
                    ingestion_method: None,
                }],
                compact: false,
            },
            None,
        )
        .await
        .expect("explain");
    assert_eq!(explanation[0].source_episode, episode_id);
}

#[tokio::test]
async fn test_policy_tag_filtering() {
    let service = common::make_service().await;
    service
        .add_fact(
            "metric",
            "Salary $100K",
            "$100K",
            "episode:hr",
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
            "private-domain",
            0.9,
            vec!["entity:a".to_string()],
            vec!["hr.salary".to_string()],
            Provenance::agent_observation("episode:hr"),
        )
        .await
        .expect("add_fact");

    let context = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Salary".to_string(),
            scope: "private-domain".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: Some(memory_mcp::models::AccessPayload {
                allowed_scopes: Some(vec!["private-domain".to_string()]),
                allowed_tags: Some(vec!["deal.pipeline".to_string()]),
                caller_id: None,
                session_vars: None,
                transport: None,
                content_type: None,
                cross_scope_allow: None,
            }),
            compact: false,
        })
        .await
        .expect("assemble");
    assert!(context.is_empty());
}

#[tokio::test]
async fn test_graph_intro_chain() {
    let service = common::make_service().await;
    let alice = service
        .resolve_entity("person", "Alice")
        .await
        .expect("alice");
    let bob = service.resolve_entity("person", "Bob").await.expect("bob");
    let openai = service
        .resolve_entity("company", "OpenAI")
        .await
        .expect("openai");

    service.relate(&alice, "knows", &bob).await.expect("relate");
    service
        .relate(&bob, "knows", &openai)
        .await
        .expect("relate");

    let chain = service
        .find_intro_chain("OpenAI", 3, None)
        .await
        .expect("chain");
    assert_eq!(chain, vec![alice, bob, openai]);
}

#[tokio::test]
async fn test_graph_intro_chain_as_of_filters_edges() {
    let service = common::make_service().await;
    let alice = service
        .resolve_entity("person", "Alice")
        .await
        .expect("alice");
    let bob = service.resolve_entity("person", "Bob").await.expect("bob");
    let openai = service
        .resolve_entity("company", "OpenAI")
        .await
        .expect("openai");

    service.relate(&alice, "knows", &bob).await.expect("relate");
    service
        .relate(&bob, "knows", &openai)
        .await
        .expect("relate");

    let past = Utc::now() - chrono::Duration::days(1);
    let chain_past = service
        .find_intro_chain("OpenAI", 3, Some(past))
        .await
        .expect("chain past");
    assert!(chain_past.is_empty());

    let future = Utc::now() + chrono::Duration::seconds(1);
    let chain_future = service
        .find_intro_chain("OpenAI", 3, Some(future))
        .await
        .expect("chain future");
    assert_eq!(chain_future, vec![alice, bob, openai]);
}

#[tokio::test]
async fn test_explain_exposes_graph_insights_for_cross_community_connection() {
    let (service, db_client) = common::make_service_with_client().await;
    let t_ref = Utc.with_ymd_and_hms(2026, 4, 8, 10, 0, 0).unwrap();

    let alice_id = service
        .resolve_entity("person", "Alice Smith")
        .await
        .expect("alice");
    let bob_id = service
        .resolve_entity("person", "Bob Jones")
        .await
        .expect("bob");
    let carol_id = service
        .resolve_entity("person", "Carol White")
        .await
        .expect("carol");
    let diana_id = service
        .resolve_entity("person", "Diana Prince")
        .await
        .expect("diana");

    service
        .relate(&alice_id, "knows", &bob_id)
        .await
        .expect("alice->bob");
    service
        .relate(&bob_id, "knows", &carol_id)
        .await
        .expect("bob->carol");

    common::seed_community(
        &db_client,
        "org",
        "community:alpha",
        &[alice_id.clone(), bob_id.clone()],
        "Alice Smith, Bob Jones",
        t_ref,
    )
    .await;
    common::seed_community(
        &db_client,
        "org",
        "community:beta",
        &[carol_id.clone(), diana_id.clone()],
        "Carol White, Diana Prince",
        t_ref,
    )
    .await;

    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "meeting".to_string(),
                source_id: "graph-insights-1".to_string(),
                content: "Alice Smith reviewed the partner map".to_string(),
                t_ref,
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");

    let fact_id = service
        .add_fact(
            "note",
            "Alice Smith reviewed the partner map",
            "Alice Smith reviewed the partner map",
            &episode_id,
            t_ref,
            "org",
            0.9,
            vec![alice_id.clone()],
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .expect("add fact");

    let explanation = service
        .explain(
            memory_mcp::models::ExplainRequest {
                context_pack: vec![memory_mcp::models::ExplainItem {
                    fact_id: Some(fact_id),
                    content: "Alice Smith reviewed the partner map".to_string(),
                    quote: "Alice Smith reviewed the partner map".to_string(),
                    source_episode: episode_id.clone(),
                    scope: None,
                    t_ref: None,
                    t_ingested: None,
                    provenance: serde_json::json!({"source_episode": episode_id}),
                    citation_context: None,
                    all_sources: vec![],
                    graph_insights: None,
                    fact_age_days: None,
                    decayed_confidence: None,
                    ingestion_method: None,
                }],
                compact: false,
            },
            None,
        )
        .await
        .expect("explain");

    let serialized = serde_json::to_value(&explanation[0]).expect("serialize explain item");
    let graph_insights = serialized
        .get("graph_insights")
        .expect("explain should expose graph_insights");
    let hub_entities = graph_insights
        .get("hub_entities")
        .and_then(serde_json::Value::as_array)
        .expect("graph_insights.hub_entities should be an array");
    let surprising_connections = graph_insights
        .get("surprising_connections")
        .and_then(serde_json::Value::as_array)
        .expect("graph_insights.surprising_connections should be an array");

    assert!(hub_entities.iter().any(|hub| {
        hub.get("entity_id") == Some(&serde_json::json!(bob_id))
            && hub.get("degree") == Some(&serde_json::json!(2))
    }));
    assert!(surprising_connections.iter().any(|connection| {
        connection.get("source_entity_id") == Some(&serde_json::json!(alice_id))
            && connection.get("target_entity_id") == Some(&serde_json::json!(carol_id))
            && connection.get("hop_count") == Some(&serde_json::json!(2))
    }));
}

#[tokio::test]
async fn test_relate_repeated_write_invalidates_previous_edge_version() {
    let (service, db_client) = common::make_service_with_client().await;
    let alice = service
        .resolve_entity("person", "Alice")
        .await
        .expect("alice");
    let bob = service.resolve_entity("person", "Bob").await.expect("bob");

    service
        .relate(&alice, "knows", &bob)
        .await
        .expect("relate 1");
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    service
        .relate(&alice, "knows", &bob)
        .await
        .expect("relate 2");

    let edges = db_client.select_table("edge", "org").await.expect("edges");
    let to_record_id = |record_id: &str| {
        let (table, key) = record_id
            .split_once(':')
            .expect("record id should contain table prefix");
        serde_json::json!({"RecordId": {"table": table, "key": key}})
    };
    let knows_edges: Vec<_> = edges
        .into_iter()
        .filter_map(|edge| edge.as_object().cloned())
        .filter(|edge| {
            edge.get("in") == Some(&to_record_id(&alice))
                && edge.get("relation").and_then(|value| value.as_str()) == Some("knows")
                && edge.get("out") == Some(&to_record_id(&bob))
        })
        .collect();

    assert_eq!(knows_edges.len(), 2);

    let invalidated_edges: Vec<_> = knows_edges
        .iter()
        .filter(|edge| edge.get("t_invalid").is_some())
        .collect();
    assert_eq!(invalidated_edges.len(), 1);
    assert!(invalidated_edges[0].get("t_invalid_ingested").is_some());

    let active_edges: Vec<_> = knows_edges
        .iter()
        .filter(|edge| edge.get("t_invalid").is_none())
        .collect();
    assert_eq!(active_edges.len(), 1);
}

#[tokio::test]
async fn test_assemble_context_uses_matching_community_summary() {
    let (service, db_client) = common::make_service_with_client().await;
    let t_ref = Utc.with_ymd_and_hms(2024, 4, 1, 10, 0, 0).unwrap();

    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "meeting".to_string(),
                source_id: "community-retrieval-1".to_string(),
                content: "Alice Smith met Bob Jones to plan next steps".to_string(),
                t_ref,
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");

    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .expect("extract");
    let alice_id = extraction
        .entities
        .iter()
        .find(|entity| entity.canonical_name == "Alice Smith")
        .map(|entity| entity.entity_id.clone())
        .expect("alice entity");

    let fact_id = service
        .add_fact(
            "note",
            "Prototype milestone is blocked",
            "Prototype milestone is blocked",
            &episode_id,
            t_ref,
            "org",
            0.8,
            vec![alice_id],
            vec![],
            Provenance::agent_observation(&episode_id),
        )
        .await
        .expect("add fact");

    let communities = db_client
        .select_table("community", "org")
        .await
        .expect("communities");
    assert!(!communities.is_empty());
    assert!(communities.iter().any(|community| {
        community
            .get("summary")
            .and_then(|value| value.as_str())
            .is_some_and(|summary| summary.contains("Bob Jones"))
    }));

    let facts = db_client.select_table("fact", "org").await.expect("facts");
    assert!(facts.iter().any(|fact| {
        fact.get("fact_id").and_then(|value| value.as_str()) == Some(fact_id.as_str())
    }));

    let context = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Bob Jones".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble");

    assert!(context.iter().any(|item| item.fact_id == fact_id));
    assert!(
        context
            .iter()
            .any(|item| item.rationale.contains("community"))
    );
}

#[tokio::test]
async fn test_rate_limit_determinism() {
    let service = common::make_service().await;
    service
        .add_fact(
            "metric",
            "ARR $1M",
            "$1M",
            "episode:vars",
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
            "org",
            0.8,
            vec!["entity:a".to_string()],
            vec![],
            Provenance::agent_observation("episode:vars"),
        )
        .await
        .expect("add_fact");

    let access = AccessPayload {
        allowed_scopes: Some(vec!["org".to_string()]),
        allowed_tags: None,
        caller_id: Some("u1".to_string()),
        session_vars: Some(serde_json::json!({"user_id": "u1"})),
        transport: None,
        content_type: None,
        cross_scope_allow: None,
    };

    let first = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: Some(memory_mcp::models::AccessPayload {
                allowed_scopes: access.allowed_scopes.clone(),
                allowed_tags: None,
                caller_id: access.caller_id.clone(),
                session_vars: access.session_vars.clone(),
                transport: None,
                content_type: None,
                cross_scope_allow: None,
            }),
            compact: false,
        })
        .await
        .expect("assemble");
    let second = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: Some(memory_mcp::models::AccessPayload {
                allowed_scopes: access.allowed_scopes.clone(),
                allowed_tags: None,
                caller_id: access.caller_id.clone(),
                session_vars: access.session_vars.clone(),
                transport: None,
                content_type: None,
                cross_scope_allow: None,
            }),
            compact: false,
        })
        .await
        .expect("assemble");

    assert_eq!(first, second);
}

#[tokio::test]
async fn test_multiword_query_retrieval_quality() {
    let service = common::make_service().await;
    let t = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();

    service
        .add_fact(
            "note",
            "Project Delta deployment includes a gateway service on port 13000",
            "Delta Gateway",
            "episode:035d8d47",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:035d8d47"),
        )
        .await
        .expect("add fact 1");

    service
        .add_fact(
            "note",
            "Fleet checklist: certs required, tokens rotated, ports 5223 and 443 must be open",
            "fleet checklist certs tokens",
            "episode:035d8d47",
            t,
            "org",
            0.85,
            vec![],
            vec![],
            Provenance::agent_observation("episode:035d8d47"),
        )
        .await
        .expect("add fact 2");

    service
        .add_fact(
            "note",
            "Module v2.2 release notes: feature set updated and component v2.1 improved",
            "Module v2.2 release",
            "episode:8de581d5",
            t,
            "org",
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:8de581d5"),
        )
        .await
        .expect("add fact 3");

    let ctx = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Delta Enrollment".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble Delta Enrollment");
    assert!(
        !ctx.is_empty(),
        "Delta Enrollment: expected matches for non-adjacent multi-word query"
    );

    let ctx2 = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "fleet checklist certs tokens ports pending checklist episode:035d8d47"
                .to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble mobile checklist");
    assert!(
        !ctx2.is_empty(),
        "mobile checklist query with episode ref: expected matches"
    );

    let ctx3 = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: r#"release notes v2.2 Module "Module_6.0_Archive - Component v2.1.md" episode:8de581d5"#.to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        compact: false,
})
        .await
        .expect("assemble Module changelog");
    assert!(
        !ctx3.is_empty(),
        "Module changelog query with quotes and episode ref: expected matches"
    );
}

#[tokio::test]
async fn test_short_natural_language_query_uses_term_fallback() {
    let service = common::make_service().await;
    let t = Utc.with_ymd_and_hms(2025, 6, 2, 0, 0, 0).unwrap();

    let answer_fact_id = service
        .add_fact(
            "note",
            "I will graduate with a degree in Business Administration next spring.",
            "Business Administration degree",
            "episode:degree-answer",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:degree-answer"),
        )
        .await
        .expect("add answer fact");

    service
        .add_fact(
            "note",
            "The degree committee meets every Thursday to review curriculum changes.",
            "degree committee review",
            "episode:degree-generic",
            t,
            "org",
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:degree-generic"),
        )
        .await
        .expect("add generic degree fact");

    service
        .add_fact(
            "note",
            "I will graduate next spring with honors after finishing my final project.",
            "graduate honors",
            "episode:graduate-generic",
            t,
            "org",
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:graduate-generic"),
        )
        .await
        .expect("add generic graduate fact");

    let ctx = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "What degree did I graduate with?".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble short natural-language query");

    assert!(
        !ctx.is_empty(),
        "expected a short natural-language query to retrieve matching facts"
    );
    assert_eq!(
        ctx.first().map(|item| item.fact_id.as_str()),
        Some(answer_fact_id.as_str()),
        "expected the answer fact to rank ahead of partial single-term distractors"
    );
}

#[tokio::test]
async fn test_assemble_context_exposes_retrieval_tier_and_rationale_metadata() {
    let service = common::make_service().await;
    let t = Utc.with_ymd_and_hms(2026, 3, 3, 10, 0, 0).unwrap();

    let fact_id = common::seed_fact_at(
        &service,
        "personal",
        "Atlas deployment checklist is approved for rollout.",
        t,
    )
    .await;

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "deployment checklist rollout".to_string(),
            scope: "personal".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble context with metadata");

    let item = items
        .iter()
        .find(|item| item.fact_id == fact_id)
        .expect("direct fact should be returned");
    let serialized = serde_json::to_value(item).expect("serialize assembled item");

    assert_eq!(
        serialized
            .get("retrieval_tier")
            .and_then(serde_json::Value::as_str),
        Some("direct")
    );
    assert!(
        item.rationale.contains("tier=direct"),
        "rationale should include tier metadata, got: {}",
        item.rationale
    );
    assert!(
        item.rationale.contains("confidence="),
        "rationale should include confidence metadata, got: {}",
        item.rationale
    );
    assert!(
        item.rationale.contains("grounding="),
        "rationale should include grounding metadata, got: {}",
        item.rationale
    );
    assert!(
        item.rationale.contains("semantic="),
        "rationale should include semantic availability metadata, got: {}",
        item.rationale
    );
    assert!(
        serialized
            .get("relevance")
            .and_then(serde_json::Value::as_f64)
            .is_some(),
        "assembled item should expose a separate relevance score"
    );
    assert!(
        serialized
            .get("grounding")
            .and_then(serde_json::Value::as_f64)
            .is_some(),
        "assembled item should expose a separate grounding score"
    );
    assert_eq!(
        serialized
            .get("semantic_available")
            .and_then(serde_json::Value::as_bool),
        Some(false),
        "default test service runs without semantic embeddings and should say so explicitly"
    );
}

#[tokio::test]
async fn test_assemble_context_graph_results_include_anchor_and_hop_trace() {
    let (service, db_client) = common::make_service_with_client().await;
    let t = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();

    common::seed_entity(
        &db_client,
        "org",
        "entity:alice",
        "person",
        "Alice Stone",
        &[],
    )
    .await;
    common::seed_entity(&db_client, "org", "entity:bob", "person", "Bob Chen", &[]).await;
    service
        .relate("entity:alice", "knows", "entity:bob")
        .await
        .expect("seed edge");
    common::seed_fact_with_links(
        &service,
        "org",
        "Bob Chen owns the Atlas launch checklist.",
        t,
        vec!["entity:bob".to_string()],
    )
    .await;

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Alice Stone".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble context");

    let graph_item = items
        .iter()
        .find(|item| item.retrieval_tier.as_deref() == Some("graph"))
        .expect("graph-expanded item should exist");

    assert!(graph_item.rationale.contains("anchor=Alice Stone"));
    assert!(graph_item.rationale.contains("hops=1"));

    let serialized = serde_json::to_value(graph_item).expect("serialize graph assembled item");
    assert_eq!(
        serialized
            .get("provenance")
            .and_then(|value| value.get("graph_trace"))
            .and_then(|value| value.get("hop_count"))
            .and_then(serde_json::Value::as_u64),
        Some(1),
    );
}

#[tokio::test]
async fn test_low_grounding_long_query_returns_empty_instead_of_generic_overlap_noise() {
    let service = common::make_service().await;
    let t = Utc.with_ymd_and_hms(2026, 4, 14, 10, 0, 0).unwrap();

    service
        .add_fact(
            "note",
            "Regional rollout checklist updated for Friday handoff.",
            "Regional rollout checklist updated",
            "episode:generic-rollout-noise-1",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:generic-rollout-noise-1"),
        )
        .await
        .expect("seed generic rollout noise 1");

    service
        .add_fact(
            "note",
            "Support workflow handoff checklist prepared for rollout review.",
            "Support workflow handoff checklist prepared",
            "episode:generic-rollout-noise-2",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:generic-rollout-noise-2"),
        )
        .await
        .expect("seed generic rollout noise 2");

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "openshift migration exception compatibility rollout controls".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 14, 12, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble low-grounding query");

    assert!(
        items.is_empty(),
        "single generic-term overlap should not survive a long grounded query: {items:?}"
    );
}

#[tokio::test]
async fn test_assemble_context_promotes_temporal_index_key_matches_to_temporal_tier() {
    let service = common::make_service().await;
    let t = Utc.with_ymd_and_hms(2026, 3, 15, 9, 0, 0).unwrap();

    let fact_id = service
        .add_fact(
            "note",
            "Quarterly launch review finalized.",
            "launch review finalized",
            "episode:temporal-tier",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:temporal-tier"),
        )
        .await
        .expect("seed temporal fact");

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "march 2026 launch review".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble temporal context with metadata");

    let item = items
        .iter()
        .find(|item| item.fact_id == fact_id)
        .expect("temporal fact should be returned");
    let serialized = serde_json::to_value(item).expect("serialize assembled temporal item");

    assert_eq!(
        serialized
            .get("retrieval_tier")
            .and_then(serde_json::Value::as_str),
        Some("temporal")
    );
    assert!(
        item.rationale.contains("tier=temporal"),
        "rationale should include temporal tier metadata, got: {}",
        item.rationale
    );
}

#[tokio::test]
async fn test_queryful_assemble_context_skips_unrelated_recent_experience_and_temporal_noise() {
    let service = common::make_service().await;

    let july_fact_id = service
        .add_fact(
            "note",
            "Requirement R-0712 was created in July 2025 for platform asset migration.",
            "Requirement R-0712 was created in July 2025 for platform asset migration.",
            "episode:july-requirement",
            Utc.with_ymd_and_hms(2025, 7, 9, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:july-requirement"),
        )
        .await
        .expect("seed july requirement fact");

    let august_noise_id = service
        .add_fact(
            "note",
            "Method for collecting observed event throughput from sandbox installations was documented in August 2025.",
            "Method for collecting observed event throughput from sandbox installations was documented in August 2025.",
            "episode:august-noise",
            Utc.with_ymd_and_hms(2025, 8, 19, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:august-noise"),
        )
        .await
        .expect("seed august noise fact");

    let recent_experience_id = service
        .add_fact(
            "experience",
            "I reviewed sample archive sizing tradeoffs this week.",
            "I reviewed sample archive sizing tradeoffs this week.",
            "episode:recent-experience",
            Utc.with_ymd_and_hms(2026, 4, 10, 11, 0, 0).unwrap(),
            "org",
            0.95,
            vec![],
            vec![],
            Provenance::agent_observation("episode:recent-experience"),
        )
        .await
        .expect("seed recent experience fact");

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "requirements created July 2025".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble temporal requirement query");

    assert!(items.iter().any(|item| item.fact_id == july_fact_id));
    assert!(
        !items.iter().any(|item| item.fact_id == august_noise_id),
        "out-of-window year-only lexical noise should not survive explicit month/year queries"
    );
    assert!(
        !items
            .iter()
            .any(|item| item.fact_id == recent_experience_id),
        "recent experience should not be appended for query-driven retrieval"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.rationale.contains("supplemental experience")),
        "query-driven retrieval should not append supplemental experience items"
    );
}

#[tokio::test]
async fn test_explicit_month_year_query_drops_out_of_window_summary_without_temporal_support() {
    let service = common::make_service().await;

    service
        .add_fact(
            "note",
            "October 2025 operations summary: Platform 2.3 Patch 4 was approved for rollout.",
            "October 2025 operations summary: Platform 2.3 Patch 4 was approved for rollout.",
            "episode:october-summary",
            Utc.with_ymd_and_hms(2025, 10, 13, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:october-summary"),
        )
        .await
        .expect("seed october summary fact");

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Platform planning notes July 2025".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble explicit month/year query");

    assert!(
        items.is_empty(),
        "when only out-of-window summaries exist without July 2025 support, explicit month/year query should return empty instead of October noise: {items:?}"
    );
}

#[tokio::test]
async fn test_query_prefers_matching_episode_content_over_irrelevant_fact_fallback() {
    use memory_mcp::models::IngestRequest;

    let service = common::make_service().await;
    let july = Utc.with_ymd_and_hms(2025, 7, 14, 10, 0, 0).unwrap();

    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "requirement".to_string(),
                source_id: "july-platform-planning".to_string(),
                content: "Platform planning notes July 2025: release scope, integrations, and response workflow updates.".to_string(),
                t_ref: july,
                scope: "org".to_string(),
                project: None,
                t_ingested: Some(july),
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest July episode");

    service
        .add_fact(
            "note",
            "July 2025 platform licensing notes for renewal workflow.",
            "July 2025 platform licensing notes for renewal workflow.",
            "episode:july-licensing-noise",
            Utc.with_ymd_and_hms(2025, 7, 13, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:july-licensing-noise"),
        )
        .await
        .expect("seed unrelated fact noise");

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Platform planning notes July 2025".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble context");

    let first = items.first().expect("expected at least one result");
    assert!(
        first.fact_id.starts_with("episode_fallback:"),
        "expected episode fallback item, got {first:?}"
    );
    assert_eq!(first.source_episode, episode_id);
    assert_eq!(first.retrieval_tier.as_deref(), Some("fallback"));
}

#[tokio::test]
async fn test_assemble_context_returns_extracted_meeting_summary_fact_for_matching_query() {
    let service = common::make_service().await;
    let t_ref = Utc.with_ymd_and_hms(2026, 4, 13, 9, 0, 0).unwrap();

    let architecture_episode = service
        .ingest(
            IngestRequest {
                source_type: "meeting_summary".to_string(),
                source_id: "meeting-archive-scan-2026-04-13-01-architecture".to_string(),
                content: "Product architecture and deployment decisions:\n\n- Decision: Use a single umbrella product identifier for the product suite.\n- Decision: Keep legacy on-premise identifier only temporarily.\n- Decision: Standardize release timelines across channels.".to_string(),
                t_ref,
                scope: "org".to_string(),
                project: Some("cloud-products".to_string()),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest architecture episode");
    service
        .extract(&architecture_episode, None, None)
        .await
        .expect("extract architecture episode");

    let documentation_episode = service
        .ingest(
            IngestRequest {
                source_type: "meeting_summary".to_string(),
                source_id: "meeting-archive-scan-2026-04-13-10-documentation".to_string(),
                content: "Documentation and localization facts for product materials:\n\n- Fact: Help kickoff is open for documentation and localization work; naming details need alignment.\n- Fact: Docs team is asking for final terminology in both languages.".to_string(),
                t_ref,
                scope: "org".to_string(),
                project: Some("cloud-products".to_string()),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest documentation episode");
    service
        .extract(&documentation_episode, None, None)
        .await
        .expect("extract documentation episode");

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "help kickoff documentation localization terminology".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
            budget: 10,
            project: Some("cloud-products".to_string()),
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble context for documentation query");

    let first = items.first().expect("expected at least one result");
    assert!(
        !first.fact_id.starts_with("episode_fallback:"),
        "expected an extracted fact to outrank raw episode fallback, got {first:?}"
    );
    assert_eq!(first.source_episode, documentation_episode);
    assert!(
        first.content.contains("documentation and localization")
            || first.content.contains("final terminology"),
        "expected documentation-specific extracted fact, got {first:?}"
    );
}

#[tokio::test]
async fn test_assemble_context_extracts_facts_from_ad_hoc_markdown_summary() {
    let service = common::make_service().await;
    let t_ref = Utc.with_ymd_and_hms(2026, 4, 13, 10, 0, 0).unwrap();

    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "ad-hoc".to_string(),
                source_id: "summary-archive-2026-04-13-adhoc-01".to_string(),
                content: "# September 2025 program summary\n\n## Launch Activities\n- Regional launch in South market (September 30)\n- Response logging discussion (September 30)\n\n## Decisions Made\n1. Regional launch in South market approved for September 30.\n2. Response logging rollout approved for September 30.\n\n## Pending Items\n1. Complete global launch follow-up.\n2. Continue platform 1.5 development.".to_string(),
                t_ref,
                scope: "org".to_string(),
                project: Some("program-rollout".to_string()),
                t_ingested: Some(t_ref),
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest ad-hoc summary episode");

    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .expect("extract ad-hoc summary episode");

    assert!(
        extraction
            .facts
            .iter()
            .any(|fact| fact.fact_type == "decision"),
        "expected decision facts from markdown summary, got {:?}",
        extraction.facts
    );
    assert!(
        extraction.facts.iter().any(|fact| fact.fact_type == "note"),
        "expected note facts from pending section, got {:?}",
        extraction.facts
    );

    let launch_items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "regional launch approved south market september 30".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
            budget: 10,
            project: Some("program-rollout".to_string()),
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble context for launch query");

    let first_launch = launch_items.first().expect("expected launch result");
    assert_eq!(first_launch.source_episode, episode_id);
    assert!(
        first_launch
            .content
            .contains("Regional launch in South market"),
        "expected launch-relevant context from the imported summary, got {first_launch:?}"
    );

    let development_items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "continue platform 1.5 development".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
            budget: 10,
            project: Some("program-rollout".to_string()),
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble context for development query");

    let first_development = development_items
        .first()
        .expect("expected development result");
    assert!(
        !first_development.fact_id.starts_with("episode_fallback:"),
        "expected extracted fact for development query, got {first_development:?}"
    );
    assert_eq!(first_development.source_episode, episode_id);
    assert!(
        first_development
            .content
            .contains("Continue platform 1.5 development."),
        "expected pending-item fact, got {first_development:?}"
    );
}

#[tokio::test]
async fn test_assemble_context_prefers_extracted_fact_from_thematic_markdown_summary() {
    let service = common::make_service().await;
    let t_ref = Utc.with_ymd_and_hms(2026, 4, 13, 10, 30, 0).unwrap();

    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "ad-hoc".to_string(),
                source_id: "summary-archive-2026-04-13-adhoc-02".to_string(),
                content: "# Monthly coordination summary\n\n## Release Activities\n- Finalize phased rollout checklist.\n- Publish support handoff notes.\n\n## Capacity Planning\n- Prepare archive review for next quarter.".to_string(),
                t_ref,
                scope: "org".to_string(),
                project: Some("general-ops".to_string()),
                t_ingested: Some(t_ref),
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest thematic ad-hoc summary episode");

    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .expect("extract thematic ad-hoc summary episode");

    assert!(
        extraction.facts.len() >= 3,
        "expected thematic markdown summary to produce line-level facts, got {extraction:?}"
    );

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "finalize phased rollout checklist".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
            budget: 10,
            project: Some("general-ops".to_string()),
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble context for thematic summary query");

    let first = items.first().expect("expected thematic summary result");
    assert!(
        !first.fact_id.starts_with("episode_fallback:"),
        "expected extracted fact to outrank raw episode fallback, got {first:?}"
    );
    assert_eq!(first.source_episode, episode_id);
    assert!(
        first.content.contains("Release Activities")
            && first.content.contains("Finalize phased rollout checklist"),
        "expected contextualized thematic section fact, got {first:?}"
    );
}

#[tokio::test]
async fn test_assemble_context_keeps_extracted_presentation_summary_facts_for_broad_query() {
    let service = common::make_service().await;
    let t_ref = Utc.with_ymd_and_hms(2026, 4, 13, 11, 0, 0).unwrap();

    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "presentation_summary".to_string(),
                source_id: "launch-brief-summary-2026-04-13".to_string(),
                content: "Quarterly launch brief:\n- Suite Alpha, Suite Beta, and Suite Gamma launch on the shared platform in Q3 2026.\n- Technical preview is September 30, 2026, with general availability in late October 2026.\n- Roadmap adds external connectors, export automation, and staged rollout controls in Q4 2026.\n- Following wave adds desktop agent support, workflow versioning, and graphical rules in H1 2027.\n- Licensing uses pooled capacity units and unified product identification.".to_string(),
                t_ref,
                scope: "org".to_string(),
                project: Some("launch-program".to_string()),
                t_ingested: Some(t_ref),
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest presentation summary episode");

    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .expect("extract presentation summary episode");

    assert!(
        extraction.facts.len() >= 5,
        "expected line-level facts from presentation summary, got {extraction:?}"
    );

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "suite alpha beta gamma shared platform q3 2026 roadmap rollout controls versioning graphical rules".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
            budget: 10,
            project: Some("launch-program".to_string()),
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        compact: false,
})
        .await
        .expect("assemble context for broad presentation summary query");

    let first = items.first().expect("expected presentation summary result");
    assert!(
        !first.fact_id.starts_with("episode_fallback:"),
        "expected extracted facts to remain ahead of raw summary fallback for broad query, got {first:?}"
    );
    assert_eq!(first.source_episode, episode_id);
    assert!(
        items.iter().all(|item| item.source_episode == episode_id),
        "expected returned items to stay anchored to the same extracted summary episode, got {items:?}"
    );
}

#[tokio::test]
async fn test_assemble_context_prefers_anchor_backed_result_over_generic_overlap_noise() {
    let service = common::make_service().await;
    let cutoff = Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap();

    let anchor_fact_id = service
        .add_fact(
            "note",
            "OpenShift migration exception approved for the platform cluster.",
            "OpenShift migration exception approved for the platform cluster.",
            "episode:openshift-anchor",
            Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:openshift-anchor"),
        )
        .await
        .expect("seed anchor fact");

    service
        .add_fact(
            "note",
            "Rollout controls checklist updated for regional launch.",
            "Rollout controls checklist updated for regional launch.",
            "episode:generic-rollout-1",
            Utc.with_ymd_and_hms(2026, 4, 12, 9, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:generic-rollout-1"),
        )
        .await
        .expect("seed generic rollout fact 1");

    service
        .add_fact(
            "note",
            "Rollout controls timeline updated for support workflow.",
            "Rollout controls timeline updated for support workflow.",
            "episode:generic-rollout-2",
            Utc.with_ymd_and_hms(2026, 4, 11, 9, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:generic-rollout-2"),
        )
        .await
        .expect("seed generic rollout fact 2");

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "openshift rollout controls".to_string(),
            scope: "org".to_string(),
            as_of: Some(cutoff),
            budget: 3,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        })
        .await
        .expect("assemble anchor-aware context");

    let first = items.first().expect("expected at least one result");
    assert_eq!(
        first.fact_id, anchor_fact_id,
        "distinctive anchor term should outrank generic overlap noise: {items:?}"
    );
    assert!(
        first.rationale.contains("alignment="),
        "rationale should expose query alignment metadata once anchor-aware ranking is enabled: {}",
        first.rationale
    );
}
