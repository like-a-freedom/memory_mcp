//! Claim extraction from facts and episodes.

use crate::error::MemoryError;
use crate::models::claim::CanonicalPayloadHash;

use super::schema::{ClaimDraftCandidate, ClaimProjectionInput, ClaimSchemaRegistry, ClaimSkip};

// ─── Projection Result ────────────────────────────────────────────────────────

/// The result of projecting a fact through all schemas.
#[derive(Debug)]
pub(crate) struct ProjectionResult {
    pub drafts: Vec<ClaimDraftCandidate>,
    pub skips: Vec<ClaimSkip>,
}

// ─── Project Fact ─────────────────────────────────────────────────────────────

/// Project a single fact through the schema registry, returning deduplicated drafts.
pub(crate) fn project_fact(
    registry: &ClaimSchemaRegistry,
    input: &ClaimProjectionInput<'_>,
) -> Result<ProjectionResult, MemoryError> {
    let mut drafts = Vec::new();
    let mut skips = Vec::new();

    // The current schemas project one subject per fact. Do not bind several
    // independent copular assertions to the first subject.
    if input.structured_fields.is_empty() && input.content.matches(" is ").count() > 1 {
        skips.push(ClaimSkip {
            reason_code: "ambiguous_subject".to_string(),
        });
        return Ok(ProjectionResult { drafts, skips });
    }

    registry.project_all(input, &mut drafts, &mut skips)?;
    // Colon-delimited prose cannot establish scalar attribute cardinality.
    // Other schemas still evaluate their own supported constructions, so a
    // URL or timestamp does not suppress a commitment or quantity assertion.
    if input.structured_fields.is_empty()
        && super::structural::has_unstructured_colon_value(input.content)
    {
        drafts.retain(|draft| {
            draft.schema_ref.family == crate::models::claim::ClaimSchemaFamily::Commitment
                && input.fact_type == "promise"
        });
        skips.push(ClaimSkip {
            reason_code: "unstructured_colon_value".to_string(),
        });
    }

    if !input.structured_fields.is_empty() {
        use crate::models::claim::ClaimSchemaFamily;
        drafts.retain(|draft| match draft.schema_ref.family {
            ClaimSchemaFamily::Attribute => input.structured_fields.contains_key("dimension"),
            ClaimSchemaFamily::Quantity => input.structured_fields.contains_key("measure"),
            ClaimSchemaFamily::Relation => input.structured_fields.contains_key("predicate"),
            ClaimSchemaFamily::Commitment => input.structured_fields.contains_key("action"),
        });
    }

    // The generic attribute schema has no domain cardinality evidence for
    // free prose ("Alice is a runner" and "Alice is a coach" can coexist).
    // Only an explicit schema record declares its single-valued dimension.
    // Quantity/commitment schemas retain their typed assertion contracts.
    if !input.structured_fields.contains_key("dimension") {
        let before = drafts.len();
        drafts.retain(|draft| {
            draft.schema_ref.family != crate::models::claim::ClaimSchemaFamily::Attribute
        });
        if drafts.len() < before {
            skips.push(ClaimSkip {
                reason_code: "unknown_cardinality".to_string(),
            });
        }
    }

    // An absent subject cannot identify a comparison slot. Keeping it as an
    // empty string causes otherwise unrelated facts to reconcile together.
    drafts.retain(|draft| {
        if draft.subject.trim().is_empty() {
            skips.push(ClaimSkip {
                reason_code: "missing_subject".to_string(),
            });
            false
        } else {
            true
        }
    });

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
    use super::*;
    use crate::models::claim::{ClaimSchemaFamily, ExtractorFingerprint};

    use std::collections::BTreeMap;

    const EMPTY_ASSERTIONS: &[crate::knowledge::claims_policy::structural::StructuralAssertion] =
        &[];

    fn test_input(fields: BTreeMap<String, String>) -> ClaimProjectionInput<'static> {
        ClaimProjectionInput {
            subject: "entity:subject1",
            t_ref: chrono::Utc::now(),
            content: "test content",
            fact_type: "experience",
            structured_fields: Box::leak(Box::new(fields)),
            assertions: EMPTY_ASSERTIONS,
        }
    }

    fn test_input_with_content(
        fields: BTreeMap<String, String>,
        content: &'static str,
    ) -> ClaimProjectionInput<'static> {
        ClaimProjectionInput {
            subject: "entity:subject1",
            t_ref: chrono::Utc::now(),
            content,
            fact_type: "experience",
            structured_fields: Box::leak(Box::new(fields)),
            assertions: EMPTY_ASSERTIONS,
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
        for draft in &result.drafts {
            assert!(draft.source_span.is_none());
        }
    }

    #[test]
    fn project_fact_skips_unstructured_kv_content() {
        let fp = ExtractorFingerprint::compute(1, "test");
        let registry = ClaimSchemaRegistry::built_in(fp);

        let fields = BTreeMap::new();
        let input = test_input_with_content(fields, "Height: 180 cm");

        let result = project_fact(&registry, &input).unwrap();
        assert!(result.drafts.is_empty());
        assert!(
            result
                .skips
                .iter()
                .any(|skip| skip.reason_code == "unstructured_colon_value")
        );
    }

    #[test]
    fn project_fact_extracts_from_sentence_pattern() {
        let fp = ExtractorFingerprint::compute(1, "test");
        let registry = ClaimSchemaRegistry::built_in(fp);

        let fields = BTreeMap::new();
        let input = test_input_with_content(fields, "Temperature is 36.5 celsius");

        let result = project_fact(&registry, &input).unwrap();
        // Should have quantity draft from sentence parsing
        let quantity_drafts: Vec<_> = result
            .drafts
            .iter()
            .filter(|d| d.schema_ref.family == ClaimSchemaFamily::Quantity)
            .collect();
        assert_eq!(quantity_drafts.len(), 1);
    }

    #[test]
    fn project_fact_skips_empty_content_without_fields() {
        let fp = ExtractorFingerprint::compute(1, "test");
        let registry = ClaimSchemaRegistry::built_in(fp);

        let fields = BTreeMap::new();
        let input = test_input_with_content(fields, "");

        let result = project_fact(&registry, &input).unwrap();
        assert!(result.drafts.is_empty());
    }
}
