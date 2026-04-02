mod eval_support;

use chrono::{DateTime, Utc};
use eval_support::dataset::parse_retrieval_cases;
use eval_support::metrics::RetrievalSuiteSummary;
use eval_support::report::print_retrieval_summary;
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::service::MemoryError;

/// Runs retrieval evals against LongMemEval oracle fixtures.
///
/// Command: cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --nocapture --test-threads=1
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "eval: manual LongMemEval retrieval quality run"]
async fn run_longmemeval_retrieval() {
    run_external_retrieval_suite(
        "tests/fixtures/evals/retrieval_longmemeval.json",
        "longmemeval",
    )
    .await;
}

/// Runs retrieval evals against MemoryAgentBench fixtures.
///
/// Command: cargo test --test eval_external_retrieval run_memory_agent_bench_retrieval -- --ignored --nocapture --test-threads=1
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "eval: manual MemoryAgentBench retrieval quality run"]
async fn run_memory_agent_bench_retrieval() {
    run_external_retrieval_suite(
        "tests/fixtures/evals/retrieval_memory_agent_bench.json",
        "memory_agent_bench",
    )
    .await;
}

async fn run_external_retrieval_suite(fixture_path: &str, suite_name: &str) {
    let raw = std::fs::read_to_string(fixture_path).unwrap_or_else(|e| {
        panic!("Failed to read {fixture_path}: {e}. Run: python3 scripts/convert_external_evals.py --all")
    });
    let cases = parse_retrieval_cases(&raw).unwrap();
    let mut summary = RetrievalSuiteSummary::default();

    println!("Running {suite_name}: {} cases", cases.len());

    // Use a single file-based RocksDB service for the entire eval run.
    // File-based storage is stable under heavy load — no need to recycle.
    let (service, _temp_dir) = eval_support::common::make_file_service().await;

    const BATCH_SIZE: usize = 1;
    for (batch_idx, case_batch) in cases.chunks(BATCH_SIZE).enumerate() {
        println!("  [batch {batch_idx}]");

        for (offset, case) in case_batch.iter().enumerate() {
            let case_idx = batch_idx * BATCH_SIZE + offset;
            summary.total_cases += 1;
            let case_prefix = format!("{suite_name}_{case_idx}");

            // Use add_fact for all suites — we're testing retrieval quality,
            // not extraction. Running GLiNER NER on 200 large chat episodes
            // on CPU takes ~50 minutes; add_fact completes in <1 minute.
            let ingest_result = async {
                for episode in &case.episodes {
                    let _fact_id = eval_support::common::add_fact(
                        &service,
                        "general",
                        &episode.content,
                        &episode.content,
                        &format!("{case_prefix}_{}", episode.source_id),
                        episode.t_ref.parse::<DateTime<Utc>>().unwrap(),
                        &episode.scope,
                        0.9,
                        vec![],
                        vec![],
                        serde_json::json!({
                            "source_type": episode.source_type,
                            "source_id": episode.source_id,
                        }),
                    )
                    .await
                    .map_err(|e| MemoryError::Storage(e.to_string()))?;
                }
                Ok::<_, MemoryError>(())
            }
            .await;

            if let Err(e) = ingest_result {
                eprintln!("INGEST ERROR {}: {} — skipping", case.id, e);
                continue;
            }

            let items = service
                .assemble_context(AssembleContextRequest {
                    query: case.query.query.clone(),
                    scope: case.query.scope.clone(),
                    as_of: case
                        .query
                        .as_of
                        .as_ref()
                        .map(|ts| ts.parse::<DateTime<Utc>>().unwrap()),
                    budget: case.query.budget,
                    view_mode: None,
                    window_start: None,
                    window_end: None,
                    access: None,
                })
                .await
                .unwrap_or_default();

            let contents: Vec<String> = items.iter().map(|item| item.content.clone()).collect();

            let case_ok = eval_support::metrics::record_retrieval_case(
                &mut summary,
                &case.expected.must_contain,
                &case.expected.must_not_contain,
                case.expected.expect_empty,
                &contents,
                &case.expected.tier,
            );

            if !case_ok {
                eprintln!(
                    "FAILED {}: query={}",
                    case.id,
                    case.query.query.chars().take(80).collect::<String>()
                );
            }
        }
    }

    print_retrieval_summary(&summary);

    let total = summary.total_cases.max(1) as f64;
    let recall_at_5 = summary.recall_at_k_sum / total;
    let empty_when_irrelevant = summary.empty_when_irrelevant_hits as f64 / total;

    println!(
        "{suite_name} summary: recall_at_5={recall_at_5:.3}, empty_when_irrelevant={empty_when_irrelevant:.3}, total={}",
        summary.total_cases
    );

    // Use relaxed thresholds for external benchmarks — these are informational
    assert!(
        recall_at_5 >= 0.10,
        "recall_at_5 dropped below 0.10 for {suite_name}"
    );
}
