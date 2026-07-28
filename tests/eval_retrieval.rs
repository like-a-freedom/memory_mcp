mod common;
mod eval_support;

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use eval_support::metrics::{
    RetrievalCaseDiagnostics, RetrievalSuiteSummary, first_relevant_rank, record_retrieval_case,
    revoke_retrieval_case_pass,
};
use eval_support::report::print_retrieval_summary;
use memory_mcp::models::AssembleContextRequest;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RetrievalEvalCase {
    id: String,
    description: String,
    query: String,
    scope: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_budget")]
    budget: i32,
    facts: Vec<SeedFact>,
    #[serde(default)]
    entities: Vec<SeedEntity>,
    #[serde(default)]
    communities: Vec<SeedCommunity>,
    #[serde(default)]
    edges: Vec<SeedEdge>,
    expected: RetrievalExpectation,
}

#[derive(Debug, Deserialize)]
struct SeedFact {
    content: String,
    t_valid: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    entity_links: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SeedEntity {
    entity_id: String,
    entity_type: String,
    canonical_name: String,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SeedCommunity {
    community_id: String,
    member_entities: Vec<String>,
    summary: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct SeedEdge {
    from_id: String,
    relation: String,
    to_id: String,
}

#[derive(Debug, Deserialize)]
struct RetrievalExpectation {
    tier: String,
    must_contain: Vec<String>,
    #[serde(default)]
    must_not_contain: Vec<String>,
    #[serde(default = "default_min_recall_at_k")]
    min_recall_at_k: f64,
    #[serde(default)]
    diversity: Option<RetrievalDiversityExpectation>,
}

#[derive(Debug, Deserialize)]
struct RetrievalDiversityExpectation {
    #[serde(default)]
    min_unique_source_episodes: Option<usize>,
    #[serde(default)]
    max_source_episode_share: Option<f64>,
}

fn default_budget() -> i32 {
    5
}

fn default_min_recall_at_k() -> f64 {
    1.0
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("retrieval_cases.json")
}

fn load_cases() -> Vec<RetrievalEvalCase> {
    let raw = std::fs::read_to_string(fixture_path()).expect("read retrieval fixture");
    serde_json::from_str(&raw).expect("parse retrieval fixture")
}

fn case_as_of(case: &RetrievalEvalCase) -> DateTime<Utc> {
    let latest_seed_timestamp = case
        .facts
        .iter()
        .map(|fact| {
            fact.t_valid
                .parse::<DateTime<Utc>>()
                .expect("fixture timestamp should parse")
        })
        .chain(case.communities.iter().map(|community| {
            community
                .updated_at
                .parse::<DateTime<Utc>>()
                .expect("community timestamp should parse")
        }))
        .max()
        .expect("retrieval eval case should contain at least one timestamp");

    std::cmp::max(Utc::now(), latest_seed_timestamp) + Duration::seconds(1)
}

const GLOBAL_RECALL_AT_5_TARGET: f64 = 0.90;
const GLOBAL_MRR_TARGET: f64 = 0.85;
const GLOBAL_TOP_1_HIT_RATE_TARGET: f64 = 0.80;
const DIVERSITY_PASS_RATE_TARGET: f64 = 0.80;
const RETRIEVAL_TIER_PASS_RATE_TARGETS: [(&str, f64); 5] = [
    ("direct", 0.95),
    ("alias", 0.85),
    ("temporal", 0.80),
    ("graph", 0.70),
    ("reasoning", 0.60),
];

fn assert_retrieval_targets(summary: &RetrievalSuiteSummary) {
    assert!(
        summary.recall_at_5() >= GLOBAL_RECALL_AT_5_TARGET,
        "expected global recall_at_5 >= {:.2}, got {:.2}",
        GLOBAL_RECALL_AT_5_TARGET,
        summary.recall_at_5(),
    );
    assert!(
        summary.mrr() >= GLOBAL_MRR_TARGET,
        "expected global mrr >= {:.2}, got {:.2}",
        GLOBAL_MRR_TARGET,
        summary.mrr(),
    );
    assert!(
        summary.top_1_hit_rate() >= GLOBAL_TOP_1_HIT_RATE_TARGET,
        "expected global top1_hit_rate >= {:.2}, got {:.2}",
        GLOBAL_TOP_1_HIT_RATE_TARGET,
        summary.top_1_hit_rate(),
    );

    if summary.diversity_expected_cases > 0 {
        let diversity_pass_rate = summary.diversity_pass_rate().unwrap_or(0.0);
        assert!(
            diversity_pass_rate >= DIVERSITY_PASS_RATE_TARGET,
            "expected diversity pass_rate >= {:.2}, got {:.2}",
            DIVERSITY_PASS_RATE_TARGET,
            diversity_pass_rate,
        );
    }

    for (tier, target) in RETRIEVAL_TIER_PASS_RATE_TARGETS {
        let total = summary
            .expected_tier_totals
            .get(tier)
            .copied()
            .unwrap_or_default();
        assert!(
            total > 0,
            "expected retrieval summary to include tier {tier} for threshold evaluation"
        );
        let pass_rate = summary.expected_tier_pass_rate(tier).unwrap_or(0.0);
        assert!(
            pass_rate >= target,
            "expected tier {tier} pass_rate >= {:.2}, got {:.2}",
            target,
            pass_rate,
        );
    }
}

#[test]
fn retrieval_fixture_provides_minimum_tier_coverage() {
    let cases = load_cases();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();

    for case in &cases {
        *counts.entry(case.expected.tier.clone()).or_insert(0) += 1;
    }

    assert!(
        cases.len() >= 50,
        "expected at least 50 retrieval eval cases, got {}",
        cases.len()
    );

    for tier in ["direct", "alias", "temporal", "graph", "reasoning"] {
        let count = counts.get(tier).copied().unwrap_or_default();
        assert!(
            count >= 10,
            "expected at least 10 retrieval cases for tier {tier}, got {count}"
        );
    }
}

#[test]
fn retrieval_fixture_provides_project_filter_coverage() {
    let cases = load_cases();
    let project_cases = cases
        .iter()
        .filter(|case| case.project.is_some())
        .collect::<Vec<_>>();

    assert!(
        project_cases.len() >= 5,
        "expected at least 5 project-filter retrieval cases, got {}",
        project_cases.len()
    );
    assert!(
        project_cases
            .iter()
            .all(|case| !case.expected.must_not_contain.is_empty()),
        "every project-filter retrieval case should define must_not_contain"
    );
}

#[test]
fn retrieval_fixture_provides_graph_and_timeline_tag_coverage() {
    let cases = load_cases();

    for tag in ["timeline_auto", "graph_anchor", "first_person_rescue"] {
        let count = cases
            .iter()
            .filter(|case| case.tags.iter().any(|value| value == tag))
            .count();
        assert!(
            count >= 1,
            "expected at least one retrieval eval case tagged {tag}, got {count}"
        );
    }
}

#[test]
fn retrieval_fixture_provides_diversity_coverage() {
    let cases = load_cases();
    let diversity_cases = cases
        .iter()
        .filter(|case| case.expected.diversity.is_some())
        .collect::<Vec<_>>();

    assert!(
        diversity_cases.len() >= 2,
        "expected at least 2 diversity-sensitive retrieval cases, got {}",
        diversity_cases.len()
    );
    assert!(
        diversity_cases
            .iter()
            .all(|case| case.expected.must_contain.len() >= 2),
        "diversity-sensitive retrieval cases should exercise multi-hit coverage"
    );
}

#[test]
fn retrieval_targets_accept_plan_thresholds() {
    let mut summary = RetrievalSuiteSummary {
        total_cases: 60,
        passed_cases: 54,
        expected_hits: 100,
        matched_hits: 90,
        reciprocal_rank_sum: 54.0,
        top_1_hits: 48,
        diversity_expected_cases: 3,
        diversity_passed_cases: 3,
        unique_source_episode_ratio_sum: 2.25,
        max_source_episode_share_sum: 1.50,
        ..RetrievalSuiteSummary::default()
    };

    for (tier, (total, passed)) in [
        ("direct", (15, 15)),
        ("alias", (10, 9)),
        ("temporal", (10, 8)),
        ("graph", (15, 11)),
        ("reasoning", (10, 6)),
    ] {
        summary.expected_tier_totals.insert(tier.to_string(), total);
        summary
            .expected_tier_passed_cases
            .insert(tier.to_string(), passed);
    }

    assert_retrieval_targets(&summary);
}

#[test]
#[should_panic(expected = "expected tier temporal pass_rate >= 0.80")]
fn retrieval_targets_reject_below_target_tier_pass_rate() {
    let mut summary = RetrievalSuiteSummary {
        total_cases: 60,
        passed_cases: 53,
        expected_hits: 100,
        matched_hits: 90,
        reciprocal_rank_sum: 54.0,
        top_1_hits: 48,
        diversity_expected_cases: 3,
        diversity_passed_cases: 3,
        unique_source_episode_ratio_sum: 2.25,
        max_source_episode_share_sum: 1.50,
        ..RetrievalSuiteSummary::default()
    };

    for (tier, (total, passed)) in [
        ("direct", (15, 15)),
        ("alias", (10, 9)),
        ("temporal", (10, 7)),
        ("graph", (15, 11)),
        ("reasoning", (10, 6)),
    ] {
        summary.expected_tier_totals.insert(tier.to_string(), total);
        summary
            .expected_tier_passed_cases
            .insert(tier.to_string(), passed);
    }

    assert_retrieval_targets(&summary);
}

#[test]
#[should_panic(expected = "expected global mrr >= 0.85")]
fn retrieval_targets_reject_below_target_mrr() {
    let mut summary = RetrievalSuiteSummary {
        total_cases: 60,
        passed_cases: 54,
        expected_hits: 100,
        matched_hits: 90,
        reciprocal_rank_sum: 48.0,
        top_1_hits: 48,
        diversity_expected_cases: 3,
        diversity_passed_cases: 3,
        unique_source_episode_ratio_sum: 2.25,
        max_source_episode_share_sum: 1.50,
        ..RetrievalSuiteSummary::default()
    };

    for (tier, (total, passed)) in [
        ("direct", (15, 15)),
        ("alias", (10, 9)),
        ("temporal", (10, 8)),
        ("graph", (15, 11)),
        ("reasoning", (10, 6)),
    ] {
        summary.expected_tier_totals.insert(tier.to_string(), total);
        summary
            .expected_tier_passed_cases
            .insert(tier.to_string(), passed);
    }

    assert_retrieval_targets(&summary);
}

#[test]
#[should_panic(expected = "expected diversity pass_rate >= 0.80")]
fn retrieval_targets_reject_below_target_diversity_pass_rate() {
    let mut summary = RetrievalSuiteSummary {
        total_cases: 60,
        passed_cases: 54,
        expected_hits: 100,
        matched_hits: 90,
        reciprocal_rank_sum: 54.0,
        top_1_hits: 48,
        diversity_expected_cases: 3,
        diversity_passed_cases: 2,
        unique_source_episode_ratio_sum: 2.25,
        max_source_episode_share_sum: 1.50,
        ..RetrievalSuiteSummary::default()
    };

    for (tier, (total, passed)) in [
        ("direct", (15, 15)),
        ("alias", (10, 9)),
        ("temporal", (10, 8)),
        ("graph", (15, 11)),
        ("reasoning", (10, 6)),
    ] {
        summary.expected_tier_totals.insert(tier.to_string(), total);
        summary
            .expected_tier_passed_cases
            .insert(tier.to_string(), passed);
    }

    assert_retrieval_targets(&summary);
}

#[tokio::test]
#[ignore]
async fn run_retrieval_evals() {
    let cases = load_cases();
    let mut summary = RetrievalSuiteSummary::default();

    for case in cases {
        let as_of = case_as_of(&case);
        let (service, db_client) = common::make_service_with_client().await;
        for entity in &case.entities {
            common::seed_entity(
                &db_client,
                &case.scope,
                &entity.entity_id,
                &entity.entity_type,
                &entity.canonical_name,
                &entity.aliases,
            )
            .await;
        }
        for edge in &case.edges {
            service
                .relate(&edge.from_id, &edge.relation, &edge.to_id)
                .await
                .unwrap_or_else(|err| panic!("case {} failed to seed edge: {err}", case.id));
        }
        for community in &case.communities {
            let updated_at = community
                .updated_at
                .parse::<DateTime<Utc>>()
                .expect("community timestamp should parse");
            common::seed_community(
                &db_client,
                &case.scope,
                &community.community_id,
                &community.member_entities,
                &community.summary,
                updated_at,
            )
            .await;
        }
        for fact in &case.facts {
            let t_valid = fact
                .t_valid
                .parse::<DateTime<Utc>>()
                .expect("fixture timestamp should parse");
            common::seed_fact_with_links_and_project(
                &service,
                &case.scope,
                &fact.content,
                t_valid,
                fact.entity_links.clone(),
                fact.project.as_deref(),
                fact.source_id.as_deref(),
            )
            .await;
        }

        let items = service
            .assemble_context(AssembleContextRequest {
                query: case.query.clone(),
                scope: case.scope.clone(),
                as_of: Some(as_of),
                budget: case.budget,
                project: case.project.clone(),
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
            .unwrap_or_else(|err| panic!("case {} failed to assemble: {err}", case.id));

        let matched_hits = case
            .expected
            .must_contain
            .iter()
            .filter(|needle| {
                items
                    .iter()
                    .any(|item| item.content.contains(needle.as_str()))
            })
            .count();
        let unexpected_hits = case
            .expected
            .must_not_contain
            .iter()
            .filter(|needle| {
                items
                    .iter()
                    .any(|item| item.content.contains(needle.as_str()))
            })
            .count();
        let actual_tiers = items
            .iter()
            .filter_map(|item| item.retrieval_tier.as_deref())
            .collect::<Vec<_>>();
        let retrieved_contents = items
            .iter()
            .map(|item| item.content.as_str())
            .collect::<Vec<_>>();
        let source_episode_refs = items
            .iter()
            .map(|item| item.source_episode.as_str())
            .collect::<Vec<_>>();
        let first_relevant_rank =
            first_relevant_rank(&retrieved_contents, &case.expected.must_contain);

        let recall_passed = record_retrieval_case(
            &mut summary,
            &case.expected.tier,
            &case.tags,
            matched_hits,
            case.expected.must_contain.len(),
            case.expected.min_recall_at_k,
            RetrievalCaseDiagnostics {
                actual_tiers: &actual_tiers,
                first_relevant_rank,
                source_episodes: &source_episode_refs,
                min_unique_source_episodes: case
                    .expected
                    .diversity
                    .as_ref()
                    .and_then(|expectation| expectation.min_unique_source_episodes),
                max_source_episode_share: case
                    .expected
                    .diversity
                    .as_ref()
                    .and_then(|expectation| expectation.max_source_episode_share),
            },
        );
        let passed = recall_passed && unexpected_hits == 0;
        if recall_passed && unexpected_hits > 0 {
            revoke_retrieval_case_pass(&mut summary, &case.expected.tier, &case.tags);
        }

        assert!(
            passed,
            "case {} ({}) failed: matched_hits={} expected_hits={} unexpected_hits={} first_relevant_rank={:?} actual_tiers={:?} source_episodes={:?} retrieved_contents={:?}",
            case.id,
            case.description,
            matched_hits,
            case.expected.must_contain.len(),
            unexpected_hits,
            first_relevant_rank,
            actual_tiers,
            source_episode_refs,
            retrieved_contents,
        );
    }

    print_retrieval_summary("eval_retrieval", &summary);
    assert!(
        summary.total_cases >= 2,
        "expected at least two retrieval eval cases"
    );
    assert_retrieval_targets(&summary);
}

/// Diagnostic-only sibling of `run_retrieval_evals`.
///
/// The canonical runner above intentionally panics on the first failing case
/// (fail-fast in CI). That makes the suite pass/fail boundary crisp, but the
/// printed summary is never reached when any case fails, so we cannot report
/// an objective breakdown (pass rate, MRR, recall@5, per-tier pass rate) for
/// the whole fixture.
///
/// This test mirrors the loop body exactly but does **not** assert — it logs
/// per-case failures via `eprintln!` and continues, then prints the same
/// summary line at the end. It is `#[ignore]`d by default to preserve the
/// canonical CI contract; run it explicitly with `--ignored` to collect
/// diagnostic metrics.
#[tokio::test]
#[ignore]
async fn run_retrieval_evals_diagnostic() {
    let cases = load_cases();
    let mut summary = RetrievalSuiteSummary::default();

    for case in cases {
        let as_of = case_as_of(&case);
        let (service, db_client) = common::make_service_with_client().await;
        for entity in &case.entities {
            common::seed_entity(
                &db_client,
                &case.scope,
                &entity.entity_id,
                &entity.entity_type,
                &entity.canonical_name,
                &entity.aliases,
            )
            .await;
        }
        for edge in &case.edges {
            if let Err(err) = service
                .relate(&edge.from_id, &edge.relation, &edge.to_id)
                .await
            {
                eprintln!(
                    "[skip] case {}: failed to seed edge {} - {} -> {}: {err}",
                    case.id, edge.relation, edge.from_id, edge.to_id
                );
            }
        }
        for community in &case.communities {
            let updated_at = community
                .updated_at
                .parse::<DateTime<Utc>>()
                .expect("community timestamp should parse");
            common::seed_community(
                &db_client,
                &case.scope,
                &community.community_id,
                &community.member_entities,
                &community.summary,
                updated_at,
            )
            .await;
        }
        for fact in &case.facts {
            let t_valid = fact
                .t_valid
                .parse::<DateTime<Utc>>()
                .expect("fixture timestamp should parse");
            common::seed_fact_with_links_and_project(
                &service,
                &case.scope,
                &fact.content,
                t_valid,
                fact.entity_links.clone(),
                fact.project.as_deref(),
                fact.source_id.as_deref(),
            )
            .await;
        }

        let items = match service
            .assemble_context(AssembleContextRequest {
                query: case.query.clone(),
                scope: case.scope.clone(),
                as_of: Some(as_of),
                budget: case.budget,
                project: case.project.clone(),
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
        {
            Ok(items) => items,
            Err(err) => {
                eprintln!("[skip] case {}: assemble_context error: {err}", case.id);
                continue;
            }
        };

        let matched_hits = case
            .expected
            .must_contain
            .iter()
            .filter(|needle| {
                items
                    .iter()
                    .any(|item| item.content.contains(needle.as_str()))
            })
            .count();
        let unexpected_hits = case
            .expected
            .must_not_contain
            .iter()
            .filter(|needle| {
                items
                    .iter()
                    .any(|item| item.content.contains(needle.as_str()))
            })
            .count();
        let actual_tiers = items
            .iter()
            .filter_map(|item| item.retrieval_tier.as_deref())
            .collect::<Vec<_>>();
        let retrieved_contents = items
            .iter()
            .map(|item| item.content.as_str())
            .collect::<Vec<_>>();
        let source_episode_refs = items
            .iter()
            .map(|item| item.source_episode.as_str())
            .collect::<Vec<_>>();
        let first_relevant_rank =
            first_relevant_rank(&retrieved_contents, &case.expected.must_contain);

        let recall_passed = record_retrieval_case(
            &mut summary,
            &case.expected.tier,
            &case.tags,
            matched_hits,
            case.expected.must_contain.len(),
            case.expected.min_recall_at_k,
            RetrievalCaseDiagnostics {
                actual_tiers: &actual_tiers,
                first_relevant_rank,
                source_episodes: &source_episode_refs,
                min_unique_source_episodes: case
                    .expected
                    .diversity
                    .as_ref()
                    .and_then(|expectation| expectation.min_unique_source_episodes),
                max_source_episode_share: case
                    .expected
                    .diversity
                    .as_ref()
                    .and_then(|expectation| expectation.max_source_episode_share),
            },
        );
        let passed = recall_passed && unexpected_hits == 0;
        if recall_passed && unexpected_hits > 0 {
            revoke_retrieval_case_pass(&mut summary, &case.expected.tier, &case.tags);
        }

        if !passed {
            eprintln!(
                "[fail] case {} ({}) matched_hits={} expected_hits={} unexpected_hits={} first_relevant_rank={:?}",
                case.id,
                case.description,
                matched_hits,
                case.expected.must_contain.len(),
                unexpected_hits,
                first_relevant_rank,
            );
        }
    }

    print_retrieval_summary("eval_retrieval", &summary);
}

/// Thin launcher that delegates to the eval-harness retrieval suite.
/// Run with: cargo test --test eval_retrieval harness_retrieval_suite -- --ignored --exact
#[tokio::test]
#[ignore]
async fn harness_retrieval_suite() {
    use eval_harness::{EvalProfile, EvalSuite, RetrievalSuite, RunContext};

    let suite = RetrievalSuite::new().expect("load retrieval suite");
    let context = RunContext {
        profile: EvalProfile::Pr,
    };
    let outcomes = suite.run(&context).await;

    let passed = outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::Passed)
        .count();
    let failed = outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::QualityFailed)
        .count();
    let invalid = outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::Invalid)
        .count();

    eprintln!(
        "harness_retrieval: total={} passed={} failed={} invalid={}",
        outcomes.len(),
        passed,
        failed,
        invalid
    );

    assert!(
        invalid == 0,
        "harness retrieval suite has {invalid} invalid cases"
    );
}
