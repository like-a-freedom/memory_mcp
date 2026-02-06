use chrono::{TimeZone, Utc};
use serde_json::Value;

use memory_mcp::models::{AccessContext, EntityCandidate, IngestRequest, InvalidateRequest};

mod common;

#[tokio::test]
async fn test_ingest_extract_and_assemble() {
    let service = common::make_service();
    let now = Utc::now();
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "email".to_string(),
                source_id: "MSG-201".to_string(),
                content: "ARR вырос до $3M. Сделаю до пятницы.".to_string(),
                t_ref: now - chrono::Duration::days(1),
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");

    let extraction = service.extract(&episode_id, None).await.expect("extract");
    let facts = extraction["facts"].as_array().unwrap();
    assert!(facts.iter().any(|fact| fact["type"] == "metric"));
    assert!(facts.iter().any(|fact| fact["type"] == "promise"));

    let context = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(now + chrono::Duration::seconds(1)),
            budget: 5,
            access: None,
        })
        .await
        .expect("assemble");
    assert!(!context.is_empty());
}

#[tokio::test]
async fn test_resolve_aliases() {
    let service = common::make_service();
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
    let service = common::make_service();
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "email".to_string(),
                source_id: "MSG-202".to_string(),
                content: "ARR is $1M".to_string(),
                t_ref: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");
    let extraction = service.extract(&episode_id, None).await.expect("extract");
    let fact_id = extraction["facts"][0]["fact_id"].as_str().unwrap();

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
            access: None,
        })
        .await
        .expect("assemble");
    assert!(context.is_empty());

    let explanation = service
        .explain(
            memory_mcp::models::ExplainRequest {
                context_pack: vec![memory_mcp::models::ExplainItem {
                    content: "ARR is $1M".to_string(),
                    quote: "ARR is $1M".to_string(),
                    source_episode: episode_id.clone(),
                }],
            },
            None,
        )
        .await
        .expect("explain");
    assert_eq!(explanation[0]["source_episode"], Value::String(episode_id));
}

#[tokio::test]
async fn test_policy_tag_filtering() {
    let service = common::make_service();
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
    let service = common::make_service();
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
    let service = common::make_service();
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
async fn test_cbor_round_trip() {
    let service = common::make_service();
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
    let service = common::make_service();
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
    let service = common::make_service();
    let t = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();

    // Add facts with various content that should match multi-word queries
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

    // Test 1: Multi-word query where words are non-adjacent in content
    let ctx = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
query: "Delta Enrollment".to_string(),
                scope: "org".to_string(),
                as_of: None, // defaults to now(), ensuring t_ingested <= cutoff
                budget: 10,
                access: None,
            })
            .await
            .expect("assemble Delta Enrollment");
    assert!(
        !ctx.is_empty(),
        "Delta Enrollment: expected matches for non-adjacent multi-word query"
    );

    // Test 2: Query with episode refs and OR — should be preprocessed and still find results
    let ctx2 = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "fleet checklist certs tokens ports pending checklist episode:035d8d47".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await
        .expect("assemble mobile checklist");
    assert!(
        !ctx2.is_empty(),
        "mobile checklist query with episode ref: expected matches"
    );

    // Test 3: Query with quotes
    let ctx3 = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: r#"release notes v2.2 Module "Module_6.0_Archive - Component v2.1.md" episode:8de581d5"#.to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await
        .expect("assemble Module changelog");
    assert!(
        !ctx3.is_empty(),
        "Module changelog query with quotes and episode ref: expected matches"
    );
}
