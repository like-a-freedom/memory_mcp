//! Placeholder extractor returned when the configured Classic GLiNER
//! checkpoint is not available locally. The active extractor is immutable
//! for the lifetime of the process; this stand-in preserves the public
//! fingerprint contract (selector, configured labels, threshold, runtime
//! version) while refusing to run inference. Extraction calls fail with a
//! stable `ModelNotReady` error that maps to a non-retryable MCP error
//! requiring a restart.

use async_trait::async_trait;

use crate::config::NativeGlinerConfig;
use crate::models::EntityCandidate;

use super::{EntityExtractor, ExtractorFingerprint, MemoryError, NerScheduling};

/// Immutable placeholder used until a real Classic GLiNER checkpoint is
/// activated on the next process start.
pub struct UnavailableEntityExtractor {
    selector: String,
    labels: Vec<String>,
    threshold: f64,
}

impl UnavailableEntityExtractor {
    /// Builds an unavailable stand-in for `config`. The fingerprint is
    /// shaped exactly like the real extractor's (provider `gliner`,
    /// `BlockingPool` scheduling, the configured labels and threshold)
    /// except for revision, identity, validation, and effective device
    /// which remain `None` because the model is not loaded.
    pub fn classic_gliner(config: &NativeGlinerConfig) -> Self {
        let labels = super::anno_onnx::normalize_labels(&config.model.labels);
        let threshold = config
            .model
            .threshold
            .unwrap_or(crate::config::DEFAULT_NER_THRESHOLD);
        Self {
            selector: crate::config::SELECTOR_CLASSIC_GLINER.to_string(),
            labels,
            threshold,
        }
    }
}

impl std::fmt::Debug for UnavailableEntityExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnavailableEntityExtractor")
            .field("selector", &self.selector)
            .field("labels", &self.labels)
            .field("threshold", &self.threshold)
            .finish()
    }
}

#[async_trait]
impl EntityExtractor for UnavailableEntityExtractor {
    fn provider_name(&self) -> &'static str {
        "gliner"
    }

    fn scheduling(&self) -> NerScheduling {
        NerScheduling::BlockingPool
    }

    fn fingerprint(&self) -> ExtractorFingerprint {
        ExtractorFingerprint::new(format!(
            "gliner:{}:{}:{}",
            self.selector,
            self.labels.join(","),
            self.threshold
        ))
    }

    async fn extract_candidates(
        &self,
        _content: &str,
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        Err(MemoryError::ModelNotReady(
            "The configured Classic GLiNER checkpoint is not available locally.".to_string(),
        ))
    }

    async fn extract_candidates_with_labels(
        &self,
        _content: &str,
        zero_shot_labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        // Empty custom labels must NOT silently return success; callers
        // expect either a result or the documented model-not-ready error.
        let _ = zero_shot_labels;
        Err(MemoryError::ModelNotReady(
            "The configured Classic GLiNER checkpoint is not available locally.".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GlinerDeviceKind, ModelBackedNerConfig, NativeGlinerConfig};

    fn config(labels: Vec<String>, threshold: Option<f64>) -> NativeGlinerConfig {
        NativeGlinerConfig {
            model: ModelBackedNerConfig {
                cache_dir: None,
                labels,
                threshold,
                max_concurrency: 1,
                idle_unload_secs: 0,
            },
            batch_size: 1,
            max_batch_tokens: 128,
            device: GlinerDeviceKind::Cpu,
        }
    }

    #[tokio::test]
    async fn unavailable_classic_gliner_preserves_provider_and_scheduling() {
        let extractor =
            UnavailableEntityExtractor::classic_gliner(&config(vec!["person".into()], Some(0.7)));
        assert_eq!(extractor.provider_name(), "gliner");
        assert_eq!(extractor.scheduling(), NerScheduling::BlockingPool);
    }

    /// An unavailable extractor must be distinguishable from the real one
    /// it stands in for, and its token must name the configuration it was
    /// asked for — labels and threshold are what a caller varies (ADR-0068).
    #[test]
    fn unavailable_fingerprint_names_the_selector_labels_and_threshold() {
        let extractor = UnavailableEntityExtractor::classic_gliner(&config(
            vec![" Person ".into(), "COMPANY".into()],
            Some(0.3),
        ));
        assert_eq!(
            extractor.fingerprint().as_str(),
            format!(
                "gliner:{}:person,company:0.3",
                crate::config::SELECTOR_CLASSIC_GLINER
            )
        );
    }

    #[test]
    fn unavailable_fingerprint_uses_default_threshold_when_unset() {
        let extractor = UnavailableEntityExtractor::classic_gliner(&config(vec![], None));
        assert!(
            extractor
                .fingerprint()
                .as_str()
                .ends_with(&format!(":{}", crate::config::DEFAULT_NER_THRESHOLD)),
            "an unset threshold must still produce a distinguishing token"
        );
    }

    #[tokio::test]
    async fn unavailable_default_extraction_returns_model_not_ready() {
        let extractor =
            UnavailableEntityExtractor::classic_gliner(&config(vec!["person".into()], Some(0.5)));
        let err = extractor
            .extract_candidates("Alice from Acme")
            .await
            .expect_err("must fail");
        assert!(matches!(err, MemoryError::ModelNotReady(_)));
    }

    #[tokio::test]
    async fn unavailable_custom_label_extraction_also_returns_model_not_ready() {
        let extractor =
            UnavailableEntityExtractor::classic_gliner(&config(vec!["person".into()], Some(0.5)));
        let err = extractor
            .extract_candidates_with_labels("Alice", &["fictional".into()])
            .await
            .expect_err("custom labels must also fail");
        assert!(matches!(err, MemoryError::ModelNotReady(_)));
    }

    #[tokio::test]
    async fn unavailable_custom_label_extraction_with_empty_labels_still_fails() {
        let extractor =
            UnavailableEntityExtractor::classic_gliner(&config(vec!["person".into()], Some(0.5)));
        // Empty custom labels must NOT silently return success.
        let err = extractor
            .extract_candidates_with_labels("Alice", &[])
            .await
            .expect_err("empty custom labels must also fail");
        assert!(matches!(err, MemoryError::ModelNotReady(_)));
    }
}
