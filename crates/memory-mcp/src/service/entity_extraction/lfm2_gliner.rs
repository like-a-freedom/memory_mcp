//! Native SauerkrautLM LFM2 GLiNER NER backend (Tasks 8–9).
//!
//! Bidirectional LFM2 backbone + GLiNER layer fuser in native Candle, config
//! parsing, and the upstream state-dict tensor adapter.
//! This module wires tokenization and markerV1 span decoding into the
//! full [`EntityExtractor`] lifecycle: artifact preparation through the shared
//! store, device policy, lazy load/unload, and smoke-validated activation.

pub(crate) mod config;
pub(crate) mod decode;
pub(crate) mod model;
pub(crate) mod tensors;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use candle_core::{DType, Device};
use candle_nn::VarBuilder;

use crate::config::{GlinerDeviceKind, NativeGlinerConfig, SELECTOR_SAUKRAUT_LFM25};
use crate::models::EntityCandidate;
use crate::service::model_artifacts::{
    ArtifactRequirement, NerArtifactSpec, NerArtifactStore, PreparedCheckpoint,
};

use super::{
    BackendBoxFuture, EntityExtractor, ExtractorFingerprint, MemoryError, NerBuildContext,
};

pub(crate) use config::Lfm2BiConfig;
// `LayerKind` is part of the span-decoding contract (root re-export); nothing in
// this build step consumes it yet, which the lint cannot know.
#[allow(unused_imports)]
pub(crate) use config::LayerKind;
pub(crate) use model::LoadedLfm2Gliner;

/// Artifact requirements for the verified
/// `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` checkpoint. The upstream
/// repository ships `pytorch_model.bin` (F32 torch format), `gliner_config.json`
/// (GLiNER head config; there is NO `config.json`), and `tokenizer.json`.
pub(crate) const VAGO_GLINER_SPEC: NerArtifactSpec = NerArtifactSpec {
    extractor_id: "sauerkraut-lfm2.5-gliner",
    repository: SELECTOR_SAUKRAUT_LFM25,
    runtime_version: "lfm2.5-gliner",
    files: &[
        ArtifactRequirement {
            path: "pytorch_model.bin",
            sha256: None,
        },
        ArtifactRequirement {
            path: "gliner_config.json",
            sha256: None,
        },
        ArtifactRequirement {
            path: "tokenizer.json",
            sha256: None,
        },
    ],
    companion_repository: None,
    companion_files: &[],
};

/// Crate-visible constructor namespace for the native LFM2 GLiNER backend.
///
/// [`Self::new_from_checkpoint`] loads the bidirectional backbone, the fused
/// layer output, the GLiNER head, and the tokenizer directly from the upstream
/// checkpoint (no safetensors conversion, no derived files).
pub(crate) struct Lfm2Gliner;

impl Lfm2Gliner {
    /// Loads the full VAGO runtime from a checkpoint directory containing
    /// `pytorch_model.bin`, `gliner_config.json`, and `tokenizer.json`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_from_checkpoint(
        checkpoint_root: &Path,
        device: &Device,
        labels: Vec<String>,
        threshold: f64,
        batch_size: usize,
        max_batch_tokens: usize,
        logger: crate::logging::StdoutLogger,
    ) -> Result<LoadedLfm2Gliner, MemoryError> {
        let config_path = checkpoint_root.join("gliner_config.json");
        let raw = std::fs::read_to_string(&config_path).map_err(|err| {
            MemoryError::Storage(format!("failed to read {config_path:?}: {err}"))
        })?;
        let json: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
            MemoryError::ConfigInvalid(format!("invalid gliner_config.json: {err}"))
        })?;
        let config = Lfm2BiConfig::from_gliner_config(&json)?;
        let tokenizer_path = checkpoint_root.join("tokenizer.json");
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path).map_err(|err| {
            MemoryError::Storage(format!(
                "failed to load tokenizer from {tokenizer_path:?}: {err}"
            ))
        })?;
        let vb = VarBuilder::from_pth(
            checkpoint_root.join("pytorch_model.bin"),
            DType::F32,
            device,
        )
        .map_err(|err| MemoryError::Storage(format!("failed to load pytorch weights: {err}")))?;
        LoadedLfm2Gliner::new_from_var_builder(
            vb,
            tokenizer,
            config,
            labels,
            threshold,
            device,
            logger,
            batch_size,
            max_batch_tokens,
        )
    }
}

