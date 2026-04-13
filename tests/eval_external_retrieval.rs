mod common;
mod eval_support;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use eval_support::external::{
    DatasetKind, NormalizedExternalRetrievalCase, NormalizedSeedFact, normalize_external_dataset,
};
use eval_support::external_full::{
    load_external_dataset_cases, raw_fixture_path, sample_pct_from_env,
};
use eval_support::metrics::{
    RetrievalCaseDiagnostics, RetrievalSuiteSummary, first_relevant_rank, record_retrieval_case,
};
use eval_support::report::print_retrieval_summary;
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::service::{hash_prefix, preprocess_search_query};
use memory_mcp::storage::DbClient;

fn raw_dataset_raw(kind: DatasetKind) -> String {
    std::fs::read_to_string(raw_fixture_path(kind))
        .unwrap_or_else(|err| panic!("read raw dataset for {:?}: {err}", kind))
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
        let source_id = format!(
            "seed:{}:{}:{}",
            scope,
            fact.t_valid,
            hash_prefix(&fact.content)
        );
        common::seed_episode_backed_fact_with_source_id(
            service,
            scope,
            &fact.content,
            t_valid,
            &source_id,
        )
        .await;
    }
}

async fn run_dataset_retrieval(suite_name: &str, kind: DatasetKind, strict_case_asserts: bool) {
    let mut cases = load_external_dataset_cases(kind)
        .await
        .unwrap_or_else(|err| panic!("load {:?} dataset cases: {err}", kind));
    let pct = sample_pct_from_env();
    let original_case_count = cases.len();
    if let Some(limit) = case_limit_from_env()
        && cases.len() > limit
    {
        cases.truncate(limit);
        println!(
            "suite={} sample_pct={} limiting cases from {} to {} via MEMORY_MCP_EVAL_MAX_CASES",
            suite_name,
            pct,
            original_case_count,
            cases.len(),
        );
    }

    let total_cases = cases.len();
    let case_batches = group_cases_by_seed_facts(cases);
    let total_batches = case_batches.len();
    let query_parallelism = eval_query_parallelism(strict_case_asserts);
    let progress_interval = if total_cases <= 20 { 1 } else { 10 };
    if total_cases > 0 {
        println!(
            "suite={} sample_pct={} grouped {} cases into {} seeded contexts query_concurrency={}",
            suite_name, pct, total_cases, total_batches, query_parallelism,
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

            if completed_cases == total_cases || completed_cases.is_multiple_of(progress_interval) {
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
    source_episodes: Vec<String>,
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

#[derive(Debug)]
struct CandidatePoolSnapshot {
    initial_has_expected: bool,
    fallback_has_expected: bool,
    active_experience_has_expected: bool,
    reranked_initial_has_expected: bool,
    reranked_fallback_has_expected: bool,
    initial_top_contents: Vec<String>,
    fallback_top_contents: Vec<String>,
    active_experience_top_contents: Vec<String>,
    reranked_initial_top_contents: Vec<String>,
    reranked_fallback_top_contents: Vec<String>,
}

async fn build_candidate_pool_snapshot(
    db_client: &dyn DbClient,
    case: &NormalizedExternalRetrievalCase,
) -> CandidatePoolSnapshot {
    let cleaned_query = preprocess_search_query(&case.query);
    let fallback_queries = build_fallback_queries_from_cleaned_query(&cleaned_query);
    let candidate_limit = 50;
    let cutoff = "2100-01-01T00:00:00Z";

    let initial_records = db_client
        .select_facts_filtered_advanced(
            "org",
            &case.scope,
            cutoff,
            Some(cleaned_query.as_str()),
            candidate_limit,
            None,
            &[],
        )
        .await
        .expect("initial lexical candidates");

    let mut fallback_records = Vec::new();
    for query in &fallback_queries {
        let term_records = db_client
            .select_facts_filtered_advanced(
                "org",
                &case.scope,
                cutoff,
                Some(query.as_str()),
                candidate_limit,
                None,
                &[],
            )
            .await
            .expect("fallback lexical candidates");
        fallback_records.extend(term_records);
    }

    let expected_needles = &case.expected.must_contain;
    let active_experience_records = db_client
        .select_active_facts("org", 500)
        .await
        .expect("active facts")
        .into_iter()
        .filter(|record| {
            record.get("fact_type").and_then(serde_json::Value::as_str) == Some("experience")
        })
        .collect::<Vec<_>>();
    let reranked_initial_records =
        snapshot_rank_lexical_records(&initial_records, cleaned_query.as_str());
    let reranked_fallback_records =
        snapshot_rank_lexical_records(&fallback_records, cleaned_query.as_str());
    let initial_top_contents = initial_records
        .iter()
        .take(10)
        .filter_map(|record| record.get("content").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let fallback_top_contents = fallback_records
        .iter()
        .take(10)
        .filter_map(|record| record.get("content").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let active_experience_top_contents = active_experience_records
        .iter()
        .take(10)
        .filter_map(|record| record.get("content").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let reranked_initial_top_contents = reranked_initial_records
        .iter()
        .take(10)
        .filter_map(|record| record.get("content").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let reranked_fallback_top_contents = reranked_fallback_records
        .iter()
        .take(10)
        .filter_map(|record| record.get("content").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();

    CandidatePoolSnapshot {
        initial_has_expected: record_set_contains_expected(&initial_records, expected_needles),
        fallback_has_expected: record_set_contains_expected(&fallback_records, expected_needles),
        active_experience_has_expected: record_set_contains_expected(
            &active_experience_records,
            expected_needles,
        ),
        reranked_initial_has_expected: record_set_contains_expected(
            &reranked_initial_records,
            expected_needles,
        ),
        reranked_fallback_has_expected: record_set_contains_expected(
            &reranked_fallback_records,
            expected_needles,
        ),
        initial_top_contents,
        fallback_top_contents,
        active_experience_top_contents,
        reranked_initial_top_contents,
        reranked_fallback_top_contents,
    }
}

fn snapshot_rank_lexical_records(
    records: &[serde_json::Value],
    cleaned_query: &str,
) -> Vec<serde_json::Value> {
    let query_terms = cleaned_query
        .split_whitespace()
        .filter(|term| !term.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut reranked = records.to_vec();

    reranked.sort_by(|left, right| {
        snapshot_lexical_query_score(right, &query_terms)
            .cmp(&snapshot_lexical_query_score(left, &query_terms))
            .then_with(|| {
                snapshot_dampened_ft_score(right).total_cmp(&snapshot_dampened_ft_score(left))
            })
            .then_with(|| snapshot_t_valid(right).cmp(&snapshot_t_valid(left)))
            .then_with(|| snapshot_fact_id(left).cmp(&snapshot_fact_id(right)))
    });

    reranked
}

fn snapshot_lexical_query_score(record: &serde_json::Value, query_terms: &[String]) -> usize {
    let content_terms = snapshot_best_matching_content_terms(
        record
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        query_terms,
    );
    let unigram_overlap = snapshot_query_term_overlap_for_terms(&content_terms, query_terms)
        + snapshot_index_key_overlap(record, query_terms);
    let phrase_overlap = snapshot_ngram_overlap_for_terms(&content_terms, query_terms, 2)
        + snapshot_ngram_overlap_for_terms(&content_terms, query_terms, 3);
    let trigram_overlap = snapshot_ngram_overlap_for_terms(&content_terms, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

fn snapshot_best_matching_content_terms(text: &str, query_terms: &[String]) -> Vec<String> {
    let fallback_terms = preprocess_search_query(text)
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if fallback_terms.is_empty() || query_terms.is_empty() {
        return fallback_terms;
    }

    let spans = snapshot_sentence_segments(text)
        .into_iter()
        .map(|segment| {
            preprocess_search_query(&segment)
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|terms| !terms.is_empty())
        .collect::<Vec<_>>();
    if spans.is_empty() {
        return fallback_terms;
    }

    let mut best_terms = spans[0].clone();
    let mut best_score = snapshot_score_content_terms(&best_terms, query_terms);
    let mut best_len = best_terms.len();

    for candidate_terms in spans.into_iter().skip(1) {
        let candidate_score = snapshot_score_content_terms(&candidate_terms, query_terms);
        let should_replace = candidate_score > best_score
            || (candidate_score == best_score && candidate_terms.len() < best_len);
        if should_replace {
            best_score = candidate_score;
            best_len = candidate_terms.len();
            best_terms = candidate_terms;
        }
    }

    best_terms
}

fn snapshot_sentence_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();

    for character in text.trim().chars() {
        current.push(character);
        if matches!(character, '.' | '!' | '?' | ';' | '\n') {
            let segment = current.trim();
            if !segment.is_empty() {
                segments.push(segment.to_string());
            }
            current.clear();
        }
    }

    let trailing = current.trim();
    if !trailing.is_empty() {
        segments.push(trailing.to_string());
    }

    segments
}

fn snapshot_score_content_terms(content_terms: &[String], query_terms: &[String]) -> usize {
    let unigram_overlap = snapshot_query_term_overlap_for_terms(content_terms, query_terms);
    let phrase_overlap = snapshot_ngram_overlap_for_terms(content_terms, query_terms, 2)
        + snapshot_ngram_overlap_for_terms(content_terms, query_terms, 3);
    let trigram_overlap = snapshot_ngram_overlap_for_terms(content_terms, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

fn snapshot_query_term_overlap_for_terms(
    content_terms: &[String],
    query_terms: &[String],
) -> usize {
    let content_terms = content_terms.iter().collect::<HashSet<_>>();
    query_terms
        .iter()
        .filter(|term| content_terms.contains(term))
        .count()
}

fn snapshot_ngram_overlap_for_terms(
    content_terms: &[String],
    query_terms: &[String],
    width: usize,
) -> usize {
    if content_terms.len() < width || query_terms.len() < width {
        return 0;
    }

    let content_ngrams = content_terms
        .windows(width)
        .map(|window| window.join(" "))
        .collect::<HashSet<_>>();
    query_terms
        .windows(width)
        .filter(|window| content_ngrams.contains(&window.join(" ")))
        .count()
}

fn snapshot_index_key_overlap(record: &serde_json::Value, query_terms: &[String]) -> usize {
    let terms = record
        .get("index_keys")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .flat_map(|index_key| {
            preprocess_search_query(index_key)
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    snapshot_query_term_overlap_for_terms(&terms, query_terms)
}

fn snapshot_dampened_ft_score(record: &serde_json::Value) -> f64 {
    record
        .get("ft_score")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
        .max(0.0)
        .ln_1p()
}

fn snapshot_t_valid(record: &serde_json::Value) -> String {
    record
        .get("t_valid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn snapshot_fact_id(record: &serde_json::Value) -> String {
    record
        .get("fact_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn build_fallback_queries_from_cleaned_query(cleaned_query: &str) -> Vec<String> {
    let query_terms = cleaned_query
        .split_whitespace()
        .filter(|term| !term.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut queries = Vec::new();

    for width in (2..=3).rev() {
        if query_terms.len() < width {
            continue;
        }
        for window in query_terms.windows(width) {
            let query = window.join(" ");
            if !queries.contains(&query) {
                queries.push(query);
            }
        }
    }

    for term in query_terms {
        if !queries.contains(&term) {
            queries.push(term);
        }
    }

    queries
}

fn record_set_contains_expected(
    records: &[serde_json::Value],
    expected_needles: &[String],
) -> bool {
    expected_needles.iter().any(|needle| {
        records.iter().any(|record| {
            record
                .get("content")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|content| content.contains(needle.as_str()))
        })
    })
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
    let source_episodes = items
        .iter()
        .map(|item| item.source_episode.clone())
        .collect::<Vec<_>>();

    RetrievalCaseOutcome {
        expected_hits: case.expected.must_contain.len(),
        case,
        matched_hits,
        actual_tiers,
        source_episodes,
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
    let retrieved_content_refs = outcome
        .retrieved_contents
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let source_episode_refs = outcome
        .source_episodes
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let passed = record_retrieval_case(
        summary,
        &outcome.case.expected.tier,
        outcome.matched_hits,
        outcome.expected_hits,
        outcome.case.expected.min_recall_at_k,
        RetrievalCaseDiagnostics {
            actual_tiers: &actual_tier_refs,
            first_relevant_rank: first_relevant_rank(
                &retrieved_content_refs,
                &outcome.case.expected.must_contain,
            ),
            source_episodes: &source_episode_refs,
            min_unique_source_episodes: None,
            max_source_episode_share: None,
        },
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
    let raw = raw_dataset_raw(DatasetKind::LongMemEvalCleaned);

    let cases = normalize_external_dataset(DatasetKind::LongMemEvalCleaned, &raw)
        .expect("normalize longmemeval fixture");

    assert_eq!(cases.len(), 500);
    let case = cases
        .iter()
        .find(|case| case.id == "longmemeval-cleaned:e47becba")
        .expect("canonical longmemeval case");
    assert_eq!(case.dataset, "longmemeval-cleaned");
    assert_eq!(case.id, "longmemeval-cleaned:e47becba");
    assert_eq!(case.query, "What degree did I graduate with?");
    assert_eq!(case.expected.tier, "direct");
    assert_eq!(case.expected.must_contain, vec!["Business Administration"]);
    assert_eq!(case.facts.len(), 550);
    assert_eq!(
        case.facts[0].content,
        "The farmer needs to transport a fox, a chicken, and some grain across a river using a boat. The fox cannot be left alone with the chicken, and the chicken cannot be left alone with the grain. The boat can only hold one item at a time, and the river is too dangerous to cross multiple times. Can you help the farmer transport all three items across the river without any of them getting eaten? Remember, strategic thinking and planning are key to solving this puzzle. If you're stuck, try thinking about how you would solve the puzzle yourself, and use that as a starting point. Be careful not to leave the chicken alone with the fox, or the chicken and the grain alone together, as this will result in a failed solution. Good luck!"
    );
    assert_eq!(case.facts[0].t_valid, "2023-05-20T02:21:00+00:00");
    assert!(case.facts.iter().any(|fact| {
        fact.content
            .contains("I graduated with a degree in Business Administration")
    }));
    assert_eq!(case.metadata["question_type"], "single-session-user");
}

#[test]
fn normalizes_locomo_fixture_into_canonical_cases() {
    let raw = raw_dataset_raw(DatasetKind::LoCoMo);

    let cases =
        normalize_external_dataset(DatasetKind::LoCoMo, &raw).expect("normalize locomo fixture");

    assert_eq!(cases.len(), 1986);
    let case = cases
        .iter()
        .find(|case| case.id == "locomo:conv-26:0")
        .expect("canonical locomo case");
    assert_eq!(case.dataset, "locomo");
    assert_eq!(case.id, "locomo:conv-26:0");
    assert_eq!(
        case.query,
        "When did Caroline go to the LGBTQ support group?"
    );
    assert_eq!(case.expected.tier, "temporal");
    assert_eq!(
        case.expected.must_contain,
        vec!["Caroline attends an LGBTQ support group for the first time."]
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
    assert_eq!(case.metadata["category"], 2);
    assert_eq!(case.metadata["evidence"][0], "D1:3");
}

#[test]
fn normalizes_personamem_fixture_into_canonical_cases() {
    let raw = raw_dataset_raw(DatasetKind::PersonaMem);

    let cases = normalize_external_dataset(DatasetKind::PersonaMem, &raw)
        .expect("normalize personamem fixture");

    assert_eq!(cases.len(), 100);
    let case = cases
        .iter()
        .find(|case| case.id == "personamem:acd74206-37dc-4756-94a8-b99a395d9a21")
        .expect("canonical personamem case");
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
    assert_eq!(case.facts.len(), 182);
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
    let raw = raw_dataset_raw(DatasetKind::PrefEval);

    let cases = normalize_external_dataset(DatasetKind::PrefEval, &raw)
        .expect("normalize prefeval fixture");

    assert_eq!(cases.len(), 52);
    let case = cases
        .iter()
        .find(|case| case.id == "prefeval:travel_hotel_overall300_topk_history_persona:0")
        .expect("canonical prefeval case");
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
        vec![
            "I usually prefer quieter hotels away from the city center, as I absolutely avoid hotels with a bustling nightlife atmosphere."
        ]
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

    let paraphrased_case = cases
        .iter()
        .find(|case| case.id == "prefeval:travel_hotel_overall300_topk_history_persona:1")
        .expect("canonical prefeval paraphrased case");
    assert_eq!(
        paraphrased_case.expected.must_contain,
        vec!["I tend to avoid high-rise buildings for accommodations."]
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
async fn external_seed_case_facts_use_distinct_source_episodes() {
    let (service, db_client) = common::make_service_with_client().await;
    let seeded_facts = vec![
        NormalizedSeedFact {
            content: "Alice Smith graduated with a degree in Business Administration.".to_string(),
            t_valid: "2023-05-20T02:21:00+00:00".to_string(),
        },
        NormalizedSeedFact {
            content: "Alice Smith later moved to Seattle to start a new role.".to_string(),
            t_valid: "2023-05-21T09:00:00+00:00".to_string(),
        },
    ];

    seed_case_facts(&service, "org", &seeded_facts).await;

    let note_facts = db_client
        .select_table("fact", "org")
        .await
        .expect("seeded facts")
        .into_iter()
        .filter_map(|record| memory_mcp::service::fact_from_record(&record))
        .filter(|fact| fact.fact_type == "note")
        .filter(|fact| {
            seeded_facts
                .iter()
                .any(|seeded| seeded.content == fact.content)
        })
        .collect::<Vec<_>>();

    assert_eq!(note_facts.len(), seeded_facts.len());

    let unique_source_episodes = note_facts
        .iter()
        .map(|fact| fact.source_episode.clone())
        .collect::<HashSet<_>>();

    assert_eq!(
        unique_source_episodes.len(),
        seeded_facts.len(),
        "expected each seeded note fact to keep its own source episode so selection caps do not collapse the batch"
    );
}

#[tokio::test]
async fn external_seed_case_facts_populate_entity_backed_index_keys() {
    let (service, db_client) = common::make_service_with_client().await;
    let seeded_fact = NormalizedSeedFact {
        content: "Alice Smith graduated with a degree in Business Administration.".to_string(),
        t_valid: "2023-05-20T02:21:00+00:00".to_string(),
    };

    seed_case_facts(&service, "org", std::slice::from_ref(&seeded_fact)).await;

    let note_fact = db_client
        .select_table("fact", "org")
        .await
        .expect("seeded facts")
        .into_iter()
        .filter_map(|record| memory_mcp::service::fact_from_record(&record))
        .find(|fact| fact.fact_type == "note" && fact.content == seeded_fact.content)
        .expect("seeded note fact should exist");

    assert!(
        note_fact.index_keys.iter().any(|key| key == "alice smith"),
        "expected external eval seeding to preserve canonical entity names in index_keys"
    );
}

#[tokio::test]
async fn personamem_music_blend_case_recalls_expected_context() {
    let case_id = "personamem:acd74206-37dc-4756-94a8-b99a395d9a21";
    let case = load_external_dataset_cases(DatasetKind::PersonaMem)
        .await
        .expect("load personamem cases")
        .into_iter()
        .find(|case| case.id == case_id)
        .unwrap_or_else(|| panic!("find personamem case {case_id}"));

    let (service, db_client) = common::make_service_with_client().await;
    seed_case_facts(&service, &case.scope, &case.facts).await;
    let snapshot = build_candidate_pool_snapshot(db_client.as_ref(), &case).await;

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
        .expect("assemble personamem case");

    assert!(
        items.iter().any(|item| {
            case.expected
                .must_contain
                .iter()
                .any(|needle| item.content.contains(needle.as_str()))
        }),
        "expected {case_id} to retrieve the gold memory; initial_has_expected={} fallback_has_expected={} reranked_initial_has_expected={} reranked_fallback_has_expected={} active_experience_has_expected={} initial_top={:?} reranked_initial_top={:?} fallback_top={:?} reranked_fallback_top={:?} active_experience_top={:?} retrieved={:?}",
        snapshot.initial_has_expected,
        snapshot.fallback_has_expected,
        snapshot.reranked_initial_has_expected,
        snapshot.reranked_fallback_has_expected,
        snapshot.active_experience_has_expected,
        snapshot.initial_top_contents,
        snapshot.reranked_initial_top_contents,
        snapshot.fallback_top_contents,
        snapshot.reranked_fallback_top_contents,
        snapshot.active_experience_top_contents,
        items
            .iter()
            .map(|item| item.content.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn personamem_fulfilling_music_expression_case_recalls_reflective_context() {
    let case_id = "personamem:b3588797-acdf-40d3-bcc5-951f81896f95";
    let case = load_external_dataset_cases(DatasetKind::PersonaMem)
        .await
        .expect("load personamem cases")
        .into_iter()
        .find(|candidate| candidate.id == case_id)
        .unwrap_or_else(|| panic!("find personamem case {case_id}"));

    let (service, db_client) = common::make_service_with_client().await;
    seed_case_facts(&service, &case.scope, &case.facts).await;

    let snapshot = build_candidate_pool_snapshot(db_client.as_ref(), &case).await;
    let items = service
        .assemble_context(AssembleContextRequest {
            query: case.query.clone(),
            scope: case.scope.clone(),
            fact_types: vec![],
            as_of: None,
            budget: case.budget,
            project: None,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble personamem case");

    assert!(
        case.expected.must_contain.iter().all(|needle| {
            items
                .iter()
                .any(|item| item.content.contains(needle.as_str()))
        }),
        "expected {case_id} to retrieve the reflective gold memory; initial_has_expected={} fallback_has_expected={} reranked_initial_has_expected={} reranked_fallback_has_expected={} active_experience_has_expected={} initial_top={:?} reranked_initial_top={:?} fallback_top={:?} reranked_fallback_top={:?} active_experience_top={:?} retrieved={:?}",
        snapshot.initial_has_expected,
        snapshot.fallback_has_expected,
        snapshot.reranked_initial_has_expected,
        snapshot.reranked_fallback_has_expected,
        snapshot.active_experience_has_expected,
        snapshot.initial_top_contents,
        snapshot.reranked_initial_top_contents,
        snapshot.fallback_top_contents,
        snapshot.reranked_fallback_top_contents,
        snapshot.active_experience_top_contents,
        items
            .iter()
            .map(|item| item.content.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn personamem_weekend_getaway_case_recalls_creative_preference_context() {
    let case_id = "personamem:f546a74f-54de-40d0-9d88-8b0e30467d7b";
    let case = load_external_dataset_cases(DatasetKind::PersonaMem)
        .await
        .expect("load personamem cases")
        .into_iter()
        .find(|candidate| candidate.id == case_id)
        .unwrap_or_else(|| panic!("find personamem case {case_id}"));

    let (service, db_client) = common::make_service_with_client().await;
    seed_case_facts(&service, &case.scope, &case.facts).await;

    let snapshot = build_candidate_pool_snapshot(db_client.as_ref(), &case).await;
    let items = service
        .assemble_context(AssembleContextRequest {
            query: case.query.clone(),
            scope: case.scope.clone(),
            fact_types: vec![],
            as_of: None,
            budget: case.budget,
            project: None,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble personamem case");

    assert!(
        case.expected.must_contain.iter().all(|needle| {
            items
                .iter()
                .any(|item| item.content.contains(needle.as_str()))
        }),
        "expected {case_id} to retrieve the creative-preference gold memory; initial_has_expected={} fallback_has_expected={} reranked_initial_has_expected={} reranked_fallback_has_expected={} active_experience_has_expected={} initial_top={:?} reranked_initial_top={:?} fallback_top={:?} reranked_fallback_top={:?} active_experience_top={:?} retrieved={:?}",
        snapshot.initial_has_expected,
        snapshot.fallback_has_expected,
        snapshot.reranked_initial_has_expected,
        snapshot.reranked_fallback_has_expected,
        snapshot.active_experience_has_expected,
        snapshot.initial_top_contents,
        snapshot.reranked_initial_top_contents,
        snapshot.fallback_top_contents,
        snapshot.reranked_fallback_top_contents,
        snapshot.active_experience_top_contents,
        items
            .iter()
            .map(|item| item.content.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn personamem_podcasting_shift_case_recalls_previous_reason_context() {
    let case_id = "personamem:a40d5f67-8ec6-480b-a901-9709eecee9b9";
    let case = load_external_dataset_cases(DatasetKind::PersonaMem)
        .await
        .expect("load personamem cases")
        .into_iter()
        .find(|candidate| candidate.id == case_id)
        .unwrap_or_else(|| panic!("find personamem case {case_id}"));

    let (service, db_client) = common::make_service_with_client().await;
    seed_case_facts(&service, &case.scope, &case.facts).await;

    let snapshot = build_candidate_pool_snapshot(db_client.as_ref(), &case).await;
    let items = service
        .assemble_context(AssembleContextRequest {
            query: case.query.clone(),
            scope: case.scope.clone(),
            fact_types: vec![],
            as_of: None,
            budget: case.budget,
            project: None,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble personamem case");

    assert!(
        case.expected.must_contain.iter().all(|needle| {
            items
                .iter()
                .any(|item| item.content.contains(needle.as_str()))
        }),
        "expected {case_id} to retrieve the prior-reason gold memory; initial_has_expected={} fallback_has_expected={} reranked_initial_has_expected={} reranked_fallback_has_expected={} active_experience_has_expected={} initial_top={:?} reranked_initial_top={:?} fallback_top={:?} reranked_fallback_top={:?} active_experience_top={:?} retrieved={:?}",
        snapshot.initial_has_expected,
        snapshot.fallback_has_expected,
        snapshot.reranked_initial_has_expected,
        snapshot.reranked_fallback_has_expected,
        snapshot.active_experience_has_expected,
        snapshot.initial_top_contents,
        snapshot.reranked_initial_top_contents,
        snapshot.fallback_top_contents,
        snapshot.reranked_fallback_top_contents,
        snapshot.active_experience_top_contents,
        items
            .iter()
            .map(|item| item.content.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn prefeval_hotel_persona_case_recalls_expected_preference() {
    let case_id = "prefeval:travel_hotel_overall300_topk_history_persona:1";
    let case = load_external_dataset_cases(DatasetKind::PrefEval)
        .await
        .expect("load prefeval cases")
        .into_iter()
        .find(|case| case.id == case_id)
        .unwrap_or_else(|| panic!("find prefeval case {case_id}"));

    let (service, db_client) = common::make_service_with_client().await;
    seed_case_facts(&service, &case.scope, &case.facts).await;
    let snapshot = build_candidate_pool_snapshot(db_client.as_ref(), &case).await;

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
        .expect("assemble prefeval case");

    assert!(
        items.iter().any(|item| {
            case.expected
                .must_contain
                .iter()
                .any(|needle| item.content.contains(needle.as_str()))
        }),
        "expected {case_id} to retrieve the gold preference; initial_has_expected={} fallback_has_expected={} reranked_initial_has_expected={} reranked_fallback_has_expected={} active_experience_has_expected={} initial_top={:?} reranked_initial_top={:?} fallback_top={:?} reranked_fallback_top={:?} active_experience_top={:?} retrieved={:?}",
        snapshot.initial_has_expected,
        snapshot.fallback_has_expected,
        snapshot.reranked_initial_has_expected,
        snapshot.reranked_fallback_has_expected,
        snapshot.active_experience_has_expected,
        snapshot.initial_top_contents,
        snapshot.reranked_initial_top_contents,
        snapshot.fallback_top_contents,
        snapshot.reranked_fallback_top_contents,
        snapshot.active_experience_top_contents,
        items
            .iter()
            .map(|item| item.content.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore]
async fn locomo_full_conv26_first_case_retrieves_expected_context() {
    let case_id =
        std::env::var("MEMORY_MCP_EVAL_CASE_ID").unwrap_or_else(|_| "locomo:conv-26:0".to_string());

    let case = load_external_dataset_cases(DatasetKind::LoCoMo)
        .await
        .expect("load locomo cases")
        .into_iter()
        .find(|case| case.id == case_id)
        .unwrap_or_else(|| panic!("find locomo case {case_id}"));

    let mut summary = RetrievalSuiteSummary::default();
    run_retrieval_case(case, &mut summary, true).await;
}

#[tokio::test]
#[ignore]
async fn longmemeval_case_58bf7951_retrieves_expected_context() {
    let case_id = std::env::var("MEMORY_MCP_EVAL_CASE_ID")
        .unwrap_or_else(|_| "longmemeval-cleaned:58bf7951".to_string());

    let case = load_external_dataset_cases(DatasetKind::LongMemEvalCleaned)
        .await
        .expect("load longmemeval cases")
        .into_iter()
        .find(|case| case.id == case_id)
        .unwrap_or_else(|| panic!("find longmemeval case {case_id}"));

    let mut summary = RetrievalSuiteSummary::default();
    run_retrieval_case(case, &mut summary, true).await;
}

#[tokio::test]
#[ignore]
async fn run_longmemeval_retrieval() {
    run_dataset_retrieval("longmemeval", DatasetKind::LongMemEvalCleaned, true).await;
}

#[tokio::test]
#[ignore]
async fn run_locomo_retrieval() {
    run_dataset_retrieval("locomo", DatasetKind::LoCoMo, true).await;
}

#[tokio::test]
#[ignore]
async fn run_personamem_retrieval() {
    run_dataset_retrieval("personamem", DatasetKind::PersonaMem, true).await;
}

#[tokio::test]
#[ignore]
async fn report_personamem_retrieval_metrics() {
    run_dataset_retrieval("personamem", DatasetKind::PersonaMem, false).await;
}

#[tokio::test]
#[ignore]
async fn run_prefeval_retrieval() {
    run_dataset_retrieval("prefeval", DatasetKind::PrefEval, true).await;
}
