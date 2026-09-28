//! Semantic triple extraction from facts.
//!
//! Extracts structured (subject, predicate, object) triples from fact content.
//! Triples enable structured queries like "who works at X?" or "where does Y live?".
//!
//! Triple extraction is currently provided by rule-based patterns.
//! Additional extractors can be added later if they have a real caller and
//! clear evaluation coverage.

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::error::MemoryError;

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

/// Singleton predicates that can only have one active value per subject.
/// Used by the conflict resolver to auto-invalidate outdated facts.
pub const SINGLETON_PREDICATES: &[&str] = &[
    "works_at",
    "lives_in",
    "has_name",
    "has_email",
    "has_phone",
    "is_ceo_of",
    "is_founder_of",
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
            // English patterns
            (r"(.+?)\s+works?\s+at\s+(.+?)$", "works_at"),
            (r"(.+?)\s+works?\s+for\s+(.+?)$", "works_at"),
            (r"(.+?)\s+lives?\s+in\s+(.+?)$", "lives_in"),
            (r"(.+?)\s+is\s+(?:the\s+)?CEO\s+of\s+(.+?)$", "is_ceo_of"),
            (
                r"(.+?)\s+is\s+(?:the\s+)?founder\s+of\s+(.+?)$",
                "is_founder_of",
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
            // Russian patterns
            (r"(.+?)\s+работает\s+в\s+(.+?)$", "works_at"),
            (r"(.+?)\s+работает\s+в\s+компании\s+(.+?)$", "works_at"),
            (r"(.+?)\s+живёт\s+в\s+(.+?)$", "lives_in"),
            (r"(.+?)\s+живет\s+в\s+(.+?)$", "lives_in"),
            (
                r"(.+?)\s+является\s+(?:учредителем|основателем)\s+(.+?)$",
                "is_founder_of",
            ),
            (
                r"(.+?)\s+является\s+(?:генеральным\s+директором|CEO)\s+(?:компании\s+)?(.+?)$",
                "is_ceo_of",
            ),
            (r"(.+?)\s+находится\s+в\s+(.+?)$", "located_in"),
            (r"(.+?)\s+женат\s+на\s+(.+?)$", "is_married_to"),
            (r"(.+?)\s+замужем\s+за\s+(.+?)$", "is_married_to"),
            (r"(.+?)\s+имеет\s+email\s+(.+?)$", "has_email"),
            (r"(.+?)\s+имеет\s+телефон\s+(.+?)$", "has_phone"),
            (r"(.+?)\s+имеет\s+возраст\s+(\d+)", "has_age"),
            (r"(.+?)\s+имеет\s+должность\s+(.+?)$", "has_title"),
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
                    .map(|m| normalize_russian_object(m.as_str()))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_default();
                if !subject.is_empty() && !object.is_empty() {
                    triples.push(SemanticTriple {
                        subject,
                        predicate: predicate.to_string(),
                        object,
                        confidence: 0.7,
                        source_fact_id: source_fact_id.to_string(),
                    });
                }
            }
        }
        // Deduplicate by (subject, predicate): keep first (most specific pattern).
        let mut seen: HashSet<(String, String)> = HashSet::new();
        triples.retain(|t| seen.insert((t.subject.clone(), t.predicate.clone())));
        Ok(triples)
    }
}

