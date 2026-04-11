//! Regex-based deterministic entity extractor.

use std::collections::HashSet;

use async_trait::async_trait;
use regex::Regex;

use crate::models::EntityCandidate;

use super::classifier::classify_entity_type;
use super::{EntityExtractor, MemoryError};

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
    fn provider_name(&self) -> &'static str {
        "regex"
    }

    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        let candidates: HashSet<_> = self
            .name_regex
            .find_iter(content)
            .map(|mat| mat.as_str().to_string())
            .collect();

        let mut entities = candidates
            .into_iter()
            .map(|candidate| {
                let entity_type = classify_entity_type(&candidate);
                EntityCandidate {
                    entity_type: entity_type.to_string(),
                    canonical_name: candidate,
                    aliases: Vec::new(),
                }
            })
            .collect::<Vec<_>>();

        entities.sort_by(|left, right| left.canonical_name.cmp(&right.canonical_name));
        Ok(entities)
    }
}
