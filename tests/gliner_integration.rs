//! End-to-end test for GLiNER entity extraction with real model weights.
//!
//! Run with: cargo test --test gliner_integration -- --ignored
//! Requires: model files in tests/fixtures/gliner_multi_v2.1/
//!
//! Setup: huggingface-cli download urchade/gliner_multi-v2.1 \
//!        --local-dir tests/fixtures/gliner_multi_v2.1

use memory_mcp::service::{EntityExtractor, GlinerEntityExtractor};
use std::path::Path;

#[tokio::test]
#[ignore] // requires model download; not run in CI by default
async fn gliner_extracts_known_entities_from_english_text() {
    let model_dir = Path::new("tests/fixtures/gliner_multi_v2.1");
    if !model_dir.join("tokenizer.json").exists() {
        eprintln!("Skipping: model files not found in {:?}", model_dir);
        return;
    }

    let labels = vec![
        "Person".to_string(),
        "Organization".to_string(),
        "Location".to_string(),
    ];
    let extractor = GlinerEntityExtractor::new(model_dir, labels, 0.1)
        .expect("failed to create GLiNER extractor");

    let text = "Tim Cook announced that Apple will open a new office in London next year.";
    let candidates = extractor.extract_candidates(text).await.unwrap();

    assert!(!candidates.is_empty(), "expected at least one entity");

    let names: Vec<&str> = candidates
        .iter()
        .map(|c| c.canonical_name.as_str())
        .collect();

    assert!(
        names
            .iter()
            .any(|n| n.contains("Tim Cook") || n.contains("Cook")),
        "expected person entity, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.contains("Apple")),
        "expected organization entity, got: {:?}",
        names
    );
}

#[tokio::test]
#[ignore]
async fn gliner_handles_multilingual_text() {
    let model_dir = Path::new("tests/fixtures/gliner_multi_v2.1");
    if !model_dir.join("tokenizer.json").exists() {
        eprintln!("Skipping: model files not found in {:?}", model_dir);
        return;
    }

    let labels = vec!["person".to_string(), "location".to_string()];
    let extractor = GlinerEntityExtractor::new(model_dir, labels, 0.1)
        .expect("failed to create GLiNER extractor");

    let text = "Анна Иванова прилетела в Париж на конференцию.";
    let candidates = extractor.extract_candidates(text).await.unwrap();

    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.canonical_name.contains("Иванова")
                || candidate.canonical_name.contains("Анна")),
        "expected person entity from Cyrillic text, got: {:?}",
        candidates
    );
}

#[tokio::test]
#[ignore]
async fn gliner_no_entities_in_noise_text() {
    let model_dir = Path::new("tests/fixtures/gliner_multi_v2.1");
    if !model_dir.join("tokenizer.json").exists() {
        eprintln!("Skipping: model files not found in {:?}", model_dir);
        return;
    }

    let labels = vec!["person".to_string(), "organization".to_string()];
    let extractor = GlinerEntityExtractor::new(model_dir, labels, 0.5)
        .expect("failed to create GLiNER extractor");

    let candidates = extractor
        .extract_candidates("the and of in to")
        .await
        .unwrap();
    assert!(
        candidates.len() <= 1,
        "expected 0-1 entities from noise text, got {}",
        candidates.len()
    );
}

/// Verifies that entities spanning window boundaries are still extracted.
/// This is a regression test for the sliding-window overlap fix.
#[tokio::test]
#[ignore] // requires model download
async fn gliner_recovers_entities_across_window_boundaries() {
    let model_dir = Path::new("tests/fixtures/gliner_multi_v2.1");
    if !model_dir.join("tokenizer.json").exists() {
        eprintln!("Skipping: model files not found in {:?}", model_dir);
        return;
    }

    let labels = vec![
        "Person".to_string(),
        "Organization".to_string(),
        "Location".to_string(),
    ];
    let extractor = GlinerEntityExtractor::new(model_dir, labels, 0.1)
        .expect("failed to create GLiNER extractor");

    // Build a text long enough to trigger multiple sliding windows.
    // The entity "Johann Sebastian Bach" should be found even if it
    // lands near a window boundary.
    let filler: Vec<String> = (0..400)
        .map(|i| format!("The event number {i} was held in a large conference room."))
        .collect();
    let text = format!(
        "{} Johann Sebastian Bach was a famous composer. {}",
        filler[..200].join(" "),
        filler[200..].join(" ")
    );

    let candidates = extractor.extract_candidates(&text).await.unwrap();
    let names: Vec<&str> = candidates
        .iter()
        .map(|c| c.canonical_name.as_str())
        .collect();

    assert!(
        names.iter().any(|n| n.contains("Bach")),
        "expected 'Bach' entity to be recovered across window boundaries, got: {:?}",
        names
    );
}
