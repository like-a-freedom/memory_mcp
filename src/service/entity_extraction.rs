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
    ///
    /// Supports both ASCII and Unicode letters (Cyrillic, etc.).
    /// Pattern matches:
    /// - Multi-word capitalized names: "Alice Smith", "Иван Петров"
    /// - Single-token CamelCase: "OpenAI", "PostgreSQL"
    ///
    /// Minimum 3 characters to avoid noise like "I", "At", "In".
    pub fn new() -> Result<Self, MemoryError> {
        Ok(Self {
            name_regex: Regex::new(
                r"[\p{Lu}][\p{Ll}]+(?:\s+[\p{Lu}][\p{Ll}]+)+|[\p{Lu}][\p{L}\p{N}]{2,}",
            )
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

    #[tokio::test]
    async fn regex_entity_extractor_includes_single_token_camel_case_names() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates(
                "OpenAI partnered with Anthropic while PostgreSQL backed Alice Smith",
            )
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "Alice Smith".to_string(),
                "Anthropic".to_string(),
                "OpenAI".to_string(),
                "PostgreSQL".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn regex_entity_extractor_filters_out_short_words() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("I met Bob at OpenAI on Monday at San Francisco")
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        // Should NOT include: I, At, In, On (1-2 letter words)
        // Should include: Bob, OpenAI, Monday, San Francisco (3+ chars)
        assert!(!names.contains(&"I".to_string()));
        assert!(!names.contains(&"At".to_string()));
        assert!(!names.contains(&"On".to_string()));

        assert!(names.contains(&"Bob".to_string()));
        assert!(names.contains(&"OpenAI".to_string()));
        assert!(names.contains(&"Monday".to_string()));
        assert!(names.contains(&"San Francisco".to_string()));
    }

    #[tokio::test]
    async fn regex_entity_extractor_supports_unicode_names() {
        let extractor = RegexEntityExtractor::new().unwrap();
        let candidates = extractor
            .extract_candidates("Иван Петров встретился с Maria Garcia в компании TechCorp")
            .await
            .unwrap();

        let names = candidates
            .into_iter()
            .map(|candidate| candidate.canonical_name)
            .collect::<Vec<_>>();

        // Should include Cyrillic and Latin names
        assert!(names.contains(&"Иван Петров".to_string()));
        assert!(names.contains(&"Maria Garcia".to_string()));
        assert!(names.contains(&"TechCorp".to_string()));
    }
}
