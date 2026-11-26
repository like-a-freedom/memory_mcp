//! anno-backed NER entity extractor.

use std::collections::BTreeMap;

use anno::{HeuristicNER, Model, RegexNER, StackedNER};
use async_trait::async_trait;

use crate::models::EntityCandidate;

use super::{BackendBoxFuture, EntityExtractor, MemoryError};

pub(crate) fn scheduling() -> super::NerScheduling {
    super::NerScheduling::Inline
}

/// Builds the anno backend — no async work needed.
pub(crate) fn build(
    config: crate::config::NerExtractorConfig,
    _context: super::NerBuildContext,
) -> BackendBoxFuture {
    Box::pin(async move {
        let crate::config::NerExtractorConfig::Anno { max_input_bytes } = config else {
            return Err(MemoryError::ConfigInvalid(
                "anno::build requires NER_EXTRACTOR=anno".to_string(),
            ));
        };
        Ok(
            std::sync::Arc::new(AnnoEntityExtractor::with_max_input_bytes(max_input_bytes)?)
                as std::sync::Arc<dyn EntityExtractor>,
        )
    })
}

/// Extracts entity candidates with `anno`'s stacked NER model.
pub struct AnnoEntityExtractor {
    model: StackedNER,
    max_input_bytes: usize,
}

impl AnnoEntityExtractor {
    /// Creates a new anno-backed extractor.
    pub fn new() -> Result<Self, MemoryError> {
        Self::with_max_input_bytes(crate::config::DEFAULT_ANNO_MAX_INPUT_BYTES)
    }

    /// Creates an extractor with an explicit whole-input UTF-8 byte limit.
    pub fn with_max_input_bytes(max_input_bytes: usize) -> Result<Self, MemoryError> {
        if !(1..=crate::config::MAX_ANNO_INPUT_BYTES).contains(&max_input_bytes) {
            return Err(MemoryError::ConfigInvalid(format!(
                "ANNO_MAX_INPUT_BYTES must be between 1 and {}",
                crate::config::MAX_ANNO_INPUT_BYTES
            )));
        }
        // Build the dependency-light rule stack explicitly. With anno's
        // `onnx` feature enabled, `StackedNER::default()` becomes
        // cache- and download-sensitive (it probes BERT/NuNER/GLiNER ONNX
        // backends through Hugging Face), which would break the
        // zero-configuration download-free default. The explicit
        // Regex + Heuristic stack is exactly what the pre-onnx default built.
        Ok(Self {
            model: StackedNER::builder()
                .layer(RegexNER::new())
                .layer(HeuristicNER::new())
                .build(),
            max_input_bytes,
        })
    }
}

impl std::fmt::Debug for AnnoEntityExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnoEntityExtractor").finish()
    }
}

#[async_trait]
impl EntityExtractor for AnnoEntityExtractor {
    fn provider_name(&self) -> &'static str {
        "anno"
    }

    fn max_input_bytes(&self) -> Option<usize> {
        Some(self.max_input_bytes)
    }

    fn scheduling(&self) -> super::NerScheduling {
        scheduling()
    }

    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        crate::knowledge::api::validate_entity_extraction_input(self, content)?;
        if content.trim().is_empty() {
            return Ok(Vec::new());
        }

        let entities = self
            .model
            .extract_entities(content, None)
            .map_err(|err| MemoryError::Validation(format!("anno NER error: {err}")))?;

        let mut candidates = BTreeMap::new();

        for entity in entities {
            let canonical_name = entity.text.trim();
            if canonical_name.is_empty() {
                continue;
            }

            let label = entity.entity_type.to_string();
            candidates.insert(
                canonical_name.to_string(),
                EntityCandidate {
                    entity_type: map_label(&label).to_string(),
                    canonical_name: canonical_name.to_string(),
                    aliases: Vec::new(),
                },
            );
        }

        Ok(candidates.into_values().collect())
    }
}

