//! Public revision-bounded reads must bind a named query to its subject.
mod common;
use chrono::{TimeZone, Utc};
use memory_mcp::models::{AssembleContextRequest, TemporalContextQuery};
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;

fn request(query: &str) -> AssembleContextRequest {
    AssembleContextRequest {
        query: query.into(),
        as_of: None,
        budget: 5,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: true,
    }
}
#[tokio::test]
async fn historical_fact_with_shared_predicate_cannot_answer_another_subject() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).unwrap();
    common::ingest_episode(
        &service,
        "unrelated-capacity",
        "subject: Copper Warehouse; dimension: capacity; value: 42 crates for Copper Warehouse",
    )
    .await;
    let observed = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("What capacity was recorded for Granite Warehouse?"),
        TemporalContextQuery {
            valid_at: Some(Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap()),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    assert!(
        observed.items.is_empty(),
        "shared predicate and one name token are not evidence of the requested subject: {:?}",
        observed.items
    );
}

#[tokio::test]
async fn historical_natural_question_preserves_fact_identity_and_current_role() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).unwrap();
    let episode = common::ingest_episode(
        &service,
        "capacity-record",
        "subject: Copper Warehouse; dimension: capacity; value: 42 crates for Copper Warehouse",
    )
    .await;
    let observed = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("What capacity was recorded for Copper Warehouse?"),
        TemporalContextQuery {
            valid_at: Some(Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap()),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    assert!(
        observed
            .items
            .iter()
            .any(|item| item.source_episode == episode
                && item.fact_id.starts_with("fact:")
                && item.evidence_status
                    == Some(memory_mcp::models::SnapshotEvidenceStatus::Current)),
        "a visible extracted claim must not degrade into an episode with unknown role: {:?}",
        observed.items
    );
}

#[tokio::test]
async fn historical_multi_subject_question_does_not_require_combined_name() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).unwrap();
    let episode = common::ingest_episode(
        &service,
        "multi-subject-record",
        "subject: Copper Warehouse; dimension: capacity; value: 42 crates for Copper Warehouse",
    )
    .await;
    let observed = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("What capacity was recorded for Copper Warehouse and Granite Warehouse?"),
        TemporalContextQuery {
            valid_at: Some(Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap()),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    assert!(
        observed
            .items
            .iter()
            .any(|item| item.source_episode == episode && item.fact_id.starts_with("fact:")),
        "separate subject names are not one concatenated entity: {:?}",
        observed.items
    );
}

#[tokio::test]
async fn historical_similar_explicit_subjects_do_not_create_false_conflict() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).unwrap();
    common::ingest_episode(&service, "west-record", "subject: Copper Warehouse West\ndimension: capacity\nvalue: 81 crates for Copper Warehouse West").await;
    let target = common::ingest_episode(&service, "east-record", "subject: Copper Warehouse East\ndimension: capacity\nvalue: 18 crates for Copper Warehouse East").await;
    let observed = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("What capacity was recorded for Copper Warehouse East?"),
        TemporalContextQuery {
            valid_at: Some(Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap()),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    assert!(
        observed
            .items
            .iter()
            .any(|item| item.source_episode == target
                && item.evidence_status
                    == Some(memory_mcp::models::SnapshotEvidenceStatus::Current)),
        "different explicit subjects must not share a contradiction slot: {:?}",
        observed.items
    );
}

