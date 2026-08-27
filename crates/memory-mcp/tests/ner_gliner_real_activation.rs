//! Real-fixture Classic GLiNER activation test.
//!
//! This test is `#[ignore]`d by default because it requires the local
//! 1.15 GB `urchade/gliner_multi-v2.1` fixture under
//! `tests/models/ner/urchade--gliner_multi-v2.1`. It seeds a candidate
//! revision, runs the production `build_from_store` path, and verifies that
//! the candidate is promoted to a `RuntimeRegressionVerified` known-good
//! only after a successful construction + smoke inference.

use std::path::PathBuf;

use tempfile::TempDir;

use memory_mcp::config::{ModelBackedNerConfig, NativeGlinerConfig};
use memory_mcp::logging::StdoutLogger;
#[allow(unused_imports)]
use memory_mcp::service::EntityExtractor;
use memory_mcp::service::NerBuildContext;
use memory_mcp::service::entity_extraction_gliner as gliner;
use memory_mcp::service::model_artifacts::{
    ArtifactRole, CapturingSink, HfArtifactFetcher, HfRevisionResolver, ModelProgressSink,
    NerArtifactStore, PersistedArtifactState, RevisionState, RevisionStatus, SystemClock,
    ValidationStatus, artifact_identity, persist_state, read_state,
};

const GLINER_FIXTURE_DIR: &str = "tests/models/ner/urchade--gliner_multi-v2.1";
const SEEDED_REVISION: &str = "443d26d654e0324125a96bebd8e796c14ff2efe6";

fn gliner_fixture_present() -> bool {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(GLINER_FIXTURE_DIR)
        .join("model.safetensors")
        .is_file()
}

fn copy_fixture_into(temp: &TempDir) -> std::path::PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GLINER_FIXTURE_DIR);
    let store_root = temp.path().join("ner-store");
    let rev_dir = store_root
        .join("gliner")
        .join("revisions")
        .join(SEEDED_REVISION);
    std::fs::create_dir_all(&rev_dir).expect("create revision dir");
    for entry in std::fs::read_dir(&src).expect("read fixture") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_file() {
            std::fs::copy(&path, rev_dir.join(entry.file_name())).expect("copy fixture file");
        }
    }
    store_root
}

#[tokio::test]
#[ignore = "requires the local Classic GLiNER checkpoint fixture under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn candidate_is_promoted_only_after_real_construction_and_smoke_probe() {
    if !gliner_fixture_present() {
        eprintln!("skipping: missing Classic GLiNER fixture");
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let store_root = copy_fixture_into(&temp);
    let layout_root = store_root.join("gliner");
    let rev_dir = layout_root.join("revisions").join(SEEDED_REVISION);

    // Seed a candidate with a valid identity computed from the fixture bytes.
    let identity = artifact_identity(
        &rev_dir,
        &gliner::CLASSIC_GLINER_SPEC
            .all_requirements()
            .copied()
            .collect::<Vec<_>>(),
    )
    .expect("identity");
    let mut state = PersistedArtifactState::new();
    state.revisions.push(RevisionState {
        revision: SEEDED_REVISION.to_string(),
        artifact_identity: identity.clone(),
        validation_status: ValidationStatus::ReleaseParityVerified,
        revision_status: RevisionStatus::Latest,
        activated_at: 1_700_000_000,
        role: ArtifactRole::Candidate,
        incompatible: None,
    });
    persist_state(&layout_root.join("state.json"), &state).expect("persist");

    // Use the production store + clock + an inert progress sink.
    let progress: std::sync::Arc<dyn ModelProgressSink> =
        std::sync::Arc::new(CapturingSink::default());
    let store_root_clone = store_root.clone();
    let store = NerArtifactStore::with_parts(
        store_root,
        std::sync::Arc::new(HfRevisionResolver::new().expect("resolver")),
        std::sync::Arc::new(HfArtifactFetcher::new().expect("fetcher")),
        progress,
        std::sync::Arc::new(SystemClock),
    );

    let native = NativeGlinerConfig {
        model: ModelBackedNerConfig {
            cache_dir: Some(store_root_clone),
            labels: vec!["person".to_string()],
            threshold: Some(0.5),
            max_concurrency: 1,
            idle_unload_secs: 0,
        },
        batch_size: 1,
        max_batch_tokens: 128,
        device: memory_mcp::config::GlinerDeviceKind::Cpu,
    };
    let context = NerBuildContext {
        data_dir: temp.path().to_path_buf(),
        logger: StdoutLogger::new("error"),
        progress: std::sync::Arc::new(CapturingSink::default()),
    };
    let extractor = gliner::build_from_store(&native, &context, &store)
        .await
        .expect("build");
    let fp = extractor.fingerprint();
    assert_eq!(fp.revision.as_deref(), Some(SEEDED_REVISION));
    assert_eq!(
        fp.validation_status,
        Some(ValidationStatus::RuntimeRegressionVerified)
    );
    // A trivial extract must work after promotion.
    let _ = extractor
        .extract_candidates("Alice Smith from Acme")
        .await
        .expect("extract");

    // State now records the role as KnownGood with RuntimeRegressionVerified.
    let reloaded = read_state(&layout_root.join("state.json")).expect("read state");
    let promoted = reloaded
        .known_goods()
        .find(|r| r.revision == SEEDED_REVISION)
        .expect("promoted known-good");
    assert_eq!(promoted.role, ArtifactRole::KnownGood);
    assert_eq!(
        promoted.validation_status,
        ValidationStatus::RuntimeRegressionVerified
    );
    assert_eq!(promoted.artifact_identity, identity);
}
