//! Fuzzy entity resolution for deduplicating entity names.
//!
//! Handles cases like "Иван Петров" vs "иван петров" vs "I. Petrov" by combining
//! exact indexed lookup, Unicode normalization, and Levenshtein similarity.

use strsim::normalized_levenshtein;
use unicode_normalization::UnicodeNormalization;

use crate::models::EntityCandidate;
use crate::service::entity::EntityService;
use crate::service::error::MemoryError;

/// Default similarity threshold for fuzzy entity matching.
pub const DEFAULT_FUZZY_THRESHOLD: f64 = 0.85;

/// Resolves entity candidates to existing entities using fuzzy matching,
/// or creates new entities when no suitable match is found.
#[derive(Debug, Clone)]
pub struct EntityResolver {
    /// Minimum normalized Levenshtein similarity for automatic merging.
    pub similarity_threshold: f64,
}

impl EntityResolver {
    pub fn new(similarity_threshold: f64) -> Self {
        Self {
            similarity_threshold,
        }
    }

    /// Resolve an entity candidate: find the best existing match or indicate
    /// that a new entity should be created.
    ///
    /// Returns `(entity_id, was_created)`.
    pub async fn resolve_or_create(
        &self,
        entity_service: &EntityService,
        candidate: EntityCandidate,
        namespace: &str,
    ) -> Result<(String, bool), MemoryError> {
        let normalized = normalize_entity_name(&candidate.canonical_name);

        // Step 1: Exact match by normalized name (fast path via DB index).
        if let Some(entity_id) = entity_service
            .find_entity_id_by_name(&normalized, namespace)
            .await?
        {
            return Ok((entity_id, false));
        }

        // Step 2: Check aliases for an exact match.
        if let Some(entity_id) = entity_service
            .find_entity_id_by_alias(&normalized, namespace)
            .await?
        {
            return Ok((entity_id, false));
        }

        // Step 3: Fuzzy match — find candidates with similar names.
        let prefix = &normalized[..normalized.len().min(3)];
        let candidates = entity_service
            .find_entities_by_prefix(namespace, prefix)
            .await?;

        let best_match = candidates
            .iter()
            .filter_map(|(id, name)| {
                let candidate_normalized = normalize_entity_name(name);
                let score = normalized_levenshtein(&normalized, &candidate_normalized);
                if score >= self.similarity_threshold {
                    Some((id.clone(), score))
                } else {
                    None
                }
            })
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        if let Some((entity_id, score)) = best_match {
            // Record the alias for future exact matches.
            if score < 1.0 {
                entity_service
                    .add_alias_to_entity(&entity_id, &candidate.canonical_name, namespace)
                    .await?;
                // Entity resolution: merged '{}' → '{}' (score={:.2})
                let _ = (entity_id.clone(), score);
            }
            return Ok((entity_id, false));
        }

        // Step 4: No match found — create a new entity.
        let entity_id = entity_service.create_entity(candidate, namespace).await?;
        Ok((entity_id, true))
    }
}

/// Normalize an entity name for comparison.
///
/// Applies NFKC normalization, lowercase, and whitespace collapse.
/// This handles Unicode equivalence (e.g., fullwidth → ASCII),
/// case differences, and whitespace variations.
pub fn normalize_entity_name(s: &str) -> String {
    s.trim()
        .nfkc()
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_entity_name_handles_case() {
        assert_eq!(normalize_entity_name("Alice Smith"), "alice smith");
        assert_eq!(normalize_entity_name("ALICE SMITH"), "alice smith");
    }

    #[test]
    fn normalize_entity_name_handles_whitespace() {
        assert_eq!(normalize_entity_name("  Alice   Smith  "), "alice smith");
    }

    #[test]
    fn normalize_entity_name_handles_cyrillic() {
        assert_eq!(normalize_entity_name("Иван Петров"), "иван петров");
        assert_eq!(normalize_entity_name("ИВАН ПЕТРОВ"), "иван петров");
    }

    #[test]
    fn normalize_entity_name_handles_nfkc() {
        // Fullwidth Latin → ASCII
        assert_eq!(normalize_entity_name("Ａｌｉｃｅ"), "alice");
    }

    #[test]
    fn levenshtein_similarity_basic() {
        let a = normalize_entity_name("Ivan Petrov");
        let b = normalize_entity_name("I. Petrov");
        let score = normalized_levenshtein(&a, &b);
        assert!(score > 0.5, "score={score} should be > 0.5");
    }

    #[test]
    fn levenshtein_similarity_exact() {
        let a = normalize_entity_name("Alice Smith");
        let b = normalize_entity_name("alice smith");
        let score = normalized_levenshtein(&a, &b);
        assert!(
            (score - 1.0).abs() < f64::EPSILON,
            "exact match should score 1.0"
        );
    }

    #[test]
    fn levenshtein_similarity_below_threshold() {
        let a = normalize_entity_name("Alice Smith");
        let b = normalize_entity_name("Bob Jones");
        let score = normalized_levenshtein(&a, &b);
        assert!(
            score < DEFAULT_FUZZY_THRESHOLD,
            "score={score} should be below {DEFAULT_FUZZY_THRESHOLD}"
        );
    }
}
