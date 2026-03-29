//! anno-backed NER entity extractor.

use std::collections::BTreeMap;

use anno::{Model, StackedNER};
use async_trait::async_trait;

use crate::models::EntityCandidate;

use super::{EntityExtractor, MemoryError};

/// Extracts entity candidates with `anno`'s stacked NER model.
pub struct AnnoEntityExtractor {
    model: StackedNER,
}

impl AnnoEntityExtractor {
    /// Creates a new anno-backed extractor.
    pub fn new() -> Result<Self, MemoryError> {
        // In this repository `anno` is built with `default-features = false`, so
        // `StackedNER::default()` stays on the dependency-light rule-based path.
        // If we later switch to GLiNER/GLiNER2 with custom type labels, batch
        // labels into groups of ~20-30 per `docs/BACKENDS.md` guidance.
        Ok(Self {
            model: StackedNER::default(),
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
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
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
