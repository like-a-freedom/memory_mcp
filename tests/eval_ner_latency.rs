use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use memory_mcp::config::{NerConfig, NerDeviceKind, NerProviderKind};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::models::EntityCandidate;
use memory_mcp::service::{EntityExtractor, create_entity_extractor};
use serde::Serialize;

const BENCHMARK_ITERATIONS: usize = 10;
const CONTENTION_CLIENTS: usize = 4;
const CONTENTION_ROUNDS: usize = 3;
const ONE_WINDOW_REPETITIONS: usize = 8;
const MULTI_WINDOW_REPETITIONS: usize = 40;
const BENCHMARK_PARAGRAPH: &str =
    "Alice Smith from OpenAI presented Project Atlas in Moscow using Rust and Kubernetes. ";

type CandidateSignature = Vec<(String, String)>;

#[derive(Serialize)]
struct NerLatencyReport {
    provider: &'static str,
    model: &'static str,
    device: &'static str,
    threshold: f64,
    batch_size: usize,
    max_batch_tokens: usize,
    max_concurrency: usize,
    iterations: usize,
    scenarios: Vec<ScenarioReport>,
}

#[derive(Serialize)]
struct ScenarioReport {
    scenario: &'static str,
    content_words: usize,
    samples_ms: Vec<f64>,
    p50_ms: f64,
    p95_ms: f64,
    candidates: CandidateSignature,
}

#[derive(Serialize)]
struct ContentionReport {
    device: &'static str,
    batch_size: usize,
    max_batch_tokens: usize,
    max_concurrency: usize,
    clients: usize,
    rounds: usize,
    wall_samples_ms: Vec<f64>,
    wall_p50_ms: f64,
    wall_p95_ms: f64,
    per_request_samples_ms: Vec<f64>,
    per_request_p95_ms: f64,
    throughput_requests_per_s: f64,
    candidates: CandidateSignature,
}

fn assert_release_build() {
    #[cfg(debug_assertions)]
    panic!("NER latency measurements are valid only in a --release build");
}

fn percentile(samples: &[f64], percentile: f64) -> f64 {
    assert!(!samples.is_empty(), "percentile requires samples");
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (percentile * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn candidate_signature(candidates: &[EntityCandidate]) -> CandidateSignature {
    candidates
        .iter()
        .map(|candidate| {
            (
                candidate.canonical_name.clone(),
                candidate.entity_type.clone(),
            )
        })
        .collect()
}

fn benchmark_config() -> NerConfig {
    let model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/models/ner/urchade--gliner_multi-v2.1");
    let mut config = NerConfig::default();
    config.provider = NerProviderKind::LocalGliner;
    config.model = Some("urchade/gliner_multi-v2.1".to_string());
    config.model_dir = Some(model_dir.to_string_lossy().to_string());
    config.threshold = 0.5;
    config.batch_size = std::env::var("NER_BENCH_BATCH_SIZE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(config.batch_size);
    config.max_batch_tokens = 1536;
    config.max_concurrency = 1;
    config.device = NerDeviceKind::Cpu;
    config
}

async fn benchmark_extractor(config: &NerConfig) -> Arc<dyn EntityExtractor> {
    create_entity_extractor(
        config,
        env!("CARGO_MANIFEST_DIR"),
        &StdoutLogger::new("debug"),
    )
    .await
    .expect("load local GLiNER model")
}

async fn measure_scenario(
    extractor: &dyn EntityExtractor,
    scenario: &'static str,
    content: &str,
) -> ScenarioReport {
    let warm_candidates = extractor
        .extract_candidates(content)
        .await
        .expect("warm GLiNER");
    let expected_candidates = candidate_signature(&warm_candidates);
    let mut samples_ms = Vec::with_capacity(BENCHMARK_ITERATIONS);

    for _ in 0..BENCHMARK_ITERATIONS {
        let started = Instant::now();
        let candidates = extractor
            .extract_candidates(content)
            .await
            .expect("extract entities");
        samples_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(
            candidate_signature(&candidates),
            expected_candidates,
            "candidate signature changed between benchmark iterations"
        );
    }

    ScenarioReport {
        scenario,
        content_words: content.split_whitespace().count(),
        p50_ms: percentile(&samples_ms, 0.50),
        p95_ms: percentile(&samples_ms, 0.95),
        samples_ms,
        candidates: expected_candidates,
    }
}

#[test]
fn percentile_uses_nearest_rank() {
    assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.50), 2.0);
    assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 0.95), 4.0);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the local GLiNER model and release-mode timing"]
