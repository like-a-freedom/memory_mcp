//! VAGO SauerkrautLM LFM2.5 GLiNER integration tests.
//!
//! The model-backed tests require the real ~1.6 GB checkpoint under
//! `tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/` and are
//! `#[ignore]`d. The build-failure test runs offline with a poisoned store
//! state, so it never touches the network.

use std::path::PathBuf;

use memory_mcp::config::{
    GlinerDeviceKind, ModelBackedNerConfig, NativeGlinerConfig, NerConfig, NerExtractorConfig,
};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::{MemoryError, create_entity_extractor};
use tempfile::TempDir;

const VAGO_CHECKPOINT_DIR: &str = "VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER";

fn vago_model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("models")
        .join("ner")
        .join(VAGO_CHECKPOINT_DIR)
}

fn vago_config(cache_dir: Option<PathBuf>) -> NerConfig {
    NerConfig {
        extractor: NerExtractorConfig::SauerkrautLfm25(NativeGlinerConfig {
            model: ModelBackedNerConfig {
                cache_dir,
                labels: vec![
                    "person".to_string(),
                    "company".to_string(),
                    "location".to_string(),
                ],
                threshold: Some(0.5),
                max_concurrency: 1,
                idle_unload_secs: 0,
            },
            batch_size: 1,
            max_batch_tokens: 1536,
            device: GlinerDeviceKind::Cpu,
        }),
    }
}

fn logger() -> StdoutLogger {
    StdoutLogger::new("error")
}

#[ignore = "requires the 1.6 GB VAGO checkpoint under tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/"]
#[tokio::test]
async fn vago_lfm2_build_prepares_checkpoint_and_extracts() {
    let config = vago_config(Some(vago_model_dir()));
    let extractor = create_entity_extractor(&config, env!("CARGO_MANIFEST_DIR"), &logger())
        .await
        .expect("vago extractor builds from a prepared checkpoint");

    let candidates = extractor
        .extract_candidates("Alice Smith from Acme Corp")
        .await
        .expect("extraction runs");
    assert!(!candidates.is_empty(), "expected entities, got none");
    for candidate in &candidates {
        assert!(
            "Alice Smith from Acme Corp".contains(&candidate.canonical_name),
            "candidate {:?} is not a substring of the input",
            candidate.canonical_name
        );
    }
}

#[ignore = "requires the 1.6 GB VAGO checkpoint under tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/"]
#[tokio::test]
async fn vago_lfm2_extracts_ru_en_entities() {
    let config = vago_config(Some(vago_model_dir()));
    let extractor = create_entity_extractor(&config, env!("CARGO_MANIFEST_DIR"), &logger())
        .await
        .expect("vago extractor builds from a prepared checkpoint");

    let candidates = extractor
        .extract_candidates("Иван Петров работает в Microsoft")
        .await
        .expect("mixed RU/EN extraction runs");
    assert!(!candidates.is_empty(), "expected entities, got none");

    let names: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.canonical_name.to_lowercase())
        .collect();
    assert!(
        names
            .iter()
            .any(|name| name.contains("иван") || name.contains("петров")),
        "expected a Cyrillic person name, got {names:?}"
    );
    assert!(
        names.iter().any(|name| name.contains("microsoft")),
        "expected the Latin company name, got {names:?}"
    );
}

#[ignore = "requires the 1.6 GB VAGO checkpoint under tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/"]
#[tokio::test]
async fn vago_lfm2_fingerprint_carries_revision_and_device() {
    let config = vago_config(Some(vago_model_dir()));
    let extractor = create_entity_extractor(&config, env!("CARGO_MANIFEST_DIR"), &logger())
        .await
        .expect("vago extractor builds from a prepared checkpoint");

    let fingerprint = extractor.fingerprint();
    assert_eq!(
        fingerprint.selector,
        "VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER"
    );
    assert_eq!(fingerprint.backend, "sauerkraut-lfm2.5-gliner");
    assert_eq!(
        fingerprint.repository.as_deref(),
        Some("VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER")
    );
    assert!(
        fingerprint
            .revision
            .as_deref()
            .is_some_and(|revision| !revision.is_empty()),
        "fingerprint must carry the resolved revision"
    );
    assert!(
        fingerprint
            .artifact_identity
            .as_deref()
            .is_some_and(|identity| !identity.is_empty()),
        "fingerprint must carry the artifact identity"
    );
    assert_eq!(fingerprint.threshold, Some(0.5));
    assert!(fingerprint.labels.contains(&"person".to_string()));
    assert_eq!(fingerprint.runtime_version, "lfm2.5-gliner");
    assert_eq!(fingerprint.effective_device.as_deref(), Some("cpu"));
    assert!(
        fingerprint.revision_status.is_some(),
        "fingerprint must carry the revision status"
    );
    assert!(
        fingerprint.validation_status.is_some(),
        "fingerprint must carry the validation status"
    );
}

/// The real `NerArtifactStore` resolves the latest upstream revision over the
/// network, so with no checkpoint present `build` could trigger a 1.6 GB
/// download when online. To keep this test deterministic and offline, the
/// store state is poisoned: `prepare` fails on the unreadable state file
/// before any resolution or download attempt.
#[tokio::test]
async fn vago_lfm2_build_fails_without_model_files() {
    let temp = TempDir::new().expect("temp dir");
    let state_dir = temp.path().join("sauerkraut-lfm2.5-gliner");
    std::fs::create_dir_all(&state_dir).expect("state dir");
    std::fs::write(state_dir.join("state.json"), b"{not valid json").expect("write state");

    let config = vago_config(Some(temp.path().to_path_buf()));
    let result = create_entity_extractor(&config, env!("CARGO_MANIFEST_DIR"), &logger()).await;
    match result {
        Err(MemoryError::Storage(message)) => {
            assert!(
                message.contains("invalid artifact state"),
                "expected invalid-store guidance, got: {message}"
            );
        }
        Err(other) => panic!("expected Storage error, got {other}"),
        Ok(_) => panic!("vago build must fail without a usable checkpoint"),
    }
}
