//! Semantic triple extraction from facts.
//!
//! Extracts structured (subject, predicate, object) triples from fact content.
//! Triples enable structured queries like "who works at X?" or "where does Y live?".
//!
//! The default `NoOpTripleExtractor` does nothing. An LLM-based extractor can be
//! enabled via configuration to extract triples asynchronously after fact creation.

use async_trait::async_trait;
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