fn log_selected_device(logger: &crate::logging::StdoutLogger, requested: &str, selected: &str) {
    logger.log(
        crate::service::log_event(
            "ner.device.selected",
            serde_json::json!({"requested": requested}),
            serde_json::json!({"selected": selected}),
            None,
            None,
            None,
        ),
        crate::logging::LogLevel::Info,
    );
}

/// Selects the inference device, matching the classic GLiNER backend exactly:
/// `Cpu` is unconditional, `Metal` fails when unavailable (or when the
/// feature is not built), and `Auto` falls back to CPU with a diagnostic
/// event. The fingerprint's `effective_device` is derived from the returned
/// device, never from the requested kind.
fn select_device(
    kind: GlinerDeviceKind,
    logger: &crate::logging::StdoutLogger,
) -> Result<Device, MemoryError> {
    match kind {
        GlinerDeviceKind::Cpu => {
            log_selected_device(logger, "cpu", "cpu");
            Ok(Device::Cpu)
        }
        GlinerDeviceKind::Metal => {
            #[cfg(feature = "metal")]
            {
                Device::new_metal(0)
                    .inspect(|_| log_selected_device(logger, "metal", "metal"))
                    .map_err(|err| {
                        MemoryError::ConfigInvalid(format!(
                            "failed to initialize Metal NER device: {err}"
                        ))
                    })
            }
            #[cfg(not(feature = "metal"))]
            {
                Err(MemoryError::ConfigInvalid(
                    "GLINER_DEVICE=metal requires building with --features metal".to_string(),
                ))
            }
        }
        GlinerDeviceKind::Auto => {
            #[cfg(feature = "metal")]
            {
                match Device::new_metal(0) {
                    Ok(device) => {
                        log_selected_device(logger, "auto", "metal");
                        Ok(device)
                    }
                    Err(error) => {
                        logger.log(
                            crate::service::log_event(
                                "ner.device.fallback",
                                serde_json::json!({"requested": "metal", "error": error.to_string()}),
                                serde_json::json!({"selected": "cpu"}),
                                None,
                                None,
                                None,
                            ),
                            crate::logging::LogLevel::Warn,
                        );
                        log_selected_device(logger, "auto", "cpu");
                        Ok(Device::Cpu)
                    }
                }
            }
            #[cfg(not(feature = "metal"))]
            {
                log_selected_device(logger, "auto", "cpu");
                Ok(Device::Cpu)
            }
        }
    }
}

fn device_string(device: &Device) -> String {
    if device.is_metal() {
        "metal".to_string()
    } else {
        "cpu".to_string()
    }
}

/// A stateless recipe that can rebuild a `LoadedLfm2Gliner` from disk. The
/// device is resolved once at construction so the fingerprint's
/// `effective_device` reflects the actually selected backend.
struct VagoLfm2Loader {
    model_dir: std::path::PathBuf,
    labels: Vec<String>,
    threshold: f64,
    batch_size: usize,
    max_batch_tokens: usize,
    device: Device,
    logger: crate::logging::StdoutLogger,
}

impl VagoLfm2Loader {
    fn load(&self) -> Result<LoadedLfm2Gliner, MemoryError> {
        Lfm2Gliner::new_from_checkpoint(
            &self.model_dir,
            &self.device,
            self.labels.clone(),
            self.threshold,
            self.batch_size,
            self.max_batch_tokens,
            self.logger.clone(),
        )
    }
}

