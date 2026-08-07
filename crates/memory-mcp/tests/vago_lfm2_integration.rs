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

fn corpus_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("evals")
        .join("corpora")
        .join("ner")
        .join(name)
}

#[derive(serde::Deserialize)]
struct ParityCase {
    id: String,
    language: String,
    text: String,
    labels: Vec<String>,
    #[serde(default)]
    entities: Vec<ParityEntity>,
}

#[derive(Debug, serde::Deserialize)]
struct ParityEntity {
    start: usize,
    end: usize,
    text: String,
    label: String,
    #[serde(default)]
    score: Option<f32>,
}

#[derive(serde::Deserialize)]
struct ParityFile {
    fixture_status: String,
    cases: Vec<ParityCase>,
}

fn load_parity_file(name: &str) -> ParityFile {
    let path = corpus_file(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read corpus {}: {err}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|err| panic!("parse corpus {}: {err}", path.display()))
}

/// Offline structural gate: every parity corpus case must be internally
/// consistent (span round-trips to the exact entity text, labels ordered,
/// RU/EN/mixed languages present) before any checkpoint is involved.
#[test]
fn vago_parity_corpus_is_structurally_valid() {
    let parity = load_parity_file("vago_release_parity.json");
    let runtime = load_parity_file("vago_runtime_regression.json");

    assert!(
        runtime
            .cases
            .iter()
            .all(|case| matches!(case.language.as_str(), "ru" | "en" | "mixed")),
        "runtime corpus must only contain ru/en/mixed cases"
    );
    assert!(!parity.cases.is_empty(), "parity corpus must not be empty");

    let mut seen = std::collections::HashSet::new();
    for case in &parity.cases {
        assert!(
            seen.insert(case.id.clone()),
            "duplicate case id {}",
            case.id
        );
        assert!(
            matches!(case.language.as_str(), "ru" | "en" | "mixed"),
            "case {} has unsupported language {}",
            case.id,
            case.language
        );
        assert!(!case.labels.is_empty(), "case {} has no labels", case.id);
        for entity in &case.entities {
            // Corpus spans are character offsets (Python reference convention).
            let actual: String = case
                .text
                .chars()
                .skip(entity.start)
                .take(entity.end.saturating_sub(entity.start))
                .collect();
            assert_eq!(
                actual, entity.text,
                "case {}: span [{}, {}) does not round-trip `{}` (got `{actual}`)",
                case.id, entity.start, entity.end, entity.text
            );
            assert!(
                case.labels.contains(&entity.label),
                "case {}: entity label `{}` is not in the ordered labels {:?}",
                case.id,
                entity.label,
                case.labels
            );
        }
    }
}

/// Offline gate mirroring the plan: the embedded runtime corpus used by the
/// probe must parse and must only contain cases that the parity corpus also
/// carries structural expectations for (RU/EN/mixed coverage).
#[test]
fn vago_embedded_runtime_corpus_matches_checked_in_corpus() {
    let checked_in = load_parity_file("vago_runtime_regression.json");
    let embedded: memory_mcp::service::model_artifacts::runtime::RuntimeCorpusFile =
        serde_json::from_str(
            memory_mcp::service::model_artifacts::runtime::RUNTIME_REGRESSION_CORPUS,
        )
        .expect("embedded runtime corpus parses");
    assert_eq!(checked_in.cases.len(), embedded.cases.len());
    for (case, embedded_case) in checked_in.cases.iter().zip(embedded.cases.iter()) {
        assert_eq!(case.id, embedded_case.id);
        assert_eq!(case.text, embedded_case.text);
    }
}

#[ignore = "requires the 1.6 GB VAGO checkpoint and Python reference scores under tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/"]
#[tokio::test]
async fn vago_release_parity_matches_python_reference() {
    use memory_mcp::service::VagoLfm2EntityExtractor;

    let parity = load_parity_file("vago_release_parity.json");
    assert_eq!(
        parity.fixture_status, "release-parity-verified",
        "parity is only claimed after the pinned Python tooling has run; \
         update evals/corpora/ner/vago_release_parity.json and re-generate \
         reference scores before enabling this gate"
    );
    for case in &parity.cases {
        for entity in &case.entities {
            assert!(
                entity.score.is_some(),
                "case {}: reference score missing for `{}`",
                case.id,
                entity.text
            );
        }
    }

    // Build directly from the local checkpoint (no artifact store) so the
    // gate is purely native-vs-reference.
    let extractor = VagoLfm2EntityExtractor::new_with_runtime(
        &vago_model_dir(),
        parity_labels(&parity),
        0.5,
        1,
        1536,
        1,
        GlinerDeviceKind::Cpu,
        0,
        logger(),
    )
    .expect("vago extractor loads from the local checkpoint");

    for case in &parity.cases {
        let scored = extractor
            .scored_extract(&case.text, &case.labels)
            .await
            .unwrap_or_else(|err| panic!("extract `{}`: {err}", case.id));

        // Structural parity: exact text/label/set equality.
        let mut actual: Vec<(String, String)> = scored
            .iter()
            .map(|span| (span.text.clone(), span.label.clone()))
            .collect();
        actual.sort();
        let mut expected: Vec<(String, String)> = case
            .entities
            .iter()
            .map(|entity| (entity.text.clone(), entity.label.clone()))
            .collect();
        expected.sort();
        assert_eq!(
            actual, expected,
            "case {}: structural parity mismatch",
            case.id
        );

        // Score parity: abs(native - reference) <= 1e-4 per entity.
        for entity in &case.entities {
            let reference = entity.score.expect("reference score present");
            let native = scored
                .iter()
                .find(|span| span.text == entity.text && span.label == entity.label)
                .unwrap_or_else(|| panic!("case {}: missing scored span {:?}", case.id, entity));
            assert!(
                (native.score - reference).abs() <= 1e-4,
                "case {}: score drift for `{}` ({}): native {:.6}, reference {:.6}",
                case.id,
                entity.text,
                entity.label,
                native.score,
                reference
            );
        }
    }
}

fn parity_labels(parity: &ParityFile) -> Vec<String> {
    let mut labels = Vec::new();
    for case in &parity.cases {
        for label in &case.labels {
            if !labels.contains(label) {
                labels.push(label.clone());
            }
        }
    }
    labels
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
