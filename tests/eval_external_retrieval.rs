mod common;
mod eval_support;

use std::collections::HashMap;
use std::fs;

use chrono::{DateTime, Utc};
use eval_support::external::{
    DatasetKind, NormalizedExternalRetrievalCase, NormalizedSeedFact, normalize_external_dataset,
};
use eval_support::external_full::{
    ExternalDatasetFlavor, load_external_dataset_cases, sample_fixture_path,
};
use eval_support::metrics::{RetrievalSuiteSummary, record_retrieval_case};
use eval_support::report::print_retrieval_summary;
use memory_mcp::models::AssembleContextRequest;

fn sample_dataset_raw(kind: DatasetKind) -> String {
    fs::read_to_string(sample_fixture_path(kind))
        .unwrap_or_else(|err| panic!("read sample dataset for {:?}: {err}", kind))
}

fn case_limit_from_env() -> Option<usize> {
    std::env::var("MEMORY_MCP_EVAL_MAX_CASES")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|limit| *limit > 0)
}

fn eval_query_parallelism(strict_case_asserts: bool) -> usize {
    if strict_case_asserts {
        return 1;
    }

    std::env::var("MEMORY_MCP_EVAL_QUERY_CONCURRENCY")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|parallelism| *parallelism > 0)
        .unwrap_or(8)
}

#[derive(Debug)]
struct RetrievalCaseBatch {
    scope: String,
    facts: Vec<NormalizedSeedFact>,
    cases: Vec<NormalizedExternalRetrievalCase>,
}

fn group_cases_by_seed_facts(
    cases: Vec<NormalizedExternalRetrievalCase>,
) -> Vec<RetrievalCaseBatch> {
    let mut batches: Vec<RetrievalCaseBatch> = Vec::new();
    let mut batch_indexes = HashMap::<String, usize>::new();

    for case in cases {
        let key = batch_seed_key(&case.scope, &case.facts);
        if let Some(batch_index) = batch_indexes.get(&key).copied() {
            batches[batch_index].cases.push(case);
            continue;
        }

        let batch_index = batches.len();
        batch_indexes.insert(key, batch_index);
        batches.push(RetrievalCaseBatch {
            scope: case.scope.clone(),
            facts: case.facts.clone(),
            cases: vec![case],
        });
    }

    batches
}

fn batch_seed_key(scope: &str, facts: &[NormalizedSeedFact]) -> String {
    let mut key = String::with_capacity(scope.len() + facts.len().saturating_mul(64));
    key.push_str(scope);
    key.push('\n');

    for fact in facts {
        key.push_str(&fact.t_valid);
        key.push('\t');
        key.push_str(&fact.content);
        key.push('\n');
    }

    key
}

async fn seed_case_facts(
    service: &memory_mcp::service::MemoryService,
    scope: &str,
    facts: &[NormalizedSeedFact],
) {
    for fact in facts {
        let t_valid = fact
            .t_valid
            .parse::<DateTime<Utc>>()
            .expect("normalized timestamps should parse");
        common::seed_fact_at(service, scope, &fact.content, t_valid).await;
    }
}