fn map_label(label: &str) -> &'static str {
    let normalized = label.trim().to_ascii_uppercase();
    match normalized.as_str() {
        "PER" | "PERSON" => "person",
        "ORG" | "ORGANIZATION" | "COMPANY" => "company",
        "LOC" | "GPE" | "LOCATION" => "location",
        "PRODUCT" | "PROD" => "product",
        "EVENT" => "event",
        "TECH" | "TECHNOLOGY" => "technology",
        _ => "concept",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[tokio::test]
    async fn anno_adapter_rejects_input_over_explicit_byte_limit() {
        let extractor = AnnoEntityExtractor::with_max_input_bytes(8)
            .expect("test limit is within the supported range");
        let input = "123456789";

        let error = extractor
            .extract_candidates(input)
            .await
            .expect_err("input over the explicit limit must be refused");

        assert!(matches!(
            &error,
            MemoryError::Validation(message)
                if message == "entity extraction input too large: provider=anno actual_bytes=9 max_bytes=8"
        ));
        assert!(!error.to_string().contains(input));
    }

    #[tokio::test]
    async fn anno_accepts_exact_byte_limit() {
        let extractor = AnnoEntityExtractor::with_max_input_bytes(8)
            .expect("test limit is within the supported range");

        assert!(extractor.extract_candidates("12345678").await.is_ok());
    }

    #[tokio::test]
    async fn anno_counts_utf8_bytes_not_characters() {
        let extractor = AnnoEntityExtractor::with_max_input_bytes(8)
            .expect("test limit is within the supported range");
        let input = "é".repeat(5);

        let error = extractor
            .extract_candidates(&input)
            .await
            .expect_err("five two-byte characters exceed an eight-byte limit");

        assert!(matches!(
            error,
            MemoryError::Validation(message)
                if message == "entity extraction input too large: provider=anno actual_bytes=10 max_bytes=8"
        ));
    }

    #[tokio::test]
    async fn anno_rejects_oversized_whitespace_before_empty_shortcut() {
        let extractor = AnnoEntityExtractor::with_max_input_bytes(8)
            .expect("test limit is within the supported range");

        let error = extractor
            .extract_candidates("         ")
            .await
            .expect_err("oversized whitespace must be refused before empty-input handling");

        assert!(matches!(
            error,
            MemoryError::Validation(message)
                if message == "entity extraction input too large: provider=anno actual_bytes=9 max_bytes=8"
        ));
    }

    #[tokio::test]
    async fn anno_labels_path_obeys_same_limit() {
        let extractor = AnnoEntityExtractor::with_max_input_bytes(8)
            .expect("test limit is within the supported range");
        let labels = vec!["person".to_string()];

        let error = extractor
            .extract_candidates_with_labels("123456789", &labels)
            .await
            .expect_err("labels path must enforce the extractor input limit");

        assert!(matches!(
            error,
            MemoryError::Validation(message)
                if message == "entity extraction input too large: provider=anno actual_bytes=9 max_bytes=8"
        ));
    }

    #[tokio::test]
    async fn anno_extractor_finds_person_names() {
        let extractor = AnnoEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Alice Smith met Bob Jones at OpenAI")
            .await
            .unwrap();

        let names: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.canonical_name.as_str())
            .collect();

        assert!(names.contains(&"Alice Smith") || names.contains(&"Bob Jones"));
    }

    #[tokio::test]
    async fn anno_extractor_returns_sorted_deduped_candidates() {
        let extractor = AnnoEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Alice Smith Alice Smith OpenAI")
            .await
            .unwrap();

        let names: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.canonical_name.as_str())
            .collect();
        let unique: HashSet<_> = names.iter().copied().collect();

        assert_eq!(names.len(), unique.len());
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[tokio::test]
    async fn anno_extractor_ignores_sentence_case_common_nouns() {
        let extractor = AnnoEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Yesterday we reviewed the draft and discussed next steps.")
            .await
            .unwrap();

        let names: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.canonical_name.as_str())
            .collect();

        assert!(
            !names.contains(&"Yesterday"),
            "anno extractor should not re-introduce regex-only sentence-case noise"
        );
    }

    #[tokio::test]
    async fn anno_extractor_empty_string_returns_empty() {
        let extractor = AnnoEntityExtractor::new().unwrap();
        let candidates = extractor.extract_candidates("").await.unwrap();

        assert!(candidates.is_empty());
    }
}
