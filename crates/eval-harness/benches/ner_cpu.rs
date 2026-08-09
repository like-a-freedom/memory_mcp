//! CPU NER latency benchmarks for every `NER_EXTRACTOR` backend.
//!
//! Model-backed backends are fixture-gated: when the local checkpoint is
//! absent the bench skips with a note so the file still compiles and runs
//! everywhere. `default_service_probe` measures the Anno + DB path and is
//! intentionally kept separate — do not compare across it.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use eval_harness::ner_fixtures;
use memory_mcp::config::NerExtractorKind;
use memory_mcp::service::EntityExtractor;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use std::sync::Arc;
use std::time::Instant;

fn bench_extractor(c: &mut Criterion, label: &str, kind: NerExtractorKind) {
    // Build the extractor once, before Criterion starts timing: the reported
    // "cold start" measures build/first-load time, and each bench then
    // measures the warm steady-state on the already-loaded model.
    let build_start = Instant::now();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let Some(extractor): Option<Arc<dyn EntityExtractor>> =
        rt.block_on(ner_fixtures::build_extractor(kind))
    else {
        eprintln!("{label} benches skipped: local fixture missing");
        return;
    };
    eprintln!("{label} cold start: {:?}", build_start.elapsed());

    let fixture = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    let single = fixture.single_window.to_string();
    let multi = fixture.multi_window.to_string();

    c.bench_function(&format!("{label}_single_window_warm"), |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(
                        extractor
                            .extract_candidates(black_box(&single))
                            .await
                            .unwrap(),
                    );
                });
            }
            start.elapsed()
        })
    });

    c.bench_function(&format!("{label}_multi_window_warm"), |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(
                        extractor
                            .extract_candidates(black_box(&multi))
                            .await
                            .unwrap(),
                    );
                });
            }
            start.elapsed()
        })
    });
}

fn bench_regex(c: &mut Criterion) {
    bench_extractor(c, "regex", NerExtractorKind::Regex);
}

fn bench_anno(c: &mut Criterion) {
    bench_extractor(c, "anno", NerExtractorKind::Anno);
}

fn bench_anno_onnx(c: &mut Criterion) {
    bench_extractor(c, "anno_onnx", NerExtractorKind::AnnoOnnx);
}

fn bench_gliner(c: &mut Criterion) {
    bench_extractor(c, "gliner", NerExtractorKind::ClassicGliner);
}

fn bench_vago(c: &mut Criterion) {
    bench_extractor(c, "vago", NerExtractorKind::SauerkrautLfm25);
}

/// Default-service path probe: measures Anno + DB overhead through the
/// production service path. Kept separate from the per-extractor benches —
/// do not compare across them.
fn bench_default_service_probe(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (service, episode_id) = rt.block_on(async {
        let service = eval_harness::test_support::make_service().await;
        let episode_id = memory_mcp::service::capabilities::ingest::IngestCapability::ingest(
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
    bench_regex,
    bench_anno,
    bench_anno_onnx,
    bench_gliner,
    bench_vago,
    bench_default_service_probe
);
criterion_main!(benches);