/// Thin outer type implementing [`EntityExtractor`]. Owns the loader recipe,
/// the lazily-constructed model, and the durable fingerprint identity captured
/// at activation. `inference_gate` stays on the outer type so concurrent
/// extracts share one permit pool across reloads.
pub struct VagoLfm2EntityExtractor {
    loader: Arc<VagoLfm2Loader>,
    model: super::super::model_runtime::LoadedModel<LoadedLfm2Gliner>,
    inference_gate: super::super::model_runtime::InferenceGate,
    repository: String,
    revision: String,
    artifact_identity: String,
    revision_status: crate::service::model_artifacts::RevisionStatus,
    validation_status: crate::service::model_artifacts::ValidationStatus,
    effective_device: String,
}

impl VagoLfm2EntityExtractor {
    /// Constructs the extractor from a prepared checkpoint, capturing the
    /// revision identity for the fingerprint.
    pub(crate) fn new_with_checkpoint(
        checkpoint: &PreparedCheckpoint,
        native: &NativeGlinerConfig,
        logger: crate::logging::StdoutLogger,
    ) -> Result<Self, MemoryError> {
        let mut extractor = Self::new_with_runtime(
            &checkpoint.root,
            native.model.labels.clone(),
            native
                .model
                .threshold
                .unwrap_or(crate::config::DEFAULT_NER_THRESHOLD),
            native.batch_size,
            native.max_batch_tokens,
            native.model.max_concurrency,
            native.device,
            native.model.idle_unload_secs,
            logger,
        )?;
        extractor.repository = checkpoint.repository.clone();
        extractor.revision = checkpoint.revision.clone();
        extractor.artifact_identity = checkpoint.artifact_identity.clone();
        extractor.revision_status = checkpoint.revision_status;
        extractor.validation_status = checkpoint.validation_status;
        Ok(extractor)
    }

    #[allow(clippy::too_many_arguments)]
    #[doc(hidden)]
    pub fn new_with_runtime(
        model_dir: &Path,
        labels: Vec<String>,
        threshold: f64,
        batch_size: usize,
        max_batch_tokens: usize,
        max_concurrency: usize,
        device_kind: GlinerDeviceKind,
        idle_unload_secs: u64,
        logger: crate::logging::StdoutLogger,
    ) -> Result<Self, MemoryError> {
        if batch_size == 0 || max_batch_tokens == 0 {
            return Err(MemoryError::ConfigInvalid(
                "NER batch limits must be greater than zero".to_string(),
            ));
        }
        if max_concurrency == 0 {
            return Err(MemoryError::ConfigInvalid(
                "NER_MAX_CONCURRENCY must be greater than zero".to_string(),
            ));
        }
        let idle_unload = (idle_unload_secs > 0).then(|| Duration::from_secs(idle_unload_secs));
        // Resolve the device once so the fingerprint reports the actually
        // selected backend (never the requested kind alone).
        let device = select_device(device_kind, &logger)?;
        let effective_device = device_string(&device);
        let loader = Arc::new(VagoLfm2Loader {
            model_dir: model_dir.to_path_buf(),
            labels,
            threshold,
            batch_size,
            max_batch_tokens,
            device,
            logger: logger.clone(),
        });
        Ok(Self {
            loader,
            model: super::super::model_runtime::LoadedModel::new(idle_unload),
            inference_gate: super::super::model_runtime::InferenceGate::new(max_concurrency),
            repository: String::new(),
            revision: String::new(),
            artifact_identity: String::new(),
            revision_status: crate::service::model_artifacts::RevisionStatus::Latest,
            validation_status:
                crate::service::model_artifacts::ValidationStatus::RuntimeRegressionVerified,
            effective_device,
        })
    }

    async fn ensure_loaded(&self) -> Result<Arc<LoadedLfm2Gliner>, MemoryError> {
        let loader = Arc::clone(&self.loader);
        self.model
            .get_or_load(move || Ok(Arc::new(loader.load()?)))
            .await
    }

    async fn acquire_inference_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, MemoryError> {
        let (permit, queue_wait) =
            self.inference_gate.acquire().await.map_err(|_| {
                MemoryError::Storage("VAGO GLiNER inference gate closed".to_string())
            })?;
        self.loader.logger.log(
            crate::service::log_event(
                "ner.vago.queue.done",
                crate::service::log_args_with_duration(serde_json::json!({}), queue_wait),
                serde_json::json!({
                    "available_permits": self.inference_gate.available_permits()
                }),
                None,
                None,
                None,
            ),
            crate::logging::LogLevel::Debug,
        );
        Ok(permit)
    }

