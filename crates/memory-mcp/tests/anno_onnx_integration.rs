//! anno-onnx (NuNerZero ONNX) integration tests.
//!
//! The model-backed tests require the real ~1.7 GB checkpoint under
//! `tests/models/ner/deepanwa--NuNerZero_onnx/` and are `#[ignore]`d. The
//! structural parity test runs offline (no checkpoint needed).
//!
//! Parity gate: `evals/corpora/ner/anno_onnx_release_parity.json` pins the
//! entities the Python reference (gliner SpanProcessor protocol,
//! `max_width=1`, threshold 0.5) extracts from the shared quality corpus; the
//! native extractor must reproduce the exact (name, label) sets.

use std::path::PathBuf;

use memory_mcp::config::{ModelBackedNerConfig, NerConfig, NerExtractorConfig};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::create_entity_extractor;

const ANNO_ONNX_CHECKPOINT_DIR: &str = "deepanwa--NuNerZero_onnx";

fn anno_model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("models")
        .join("ner")
        .join(ANNO_ONNX_CHECKPOINT_DIR)
}

fn anno_config(cache_dir: Option<PathBuf>) -> NerConfig {
    NerConfig {
        extractor: NerExtractorConfig::AnnoOnnx(ModelBackedNerConfig {
            cache_dir,
            labels: vec![
                "person".to_string(),
                "company".to_string(),
                "location".to_string(),
            ],
            threshold: Some(0.5),
            max_concurrency: 1,
            idle_unload_secs: 0,
        }),
    }
}

fn logger() -> StdoutLogger {
    StdoutLogger::new("error")
}

async fn build_extractor() -> std::sync::Arc<dyn memory_mcp::service::EntityExtractor> {
    create_entity_extractor(
        &anno_config(Some(anno_model_dir())),
        env!("CARGO_MANIFEST_DIR"),
        &logger(),
    )
    .await
    .expect("anno-onnx extractor builds from a prepared checkpoint")
}

#[tokio::test]
#[ignore = "requires the 1.7 GB anno-onnx checkpoint under tests/models/ner/deepanwa--NuNerZero_onnx/"]
async fn anno_onnx_extracts_en_entities() {
    let extractor = build_extractor().await;
    let candidates = extractor
        .extract_candidates(
            "Alice Smith from OpenAI presented the Surface Laptop 6 at Build 2026 in Seattle.",
        )
        .await
        .expect("extraction runs");

    let names: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.canonical_name.to_lowercase())
        .collect();
    for expected in ["alice", "openai", "seattle."] {
        assert!(
            names.iter().any(|name| name.contains(expected)),
            "expected `{expected}` in candidates, got {names:?}"
        );
    }
    for candidate in &candidates {
        if candidate.canonical_name.eq_ignore_ascii_case("openai") {
            assert_eq!(candidate.entity_type, "company");
        }
        if candidate.canonical_name.eq_ignore_ascii_case("seattle.") {
            assert_eq!(candidate.entity_type, "location");
        }
        if candidate.canonical_name.eq_ignore_ascii_case("alice") {
            assert_eq!(candidate.entity_type, "person");
        }
    }
}

#[tokio::test]
#[ignore = "requires the 1.7 GB anno-onnx checkpoint under tests/models/ner/deepanwa--NuNerZero_onnx/"]
async fn anno_onnx_extracts_ru_entities() {
    let extractor = build_extractor().await;
    let candidates = extractor
        .extract_candidates("Иван Петров работает в Яндексе в Москве.")
        .await
        .expect("RU extraction runs");
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
}

#[tokio::test]
#[ignore = "requires the 1.7 GB anno-onnx checkpoint under tests/models/ner/deepanwa--NuNerZero_onnx/"]
async fn anno_onnx_empty_text_yields_no_candidates() {
    let extractor = build_extractor().await;
    let candidates = extractor
        .extract_candidates("   ")
        .await
        .expect("empty extraction runs");
    assert!(
        candidates.is_empty(),
        "empty text must produce no candidates"
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
    text: String,
    labels: Vec<String>,
    #[serde(default)]
    entities: Vec<ParityEntity>,
}

#[derive(Debug, serde::Deserialize)]
struct ParityEntity {
    name: String,
    label: String,
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

/// Offline structural gate: the pinned Python reference must be internally
/// consistent (unique case ids, ordered labels, entity labels within the case
/// label set) before any checkpoint is involved.
#[test]
fn anno_onnx_parity_corpus_is_structurally_valid() {
    let parity = load_parity_file("anno_onnx_release_parity.json");
    assert_eq!(
        parity.fixture_status, "release-parity-verified",
        "parity is only claimed after the pinned Python tooling has run; \
         re-generate evals/corpora/ner/anno_onnx_release_parity.json \
         before enabling this gate"
    );
    assert!(!parity.cases.is_empty(), "parity corpus must not be empty");

    let mut seen = std::collections::HashSet::new();
    for case in &parity.cases {
        assert!(
            seen.insert(case.id.clone()),
            "duplicate case id {}",
            case.id
        );
        assert!(!case.labels.is_empty(), "case {} has no labels", case.id);
        for entity in &case.entities {
            assert!(
                !entity.name.is_empty() && !entity.label.is_empty(),
                "case {}: empty reference entity",
                case.id
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

#[tokio::test]
#[ignore = "requires the 1.7 GB anno-onnx checkpoint under tests/models/ner/deepanwa--NuNerZero_onnx/"]
async fn anno_onnx_release_parity_matches_python_reference() {
    let parity = load_parity_file("anno_onnx_release_parity.json");
    assert_eq!(
        parity.fixture_status, "release-parity-verified",
        "parity is only claimed after the pinned Python tooling has run"
    );

    // Build directly from the local checkpoint (the KISS cache_dir path) so
    // the gate is purely native-vs-reference.
    let extractor = create_entity_extractor(
        &anno_config(Some(anno_model_dir())),
        env!("CARGO_MANIFEST_DIR"),
        &logger(),
    )
    .await
    .expect("anno-onnx extractor loads from the local checkpoint");

    for case in &parity.cases {
        let candidates = extractor
            .extract_candidates_with_labels(&case.text, &case.labels)
            .await
            .unwrap_or_else(|err| panic!("extract `{}`: {err}", case.id));

        let mut actual: Vec<(String, String)> = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.canonical_name.to_lowercase(),
                    candidate.entity_type.to_lowercase(),
                )
            })
            .collect();
        actual.sort();
        let mut expected: Vec<(String, String)> = case
            .entities
            .iter()
            .map(|entity| (entity.name.to_lowercase(), entity.label.to_lowercase()))
            .collect();
        expected.sort();
        assert_eq!(
            actual, expected,
            "case {}: structural parity mismatch",
            case.id
        );
    }
}
