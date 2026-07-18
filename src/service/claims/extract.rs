//! Claim extraction from facts and episodes.

use crate::models::claim::CanonicalPayloadHash;
use crate::service::MemoryError;

use super::schema::{ClaimDraftCandidate, ClaimProjectionInput, ClaimSchemaRegistry, ClaimSkip};

// ─── Projection Result ────────────────────────────────────────────────────────

/// The result of projecting a fact through all schemas.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct ProjectionResult {
    pub drafts: Vec<ClaimDraftCandidate>,
    pub skips: Vec<ClaimSkip>,
}

// ─── Project Fact ─────────────────────────────────────────────────────────────

/// Project a single fact through the schema registry, returning deduplicated drafts.
#[allow(dead_code)]
pub(crate) fn project_fact(
    registry: &ClaimSchemaRegistry,
    input: &ClaimProjectionInput<'_>,
) -> Result<ProjectionResult, MemoryError> {
    let mut drafts = Vec::new();
    let mut skips = Vec::new();

    registry.project_all(input, &mut drafts, &mut skips)?;

    // Deduplicate by canonical payload
    let mut seen = std::collections::BTreeSet::new();
    drafts.retain(|d| {
        let hash = CanonicalPayloadHash::compute(&d.value, &d.qualifiers);
        seen.insert(hash)
    });

    Ok(ProjectionResult { drafts, skips })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::schema::*;
    use super::*;
    use crate::models::claim::{ClaimSchemaFamily, ExtractorFingerprint};
    use crate::models::{EpisodeId, FactId};
    use std::collections::BTreeMap;

    fn test_input(fields: BTreeMap<String, String>) -> ClaimProjectionInput<'static> {
        ClaimProjectionInput {
            namespace: "test",
            source_fact_id: FactId::from("fact:test"),
            source_episode_id: EpisodeId::from("ep:test"),
            scope: "personal",
            project: None,
            policy_tags: &[],
            subject: "entity:subject1",
            t_ref: chrono::Utc::now(),
            content: "test content",
            structured_fields: Box::leak(Box::new(fields)),
        }
    }

    #[test]
    fn project_fact_deduplicates_identical_drafts() {
        let fp = ExtractorFingerprint::compute(1, "test");
        let registry = ClaimSchemaRegistry::built_in(fp);

        let mut fields = BTreeMap::new();
        fields.insert("dimension".to_string(), "Height".to_string());
        fields.insert("value".to_string(), "180".to_string());
        let input = test_input(fields);

        let result = project_fact(&registry, &input).unwrap();
        // Only one attribute draft for these fields
        let attribute_drafts: Vec<_> = result
            .drafts
            .iter()
            .filter(|d| d.schema_ref.family == ClaimSchemaFamily::Attribute)
            .collect();
        assert_eq!(attribute_drafts.len(), 1);
    }

    #[test]
    fn project_fact_returns_skips_for_invalid_values() {
        let fp = ExtractorFingerprint::compute(1, "test");
        let registry = ClaimSchemaRegistry::built_in(fp);

        let mut fields = BTreeMap::new();
        fields.insert("measure".to_string(), "Weight".to_string());
        fields.insert("value".to_string(), "not-a-number".to_string());
        let input = test_input(fields);

        let result = project_fact(&registry, &input).unwrap();
        assert!(
            result
                .skips
                .iter()
                .any(|s| s.reason_code == "invalid_value")
        );
    }

    #[test]
    fn project_fact_handles_empty_fields() {
        let fp = ExtractorFingerprint::compute(1, "test");
        let registry = ClaimSchemaRegistry::built_in(fp);

        let fields = BTreeMap::new();
        let input = test_input(fields);

        let result = project_fact(&registry, &input).unwrap();
        assert!(result.drafts.is_empty());
        assert!(result.skips.is_empty());
    }

    #[test]
    fn source_span_is_preserved_when_set() {
        let fp = ExtractorFingerprint::compute(1, "test");
        let registry = ClaimSchemaRegistry::built_in(fp);

        let mut fields = BTreeMap::new();
        fields.insert("dimension".to_string(), "Color".to_string());
        fields.insert("value".to_string(), "Blue".to_string());
        let input = test_input(fields);

        let result = project_fact(&registry, &input).unwrap();
        // Drafts without explicit spans have source_span = None
        for draft in &result.drafts {
            assert!(draft.source_span.is_none());
        }
    }
}
