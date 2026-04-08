mod common;

use std::time::Instant;

use chrono::{TimeZone, Utc};
use memory_mcp::models::{AssembleContextRequest, IngestRequest};

const INGEST_P95_TARGET_MS: f64 = 200.0;
const ASSEMBLE_P95_TARGET_MS: f64 = 50.0;

fn percentile_ms(samples: &[f64], percentile: f64) -> f64 {
    assert!(!samples.is_empty(), "samples must not be empty");
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let index = (((sorted.len() - 1) as f64) * percentile).ceil() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn assert_latency_targets(ingest_p95_ms: f64, assemble_p95_ms: f64) {
    assert!(
        ingest_p95_ms <= INGEST_P95_TARGET_MS,
        "expected ingest_p95 <= {:.2}ms, got {:.2}ms",
        INGEST_P95_TARGET_MS,
        ingest_p95_ms,
    );
    assert!(
        assemble_p95_ms <= ASSEMBLE_P95_TARGET_MS,
        "expected assemble_p95 <= {:.2}ms, got {:.2}ms",
        ASSEMBLE_P95_TARGET_MS,
        assemble_p95_ms,
    );
}

#[tokio::test]
#[ignore]
async fn run_latency_evals() {
    const SAMPLE_COUNT: usize = 20;

    let service = common::make_service().await;
    let mut ingest_ms = Vec::with_capacity(SAMPLE_COUNT);
    let mut assemble_ms = Vec::with_capacity(SAMPLE_COUNT);

    for idx in 0..SAMPLE_COUNT {
        let start = Instant::now();
        service
            .ingest(
                IngestRequest {
                    source_type: "email".to_string(),
                    source_id: format!("latency-ingest-{idx}"),
                    content: format!(
                        "ARR grew to ${}M. I will send update {} by Friday.",
                        idx + 1,
                        idx
                    ),
                    t_ref: Utc.with_ymd_and_hms(2026, 4, 7, 10, 0, idx as u32).unwrap(),
                    scope: "org".to_string(),
                    project: None,
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .unwrap_or_else(|err| panic!("latency ingest case {idx} failed: {err}"));
        ingest_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    for idx in 0..SAMPLE_COUNT {
        common::seed_fact_at(
            &service,
            "org",
            &format!("latency-case-{idx} owner is Alice"),
            Utc.with_ymd_and_hms(2026, 4, 7, 12, 0, idx as u32).unwrap(),
        )
        .await;
    }

    for idx in 0..SAMPLE_COUNT {
        let start = Instant::now();
        let items = service
            .assemble_context(AssembleContextRequest {
                query: format!("latency-case-{idx}"),
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
            .unwrap_or_else(|err| panic!("latency assemble case {idx} failed: {err}"));
        assert!(
            !items.is_empty(),
            "expected latency assemble case {idx} to return results"
        );
        assemble_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let ingest_p50 = percentile_ms(&ingest_ms, 0.50);
    let ingest_p95 = percentile_ms(&ingest_ms, 0.95);
    let assemble_p50 = percentile_ms(&assemble_ms, 0.50);
    let assemble_p95 = percentile_ms(&assemble_ms, 0.95);

    println!(
        "suite=eval_latency ingest_p50_ms={:.2} ingest_p95_ms={:.2} assemble_p50_ms={:.2} assemble_p95_ms={:.2}",
        ingest_p50, ingest_p95, assemble_p50, assemble_p95,
    );

    assert_latency_targets(ingest_p95, assemble_p95);
}

#[test]
fn percentile_ms_uses_upper_rank_rounding() {
    let samples = vec![1.0, 2.0, 3.0, 4.0, 5.0];

    assert_eq!(percentile_ms(&samples, 0.50), 3.0);
    assert_eq!(percentile_ms(&samples, 0.95), 5.0);
}

#[test]
fn latency_targets_accept_plan_thresholds() {
    assert_latency_targets(1.70, 12.41);
}

#[test]
#[should_panic(expected = "expected assemble_p95 <= 50.00ms")]
fn latency_targets_reject_slow_assemble_p95() {
    assert_latency_targets(10.0, 55.0);
}