async fn run_dataset_retrieval(
    suite_name: &str,
    kind: DatasetKind,
    flavor: ExternalDatasetFlavor,
    strict_case_asserts: bool,
) {
    let mut cases = load_external_dataset_cases(kind, flavor)
        .await
        .unwrap_or_else(|err| panic!("load {:?} {:?} dataset cases: {err}", flavor, kind));
    let original_case_count = cases.len();
    if let Some(limit) = case_limit_from_env()
        && cases.len() > limit
    {
        cases.truncate(limit);
        println!(
            "suite={} mode={:?} limiting cases from {} to {} via MEMORY_MCP_EVAL_MAX_CASES",
            suite_name,
            flavor,
            original_case_count,
            cases.len(),
        );
    }

    let total_cases = cases.len();
    let case_batches = group_cases_by_seed_facts(cases);
    let total_batches = case_batches.len();
    let query_parallelism = eval_query_parallelism(strict_case_asserts);
    if total_cases > 0 {
        println!(
            "suite={} mode={:?} grouped {} cases into {} seeded contexts query_concurrency={}",
            suite_name, flavor, total_cases, total_batches, query_parallelism,
        );
    }

    let mut summary = RetrievalSuiteSummary::default();
    let mut failed_case_count = 0usize;
    let mut logged_failures = 0usize;
    let mut completed_cases = 0usize;
    for (batch_index, batch) in case_batches.into_iter().enumerate() {
        let service = common::make_service().await;
        seed_case_facts(&service, &batch.scope, &batch.facts).await;

        let case_outcomes =
            evaluate_retrieval_batch(&service, batch.cases, query_parallelism).await;

        for outcome in case_outcomes {
            let failure = finalize_retrieval_case(outcome, &mut summary, strict_case_asserts);
            completed_cases += 1;

            if let Some(failure) = failure {
                failed_case_count += 1;
                if logged_failures < 10 {
                    println!(
                        "failed_case={} matched_hits={} expected_hits={} actual_tiers={:?}",
                        failure.case_id,
                        failure.matched_hits,
                        failure.expected_hits,
                        failure.actual_tiers,
                    );
                    logged_failures += 1;
                }
            }

            if completed_cases == total_cases || completed_cases % 25 == 0 {
                println!(
                    "suite={} progress={}/{} seeded_contexts={}/{}",
                    suite_name,
                    completed_cases,
                    total_cases,
                    batch_index + 1,
                    total_batches,
                );
            }
        }
    }

    if failed_case_count > logged_failures {
        println!(
            "suite={} additional_failed_cases={}",
            suite_name,
            failed_case_count - logged_failures,
        );
    }
    print_retrieval_summary(suite_name, &summary);
    assert!(
        summary.total_cases > 0,
        "expected {suite_name} to execute at least one case"
    );
}

#[derive(Debug)]
struct RetrievalCaseFailure {
    case_id: String,
    matched_hits: usize,
    expected_hits: usize,
    actual_tiers: Vec<String>,
}

#[derive(Debug)]
struct RetrievalCaseOutcome {
    case: NormalizedExternalRetrievalCase,
    matched_hits: usize,
    expected_hits: usize,
    actual_tiers: Vec<String>,
    retrieved_contents: Vec<String>,
}

async fn run_retrieval_case(
    case: NormalizedExternalRetrievalCase,
    summary: &mut RetrievalSuiteSummary,
    strict_case_asserts: bool,
) -> Option<RetrievalCaseFailure> {
    let service = common::make_service().await;
    seed_case_facts(&service, &case.scope, &case.facts).await;

    run_retrieval_case_with_service(&service, case, summary, strict_case_asserts).await
}

async fn run_retrieval_case_with_service(
    service: &memory_mcp::service::MemoryService,
    case: NormalizedExternalRetrievalCase,
    summary: &mut RetrievalSuiteSummary,
    strict_case_asserts: bool,
) -> Option<RetrievalCaseFailure> {
    let outcome = evaluate_retrieval_case(service, case).await;

    finalize_retrieval_case(outcome, summary, strict_case_asserts)
}

async fn evaluate_retrieval_batch(
    service: &memory_mcp::service::MemoryService,
    cases: Vec<NormalizedExternalRetrievalCase>,
    query_parallelism: usize,
) -> Vec<RetrievalCaseOutcome> {
    if query_parallelism <= 1 || cases.len() <= 1 {
        let mut outcomes = Vec::with_capacity(cases.len());
        for case in cases {
            outcomes.push(evaluate_retrieval_case(service, case).await);
        }
        return outcomes;
    }

    let total_cases = cases.len();
    let mut outputs = Vec::with_capacity(total_cases);
    let mut pending_cases = cases.into_iter().enumerate();
    let mut join_set = tokio::task::JoinSet::new();

    for _ in 0..query_parallelism {
        let Some((case_index, case)) = pending_cases.next() else {
            break;
        };
        let service = service.clone();
        join_set.spawn(async move { (case_index, evaluate_retrieval_case(&service, case).await) });
    }

    while let Some(joined) = join_set.join_next().await {
        let (case_index, outcome) = joined.expect("retrieval case task should complete");
        outputs.push((case_index, outcome));

        if let Some((next_case_index, next_case)) = pending_cases.next() {
            let service = service.clone();
            join_set.spawn(async move {
                (
                    next_case_index,
                    evaluate_retrieval_case(&service, next_case).await,
                )
            });
        }
    }

    outputs.sort_by_key(|(case_index, _)| *case_index);
    outputs.into_iter().map(|(_, outcome)| outcome).collect()
}

