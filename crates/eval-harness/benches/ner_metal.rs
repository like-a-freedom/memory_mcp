//! Apple-Silicon production extraction benchmark.
//!
//! This benchmark is intentionally limited to the VAGO/Metal extractor path.
//! It is a performance measurement, not a correctness gate. Other platforms
//! must not publish a successful placeholder for an unavailable device.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::sync::Arc;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use criterion::{Criterion, criterion_group, criterion_main};

/// Apple-Silicon VAGO extractor benchmark, fixture-gated: builds the native
/// LFM2 GLiNER extractor on the Metal device when the 1.6 GB checkpoint is
/// present locally. `MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1` turns a missing
/// fixture into a failed benchmark run; the default remains compile-friendly.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn bench_vago_apple_silicon_single_window(c: &mut Criterion) {
    use memory_mcp::config::{GlinerDeviceKind, NerExtractorKind};
    use memory_mcp::service::EntityExtractor;

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let Some(extractor): Option<Arc<dyn EntityExtractor>> =
        rt.block_on(eval_harness::ner_fixtures::build_extractor_for(
            NerExtractorKind::SauerkrautLfm25,
            GlinerDeviceKind::Metal,
        ))
    else {
        if std::env::var("MEMORY_MCP_BENCH_REQUIRE_FIXTURES").as_deref() == Ok("1") {
            panic!("VAGO Metal bench requires the local model fixture");
        }
        eprintln!("VAGO Metal bench skipped: local fixture missing");
        return;
    };

    let text = "Alice Smith from Acme Corp presented the quarterly revenue report.".to_string();
    rt.block_on(extractor.extract_candidates(&text))
        .expect("Metal fixture must load and infer before timing");
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
criterion_group!(benches, bench_vago_apple_silicon_single_window);

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
criterion_main!(benches);

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    eprintln!("ner_metal benchmark unsupported: requires macOS aarch64");
    std::process::exit(2);
}
