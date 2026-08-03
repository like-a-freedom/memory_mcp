//! Apple-Silicon production extraction benchmark.
//!
//! This deliberately measures the same production service path as `ner_cpu`.
//! It is not a Metal-only proof: the service builder used by the eval harness
//! currently defaults to the configured production extractor and does not
//! expose a per-instance device override.  Until that seam exists, this file
//! must not be used as a Metal performance gate.  Other platforms must not
//! publish a successful nanosecond placeholder for an unavailable device.

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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
criterion_group!(benches, bench_ner_apple_silicon_single_window);

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
criterion_main!(benches);

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    eprintln!("ner_metal benchmark unsupported: requires macOS aarch64");
}
