mod eval_support;

use chrono::{Duration, TimeZone, Utc};
use eval_support::dataset::parse_latency_cases;
use eval_support::metrics::LatencySuiteSummary;
use eval_support::report::print_latency_summary;
use memory_mcp::models::{AssembleContextRequest, IngestRequest};
use std::time::Instant;

// Important: this runner must use the existing in-memory test service only.
// Do not switch to RocksDB, remote SurrealDB, or Criterion baseline storage.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "eval: manual latency run"]
async fn run_latency_evals() {
    let raw = std::fs::read_to_string("tests/fixtures/evals/latency_cases.json").unwrap();
    let cases = parse_latency_cases(&raw).unwrap();
    let mut summary = LatencySuiteSummary::default();

    for case in cases {
        // Use GLiNER + LocalCandle embeddings for accurate eval
        let service = eval_support::common::make_service_with_gliner_and_embeddings().await;

        for index in 0..case.episode_count {
            let t_ref = Utc.with_ymd_and_hms(2026, 3, 1, 9, 0, 0).unwrap()
                + Duration::minutes(index as i64);
            let episode_id = service
                .ingest(
                    IngestRequest {
                        source_type: "eval".to_string(),
                        source_id: format!("{}-seed-{}", case.id, index),
                        content: format!("Project Atlas status note {}", index),
                        t_ref,
                        scope: case.scope.clone(),
                        t_ingested: None,
                        visibility_scope: None,
                        policy_tags: vec![],
                    },
                    None,
                )
                .await
                .unwrap();
            service.extract(&episode_id, None).await.unwrap();
        }

        for _ in 0..case.warmup_iterations {
            let _ = service
                .assemble_context(AssembleContextRequest {
                    query: case.query.clone(),
                    scope: case.scope.clone(),
                    as_of: None,
                    budget: 5,
                    view_mode: None,
                    window_start: None,
                    window_end: None,
                    access: None,
                })
                .await
                .unwrap();
        }

        for iteration in 0..case.measured_iterations {
            let ingest_started = Instant::now();
            let episode_id = service
                .ingest(
                    IngestRequest {
                        source_type: "eval".to_string(),
                        source_id: format!("{}-measure-{}", case.id, iteration),
                        content: format!("Measured Atlas event {}", iteration),
                        t_ref: Utc::now(),
                        scope: case.scope.clone(),
                        t_ingested: None,
                        visibility_scope: None,
                        policy_tags: vec![],
                    },
                    None,
                )
                .await
                .unwrap();
            summary
                .ingest_ms
                .push(ingest_started.elapsed().as_secs_f64() * 1000.0);

            let extract_started = Instant::now();
            service.extract(&episode_id, None).await.unwrap();
            summary
                .extract_ms
                .push(extract_started.elapsed().as_secs_f64() * 1000.0);

            let assemble_started = Instant::now();
            let _ = service
                .assemble_context(AssembleContextRequest {
                    query: case.query.clone(),
                    scope: case.scope.clone(),
                    as_of: None,
                    budget: 5,
                    view_mode: None,
                    window_start: None,
                    window_end: None,
                    access: None,
                })
                .await
                .unwrap();
            summary
                .assemble_ms
                .push(assemble_started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    print_latency_summary(&summary);
    assert!(!summary.ingest_ms.is_empty());
    assert!(!summary.extract_ms.is_empty());
    assert!(!summary.assemble_ms.is_empty());
}
