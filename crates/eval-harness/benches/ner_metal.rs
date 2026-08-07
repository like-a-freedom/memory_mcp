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
use memory_mcp::service::capabilities::ingest::IngestCapability;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn bench_ner_apple_silicon_single_window(c: &mut Criterion) {
    c.bench_function("ner_apple_silicon_production_single_window", |b| {
        b.iter(|| {
            let service = tokio::runtime::Runtime::new().expect("runtime");
            service.block_on(async {
                let memory = eval_harness::test_support::make_service().await;
                let episode = IngestCapability::ingest(
                    &memory.build_context(),
                    memory_mcp::models::IngestRequest {
                        source_type: "bench".into(),
                        source_id: "ner-metal-bench".into(),
                        content:
                            "Alice Smith from Acme Corp presented the quarterly revenue report."
                                .into(),
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
                .expect("ingest");
                criterion::black_box(
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
    use memory_mcp::config::{
        GlinerDeviceKind, ModelBackedNerConfig, NativeGlinerConfig, NerConfig, NerExtractorConfig,
    };
    use memory_mcp::logging::StdoutLogger;
    use memory_mcp::service::{EntityExtractor, create_entity_extractor};

    let fixture = std::path::PathBuf::from(format!(
        "{}/../memory-mcp/tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER",
        env!("CARGO_MANIFEST_DIR")
    ));
    if !fixture.join("pytorch_model.bin").is_file() {
        eprintln!("VAGO Metal bench skipped: missing {}", fixture.display());
        return;
    }
    let extractor: Arc<dyn EntityExtractor> = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(async {
            let config = NerConfig {
                extractor: NerExtractorConfig::SauerkrautLfm25(NativeGlinerConfig {
                    model: ModelBackedNerConfig {
                        cache_dir: Some(fixture),
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
                    device: GlinerDeviceKind::Auto,
                }),
            };
            create_entity_extractor(
                &config,
                env!("CARGO_MANIFEST_DIR"),
                &StdoutLogger::new("error"),
            )
            .await
            .expect("VAGO extractor must load")
        });

    let text = "Alice Smith from Acme Corp presented the quarterly revenue report.".to_string();
    c.bench_function("vago_apple_silicon_single_window", |b| {
        b.iter_custom(|iters| {
            let start = std::time::Instant::now();
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            for _ in 0..iters {
                rt.block_on(async {
                    criterion::black_box(
                        extractor
                            .extract_candidates(criterion::black_box(&text))
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