async fn assert_unrelated_declared_subject_is_excluded(content: &str, query: &str) {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).unwrap();
    common::ingest_episode(&service, "declared-subject-control", content).await;
    let result = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request(query),
        TemporalContextQuery {
            valid_at: Some(Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap()),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    assert!(
        result.items.is_empty(),
        "a mention or shared name prefix does not establish the declared subject: {:?}",
        result.items
    );
}
#[tokio::test]
async fn historical_mentioned_subject_cannot_replace_declared_subject() {
    assert_unrelated_declared_subject_is_excluded("subject: Copper Warehouse West\ndimension: capacity\nvalue: 81 crates moved from Copper Warehouse East","What capacity was recorded for Copper Warehouse East?").await;
}
#[tokio::test]
async fn historical_numeric_subject_identifiers_remain_distinct() {
    assert_unrelated_declared_subject_is_excluded(
        "subject: Depot 12\ndimension: capacity\nvalue: 50 crates for Depot 12",
        "What capacity was recorded for Depot 13?",
    )
    .await;
}
#[tokio::test]
async fn historical_explicit_subject_does_not_trust_prior_fuzzy_alias() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).unwrap();
    common::ingest_episode(&service,"original-west","subject: Copper Warehouse West\ndimension: capacity\nvalue: 81 crates for Copper Warehouse West").await;
    common::ingest_episode(
        &service,
        "fuzzy-east-mention",
        "Copper Warehouse East reports 13 crates.",
    )
    .await;
    let target=common::ingest_episode(&service,"explicit-east","subject: Copper Warehouse East\ndimension: capacity\nvalue: 18 crates for Copper Warehouse East").await;
    let result = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("What capacity was recorded for Copper Warehouse East?"),
        TemporalContextQuery {
            valid_at: Some(Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap()),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    assert!(
        result.items.iter().any(|item| item.source_episode == target
            && item.evidence_status == Some(memory_mcp::models::SnapshotEvidenceStatus::Current)),
        "fuzzy alias cannot override a declared subject: {:?}",
        result.items
    );
}

#[tokio::test]
async fn historical_ranking_does_not_use_future_fts_statistics() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).unwrap();
    common::ingest_episode(
        &service,
        "fts-unit-1",
        "subject: Unit 1\ndimension: signal\nvalue: turquoise turquoise turquoise magenta",
    )
    .await;
    common::ingest_episode(
        &service,
        "fts-unit-2",
        "subject: Unit 2\ndimension: signal\nvalue: turquoise magenta magenta magenta",
    )
    .await;
    for i in 0..9 {
        common::ingest_episode(
            &service,
            &format!("plain-{i}"),
            &format!("subject: Plain {i}\ndimension: signal\nvalue: plain plain plain plain"),
        )
        .await;
    }
    let mut req = request("turquoise magenta");
    req.budget = 1;
    let time = Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap();
    let before = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        req.clone(),
        TemporalContextQuery {
            valid_at: Some(time),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    let revision = before.metadata.as_ref().unwrap().snapshot.known_revision;
    for i in 0..10 {
        common::ingest_episode(
            &service,
            &format!("future-turquoise-{i}"),
            &format!("subject: Future {i}\ndimension: signal\nvalue: turquoise"),
        )
        .await;
    }
    let after = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        req,
        TemporalContextQuery {
            valid_at: Some(time),
            known_at: None,
            known_revision: Some(revision),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        before.items.first().map(|x| &x.fact_id),
        after.items.first().map(|x| &x.fact_id),
        "future corpus statistics cannot reorder a fixed revision"
    );
}

#[tokio::test]
async fn historical_249_facts_fit_candidate_scan() {
    assert_historical_scan_boundary(249, 249, false).await;
}

