//! Shared NER fixture resolution for benches and evaluation suites.
//!
//! Single source of truth for where local NER checkpoints live, the default
//! label set, and how to build any `NER_EXTRACTOR` backend for benchmarking.
//!
//! Model-backed kinds use the production **store-free** constructors
//! (`GlinerEntityExtractor::new`, `VagoLfm2EntityExtractor::new_with_runtime`,
//! `create_entity_extractor` with an explicit `cache_dir` for anno-onnx) so
//! evaluation never resolves upstream revisions or downloads checkpoints.
//! Kinds are fixture-gated: `build_extractor` returns `None` when the local
//! checkpoint (or any required file) is absent, so benches and suites can
//! skip honestly.

use std::path::PathBuf;
use std::sync::Arc;

use memory_mcp::config::{
    GlinerDeviceKind, ModelBackedNerConfig, NerConfig, NerExtractorConfig, NerExtractorKind,
};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::{
    EntityExtractor, GlinerEntityExtractor, VagoLfm2EntityExtractor, create_entity_extractor,
};

/// Root of the local (gitignored) NER checkpoints.
pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("memory-mcp")
        .join("tests")
        .join("models")
        .join("ner")
}

/// Required checkpoint files per model-backed kind.
fn required_files(kind: NerExtractorKind) -> &'static [&'static str] {
    match kind {
        // The direct-dir loader reads only the ONNX session and tokenizer.
        NerExtractorKind::AnnoOnnx => &["model.onnx", "tokenizer.json"],
        NerExtractorKind::ClassicGliner => {
            &["model.safetensors", "gliner_config.json", "tokenizer.json"]
        }
        NerExtractorKind::SauerkrautLfm25 => {
            &["pytorch_model.bin", "gliner_config.json", "tokenizer.json"]
        }
        NerExtractorKind::Anno | NerExtractorKind::Regex => &[],
    }
}

/// Fixture directory for a model-backed kind that is present and complete.
/// Lightweight kinds have no fixture directory (`None`).
fn fixture_dir(kind: NerExtractorKind) -> Option<PathBuf> {
    let dir = match kind {
        NerExtractorKind::AnnoOnnx => fixture_root().join("deepanwa--NuNerZero_onnx"),
        NerExtractorKind::ClassicGliner => fixture_root().join("urchade--gliner_multi-v2.1"),
        NerExtractorKind::SauerkrautLfm25 => {
            fixture_root().join("VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER")
        }
        // Lightweight kinds never consult the filesystem.
        NerExtractorKind::Anno | NerExtractorKind::Regex => return None,
    };
    if dir.is_dir()
        && required_files(kind)
            .iter()
            .all(|file| dir.join(file).is_file())
    {
        Some(dir)
    } else {
        None
    }
}

/// Whether a usable checkpoint for `kind` exists locally. Lightweight kinds
/// are always "present".
pub fn fixture_present(kind: NerExtractorKind) -> bool {
    matches!(kind, NerExtractorKind::Anno | NerExtractorKind::Regex) || fixture_dir(kind).is_some()
}

/// Default label set shared by benches and the quality suite (matches the
/// corpus label vocabulary).
pub fn default_labels() -> Vec<String> {
    vec![
        "person".to_string(),
        "company".to_string(),
        "location".to_string(),
        "product".to_string(),
        "event".to_string(),
        "technology".to_string(),
    ]
}

fn logger() -> StdoutLogger {
    StdoutLogger::new("error")
}

/// Builds a model-backed extractor from a prepared fixture directory through
/// the production store-free constructors.
///
/// Returns `None` when construction fails (e.g. an unloadable ONNX session),
/// never panicking. GLiNER/VAGO loaders defer to first inference, so a
/// present-but-corrupt checkpoint may construct and then fail per case.
async fn build_model_extractor(
    kind: NerExtractorKind,
    dir: &std::path::Path,
    device: GlinerDeviceKind,
) -> Option<Arc<dyn EntityExtractor>> {
    match kind {
        NerExtractorKind::AnnoOnnx => {
            // `create_entity_extractor` with an explicit `cache_dir` treats it
            // as a raw model directory (anno_onnx::build), so the ONNX fixture
            // is used directly with no store and no download. The session is
            // built eagerly, so a corrupt fixture surfaces here as `None`.
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::AnnoOnnx(ModelBackedNerConfig {
                        cache_dir: Some(dir.to_path_buf()),
                        labels: default_labels(),
                        threshold: Some(0.5),
                        max_concurrency: 1,
                        idle_unload_secs: 0,
                    }),
                },
                env!("CARGO_MANIFEST_DIR"),
                &logger(),
            )
            .await
            .ok()
        }
        NerExtractorKind::ClassicGliner => {
            // Store-free production constructor: direct lazy loader, no
            // revision resolution, no download.
            GlinerEntityExtractor::new(dir, default_labels(), 0.5)
                .ok()
                .map(|e| Arc::new(e) as Arc<dyn EntityExtractor>)
        }
        NerExtractorKind::SauerkrautLfm25 => {
            // Store-free production constructor (same path the release-parity
            // gate uses): direct lazy loader over `pytorch_model.bin`.
            VagoLfm2EntityExtractor::new_with_runtime(
                dir,
                default_labels(),
                0.5,
                1,    // batch_size
                1536, // max_batch_tokens
                1,    // max_concurrency
                device,
                0, // idle_unload_secs (retain)
                logger(),
            )
            .ok()
            .map(|e| Arc::new(e) as Arc<dyn EntityExtractor>)
        }
        // Lightweight kinds never reach the model-backed path.
        NerExtractorKind::Anno | NerExtractorKind::Regex => None,
    }
}