async fn run_gliner_latency_eval() {
    assert_release_build();
    let config = benchmark_config();
    let extractor = benchmark_extractor(&config).await;
    let one_window = BENCHMARK_PARAGRAPH.repeat(ONE_WINDOW_REPETITIONS);
    let multi_window = BENCHMARK_PARAGRAPH.repeat(MULTI_WINDOW_REPETITIONS);
    let scenarios = vec![
        measure_scenario(extractor.as_ref(), "one_window", &one_window).await,
        measure_scenario(extractor.as_ref(), "multi_window", &multi_window).await,
    ];

    let report = NerLatencyReport {
        provider: extractor.provider_name(),
        model: "urchade/gliner_multi-v2.1",
        device: "cpu",
        threshold: config.threshold,
        batch_size: config.batch_size,
        max_batch_tokens: config.max_batch_tokens,
        max_concurrency: config.max_concurrency,
        iterations: BENCHMARK_ITERATIONS,
        scenarios,
    };
    println!(
        "{}",
        serde_json::to_string(&report).expect("serialize report")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the local GLiNER model and release-mode timing"]
async fn run_contention_eval() {
    assert_release_build();
    let config = benchmark_config();
    let extractor = benchmark_extractor(&config).await;
    let content = BENCHMARK_PARAGRAPH.repeat(MULTI_WINDOW_REPETITIONS);
    let expected_candidates = candidate_signature(
        &extractor
            .extract_candidates(&content)
            .await
            .expect("warm GLiNER"),
    );

    let mut wall_samples_ms = Vec::with_capacity(CONTENTION_ROUNDS);
    let mut per_request_samples_ms = Vec::with_capacity(CONTENTION_CLIENTS * CONTENTION_ROUNDS);
    for _ in 0..CONTENTION_ROUNDS {
        let round_started = Instant::now();
        let handles = (0..CONTENTION_CLIENTS)
            .map(|_| {
                let extractor = Arc::clone(&extractor);
                let content = content.clone();
                tokio::spawn(async move {
                    let started = Instant::now();
                    let candidates = extractor.extract_candidates(&content).await?;
                    Ok::<_, memory_mcp::MemoryError>((
                        started.elapsed().as_secs_f64() * 1_000.0,
                        candidate_signature(&candidates),
                    ))
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            let (request_ms, candidates) = handle
                .await
                .expect("client task panicked")
                .expect("client extraction failed");
            assert_eq!(candidates, expected_candidates);
            per_request_samples_ms.push(request_ms);
        }
        wall_samples_ms.push(round_started.elapsed().as_secs_f64() * 1_000.0);
    }

    let total_wall_seconds = wall_samples_ms.iter().sum::<f64>() / 1_000.0;
    let report = ContentionReport {
        device: "cpu",
        batch_size: config.batch_size,
        max_batch_tokens: config.max_batch_tokens,
        max_concurrency: config.max_concurrency,
        clients: CONTENTION_CLIENTS,
        rounds: CONTENTION_ROUNDS,
        wall_p50_ms: percentile(&wall_samples_ms, 0.50),
        wall_p95_ms: percentile(&wall_samples_ms, 0.95),
        per_request_p95_ms: percentile(&per_request_samples_ms, 0.95),
        throughput_requests_per_s: per_request_samples_ms.len() as f64 / total_wall_seconds,
        wall_samples_ms,
        per_request_samples_ms,
        candidates: expected_candidates,
    };
    println!(
        "{}",
        serde_json::to_string(&report).expect("serialize contention report")
    );
}