async fn assert_historical_scan_boundary(seed_count: u32, expected_count: usize, truncated: bool) {
    use std::collections::BTreeSet;

    use chrono::TimeDelta;
    use memory_mcp::models::ContextCompleteness;

    let memory = common::TestMemory::new(false).await;
    let valid_at = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
    for index in 0..seed_count {
        common::seed_fact_at(
            &memory.service,
            "main",
            &format!("Independent inventory note {index:03}"),
            valid_at - TimeDelta::seconds(i64::from(index) + 1),
        )
        .await;
    }
    let result = AssembleContextCapability::assemble_context_temporal_from_service(
        &memory.service,
        AssembleContextRequest {
            budget: 250,
            ..request("")
        },
        TemporalContextQuery {
            valid_at: Some(valid_at),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();

    let metadata = result.metadata.expect("revision-bounded context metadata");
    assert_eq!(result.items.len(), expected_count);
    let distinct: BTreeSet<_> = result.items.iter().map(|item| &item.fact_id).collect();
    assert_eq!(distinct.len(), expected_count);
    assert_eq!(metadata.candidate_scan_truncated, truncated);
    assert_eq!(metadata.completeness, ContextCompleteness::Partial);
}

#[tokio::test]
async fn historical_250_facts_fit_candidate_scan() {
    assert_historical_scan_boundary(250, 250, false).await;
}

#[tokio::test]
async fn historical_251_facts_report_truncated_scan() {
    assert_historical_scan_boundary(251, 250, true).await;
}

#[tokio::test]
async fn historical_old_declared_subject_survives_distractor_history() {
    use chrono::TimeDelta;
    use memory_mcp::models::ContextCompleteness;

    let memory = common::TestMemory::new(false).await;
    let valid_at = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
    let content =
        "subject: Copper Warehouse\ndimension: capacity\nvalue: 42 crates for Copper Warehouse";
    let target = common::seed_fact_at(
        &memory.service,
        "main",
        content,
        valid_at - TimeDelta::days(1),
    )
    .await;
    for index in 0..250 {
        common::seed_fact_at(
            &memory.service,
            "main",
            &format!("subject: Depot {index:03}\ndimension: capacity\nvalue: 16 crates"),
            valid_at - TimeDelta::seconds(index + 1),
        )
        .await;
    }
    let cutoff = TemporalContextQuery {
        valid_at: Some(valid_at),
        known_at: None,
        known_revision: None,
    };
    let exact = AssembleContextCapability::assemble_context_temporal_from_service(
        &memory.service,
        request("Copper Warehouse"),
        cutoff,
    )
    .await
    .unwrap();
    assert_eq!(
        exact.items[0].fact_id, target,
        "exact-query visibility control"
    );

    let observed = AssembleContextCapability::assemble_context_temporal_from_service(
        &memory.service,
        request("What capacity was recorded for Copper Warehouse?"),
        cutoff,
    )
    .await
    .unwrap();
    assert_eq!(
        observed
            .items
            .iter()
            .map(|item| item.fact_id.as_str())
            .collect::<Vec<_>>(),
        [target.as_str()],
    );
    let metadata = observed
        .metadata
        .expect("revision-bounded context metadata");
    assert_eq!(metadata.completeness, ContextCompleteness::Partial);
    assert!(metadata.candidate_scan_truncated);
}

#[tokio::test]
async fn historical_transliterated_alias_reports_omitted_expansion() {
    use memory_mcp::models::ContextCompleteness;

    let memory = common::TestMemory::new(false).await;
    let service = common::exposing_claims(memory.service).unwrap();
    let source = common::ingest_episode(
        &service,
        "northern-warehouse",
        "subject: Северный склад; dimension: capacity; value: 42 crates",
    )
    .await;
    let valid_at = Utc.with_ymd_and_hms(2026, 7, 2, 9, 0, 0).unwrap();
    let canonical = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("Северный склад"),
        TemporalContextQuery {
            valid_at: Some(valid_at),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(canonical.items[0].source_episode, source);

    let historical = TemporalContextQuery {
        valid_at: Some(valid_at),
        known_at: None,
        known_revision: Some(canonical.metadata.unwrap().snapshot.known_revision),
    };
    let before = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("Severnyi sklad"),
        historical,
    )
    .await
    .unwrap();
    common::seed_entity(
        &memory.db_client,
        "org",
        "entity:northern_warehouse",
        "concept",
        "Северный склад",
        &["Severnyi sklad".into()],
    )
    .await;
    let after = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("Severnyi sklad"),
        historical,
    )
    .await
    .unwrap();
    assert!(after.items.is_empty());
    let metadata = after.metadata.as_ref().unwrap();
    assert_eq!(metadata.completeness, ContextCompleteness::Partial);
    assert!(
        metadata
            .omitted_channels
            .iter()
            .any(|channel| channel == "alias expansion")
    );
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[tokio::test]
async fn historical_explicit_subject_matches_case_variant() {
    let memory = common::TestMemory::new(false).await;
    let service = common::exposing_claims(memory.service).unwrap();
    let target = common::ingest_episode(
        &service,
        "copper-ledger",
        "subject: Copper Warehouse\ndimension: capacity\nvalue: 42 crates for Copper Warehouse",
    )
    .await;
    common::ingest_episode(
        &service,
        "neighbor-ledger",
        "subject: Copper Warehouse West\ndimension: capacity\nvalue: 48 crates for Copper Warehouse West",
    )
    .await;
    let result = AssembleContextCapability::assemble_context_temporal_from_service(
        &service,
        request("What capacity was recorded for COPPER WAREHOUSE?"),
        TemporalContextQuery {
            valid_at: Some(Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap()),
            known_at: None,
            known_revision: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.source_episode.as_str())
            .collect::<Vec<_>>(),
        [target.as_str()],
    );
    assert!(result.items[0].reconciliation.is_none());
}