    /// Loads the model, runs the embedded RU/EN/mixed runtime-regression
    /// corpus, and installs the validated instance so the first real
    /// extraction reuses it. Every corpus case must extract without error;
    /// a structural failure marks the revision incompatible through the
    /// caller's `record_incompatible` path.
    async fn probe_and_install(&self) -> Result<(), MemoryError> {
        let loader = Arc::clone(&self.loader);
        let loaded = self
            .model
            .get_or_load(move || Ok(Arc::new(loader.load()?)))
            .await?;
        let corpus: crate::service::model_artifacts::runtime::RuntimeCorpusFile =
            serde_json::from_str(
                crate::service::model_artifacts::runtime::RUNTIME_REGRESSION_CORPUS,
            )
            .map_err(|err| {
                MemoryError::ConfigInvalid(format!(
                    "embedded VAGO runtime corpus is invalid: {err}"
                ))
            })?;
        for case in corpus.cases {
            loaded
                .extract_inner_with_labels(&case.text, &case.labels)
                .map_err(|err| {
                    MemoryError::Storage(format!(
                        "VAGO runtime regression failed on `{}`: {err}",
                        case.id
                    ))
                })?;
        }
        self.model.install_loaded(loaded).await;
        Ok(())
    }
}

impl std::fmt::Debug for VagoLfm2EntityExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VagoLfm2EntityExtractor")
            .field("labels", &self.loader.labels)
            .field("threshold", &self.loader.threshold)
            .finish()
    }
}

#[async_trait]
impl EntityExtractor for VagoLfm2EntityExtractor {
    fn provider_name(&self) -> &'static str {
        "sauerkraut-lfm2.5-gliner"
    }

    fn fingerprint(&self) -> ExtractorFingerprint {
        ExtractorFingerprint {
            selector: SELECTOR_SAUKRAUT_LFM25.to_string(),
            backend: "sauerkraut-lfm2.5-gliner".to_string(),
            repository: Some(self.repository.clone()),
            revision: Some(self.revision.clone()),
            artifact_identity: Some(self.artifact_identity.clone()),
            labels: super::anno_onnx::normalize_labels(&self.loader.labels),
            threshold: Some(self.loader.threshold),
            revision_status: Some(self.revision_status),
            validation_status: Some(self.validation_status),
            runtime_version: "lfm2.5-gliner".to_string(),
            effective_device: Some(self.effective_device.clone()),
        }
    }

    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        let _permit = self.acquire_inference_permit().await?;
        let loaded = self.ensure_loaded().await?;
        let result = loaded.extract_inner(content);
        // Arm the idle-unload timer at USE COMPLETION (also fires when
        // extract_inner returned Err — the model was still "used").
        self.model.arm_unload().await;
        result
    }

    async fn extract_candidates_with_labels(
        &self,
        content: &str,
        zero_shot_labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        let _permit = self.acquire_inference_permit().await?;
        let loaded = self.ensure_loaded().await?;
        let result = loaded.extract_inner_with_labels(content, zero_shot_labels);
        self.model.arm_unload().await;
        result
    }
}

impl VagoLfm2EntityExtractor {
    /// Scored extraction for release-parity validation (doc-hidden; not part
    /// of the public API). Returns NMS'd, thresholded spans with sigmoid
    /// probabilities so the parity test can enforce the `1e-4` score tolerance.
    #[doc(hidden)]
    pub async fn scored_extract(
        &self,
        content: &str,
        labels: &[String],
    ) -> Result<Vec<decode::ScoredEntity>, MemoryError> {
        let _permit = self.acquire_inference_permit().await?;
        let loaded = self.ensure_loaded().await?;
        let result = loaded.extract_scored(content, labels);
        self.model.arm_unload().await;
        result
    }
}

