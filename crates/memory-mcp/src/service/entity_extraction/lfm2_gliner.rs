//! Native SauerkrautLM LFM2 GLiNER NER backend (Tasks 8–9).
//!
//! Task 8 (this step): bidirectional LFM2 backbone + GLiNER layer fuser in
//! native Candle, config parsing, and the upstream state-dict tensor adapter.
//! Task 9 adds tokenization and span decoding on top of [`Lfm2BiModel`].

pub(crate) mod config;
pub(crate) mod model;
pub(crate) mod tensors;

use candle_core::{DType, Device};
use candle_nn::VarBuilder;

use super::{BackendBoxFuture, MemoryError, NerBuildContext};

pub(crate) use config::Lfm2BiConfig;
// `LayerKind` is part of the Task 9 contract (root re-export); nothing in this
// build step consumes it yet, which the lint cannot know.
#[allow(unused_imports)]
pub(crate) use config::LayerKind;
pub(crate) use model::{Lfm2BiModel, LoadedLfm2Gliner};

/// Placeholder build hook: satisfies the registry entry until Tasks 8–9 add
/// the real native LFM2 GLiNER construction path.
pub(crate) fn build(
    config: crate::config::NerExtractorConfig,
    _context: NerBuildContext,
) -> BackendBoxFuture {
    Box::pin(async move {
        if !matches!(
            config,
            crate::config::NerExtractorConfig::SauerkrautLfm25(_)
        ) {
            return Err(MemoryError::ConfigInvalid(
                "lfm2_gliner::build requires NER_EXTRACTOR=VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER"
                    .to_string(),
            ));
        }
        Err(MemoryError::ConfigInvalid(
            "extractor backend is not implemented in this build step".to_string(),
        ))
    })
}

/// Crate-visible constructor namespace for the native LFM2 GLiNER backend.
///
/// Task 9 wires [`Self::new_from_checkpoint`] into [`build`]; it loads the
/// bidirectional backbone and the fused-layer output directly from the
/// upstream checkpoint (no safetensors conversion, no derived files).
/// Dormant until then, hence the lint allowance.
#[allow(dead_code)]
pub(crate) struct Lfm2Gliner;

#[allow(dead_code)]
impl Lfm2Gliner {
    /// Loads the LFM2 backbone + layer fuser from an upstream checkpoint
    /// directory containing `pytorch_model.bin` and `gliner_config.json`.
    pub(crate) fn new_from_checkpoint(
        checkpoint_root: &std::path::Path,
        device: &Device,
    ) -> Result<LoadedLfm2Gliner, MemoryError> {
        let config_path = checkpoint_root.join("gliner_config.json");
        let raw = std::fs::read_to_string(&config_path).map_err(|err| {
            MemoryError::Storage(format!("failed to read {config_path:?}: {err}"))
        })?;
        let json: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
            MemoryError::ConfigInvalid(format!("invalid gliner_config.json: {err}"))
        })?;
        let config = Lfm2BiConfig::from_gliner_config(&json)?;
        let vb = VarBuilder::from_pth(
            checkpoint_root.join("pytorch_model.bin"),
            DType::F32,
            device,
        )
        .map_err(|err| MemoryError::Storage(format!("failed to load pytorch weights: {err}")))?;
        let model = Lfm2BiModel::load(vb, &config)
            .map_err(|err| MemoryError::Storage(format!("failed to build LFM2 backbone: {err}")))?;
        Ok(LoadedLfm2Gliner {
            model,
            config,
            device: device.clone(),
        })
    }
}
