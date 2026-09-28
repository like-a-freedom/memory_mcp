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
    runtime_version: String,
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
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
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
        ExtractorFingerprint {
            selector: self.selector.clone(),
            backend: "gliner".to_string(),
            repository: Some(self.selector.clone()),
            revision: None,
            artifact_identity: None,
            labels: self.labels.clone(),
            threshold: Some(self.threshold),
            revision_status: None,
            validation_status: None,
            runtime_version: self.runtime_version.clone(),
            effective_device: None,
        }
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

    #[test]
    fn unavailable_fingerprint_preserves_selector_labels_threshold_and_runtime() {
        let extractor = UnavailableEntityExtractor::classic_gliner(&config(
            vec![" Person ".into(), "COMPANY".into()],
            Some(0.3),
        ));
        let fp = extractor.fingerprint();
        assert_eq!(fp.selector, crate::config::SELECTOR_CLASSIC_GLINER);
        assert_eq!(fp.backend, "gliner");
        assert_eq!(
            fp.repository.as_deref(),
            Some(crate::config::SELECTOR_CLASSIC_GLINER)
        );
        assert_eq!(fp.labels, vec!["person", "company"]);
        assert_eq!(fp.threshold, Some(0.3));
        assert_eq!(fp.revision, None);
        assert_eq!(fp.artifact_identity, None);
        assert_eq!(fp.revision_status, None);
        assert_eq!(fp.validation_status, None);
        assert_eq!(fp.effective_device, None);
        assert_eq!(fp.runtime_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn unavailable_fingerprint_uses_default_threshold_when_unset() {
        let extractor = UnavailableEntityExtractor::classic_gliner(&config(vec![], None));
        assert_eq!(
            extractor.fingerprint().threshold,
            Some(crate::config::DEFAULT_NER_THRESHOLD)
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
