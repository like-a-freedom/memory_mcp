//! Semantic triple extraction from facts.
//!
//! Extracts structured (subject, predicate, object) triples from fact content.
//! Triples enable structured queries like "who works at X?" or "where does Y live?".
//!
//! The default `NoOpTripleExtractor` does nothing. An LLM-based extractor can be
//! enabled via configuration to extract triples asynchronously after fact creation.

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::service::error::MemoryError;

/// A semantic triple: (subject, predicate, object).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticTriple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
    pub source_fact_id: String,
}

/// Trait for extracting semantic triples from text.
#[async_trait]
pub trait TripleExtractor: Send + Sync {
    /// Extract semantic triples from the given text.
    async fn extract(
        &self,
        text: &str,
        source_fact_id: &str,
    ) -> Result<Vec<SemanticTriple>, MemoryError>;
}

/// No-op triple extractor — returns an empty list (default).
#[allow(dead_code)]
pub struct NoOpTripleExtractor;

#[async_trait]
impl TripleExtractor for NoOpTripleExtractor {
    async fn extract(
        &self,
        _text: &str,
        _source_fact_id: &str,
    ) -> Result<Vec<SemanticTriple>, MemoryError> {
        Ok(vec![])
    }
}

/// Singleton predicates that can only have one active value per subject.
/// Used by the conflict resolver to auto-invalidate outdated facts.
pub const SINGLETON_PREDICATES: &[&str] = &[
    "works_at",
    "lives_in",
    "has_name",
    "has_email",
    "has_phone",
    "is_ceo_of",
    "is_married_to",
    "has_birthday",
    "has_age",
    "has_title",
    "located_in",
    "has_address",
];

/// Check if a predicate is a singleton (can only have one active value per subject).
pub fn is_singleton_predicate(predicate: &str) -> bool {
    SINGLETON_PREDICATES.contains(&predicate)
}

/// Rule-based triple extractor that uses regex patterns to identify
/// common subject-predicate-object patterns in natural language text.
///
/// Supports patterns like:
/// - "X works at Y" → (X, works_at, Y)
/// - "X lives in Y" → (X, lives_in, Y)
/// - "X is the CEO of Y" → (X, is_ceo_of, Y)
/// - "X has email Y" → (X, has_email, Y)
pub struct RuleBasedTripleExtractor {
    patterns: Vec<(Regex, &'static str)>,
}

impl Default for RuleBasedTripleExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl RuleBasedTripleExtractor {
    /// Create a new extractor with all built-in patterns.
    pub fn new() -> Self {
        // Patterns: list of (regex, predicate_name)
        // Each regex must have exactly two capture groups: subject and object.
        let raw_patterns: Vec<(&str, &str)> = vec![
            (r"(.+?)\s+works?\s+at\s+(.+?)$", "works_at"),
            (r"(.+?)\s+works?\s+for\s+(.+?)$", "works_at"),
            (r"(.+?)\s+lives?\s+in\s+(.+?)$", "lives_in"),
            (r"(.+?)\s+is\s+(?:the\s+)?CEO\s+of\s+(.+?)$", "is_ceo_of"),
            (
                r"(.+?)\s+is\s+(?:the\s+)?founder\s+of\s+(.+?)$",
                "is_ceo_of",
            ),
            (r"(.+?)\s+is\s+(?:the\s+)?CTO\s+of\s+(.+?)$", "has_title"),
            (
                r"(.+?)\s+is\s+(?:the\s+)?(?:lead\s+)?engineer\s+at\s+(.+?)$",
                "works_at",
            ),
            (r"(.+?)\s+has\s+email\s+(.+?)$", "has_email"),
            (r"(.+?)\'?s?\s+email\s+is\s+(.+?)$", "has_email"),
            (r"(.+?)\s+has\s+phone\s+(.+?)$", "has_phone"),
            (r"(.+?)\s+has\s+title\s+(.+?)$", "has_title"),
            (r"(.+?)\'?s?\s+title\s+is\s+(.+?)$", "has_title"),
            (r"(.+?)\s+has\s+birthday\s+(.+?)$", "has_birthday"),
            (r"(.+?)\'?s?\s+birthday\s+is\s+(.+?)$", "has_birthday"),
            (r"(.+?)\s+has\s+age\s+(\d+)$", "has_age"),
            (r"(.+?)\s+is\s+located\s+in\s+(.+?)$", "located_in"),
            (r"(.+?)\s+has\s+address\s+(.+?)$", "has_address"),
            (r"(.+?)\s+is\s+married\s+to\s+(.+?)$", "is_married_to"),
            (r"(.+?)\s+lives?\s+at\s+(.+?)$", "lives_in"),
        ];
        let patterns = raw_patterns
            .into_iter()
            .filter_map(|(re_str, pred)| Regex::new(re_str).ok().map(|re| (re, pred)))
            .collect();
        Self { patterns }
    }
}

#[async_trait]
impl TripleExtractor for RuleBasedTripleExtractor {
    async fn extract(
        &self,
        text: &str,
        source_fact_id: &str,
    ) -> Result<Vec<SemanticTriple>, MemoryError> {
        let mut triples = Vec::new();
        for (regex, predicate) in &self.patterns {
            if let Some(caps) = regex.captures(text) {
                let subject = caps
                    .get(1)
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_default();
                let object = caps
                    .get(2)
                    .map(|m| m.as_str().trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_default();
                if !subject.is_empty() && !object.is_empty() {
                    triples.push(SemanticTriple {
                        subject,
                        predicate: predicate.to_string(),
                        object,
                        confidence: 0.7, // rule-based = moderate confidence
                        source_fact_id: source_fact_id.to_string(),
                    });
                }
            }
        }
        Ok(triples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn extract_works_at_triple() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("Alice Smith works at Acme Corp", "fact:1")
            .await
            .unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject, "Alice Smith");
        assert_eq!(triples[0].predicate, "works_at");
        assert_eq!(triples[0].object, "Acme Corp");
    }

    #[tokio::test]
    async fn extract_lives_in_triple() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("Bob lives in New York", "fact:2")
            .await
            .unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, "lives_in");
        assert_eq!(triples[0].object, "New York");
    }

    #[tokio::test]
    async fn extract_is_ceo_of_triple() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("John Smith is the CEO of Tech Inc", "fact:3")
            .await
            .unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, "is_ceo_of");
        assert_eq!(triples[0].object, "Tech Inc");
    }

    #[tokio::test]
    async fn extract_has_email_triple() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("Alice Smith has email alice@acme.com", "fact:4")
            .await
            .unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, "has_email");
        assert_eq!(triples[0].object, "alice@acme.com");
    }

    #[tokio::test]
    async fn extract_returns_empty_for_no_match() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("This text contains no known triple patterns", "fact:6")
            .await
            .unwrap();
        assert!(triples.is_empty());
    }

    #[test]
    fn singleton_predicates_are_recognized() {
        assert!(is_singleton_predicate("works_at"));
        assert!(is_singleton_predicate("lives_in"));
        assert!(is_singleton_predicate("has_email"));
        assert!(!is_singleton_predicate("knows"));
        assert!(!is_singleton_predicate("met"));
    }
}
