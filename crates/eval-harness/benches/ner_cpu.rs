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

fn gliner_fixture_dir() -> PathBuf {
    PathBuf::from(format!(
        "{}/../memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1",
        env!("CARGO_MANIFEST_DIR")
    ))
}

fn vago_fixture_dir() -> PathBuf {
    PathBuf::from(format!(
        "{}/../memory-mcp/tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Seeds a leaked store root from the local GLiNER fixture so the artifact
/// store reuses the checkpoint instead of re-downloading 1.1 GB (the upstream
/// revision is pinned in the seed; if HEAD moves, the first run re-downloads
/// once and the store then caches it).
fn seeded_gliner_store_root() -> PathBuf {
    use memory_mcp::service::model_artifacts::{
        PersistedArtifactState, RevisionState, RevisionStatus, ValidationStatus, persist_state,
    };
    const SEEDED_REVISION: &str = "443d26d654e0324125a96bebd8e796c14ff2efe6";

    let temp = tempfile::TempDir::new().expect("temp dir for seeded store");
    let store_root = temp.path().join("ner-store");
    let revision_dir = store_root
        .join("gliner")
        .join("revisions")
        .join(SEEDED_REVISION);
    std::fs::create_dir_all(&revision_dir).expect("create seeded revision dir");
    for file_name in ["gliner_config.json", "model.safetensors", "tokenizer.json"] {
        std::fs::copy(
            gliner_fixture_dir().join(file_name),
            revision_dir.join(file_name),
        )
        .expect("copy GLiNER fixture into seeded store");
    }
    let mut state = PersistedArtifactState::new();
    state.revisions.push(RevisionState {
        revision: SEEDED_REVISION.to_string(),
        artifact_identity: "seeded-local-fixture".to_string(),
        validation_status: ValidationStatus::RuntimeRegressionVerified,
        revision_status: RevisionStatus::Latest,
        activated_at: 1_700_000_000,
        incompatible: None,
    });
    persist_state(&store_root.join("gliner").join("state.json"), &state)
        .expect("persist seeded state");
    // The store lives for the whole bench process; drop only the guard.
    std::mem::forget(temp);
    store_root
}

fn default_labels() -> Vec<String> {
    vec![
        "person".to_string(),
        "company".to_string(),
        "location".to_string(),
        "product".to_string(),
        "event".to_string(),
        "technology".to_string(),
    ]
}

// Real GLiNER bench: hoist model load out of the timed loop.
// Uses the local model fixture at tests/models/ner/urchade--gliner_multi-v2.1
fn build_gliner_extractor() -> Arc<dyn EntityExtractor> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let config = NerConfig {
            extractor: NerExtractorConfig::ClassicGliner(NativeGlinerConfig {
                model: ModelBackedNerConfig {
                    cache_dir: Some(seeded_gliner_store_root()),
                    labels: default_labels(),
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

/// Builds the VAGO extractor from the local checkpoint when present.
/// Returns `None` (and prints a note) when the 1.6 GB fixture is absent so
/// the bench file still compiles and runs everywhere.
fn build_vago_extractor() -> Option<Arc<dyn EntityExtractor>> {
    let fixture = vago_fixture_dir();
    if !fixture.join("pytorch_model.bin").is_file() {
        eprintln!("VAGO benches skipped: missing {}", fixture.display());
        return None;
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    Some(rt.block_on(async {
        let config = NerConfig {
            extractor: NerExtractorConfig::SauerkrautLfm25(NativeGlinerConfig {
                model: ModelBackedNerConfig {
                    cache_dir: Some(fixture),
                    labels: default_labels(),
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
        .expect("VAGO extractor must load")
    }))
}

fn c_bench_single(
    c: &mut Criterion,
    name: &str,
    extractor: &Arc<dyn EntityExtractor>,
    text: String,
) {
    c.bench_function(name, |b| {
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

fn bench_gliner_single_window(c: &mut Criterion) {
    let extractor = build_gliner_extractor();
    let fixture = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    c_bench_single(
        c,
        "gliner_single_window_warm",
        &extractor,
        fixture.single_window,
    );
}

fn bench_gliner_multi_window(c: &mut Criterion) {
    let extractor = build_gliner_extractor();
    let fixture = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    c_bench_single(
        c,
        "gliner_multi_window_warm",
        &extractor,
        fixture.multi_window,
    );
}

fn bench_vago_single_window(c: &mut Criterion) {
    let Some(extractor) = build_vago_extractor() else {
        return;
    };
    let fixture = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    c_bench_single(
        c,
        "vago_single_window_warm",
        &extractor,
        fixture.single_window,
    );
}

fn bench_vago_multi_window(c: &mut Criterion) {
    let Some(extractor) = build_vago_extractor() else {
        return;
    };
    let fixture = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    c_bench_single(
        c,
        "vago_multi_window_warm",
        &extractor,
        fixture.multi_window,
    );
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
    bench_vago_single_window,
    bench_vago_multi_window,
    bench_default_service_probe
);
criterion_main!(benches);
