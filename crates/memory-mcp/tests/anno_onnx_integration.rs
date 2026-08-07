//! Anno NuNER ONNX integration tests.
//!
//! Ignored by default: they require a locally prepared NuNER ONNX checkpoint
//! under `tests/models/ner/deepanwa--NuNerZero_onnx` containing `model.onnx`
//! (≈1.85 GB), `tokenizer.json`, and `config.json`. The artifact store that
//! prepares the production default path is the same shape; the fixture
//! directory is used directly via `NER_CACHE_DIR`-style `cache_dir`.

use std::path::Path;

use memory_mcp::config::{ModelBackedNerConfig, NerConfig, NerExtractorConfig};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::create_entity_extractor;

const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/models/ner/deepanwa--NuNerZero_onnx"
);

#[test]
#[ignore = "requires local NuNER ONNX files under tests/models/ner/deepanwa--NuNerZero_onnx"]
fn fixture_contains_required_onnx_files() {
    for file in ["model.onnx", "tokenizer.json", "config.json"] {
        assert!(
            Path::new(FIXTURE_DIR).join(file).is_file(),
            "missing {file} under {FIXTURE_DIR}"
        );
    }
}

#[tokio::test]
#[ignore = "requires local NuNER ONNX files under tests/models/ner/deepanwa--NuNerZero_onnx"]
async fn builds_from_prepared_root_and_never_falls_back_to_heuristics() {
    let logger = StdoutLogger::new("error");
    let config = NerConfig {
        extractor: NerExtractorConfig::AnnoOnnx(ModelBackedNerConfig {
            cache_dir: Some(FIXTURE_DIR.into()),
            labels: vec!["person".to_string(), "company".to_string()],
            threshold: Some(0.5),
            max_concurrency: 1,
            idle_unload_secs: 0,
        }),
    };
    let extractor = create_entity_extractor(&config, "/tmp/memory-mcp-tests", &logger)
        .await
        .expect("anno-onnx extractor builds from prepared root");

    assert_eq!(extractor.provider_name(), "anno-onnx");
    let fp = extractor.fingerprint();
    assert_eq!(fp.backend, "anno-onnx");
    assert_eq!(fp.repository.as_deref(), Some("deepanwa/NuNerZero_onnx"));
    assert_eq!(fp.effective_device.as_deref(), Some("cpu"));

    // Real NuNER inference: a person/company mention must surface entities.
    // Regex/heuristic fallback would either produce nothing or non-model
    // labels, so non-empty person/company results prove the ONNX path ran.
    let candidates = extractor
        .extract_candidates("Alice Smith works at OpenAI.")
        .await
        .expect("extract candidates");
    assert!(
        !candidates.is_empty(),
        "expected ONNX-extracted entities; heuristics must never be used"
    );
    for candidate in &candidates {
        assert!(
            candidate.entity_type == "person" || candidate.entity_type == "company",
            "unexpected label {} for {}",
            candidate.entity_type,
            candidate.canonical_name
        );
    }
}