/// Builds the extractor for `kind` on the requested device, fixture-gated for
/// model-backed kinds.
///
/// Lightweight kinds always build. A model-backed kind returns `None` when
/// its fixture is absent, incomplete, or fails to construct — never a panic
/// and never a download.
pub async fn build_extractor_for(
    kind: NerExtractorKind,
    device: GlinerDeviceKind,
) -> Option<Arc<dyn EntityExtractor>> {
    if let Some(dir) = fixture_dir(kind) {
        return build_model_extractor(kind, &dir, device).await;
    }
    match kind {
        NerExtractorKind::Anno => Some(
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::Anno,
                },
                env!("CARGO_MANIFEST_DIR"),
                &logger(),
            )
            .await
            .expect("anno extractor must build"),
        ),
        NerExtractorKind::Regex => Some(
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::Regex,
                },
                env!("CARGO_MANIFEST_DIR"),
                &logger(),
            )
            .await
            .expect("regex extractor must build"),
        ),
        // Model-backed kind without a complete local fixture.
        NerExtractorKind::AnnoOnnx
        | NerExtractorKind::ClassicGliner
        | NerExtractorKind::SauerkrautLfm25 => None,
    }
}

/// Builds the extractor for `kind` on the CPU device.
pub async fn build_extractor(kind: NerExtractorKind) -> Option<Arc<dyn EntityExtractor>> {
    build_extractor_for(kind, GlinerDeviceKind::Cpu).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_root_points_at_memory_mcp_model_dir() {
        assert!(fixture_root().ends_with("memory-mcp/tests/models/ner"));
    }

    #[test]
    fn default_labels_cover_corpus_labels() {
        let labels = default_labels();
        for required in [
            "person",
            "company",
            "location",
            "product",
            "event",
            "technology",
        ] {
            assert!(labels.iter().any(|l| l == required), "missing {required}");
        }
    }

    #[test]
    fn lightweight_kinds_build_without_fixtures() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for kind in [NerExtractorKind::Anno, NerExtractorKind::Regex] {
            let extractor = rt.block_on(build_extractor(kind));
            assert!(extractor.is_some(), "{kind:?} must build offline");
        }
    }

    #[test]
    fn model_kinds_are_fixture_gated() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for kind in [
            NerExtractorKind::AnnoOnnx,
            NerExtractorKind::ClassicGliner,
            NerExtractorKind::SauerkrautLfm25,
        ] {
            let extractor = rt.block_on(build_extractor(kind));
            assert_eq!(fixture_present(kind), extractor.is_some(), "{kind:?}");
        }
    }

    #[test]
    fn model_kinds_declare_required_checkpoint_files() {
        // The completeness contract that keeps `build_extractor` panic-free:
        // every model-backed kind declares the exact files its loader reads.
        assert_eq!(
            required_files(NerExtractorKind::AnnoOnnx),
            &["model.onnx", "tokenizer.json"]
        );
        assert_eq!(
            required_files(NerExtractorKind::ClassicGliner),
            &["model.safetensors", "gliner_config.json", "tokenizer.json"]
        );
        assert_eq!(
            required_files(NerExtractorKind::SauerkrautLfm25),
            &["pytorch_model.bin", "gliner_config.json", "tokenizer.json"]
        );
        // Lightweight kinds never consult the filesystem.
        assert!(required_files(NerExtractorKind::Anno).is_empty());
        assert!(required_files(NerExtractorKind::Regex).is_empty());
    }

    #[test]
    fn corrupt_model_fixture_never_panics() {
        // A fixture whose files exist but fail to load (a truncated ONNX
        // session or unparseable tokenizer) must yield `None`, not panic: the
        // completeness check proves existence only, so construction errors are
        // mapped to `None` by `build_model_extractor`.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let temp = tempfile::TempDir::new().expect("temp dir");
        for file in ["model.onnx", "tokenizer.json"] {
            std::fs::write(temp.path().join(file), b"not a real model")
                .expect("write corrupt file");
        }
        let extractor = rt.block_on(build_model_extractor(
            NerExtractorKind::AnnoOnnx,
            temp.path(),
            GlinerDeviceKind::Cpu,
        ));
        assert!(
            extractor.is_none(),
            "corrupt ONNX fixture must not construct"
        );
    }
}