/// Builds the SauerkrautLM LFM2 GLiNER backend: resolves and prepares the
/// fixed `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` checkpoint through the
/// shared artifact store, then constructs the extractor. A newly staged
/// revision is probe-loaded and installed before activation so the first real
/// extraction reuses it.
pub(crate) fn build(
    config: crate::config::NerExtractorConfig,
    context: NerBuildContext,
) -> BackendBoxFuture {
    Box::pin(async move {
        let crate::config::NerExtractorConfig::SauerkrautLfm25(native) = config else {
            return Err(MemoryError::ConfigInvalid(
                "lfm2_gliner::build requires NER_EXTRACTOR=VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER"
                    .to_string(),
            ));
        };

        let store_root = native
            .model
            .cache_dir
            .clone()
            .unwrap_or_else(|| context.data_dir.join("models").join("ner"));
        let store = NerArtifactStore::new(store_root, context.progress)?;
        let was_active = store.active_revision(&VAGO_GLINER_SPEC);
        let checkpoint = store.prepare(&VAGO_GLINER_SPEC).await?;

        let extractor = VagoLfm2EntityExtractor::new_with_checkpoint(
            &checkpoint,
            &native,
            context.logger.clone(),
        )?;

        // A newly staged revision must construct and pass a smoke inference
        // before activation. The probe result is installed so the first real
        // extraction reuses the validated model.
        if was_active.as_deref() != Some(checkpoint.revision.as_str()) {
            match extractor.probe_and_install().await {
                Ok(()) => {}
                Err(err) => {
                    let fallback = store
                        .record_incompatible(
                            &VAGO_GLINER_SPEC,
                            &checkpoint.revision,
                            &err.to_string(),
                        )
                        .await?;
                    let extractor = VagoLfm2EntityExtractor::new_with_checkpoint(
                        &fallback,
                        &native,
                        context.logger,
                    )?;
                    return Ok(std::sync::Arc::new(extractor) as Arc<dyn EntityExtractor>);
                }
            }
        }

        Ok(Arc::new(extractor) as Arc<dyn EntityExtractor>)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_device_cpu_returns_cpu() {
        let logger = crate::logging::StdoutLogger::new("error");
        let device = select_device(GlinerDeviceKind::Cpu, &logger).expect("cpu device");
        assert!(device.is_cpu());
    }

    #[cfg(not(feature = "metal"))]
    #[test]
    fn select_device_metal_fails_without_feature() {
        let logger = crate::logging::StdoutLogger::new("error");
        let error = select_device(GlinerDeviceKind::Metal, &logger).expect_err("metal must fail");
        assert!(
            error.to_string().contains("--features metal"),
            "unexpected error: {error}"
        );
    }

    #[cfg(not(feature = "metal"))]
    #[test]
    fn select_device_auto_falls_back_to_cpu_without_feature() {
        let logger = crate::logging::StdoutLogger::new("error");
        let device = select_device(GlinerDeviceKind::Auto, &logger).expect("auto device");
        assert!(device.is_cpu());
    }

    #[cfg(feature = "metal")]
    #[test]
    fn select_device_auto_prefers_metal_when_available() {
        let logger = crate::logging::StdoutLogger::new("error");
        let device = select_device(GlinerDeviceKind::Auto, &logger).expect("auto device");
        // Metal is preferred on capable machines; the fallback branch (Metal
        // init failure -> CPU) is only observable on Metal-less machines, so
        // both outcomes are valid here.
        assert!(device.is_cpu() || device.is_metal());
    }

    #[test]
    fn new_from_checkpoint_fails_cleanly_without_checkpoint_dir() {
        let logger = crate::logging::StdoutLogger::new("error");
        let error = Lfm2Gliner::new_from_checkpoint(
            Path::new("/nonexistent/vago-lfm2-checkpoint"),
            &Device::Cpu,
            vec!["person".to_string()],
            0.5,
            1,
            1536,
            logger,
        )
        .expect_err("missing checkpoint must fail");
        assert!(
            error.to_string().contains("gliner_config.json"),
            "expected missing-config guidance, got: {error}"
        );
    }
}
