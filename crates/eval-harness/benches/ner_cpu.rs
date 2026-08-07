use criterion::{Criterion, black_box, criterion_group, criterion_main};
use memory_mcp::config::{
    GlinerDeviceKind, ModelBackedNerConfig, NativeGlinerConfig, NerConfig, NerExtractorConfig,
};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::service::{EntityExtractor, create_entity_extractor};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

// Real GLiNER bench: hoist model load out of the timed loop.
// Uses the local model fixture at tests/models/ner/urchade--gliner_multi-v2.1
fn build_gliner_extractor() -> Arc<dyn EntityExtractor> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let model_dir = PathBuf::from(format!(
            "{}/../memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1",
            env!("CARGO_MANIFEST_DIR")
        ));
        let config = NerConfig {
            extractor: NerExtractorConfig::ClassicGliner(NativeGlinerConfig {
                model: ModelBackedNerConfig {
                    cache_dir: Some(model_dir),
                    labels: vec![
                        "person".to_string(),
                        "company".to_string(),
                        "location".to_string(),
                        "product".to_string(),
                        "event".to_string(),
                        "technology".to_string(),
                    ],
                    threshold: Some(0.5),
                    max_concurrency: 1,
                    idle_unload_secs: 0,
                },
                batch_size: 1,
                max_batch_tokens: 1536,
                device: GlinerDeviceKind::Cpu,
            }),
        };
        create_entity_extractor(
            &config,
            env!("CARGO_MANIFEST_DIR"),
            &StdoutLogger::new("error"),
        )
        .await
        .expect("GLiNER extractor must load")
    })
}

fn bench_gliner_single_window(c: &mut Criterion) {
    let extractor = build_gliner_extractor();
    let fixture = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    let text = fixture.single_window;

    c.bench_function("gliner_single_window_warm", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(
                        extractor
                            .extract_candidates(black_box(&text))
                            .await
                            .unwrap(),
                    );
                });
            }
            start.elapsed()
        })
    });
}

fn bench_gliner_multi_window(c: &mut Criterion) {
    let extractor = build_gliner_extractor();
    let fixture = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    let text = fixture.multi_window;

    c.bench_function("gliner_multi_window_warm", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(
                        extractor
                            .extract_candidates(black_box(&text))
                            .await
                            .unwrap(),
                    );
                });
            }
            start.elapsed()
        })
    });
}

// Default-service path probe: measures Anno + DB overhead.
// Kept separate from the GLiNER model bench — do not compare across.
fn bench_default_service_probe(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (service, episode_id) = rt.block_on(async {
        let service = eval_harness::test_support::make_service().await;
        let episode_id = IngestCapability::ingest(
            &service.build_context(),
            memory_mcp::models::IngestRequest {
                source_type: "bench".into(),
                source_id: "probe-001".into(),
                content: "Alice Smith from Acme Corp presented quarterly revenue.".into(),
                t_ref: chrono::Utc::now(),
                scope: "org".into(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .unwrap();
        (service, episode_id)
    });

    c.bench_function("default_service_extract_warm", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(
                        ExtractCapability::extract(
                            &service.build_context(),
                            &episode_id,
                            None,
                            None,
                        )
                        .await
                        .unwrap(),
                    );
                });
            }
            start.elapsed()
        })
    });
}

criterion_group!(
    benches,
    bench_gliner_single_window,
    bench_gliner_multi_window,
    bench_default_service_probe
);
criterion_main!(benches);
