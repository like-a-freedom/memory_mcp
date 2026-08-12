//! Apple-Silicon production extraction benchmark.
//!
//! This deliberately measures the same production service path as `ner_cpu`.
//! It is not a Metal-only proof: the service builder used by the eval harness
//! currently defaults to the configured production extractor and does not
//! expose a per-instance device override.  Until that seam exists, this file
//! must not be used as a Metal performance gate.  Other platforms must not
//! publish a successful nanosecond placeholder for an unavailable device.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::sync::Arc;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use criterion::{Criterion, criterion_group, criterion_main};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use memory_mcp::service::capabilities::extract::ExtractCapability;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn bench_ner_apple_silicon_single_window(c: &mut Criterion) {
    c.bench_function("ner_apple_silicon_production_single_window", |b| {
        b.iter(|| {
            let service = tokio::runtime::Runtime::new().expect("runtime");
            service.block_on(async {
                let memory = eval_harness::test_support::make_service().await;
                let episode = eval_harness::test_support::ingest_probe(
                    &memory,
                    "ner-metal-bench",
                    "Alice Smith from Acme Corp presented the quarterly revenue report.",
                )
                .await;
                std::hint::black_box(
                    ExtractCapability::extract(&memory.build_context(), &episode, None, None)
                        .await
                        .expect("extract"),
                );
            });
        });
    });
}

/// Apple-Silicon VAGO extractor benchmark, fixture-gated: builds the native
/// LFM2 GLiNER extractor on the Metal device when the 1.6 GB checkpoint is
/// present locally, otherwise skips with a note (the bench file must still
/// compile and run on machines without the fixture).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn bench_vago_apple_silicon_single_window(c: &mut Criterion) {
    use memory_mcp::config::{GlinerDeviceKind, NerExtractorKind};
    use memory_mcp::service::EntityExtractor;

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let Some(extractor): Option<Arc<dyn EntityExtractor>> =
        rt.block_on(eval_harness::ner_fixtures::build_extractor_for(
            NerExtractorKind::SauerkrautLfm25,
            GlinerDeviceKind::Auto,
        ))
    else {
        eprintln!("VAGO Metal bench skipped: local fixture missing");
        return;
    };

    let text = "Alice Smith from Acme Corp presented the quarterly revenue report.".to_string();
    c.bench_function("vago_apple_silicon_single_window", |b| {
        b.iter_custom(|iters| {
            let start = std::time::Instant::now();
            for _ in 0..iters {
                rt.block_on(async {
                    std::hint::black_box(
                        extractor
                            .extract_candidates(std::hint::black_box(&text))
                            .await
                            .expect("extract"),
                    );
                });
            }
            start.elapsed()
        });
    });
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
criterion_group!(
    benches,
    bench_ner_apple_silicon_single_window,
    bench_vago_apple_silicon_single_window
);

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
criterion_main!(benches);

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    eprintln!("ner_metal benchmark unsupported: requires macOS aarch64");
}
