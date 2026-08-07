//! Native SauerkrautLM LFM2 GLiNER NER backend (Tasks 8–9).

use super::{BackendBoxFuture, MemoryError, NerBuildContext};

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
