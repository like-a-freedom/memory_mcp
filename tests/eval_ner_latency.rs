use std::path::PathBuf;
use std::time::Instant;

use memory_mcp::config::{NerConfig, NerProviderKind};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::create_entity_extractor;
use serde::Serialize;

#[derive(Serialize)]
struct NerLatencyReport {
    provider: &'static str,
    iterations: usize,
    content_words: usize,
    p50_ms: f64,
    p95_ms: f64,
    candidates: Vec<(String, String)>,
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index]
}

#[tokio::test]
#[ignore = "requires the local GLiNER model and release-mode timing"]
async fn run_gliner_latency_eval() {
    let model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/models/ner/urchade--gliner_multi-v2.1");
    let mut config = NerConfig::from_env().expect("load NER environment");
    config.provider = NerProviderKind::LocalGliner;
    config.model = Some("urchade/gliner_multi-v2.1".to_string());
    config.model_dir = Some(model_dir.to_string_lossy().to_string());
    let extractor = create_entity_extractor(
        &config,
        env!("CARGO_MANIFEST_DIR"),
        &StdoutLogger::new("debug"),
    )
    .await
    .expect("load local GLiNER model");
    let paragraph =
        "Alice Smith from OpenAI presented Project Atlas in Moscow using Rust and Kubernetes. ";
    let content = paragraph.repeat(40);

    extractor
        .extract_candidates(&content)
        .await
        .expect("warm GLiNER");
    let mut samples = Vec::with_capacity(10);
    let mut last_candidates = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        let candidates = extractor
            .extract_candidates(&content)
            .await
            .expect("extract entities");
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        last_candidates = candidates;
    }
    samples.sort_by(f64::total_cmp);
    let report = NerLatencyReport {
        provider: extractor.provider_name(),
        iterations: samples.len(),
        content_words: content.split_whitespace().count(),
        p50_ms: percentile(&samples, 0.50),
        p95_ms: percentile(&samples, 0.95),
        candidates: last_candidates
            .into_iter()
            .map(|candidate| (candidate.canonical_name, candidate.entity_type))
            .collect(),
    };
    println!(
        "{}",
        serde_json::to_string(&report).expect("serialize report")
    );
}
