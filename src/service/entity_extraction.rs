//! Pluggable entity extraction abstractions.

use async_trait::async_trait;
use regex::Regex;

use crate::models::EntityCandidate;

use super::MemoryError;

/// Extracts entity candidates from text.
#[async_trait]
pub trait EntityExtractor: Send + Sync {
    /// Returns normalized entity candidates discovered in the supplied content.
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError>;
}

/// Regex-based deterministic extractor used as the default fallback implementation.
#[derive(Debug)]
pub struct RegexEntityExtractor {
    name_regex: Regex,
}

impl RegexEntityExtractor {
    /// Creates a new regex-backed entity extractor.
    pub fn new() -> Result<Self, MemoryError> {
        Ok(Self {
            name_regex: Regex::new(r"[A-Z][a-z]+(?:\s+[A-Z][a-z]+)+")
                .map_err(|err| MemoryError::Validation(format!("regex error: {err}")))?,
        })
    }
}

#[async_trait]
impl EntityExtractor for RegexEntityExtractor {
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        let candidates: std::collections::HashSet<_> = self
            .name_regex
            .find_iter(content)
            .map(|mat| mat.as_str().to_string())
            .collect();

        let mut entities = candidates
            .into_iter()
            .map(|candidate| EntityCandidate {
                entity_type: if candidate.contains("Corp") || candidate.contains("Inc") {
                    "company".to_string()
                } else {
                    "person".to_string()
                },
                canonical_name: candidate,
                aliases: Vec::new(),
            })
            .collect::<Vec<_>>();

        entities.sort_by(|left, right| left.canonical_name.cmp(&right.canonical_name));
        Ok(entities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn regex_entity_extractor_returns_deterministic_candidates() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Alice Smith met Bob Jones at Acme Inc")
            .await
            .unwrap();

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].canonical_name, "Acme Inc");
        assert_eq!(candidates[1].canonical_name, "Alice Smith");
        assert_eq!(candidates[2].canonical_name, "Bob Jones");
    }
}
