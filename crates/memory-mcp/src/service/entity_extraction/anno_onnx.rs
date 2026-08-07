//! Anno NuNER ONNX NER backend (Task 7).

use super::{BackendBoxFuture, MemoryError, NerBuildContext};

/// Placeholder build hook: satisfies the registry entry until Task 7 adds the
/// real local-NuNER ONNX construction path.
pub(crate) fn build(
    config: crate::config::NerExtractorConfig,
    _context: NerBuildContext,
) -> BackendBoxFuture {
    Box::pin(async move {
        if !matches!(config, crate::config::NerExtractorConfig::AnnoOnnx(_)) {
            return Err(MemoryError::ConfigInvalid(
                "anno_onnx::build requires NER_EXTRACTOR=anno-onnx".to_string(),
            ));
        }
        Err(MemoryError::ConfigInvalid(
            "extractor backend is not implemented in this build step".to_string(),
        ))
    })
}
