//! Integration coverage for corrupt local model artifacts through the
//! production extractor constructor. This fixture is synthetic and never
//! downloads an optional model.

use memory_mcp::config::{ModelBackedNerConfig, NerConfig, NerExtractorConfig};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::create_entity_extractor;

#[tokio::test]
async fn the_public_extractor_constructor_rejects_a_corrupt_local_onnx_model() {
    let dir = tempfile::tempdir().expect("model fixture directory");
    std::fs::write(dir.path().join("model.onnx"), b"not an ONNX model")
        .expect("write corrupt ONNX fixture");
    std::fs::write(
        dir.path().join("tokenizer.json"),
        r###"{"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":null,"post_processor":null,"decoder":null,"model":{"type":"WordPiece","unk_token":"[UNK]","continuing_subword_prefix":"##","max_input_chars_per_word":100,"vocab":{"[PAD]":0,"[UNK]":1,"person":2}}}"###,
    )
    .expect("write valid tokenizer fixture");
    let config = NerConfig {
        extractor: NerExtractorConfig::AnnoOnnx(ModelBackedNerConfig {
            cache_dir: Some(dir.path().to_path_buf()),
            labels: vec!["person".to_owned()],
            threshold: Some(0.5),
            max_concurrency: 1,
            idle_unload_secs: 0,
        }),
    };

    let result = create_entity_extractor(
        &config,
        env!("CARGO_MANIFEST_DIR"),
        &StdoutLogger::new("error"),
    )
    .await;

    let error = result
        .err()
        .expect("the invalid model must be rejected by the production constructor");
    assert!(
        error.to_string().contains("anno-onnx session load failed"),
        "the valid tokenizer must reach ONNX session construction: {error}"
    );
}
