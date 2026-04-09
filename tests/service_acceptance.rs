use chrono::{TimeZone, Utc};
use memory_mcp::models::{AccessContext, EntityCandidate, IngestRequest, InvalidateRequest};
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
        })
        .await
        .expect("assemble");
    assert!(!context.is_empty());
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
                }],
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
            "private-hr",
            0.9,
            vec!["entity:a".to_string()],
            vec!["hr.salary".to_string()],
            serde_json::json!({"source_episode": "episode:hr"}),
        )
        .await
        .expect("add_fact");

    let context = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Salary".to_string(),
            scope: "private-hr".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: Some(memory_mcp::models::AccessPayload {
                allowed_scopes: Some(vec!["private-hr".to_string()]),
                allowed_tags: Some(vec!["deal.pipeline".to_string()]),
                caller_id: None,
                session_vars: None,
                transport: None,
                content_type: None,
                cross_scope_allow: None,
            }),
        })
        .await
        .expect("assemble");
    assert!(context.is_empty());
}

#[tokio::test]
async fn test_graph_intro_chain() {
    let service = common::make_service().await;
    let alice = service.resolve_person("Alice").await.expect("alice");
    let bob = service.resolve_person("Bob").await.expect("bob");
    let openai = service.resolve_company("OpenAI").await.expect("openai");

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
    let alice = service.resolve_person("Alice").await.expect("alice");
    let bob = service.resolve_person("Bob").await.expect("bob");
    let openai = service.resolve_company("OpenAI").await.expect("openai");

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

    let alice_id = service.resolve_person("Alice Smith").await.expect("alice");
    let bob_id = service.resolve_person("Bob Jones").await.expect("bob");
    let carol_id = service.resolve_person("Carol White").await.expect("carol");
    let diana_id = service.resolve_person("Diana Prince").await.expect("diana");

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
            serde_json::json!({"source_episode": episode_id}),
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
                }],
            },
            None,
        )
        .await
        .expect("explain");

    let serialized = serde_json::to_value(&explanation[0]).expect("serialize explain item");
    let graph_insights = serialized
        .get("graphInsights")
        .expect("explain should expose graphInsights");
    let hub_entities = graph_insights
        .get("hubEntities")
        .and_then(serde_json::Value::as_array)
        .expect("graphInsights.hubEntities should be an array");
    let surprising_connections = graph_insights
        .get("surprisingConnections")
        .and_then(serde_json::Value::as_array)
        .expect("graphInsights.surprisingConnections should be an array");

    assert!(hub_entities.iter().any(|hub| {
        hub.get("entityId") == Some(&serde_json::json!(bob_id))
            && hub.get("degree") == Some(&serde_json::json!(2))
    }));
    assert!(surprising_connections.iter().any(|connection| {
        connection.get("sourceEntityId") == Some(&serde_json::json!(alice_id))
            && connection.get("targetEntityId") == Some(&serde_json::json!(carol_id))
            && connection.get("hopCount") == Some(&serde_json::json!(2))
    }));
}

#[tokio::test]
async fn test_relate_repeated_write_invalidates_previous_edge_version() {
    let (service, db_client) = common::make_service_with_client().await;
    let alice = service.resolve_person("Alice").await.expect("alice");
    let bob = service.resolve_person("Bob").await.expect("bob");

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
            serde_json::json!({"source_episode": episode_id}),
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
async fn test_cbor_round_trip() {
    let service = common::make_service().await;
    let payload = serde_json::json!({
        "datetime": "2026-01-01T00:00:00Z",
        "record_id": "episode:abc123",
        "decimal": "1000000.50"
    });

    let restored = service.cbor_round_trip(&payload).expect("cbor");
    assert_eq!(restored["record_id"], payload["record_id"]);
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
            serde_json::json!({"source_episode": "episode:vars"}),
        )
        .await
        .expect("add_fact");

    let access = AccessContext {
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
            serde_json::json!({"source_episode": "episode:035d8d47"}),
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
            serde_json::json!({"source_episode": "episode:035d8d47"}),
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
            serde_json::json!({"source_episode": "episode:8de581d5"}),
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
            serde_json::json!({"source_episode": "episode:degree-answer"}),
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
            serde_json::json!({"source_episode": "episode:degree-generic"}),
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
            serde_json::json!({"source_episode": "episode:graduate-generic"}),
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
            .get("retrievalTier")
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
            serde_json::json!({"source_episode": "episode:temporal-tier"}),
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
            .get("retrievalTier")
            .and_then(serde_json::Value::as_str),
        Some("temporal")
    );
    assert!(
        item.rationale.contains("tier=temporal"),
        "rationale should include temporal tier metadata, got: {}",
        item.rationale
    );
}
