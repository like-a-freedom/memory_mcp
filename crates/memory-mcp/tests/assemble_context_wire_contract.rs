mod common;

use chrono::Utc;
use memory_mcp::models::IngestRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::storage::DbClient;
use memory_mcp::tools::assemble_context::assemble_context;
use memory_mcp::tools::context::ToolContext;
use memory_mcp::tools::params::AssembleContextParams;

const EMPTY_GUIDANCE: &str =
    "No sufficient matching context was found. Refine the query or verify primary sources.";

fn params(query: &str, compact: bool) -> AssembleContextParams {
    AssembleContextParams {
        query: query.to_string(),
        fact_types: Vec::new(),
        as_of: String::new(),
        valid_at: None,
        known_at: None,
        known_revision: None,
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

#[tokio::test]
async fn exact_knowledge_revision_hides_a_later_invalidation_without_future_leakage() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = common::seed_fact_at(
        &memory.service,
        "temporal-wire",
        "The archive key is cobalt during November maintenance.",
        chrono::DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .expect("valid fixture time")
            .with_timezone(&Utc),
    )
    .await;
    let created_revision = latest_revision(memory.db_client.as_ref()).await;

    memory_mcp::tools::invalidate(
        &memory.service,
        memory_mcp::tools::params::InvalidateParams {
            fact_id: fact_id.clone(),
            reason: "The key was rotated".into(),
            t_invalid: "2026-11-05T00:00:00Z".into(),
        },
    )
    .await
    .expect("invalidate fact");
    let closed_revision = latest_revision(memory.db_client.as_ref()).await;
    assert!(closed_revision > created_revision);

    let mut before_params = params("archive key cobalt", true);
    before_params.valid_at = Some("2026-11-10T00:00:00Z".into());
    before_params.known_revision = Some(created_revision);
    let before = assemble_context(&memory.service, before_params)
        .await
        .expect("read before the closure revision");
    let before_items = before.result.as_array().expect("context item array");
    assert!(before_items.iter().any(|item| item["fact_id"] == fact_id));
    assert_eq!(
        before.context.as_ref().unwrap().snapshot.known_revision,
        created_revision
    );
    assert_eq!(
        before.context.as_ref().unwrap().completeness,
        memory_mcp::models::ContextCompleteness::Partial
    );
    assert!(!before.context.as_ref().unwrap().candidate_scan_truncated);

    let mut after_params = params("archive key cobalt", true);
    after_params.valid_at = Some("2026-11-10T00:00:00Z".into());
    after_params.known_revision = Some(closed_revision);
    let after = assemble_context(&memory.service, after_params)
        .await
        .expect("read at the closure revision");
    let after_items = after.result.as_array().expect("context item array");
    assert!(!after_items.iter().any(|item| item["fact_id"] == fact_id));
    assert_eq!(
        after.context.as_ref().unwrap().snapshot.known_revision,
        closed_revision
    );
}

#[tokio::test]
async fn known_at_includes_all_commits_with_the_same_recorded_timestamp() {
    let memory = common::TestMemory::new(false).await;
    let recorded_at = "2099-12-31T23:59:59Z";
    memory
        .db_client
        .query(
            "UPDATE knowledge_revision_clock:active SET last_recorded_at = type::datetime($recorded_at)",
            Some(serde_json::json!({ "recorded_at": recorded_at })),
            "org",
        )
        .await
        .expect("seed a shared recorded-time boundary");

    let first_id = common::seed_fact_at(
        &memory.service,
        "timestamp-boundary-first",
        "The archive key stores the shared boundary detail alpha.",
        chrono::DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .expect("valid fixture time")
            .with_timezone(&Utc),
    )
    .await;
    let second_id = common::seed_fact_at(
        &memory.service,
        "timestamp-boundary-second",
        "The archive key stores the shared boundary detail beta.",
        chrono::DateTime::parse_from_rfc3339("2026-11-02T00:00:00Z")
            .expect("valid fixture time")
            .with_timezone(&Utc),
    )
    .await;
    let expected_revision = latest_revision(memory.db_client.as_ref()).await;

    let mut request = params("archive key shared boundary detail", true);
    request.valid_at = Some("2026-11-10T00:00:00Z".into());
    request.known_at = Some(recorded_at.into());
    let response = assemble_context(&memory.service, request)
        .await
        .expect("timestamp-bounded context");

    assert_eq!(
        response.context.as_ref().unwrap().snapshot.known_revision,
        expected_revision
    );
    let items = response.result.as_array().expect("context item array");
    assert!(items.iter().any(|item| item["fact_id"] == first_id));
    assert!(items.iter().any(|item| item["fact_id"] == second_id));
}

#[tokio::test]
async fn historical_lexical_read_does_not_select_from_mutable_index_keys() {
    let memory = common::TestMemory::new(false).await;
    let fact_id = common::seed_fact_at(
        &memory.service,
        "mutable-index-selector",
        "The source text describes a stable archived note.",
        chrono::DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
            .expect("valid fixture time")
            .with_timezone(&Utc),
    )
    .await;
    let revision = latest_revision(memory.db_client.as_ref()).await;
    memory
        .db_client
        .update(
            &fact_id,
            serde_json::json!({ "index_keys": ["mutable-selector-token"] }),
            "org",
            &[],
        )
        .await
        .expect("mutate reindexable selector projection");

    let mut request = params("mutable selector token", true);
    request.valid_at = Some("2026-11-10T00:00:00Z".into());
    request.known_revision = Some(revision);
    let response = assemble_context(&memory.service, request)
        .await
        .expect("revision-bounded lexical read");

    assert!(response.result.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn historical_episode_fallback_obeys_the_exact_knowledge_revision() {
    let memory = common::TestMemory::new(false).await;
    let valid_at = chrono::DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
        .expect("valid fixture time")
        .with_timezone(&Utc);
    let first_episode = memory
        .service
        .ingest(
            IngestRequest {
                source_type: "conversation".into(),
                source_id: "archive-initial".into(),
                content: "The archive key is north for the November maintenance window.".into(),
                t_ref: valid_at,
                t_ingested: None,
                policy_tags: Vec::new(),
                source_context: None,
            },
            None,
        )
        .await
        .expect("ingest initial source");
    let before_correction_revision = latest_revision(memory.db_client.as_ref()).await;
    let correction_episode = memory
        .service
        .ingest(
            IngestRequest {
                source_type: "conversation".into(),
                source_id: "archive-correction".into(),
                content: "The archive key is south for the November maintenance window.".into(),
                t_ref: valid_at,
                t_ingested: None,
                policy_tags: Vec::new(),
                source_context: None,
            },
            None,
        )
        .await
        .expect("ingest later correction");
    let latest_revision = latest_revision(memory.db_client.as_ref()).await;

    let mut before_params = params("archive key November maintenance", true);
    before_params.valid_at = Some("2026-11-10T00:00:00Z".into());
    before_params.known_revision = Some(before_correction_revision);
    let before = assemble_context(&memory.service, before_params)
        .await
        .expect("read before correction was known");
    let before_items = before.result.as_array().expect("context item array");
    assert!(before_items.iter().any(|item| {
        item["source_episode"] == first_episode
            && item["fact_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("episode_fallback:"))
    }));
    assert!(
        !before_items
            .iter()
            .any(|item| item["source_episode"] == correction_episode)
    );

    let mut after_params = params("archive key November maintenance", true);
    after_params.valid_at = Some("2026-11-10T00:00:00Z".into());
    after_params.known_revision = Some(latest_revision);
    let after = assemble_context(&memory.service, after_params)
        .await
        .expect("read after correction became known");
    let after_items = after.result.as_array().expect("context item array");
    assert!(
        after_items
            .iter()
            .any(|item| item["source_episode"] == correction_episode)
    );
}

#[tokio::test]
async fn exact_snapshot_between_ingest_and_extract_keeps_the_episode_fallback() {
    let memory = common::TestMemory::new(false).await;
    let valid_at = chrono::DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z")
        .expect("valid fixture time")
        .with_timezone(&Utc);
    let episode_id = memory
        .service
        .ingest(
            IngestRequest {
                source_type: "conversation".into(),
                source_id: "intermediate-stage-source".into(),
                content: "subject: ARR; dimension: ARR; value: intermediate".into(),
                t_ref: valid_at,
                t_ingested: None,
                policy_tags: Vec::new(),
                source_context: None,
            },
            None,
        )
        .await
        .expect("commit the ingest stage");
    let ingest_revision = latest_revision(memory.db_client.as_ref()).await;

    let extracted =
        ExtractCapability::extract_from_service(&memory.service, &episode_id, None, None)
            .await
            .expect("commit the later extraction stage");
    assert!(
        !extracted.facts.is_empty(),
        "the later stage must publish facts"
    );
    let extraction_revision = latest_revision(memory.db_client.as_ref()).await;
    assert!(extraction_revision > ingest_revision);

    let mut request = params("ARR intermediate", true);
    request.valid_at = Some("2026-11-01T00:00:00Z".into());
    request.known_revision = Some(ingest_revision);
    let before_extraction = assemble_context(&memory.service, request)
        .await
        .expect("read the exact snapshot between stages");
    let items = before_extraction
        .result
        .as_array()
        .expect("context item array");
    assert!(items.iter().any(|item| {
        item["source_episode"] == episode_id
            && item["fact_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("episode_fallback:"))
    }));
    assert!(items.iter().all(|item| {
        item["fact_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("episode_fallback:"))
    }));
}

#[tokio::test]
async fn legacy_episode_remains_current_but_is_unknown_to_exact_history() {
    let memory = common::TestMemory::new(false).await;
    memory
        .db_client
        .query(
            "CREATE episode:⟨legacy-unknown⟩ SET \
             episode_id = 'episode:legacy-unknown', \
             source_type = 'conversation', \
             source_id = 'legacy/archive', \
             content = 'The legacy archive marker is violet for November.', \
             t_ref = type::datetime('2026-11-01T00:00:00Z'), \
             t_ingested = type::datetime('2026-11-01T00:00:00Z'), \
             scope = 'org', visibility_scope = 'org', policy_tags = []",
            None,
            "org",
        )
        .await
        .expect("seed a pre-revision legacy episode");

    let current = assemble_context(
        &memory.service,
        params("legacy archive marker violet", true),
    )
    .await
    .expect("current retrieval keeps legacy compatibility");
    let current_items = current.result.as_array().expect("current item array");
    assert!(current_items.iter().any(|item| {
        item["source_episode"] == "episode:legacy-unknown"
            && item["fact_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("episode_fallback:"))
    }));

    let mut historical_params = params("legacy archive marker violet", true);
    historical_params.valid_at = Some("2026-11-21T00:00:00Z".into());
    historical_params.known_revision = Some(0);
    let historical = assemble_context(&memory.service, historical_params)
        .await
        .expect("exact historical read excludes unknown provenance");
    let historical_items = historical.result.as_array().expect("historical item array");
    assert!(historical_items.is_empty());
}

#[tokio::test]
async fn temporal_read_captures_default_request_time_once() {
    let memory = common::TestMemory::new(false).await;
    let result = memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_temporal_from_service(
        &memory.service,
        memory_mcp::models::AssembleContextRequest {
            query: "no matching fixture content".into(),
            fact_types: Vec::new(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: true,
        },
        memory_mcp::models::TemporalContextQuery::default(),
    )
    .await
    .expect("resolve the default temporal snapshot");

    let snapshot = result
        .metadata
        .expect("historical snapshot metadata")
        .snapshot;
    assert_eq!(
        snapshot.valid_at, snapshot.known_at,
        "both default axes must use one request-time capture"
    );
}

#[tokio::test]
async fn historical_response_names_each_unversioned_selector_it_omits() {
    let memory = common::TestMemory::new(false).await;
    let mut request = params("unmatched historical selector query", true);
    request.valid_at = Some("2026-11-21T00:00:00Z".into());
    request.known_revision = Some(0);
    let response = assemble_context(&memory.service, request)
        .await
        .expect("produce a bounded historical response");

    let metadata = response.context.expect("historical completeness metadata");
    assert_eq!(
        metadata.omitted_channels,
        [
            "alias expansion",
            "experience, source episode siblings and source context neighborhoods",
            "triples, graph edges and community summaries",
            "semantic ANN and mutable embeddings",
            "mutable fact access heat",
            "legacy facts and episodes without committed revision provenance",
        ]
    );
    assert_eq!(
        metadata.completeness,
        memory_mcp::models::ContextCompleteness::Partial
    );
}

async fn latest_revision(database: &impl memory_mcp::storage::DbClient) -> i64 {
    let rows = database
        .query(
            "SELECT latest_revision FROM knowledge_revision_clock:active LIMIT 1",
            None,
            "org",
        )
        .await
        .expect("read revision clock");
    rows.as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row["latest_revision"].as_i64())
        .expect("revision clock row")
}

#[tokio::test]
async fn historical_large_history_replay_excludes_a_late_backdated_correction() {
    use chrono::{TimeDelta, TimeZone};

    let memory = common::TestMemory::new(false).await;
    let service = common::exposing_claims(memory.service).unwrap();
    let valid_at = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
    let original = common::ingest_episode(
        &service,
        "warehouse-ledger",
        "subject: Copper Warehouse\ndimension: capacity\nvalue: 42 crates for Copper Warehouse",
    )
    .await;
    for index in 0..250 {
        common::seed_fact_at(
            &service,
            "wire-contract",
            &format!("subject: Depot {index:03}\ndimension: capacity\nvalue: 16 crates"),
            valid_at - TimeDelta::seconds(index + 1),
        )
        .await;
    }

    let request = |revision| {
        let mut request = params("What capacity was recorded for Copper Warehouse?", true);
        request.valid_at = Some(valid_at.to_rfc3339());
        request.known_revision = revision;
        request
    };
    let checkpoint = assemble_context(&service, request(None))
        .await
        .unwrap()
        .context
        .expect("revision-bounded metadata")
        .snapshot
        .known_revision;
    let original_read = assemble_context(&service, request(Some(checkpoint)))
        .await
        .unwrap();
    let original_metadata = original_read.context.expect("revision-bounded metadata");
    assert!(original_metadata.candidate_scan_truncated);
    assert_eq!(
        original_metadata.completeness,
        memory_mcp::models::ContextCompleteness::Partial,
    );
    assert_eq!(original_read.result[0]["source_episode"], original);
    let revision = original_metadata.snapshot.known_revision;

    let correction = common::ingest_episode(
        &service,
        "warehouse-correction",
        "subject: Copper Warehouse\ndimension: capacity\nvalue: 48 crates, correction reported for Copper Warehouse",
    )
    .await;

    let replay = assemble_context(&service, request(Some(revision)))
        .await
        .unwrap();
    assert_eq!(replay.result, original_read.result);
    assert_eq!(replay.context.as_ref(), Some(&original_metadata));

    let latest = assemble_context(&service, request(None)).await.unwrap();
    assert!(
        latest
            .result
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["source_episode"] == correction)
    );
}