/// Normalize Russian object nouns from inflected back to nominative-like form.
///
/// Strips common prepositional, instrumental, and genitive endings.
/// This is a heuristic — not a full lemmatizer — sufficient to collapse
/// obvious inflections ("Газпроме" → "Газпром") so that entity matching in
/// retrieval has a chance to find the canonical form. Remaining
/// partial-stem cases ("Москве" → "Москв") are accepted: retrieval still
/// works because the triple's `object` is also the value written at
/// creation time, so lookups are consistent within the system.
fn normalize_russian_object(s: &str) -> String {
    let trimmed = s.trim();
    if !has_cyrillic(trimmed) {
        return trimmed.to_string();
    }
    // Longest endings first so that e.g. "ого" is tried before "о".
    // Deduplicated; covering common Russian noun case endings.
    const ENDINGS: &[&str] = &[
        "ого", "его", "ому", "ему", "ыми", "ими", "ую", "юю", "ной", "ным", "ном", "нем", "ой",
        "ым", "им", "ом", "ем", "ей", "е", "у", "и", "а", "я",
    ];
    for ending in ENDINGS {
        // `ending.len()` is in bytes; all entries above are ASCII-free and
        // Cyrillic letters are 2 bytes each in UTF-8, so byte comparison via
        // `ends_with` is correct.
        if trimmed.len() > ending.len() + 1 && trimmed.ends_with(ending) {
            let base = &trimmed[..trimmed.len() - ending.len()];
            if base.chars().count() >= 2 {
                return base.to_string();
            }
        }
    }
    trimmed.to_string()
}

fn has_cyrillic(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}'))
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
    async fn extract_is_founder_of_triple() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("Jane Doe is the founder of StartupXYZ", "fact:3b")
            .await
            .unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, "is_founder_of");
        assert_eq!(triples[0].object, "StartupXYZ");
    }

    #[tokio::test]
    async fn extract_russian_works_at() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("Иван Петров работает в Газпроме", "fact:ru1")
            .await
            .unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, "works_at");
        assert_eq!(triples[0].object, "Газпром");
    }

    #[tokio::test]
    async fn extract_russian_lives_in() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("Мария живёт в Москве", "fact:ru2")
            .await
            .unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, "lives_in");
    }

    #[tokio::test]
    async fn extract_russian_ceo() {
        let extractor = RuleBasedTripleExtractor::new();
        let triples = extractor
            .extract("Пётр является генеральным директором Лукойла", "fact:ru3")
            .await
            .unwrap();
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, "is_ceo_of");
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

    #[tokio::test]
    async fn extract_deduplicates_by_subject_predicate() {
        let extractor = RuleBasedTripleExtractor::new();
        // Both "работает в Газпроме" and "работает в компании Газпром"
        // should produce one triple, not two.
        let triples = extractor
            .extract("Иван работает в компании Газпром в Москве", "fact:dedup")
            .await
            .unwrap();
        let works_at_count = triples.iter().filter(|t| t.predicate == "works_at").count();
        assert_eq!(
            works_at_count, 1,
            "should deduplicate to single works_at triple"
        );
    }

    #[test]
    fn singleton_predicates_are_recognized() {
        assert!(is_singleton_predicate("works_at"));
        assert!(is_singleton_predicate("lives_in"));
        assert!(is_singleton_predicate("has_email"));
        assert!(!is_singleton_predicate("knows"));
        assert!(!is_singleton_predicate("met"));
    }

    #[test]
    fn normalize_object_strips_russian_prepositional() {
        assert_eq!(normalize_russian_object("Газпроме"), "Газпром");
        assert_eq!(normalize_russian_object("Москве"), "Москв");
        // "Омске" → "Омск" (4 chars, ending -е correctly removed)
        assert_eq!(normalize_russian_object("Омске"), "Омск");
    }

    #[test]
    fn normalize_object_does_not_strip_short_english() {
        // English text should be unchanged (no Cyrillic = pass-through)
        assert_eq!(normalize_russian_object("Alice"), "Alice");
        assert_eq!(normalize_russian_object("New York"), "New York");
    }

    #[test]
    fn normalize_object_strips_russian_instrumental() {
        assert_eq!(normalize_russian_object("Газпромом"), "Газпром");
        assert_eq!(normalize_russian_object("Москвой"), "Москв");
    }

    #[test]
    fn has_cyrillic_detects_russian_text() {
        assert!(has_cyrillic("Газпром"));
        assert!(has_cyrillic("Москва"));
        assert!(!has_cyrillic("Acme Corp"));
        assert!(!has_cyrillic("New York"));
        assert!(!has_cyrillic(""));
    }
}