async fn evaluate_retrieval_case(
    service: &memory_mcp::service::MemoryService,
    case: NormalizedExternalRetrievalCase,
) -> RetrievalCaseOutcome {
    let items = service
        .assemble_context(AssembleContextRequest {
            query: case.query.clone(),
            scope: case.scope.clone(),
            as_of: None,
            budget: case.budget,
            project: None,
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
    let actual_tiers = items
        .iter()
        .filter_map(|item| item.retrieval_tier.as_deref())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let retrieved_contents = items
        .iter()
        .map(|item| item.content.clone())
        .collect::<Vec<_>>();

    RetrievalCaseOutcome {
        expected_hits: case.expected.must_contain.len(),
        case,
        matched_hits,
        actual_tiers,
        retrieved_contents,
    }
}

fn finalize_retrieval_case(
    outcome: RetrievalCaseOutcome,
    summary: &mut RetrievalSuiteSummary,
    strict_case_asserts: bool,
) -> Option<RetrievalCaseFailure> {
    let actual_tier_refs = outcome
        .actual_tiers
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let passed = record_retrieval_case(
        summary,
        &outcome.case.expected.tier,
        outcome.matched_hits,
        outcome.expected_hits,
        &actual_tier_refs,
        outcome.case.expected.min_recall_at_k,
    );

    if strict_case_asserts {
        assert!(
            passed,
            "case {} failed: matched_hits={} expected_hits={} actual_tiers={:?} retrieved_contents={:?}",
            outcome.case.id,
            outcome.matched_hits,
            outcome.expected_hits,
            outcome.actual_tiers,
            outcome.retrieved_contents,
        );
        None
    } else if !passed {
        Some(RetrievalCaseFailure {
            case_id: outcome.case.id,
            matched_hits: outcome.matched_hits,
            expected_hits: outcome.expected_hits,
            actual_tiers: outcome.actual_tiers,
        })
    } else {
        None
    }
}

#[test]
fn normalizes_longmemeval_fixture_into_canonical_cases() {
    let raw = sample_dataset_raw(DatasetKind::LongMemEvalCleaned);

    let cases = normalize_external_dataset(DatasetKind::LongMemEvalCleaned, &raw)
        .expect("normalize longmemeval fixture");

    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    assert_eq!(case.dataset, "longmemeval-cleaned");
    assert_eq!(case.id, "longmemeval-cleaned:e47becba");
    assert_eq!(case.query, "What degree did I graduate with?");
    assert_eq!(case.expected.tier, "direct");
    assert_eq!(case.expected.must_contain, vec!["Business Administration"]);
    assert_eq!(case.facts.len(), 2);
    assert_eq!(
        case.facts[0].content,
        "The farmer needs to transport a fox, a chicken, and some grain across a river using a boat. The fox cannot be left alone with the chicken, and the chicken cannot be left alone with the grain. The boat can only hold one item at a time, and the river is too dangerous to cross multiple times. Can you help the farmer transport all three items across the river without any of them getting eaten? Remember, strategic thinking and planning are key to solving this puzzle. If you're stuck, try thinking about how you would solve the puzzle yourself, and use that as a starting point. Be careful not to leave the chicken alone with the fox, or the chicken and the grain alone together, as this will result in a failed solution. Good luck!"
    );
    assert_eq!(case.facts[0].t_valid, "2023-05-20T02:21:00+00:00");
    assert!(
        case.facts[1]
            .content
            .contains("I graduated with a degree in Business Administration")
    );
    assert_eq!(case.metadata["question_type"], "single-session-user");
}

#[test]
fn normalizes_locomo_fixture_into_canonical_cases() {
    let raw = sample_dataset_raw(DatasetKind::LoCoMo);

    let cases =
        normalize_external_dataset(DatasetKind::LoCoMo, &raw).expect("normalize locomo fixture");

    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    assert_eq!(case.dataset, "locomo");
    assert_eq!(case.id, "locomo:conv-26:0");
    assert_eq!(case.query, "What did Caroline research?");
    assert_eq!(case.expected.tier, "direct");
    assert_eq!(
        case.expected.must_contain,
        vec![
            "Caroline is researching adoption agencies with the dream of having a family and providing a loving home to kids in need."
        ]
    );
    assert!(case.facts.len() > 6);
    assert_eq!(
        case.facts[0].content,
        "Caroline: Hey Mel! Good to see you! How have you been?"
    );
    assert_eq!(case.facts[0].t_valid, "2023-05-08T13:56:00+00:00");
    assert!(
        case.facts
            .iter()
            .any(|fact| { fact.content.contains("Researching adoption agencies") })
    );
    assert!(case.facts.iter().any(|fact| {
        fact.content
            .contains("Caroline is researching adoption agencies")
    }));
    assert_eq!(case.metadata["category"], 1);
    assert_eq!(case.metadata["evidence"][0], "D2:8");
}

#[test]
fn normalizes_personamem_fixture_into_canonical_cases() {
    let raw = sample_dataset_raw(DatasetKind::PersonaMem);

    let cases = normalize_external_dataset(DatasetKind::PersonaMem, &raw)
        .expect("normalize personamem fixture");

    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    assert_eq!(case.dataset, "personamem");
    assert_eq!(case.id, "personamem:acd74206-37dc-4756-94a8-b99a395d9a21");
    assert_eq!(
        case.query,
        "I recently attended an event where there was a unique blend of modern beats with Pacific sounds."
    );
    assert_eq!(case.expected.tier, "direct");
    assert_eq!(
        case.expected.must_contain,
        vec![
            "The blend of traditional Pacific sounds with modern beats created a captivating experience that resonated deeply with the audience"
        ]
    );
    assert_eq!(case.facts.len(), 4);
    assert!(
        case.facts[0]
            .content
            .contains("Current user persona: Name: Kanoa Manu")
    );
    assert_eq!(case.facts[0].t_valid, "2000-01-01T00:00:00+00:00");
    assert_eq!(case.metadata["topic"], "musicRecommendation");
    assert_eq!(case.metadata["correct_answer"], "(c)");
    assert!(
        case.metadata["selected_option"]
            .as_str()
            .expect("selected option text")
            .contains("Since you like producing music with software")
    );
}

#[test]
fn normalizes_prefeval_fixture_into_canonical_cases() {
    let raw = sample_dataset_raw(DatasetKind::PrefEval);

    let cases = normalize_external_dataset(DatasetKind::PrefEval, &raw)
        .expect("normalize prefeval fixture");

    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    assert_eq!(case.dataset, "prefeval");
    assert_eq!(
        case.id,
        "prefeval:travel_hotel_overall300_topk_history_persona:0"
    );
    assert_eq!(
        case.query,
        "Can you suggest some great hotels for my upcoming trip to Las Vegas?"
    );
    assert_eq!(case.expected.tier, "reasoning");
    assert_eq!(
        case.expected.must_contain,
        vec!["I absolutely avoid hotels with a bustling nightlife atmosphere."]
    );
    assert!(case.facts.len() >= 8);
    assert!(case.facts.iter().any(|fact| {
        fact.content
            == "User: I usually prefer quieter hotels away from the city center, as I absolutely avoid hotels with a bustling nightlife atmosphere."
    }));
    assert_eq!(
        case.metadata["persona"],
        "A police officer specializing in community outreach programs"
    );
    assert_eq!(
        case.metadata["track"],
        "travel_hotel_overall300_topk_history_persona"
    );
}

#[test]
fn group_cases_by_seed_facts_batches_shared_contexts() {
    let shared_facts = vec![NormalizedSeedFact {
        content: "Caroline is researching adoption agencies.".to_string(),
        t_valid: "2023-05-08T13:56:00+00:00".to_string(),
    }];

    let mut cases = Vec::new();
    for (id_suffix, query) in [
        ("0", "What did Caroline research?"),
        ("1", "Why was Caroline researching agencies?"),
    ] {
        cases.push(NormalizedExternalRetrievalCase {
            id: format!("locomo:conv-26:{id_suffix}"),
            dataset: "locomo".to_string(),
            description: query.to_string(),
            query: query.to_string(),
            scope: "org".to_string(),
            budget: 5,
            facts: shared_facts.clone(),
            expected: eval_support::external::NormalizedRetrievalExpectation {
                tier: "direct".to_string(),
                must_contain: vec!["adoption agencies".to_string()],
                min_recall_at_k: 1.0,
            },
            metadata: serde_json::json!({"sample_id": "conv-26"}),
        });
    }

    cases.push(NormalizedExternalRetrievalCase {
        id: "locomo:conv-77:0".to_string(),
        dataset: "locomo".to_string(),
        description: "Different conversation".to_string(),
        query: "What class did Caroline attend?".to_string(),
        scope: "org".to_string(),
        budget: 5,
        facts: vec![NormalizedSeedFact {
            content: "Caroline attended a cooking class.".to_string(),
            t_valid: "2023-05-09T09:00:00+00:00".to_string(),
        }],
        expected: eval_support::external::NormalizedRetrievalExpectation {
            tier: "direct".to_string(),
            must_contain: vec!["cooking class".to_string()],
            min_recall_at_k: 1.0,
        },
        metadata: serde_json::json!({"sample_id": "conv-77"}),
    });

    let grouped = group_cases_by_seed_facts(cases);
    let mut batch_sizes = grouped
        .into_iter()
        .map(|batch| batch.cases.len())
        .collect::<Vec<_>>();
    batch_sizes.sort_unstable();

    assert_eq!(batch_sizes, vec![1, 2]);
}

#[tokio::test]
async fn locomo_retrieval_uses_observation_context_for_identity_question() {
    let raw = r#"[
        {
            "sample_id": "conv-regression",
            "conversation": {
                "speaker_a": "Caroline",
                "speaker_b": "Melanie",
                "session_1_date_time": "09:00 AM on 07 May, 2023",
                "session_1": [
                    {
                        "speaker": "Caroline",
                        "dia_id": "D1:5",
                        "text": "I was so happy and thankful for all the support."
                    }
                ]
            },
            "qa": [
                {
                    "question": "What is Caroline's identity?",
                    "answer": "Transgender woman",
                    "evidence": ["D1:5"],
                    "category": 1
                }
            ],
            "observation": {
                "session_1_observation": {
                    "Caroline": [
                        "Caroline is a transgender woman."
                    ]
                }
            }
        }
    ]"#;

    let cases = normalize_external_dataset(DatasetKind::LoCoMo, raw)
        .expect("normalize locomo regression fixture");
    assert_eq!(cases.len(), 1);

    let mut summary = RetrievalSuiteSummary::default();
    let failure =
        run_retrieval_case(cases.into_iter().next().expect("case"), &mut summary, false).await;

    assert!(
        failure.is_none(),
        "expected observation-backed locomo retrieval to pass, got {failure:?}"
    );
}

#[tokio::test]
#[ignore]
async fn locomo_full_conv26_first_case_retrieves_expected_context() {
    let case_id =
        std::env::var("MEMORY_MCP_EVAL_CASE_ID").unwrap_or_else(|_| "locomo:conv-26:0".to_string());

    let case = load_external_dataset_cases(DatasetKind::LoCoMo, ExternalDatasetFlavor::Full)
        .await
        .expect("load full locomo cases")
        .into_iter()
        .find(|case| case.id == case_id)
        .unwrap_or_else(|| panic!("find full locomo case {case_id}"));

    let mut summary = RetrievalSuiteSummary::default();
    run_retrieval_case(case, &mut summary, true).await;
}

#[tokio::test]
#[ignore]
async fn run_longmemeval_retrieval() {
    run_dataset_retrieval(
        "longmemeval",
        DatasetKind::LongMemEvalCleaned,
        ExternalDatasetFlavor::Sample,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn run_locomo_retrieval() {
    run_dataset_retrieval(
        "locomo",
        DatasetKind::LoCoMo,
        ExternalDatasetFlavor::Sample,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn run_personamem_retrieval() {
    run_dataset_retrieval(
        "personamem",
        DatasetKind::PersonaMem,
        ExternalDatasetFlavor::Sample,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn run_prefeval_retrieval() {
    run_dataset_retrieval(
        "prefeval",
        DatasetKind::PrefEval,
        ExternalDatasetFlavor::Sample,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn run_longmemeval_full_retrieval() {
    run_dataset_retrieval(
        "longmemeval_full",
        DatasetKind::LongMemEvalCleaned,
        ExternalDatasetFlavor::Full,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn run_locomo_full_retrieval() {
    run_dataset_retrieval(
        "locomo_full",
        DatasetKind::LoCoMo,
        ExternalDatasetFlavor::Full,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn run_personamem_full_retrieval() {
    run_dataset_retrieval(
        "personamem_full",
        DatasetKind::PersonaMem,
        ExternalDatasetFlavor::Full,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn run_prefeval_full_retrieval() {
    run_dataset_retrieval(
        "prefeval_full",
        DatasetKind::PrefEval,
        ExternalDatasetFlavor::Full,
        false,
    )
    .await;
}
