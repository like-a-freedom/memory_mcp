//! Shared NER fixture resolution for benches and evaluation suites.
//!
//! Single source of truth for where local NER checkpoints live, the default
//! label set, and how to build any `NER_EXTRACTOR` backend through the
//! production `create_entity_extractor` path. Model-backed kinds are
//! fixture-gated: they return `None` when the local checkpoint is absent so
//! benches and suites can skip honestly instead of downloading models.

use std::path::PathBuf;
use std::sync::Arc;

use memory_mcp::config::{
    GlinerDeviceKind, ModelBackedNerConfig, NativeGlinerConfig, NerConfig, NerExtractorConfig,
    NerExtractorKind,
};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::{EntityExtractor, create_entity_extractor};

/// Root of the local (gitignored) NER checkpoints.
pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("memory-mcp")
        .join("tests")
        .join("models")
        .join("ner")
}

/// Fixture directory for a model-backed kind, when present. Lightweight
/// kinds have no fixture directory (`None`).
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
    if dir.is_dir() { Some(dir) } else { None }
}

/// Whether the checkpoint for `kind` exists locally. Lightweight kinds are
/// always "present".
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

/// Builds the extractor for `kind` through the production factory.
///
/// Returns `None` when a model-backed kind has no local fixture. Classic
/// GLiNER uses the seeded artifact-store pattern (revision pinned, no
/// network); the other kinds build directly from their prepared checkpoint
/// directory via `cache_dir`.
#[allow(clippy::question_mark)] // `?` on `Option` inside async: verifier flow is clearer as `return None` here.
pub async fn build_extractor(kind: NerExtractorKind) -> Option<Arc<dyn EntityExtractor>> {
    let extractor = match kind {
        NerExtractorKind::Anno => create_entity_extractor(
            &NerConfig {
                extractor: NerExtractorConfig::Anno,
            },
            env!("CARGO_MANIFEST_DIR"),
            &logger(),
        )
        .await
        .expect("anno extractor must build"),
        NerExtractorKind::Regex => create_entity_extractor(
            &NerConfig {
                extractor: NerExtractorConfig::Regex,
            },
            env!("CARGO_MANIFEST_DIR"),
            &logger(),
        )
        .await
        .expect("regex extractor must build"),
        NerExtractorKind::AnnoOnnx => {
            let Some(dir) = fixture_dir(kind) else {
                return None;
            };
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::AnnoOnnx(ModelBackedNerConfig {
                        cache_dir: Some(dir),
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
            .expect("anno-onnx extractor must build from a prepared checkpoint")
        }
        NerExtractorKind::ClassicGliner => {
            let Some(dir) = fixture_dir(kind) else {
                return None;
            };
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::ClassicGliner(NativeGlinerConfig {
                        model: ModelBackedNerConfig {
                            cache_dir: Some(seeded_gliner_store_root(&dir)),
                            labels: default_labels(),
                            threshold: Some(0.5),
                            max_concurrency: 1,
                            idle_unload_secs: 0,
                        },
                        batch_size: 1,
                        max_batch_tokens: 1536,
                        device: GlinerDeviceKind::Cpu,
                    }),
                },
                env!("CARGO_MANIFEST_DIR"),
                &logger(),
            )
            .await
            .expect("GLiNER extractor must build from the seeded store")
        }
        NerExtractorKind::SauerkrautLfm25 => {
            let Some(dir) = fixture_dir(kind) else {
                return None;
            };
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::SauerkrautLfm25(NativeGlinerConfig {
                        model: ModelBackedNerConfig {
                            cache_dir: Some(dir),
                            labels: default_labels(),
                            threshold: Some(0.5),
                            max_concurrency: 1,
                            idle_unload_secs: 0,
                        },
                        batch_size: 1,
                        max_batch_tokens: 1536,
                        device: GlinerDeviceKind::Cpu,
                    }),
                },
                env!("CARGO_MANIFEST_DIR"),
                &logger(),
            )
            .await
            .expect("VAGO extractor must build from a prepared checkpoint")
        }
    };
    Some(extractor)
}

/// Seeds a leaked artifact-store root from the local GLiNER fixture so the
/// production store reuses the checkpoint instead of downloading 1.1 GB.
/// The upstream revision is pinned; if upstream HEAD moves, the first run
/// re-downloads once and the store then caches it (documented limitation).
fn seeded_gliner_store_root(fixture_dir: &std::path::Path) -> PathBuf {
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
        std::fs::copy(fixture_dir.join(file_name), revision_dir.join(file_name))
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
    // The store lives for the whole process; drop only the guard.
    std::mem::forget(temp);
    store_root
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
}
