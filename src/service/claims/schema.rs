//! Structural schema registry for claim extraction.
//!
//! Each schema defines how to project source content into claim drafts
//! and what policy applies to a comparison key.

use std::collections::BTreeMap;

use crate::models::claim::{
    CanonicalDecimal, CanonicalUnit, ClaimCardinality, ClaimSchemaFamily, ClaimSchemaRef,
    ClaimValiditySource, ClaimValue, ComparisonKey, NormalizedText,
};
use crate::models::{EpisodeId, FactId};
use crate::service::MemoryError;

// ─── Projection Input ─────────────────────────────────────────────────────────

/// Borrowed inputs for a single projection call.
#[allow(dead_code)]
pub(crate) struct ClaimProjectionInput<'a> {
    pub namespace: &'a str,
    pub source_fact_id: FactId,
    pub source_episode_id: EpisodeId,
    pub scope: &'a str,
    pub project: Option<&'a str>,
    pub policy_tags: &'a [String],
    pub subject: &'a str,
    pub t_ref: chrono::DateTime<chrono::Utc>,
    /// The raw source content text.
    pub content: &'a str,
    /// Structured fields parsed from connector/record sources.
    pub structured_fields: &'a BTreeMap<String, String>,
}

// ─── Claim Draft (local to extraction) ───────────────────────────────────────

/// An intermediate claim draft produced by a projector before ID assignment.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ClaimDraftCandidate {
    pub schema_ref: ClaimSchemaRef,
    pub subject: String,
    pub comparison_key: ComparisonKey,
    pub qualifiers: BTreeMap<String, String>,
    pub value: ClaimValue,
    pub cardinality: ClaimCardinality,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub valid_from: Option<chrono::DateTime<chrono::Utc>>,
    pub valid_to: Option<chrono::DateTime<chrono::Utc>>,
    pub validity_source: ClaimValiditySource,
    pub source_lineage: Option<String>,
    /// Byte offset range in the original source content.
    pub source_span: Option<(usize, usize)>,
}

// ─── Claim Policy ─────────────────────────────────────────────────────────────

/// The reconciliation policy for a comparison key.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ClaimPolicy {
    pub cardinality: ClaimCardinality,
}

// ─── Claim Skip ───────────────────────────────────────────────────────────────

/// A bounded reason a projector skipped an extraction.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ClaimSkip {
    pub reason_code: String,
    pub detail: Option<String>,
}

// ─── Claim Schema Trait ───────────────────────────────────────────────────────

/// A structural schema that projects source content into claim drafts.
#[allow(dead_code)]
pub(crate) trait ClaimSchema: Send + Sync {
    fn schema_ref(&self) -> ClaimSchemaRef;
    fn project(
        &self,
        input: &ClaimProjectionInput<'_>,
        output: &mut Vec<ClaimDraftCandidate>,
        skips: &mut Vec<ClaimSkip>,
    ) -> Result<(), MemoryError>;
    fn policy(&self, key: &ComparisonKey) -> ClaimPolicy;
}

// ─── Schema Registry ──────────────────────────────────────────────────────────

/// A compiled registry of all built-in claim schemas.
#[allow(dead_code)]
pub(crate) struct ClaimSchemaRegistry {
    schemas: Vec<Box<dyn ClaimSchema>>,
    extractor_fingerprint: crate::models::claim::ExtractorFingerprint,
}

#[allow(dead_code)]
impl ClaimSchemaRegistry {
    /// Build the built-in schema registry.
    pub fn built_in(extractor_fingerprint: crate::models::claim::ExtractorFingerprint) -> Self {
        let schemas: Vec<Box<dyn ClaimSchema>> = vec![
            Box::new(AttributeV1),
            Box::new(QuantityV1),
            Box::new(RelationV1),
            Box::new(CommitmentV1),
        ];
        Self {
            schemas,
            extractor_fingerprint,
        }
    }

    /// Project a fact through all registered schemas, collecting drafts and skips.
    pub fn project_all(
        &self,
        input: &ClaimProjectionInput<'_>,
        output: &mut Vec<ClaimDraftCandidate>,
        skips: &mut Vec<ClaimSkip>,
    ) -> Result<(), MemoryError> {
        for schema in &self.schemas {
            schema.project(input, output, skips)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn extractor_fingerprint(&self) -> &crate::models::claim::ExtractorFingerprint {
        &self.extractor_fingerprint
    }
}

// ─── Built-in Schemas ─────────────────────────────────────────────────────────

struct AttributeV1;

#[allow(dead_code)]
impl ClaimSchema for AttributeV1 {
    fn schema_ref(&self) -> ClaimSchemaRef {
        ClaimSchemaRef {
            family: ClaimSchemaFamily::Attribute,
            version: std::num::NonZeroU16::new(1).unwrap(),
        }
    }

    fn project(
        &self,
        input: &ClaimProjectionInput<'_>,
        output: &mut Vec<ClaimDraftCandidate>,
        _skips: &mut Vec<ClaimSkip>,
    ) -> Result<(), MemoryError> {
        // Extract attribute claims from structured fields
        // Pattern: "dimension" key with a value
        if let Some(dimension) = input.structured_fields.get("dimension")
            && let Some(val) = input.structured_fields.get("value")
        {
            let mut components = BTreeMap::new();
            components.insert(
                "dimension".to_string(),
                NormalizedText::new(dimension).to_string(),
            );
            let key = ComparisonKey::new(self.schema_ref(), components)?;
            output.push(ClaimDraftCandidate {
                schema_ref: self.schema_ref(),
                subject: input.subject.to_string(),
                comparison_key: key,
                qualifiers: BTreeMap::new(),
                value: ClaimValue::Text(NormalizedText::new(val)),
                cardinality: ClaimCardinality::SingleValued,
                observed_at: input.t_ref,
                valid_from: None,
                valid_to: None,
                validity_source: ClaimValiditySource::Explicit,
                source_lineage: None,
                source_span: None,
            });
        }
        Ok(())
    }

    fn policy(&self, _key: &ComparisonKey) -> ClaimPolicy {
        ClaimPolicy {
            cardinality: ClaimCardinality::SingleValued,
        }
    }
}

struct QuantityV1;

#[allow(dead_code)]
impl ClaimSchema for QuantityV1 {
    fn schema_ref(&self) -> ClaimSchemaRef {
        ClaimSchemaRef {
            family: ClaimSchemaFamily::Quantity,
            version: std::num::NonZeroU16::new(1).unwrap(),
        }
    }

    fn project(
        &self,
        input: &ClaimProjectionInput<'_>,
        output: &mut Vec<ClaimDraftCandidate>,
        _skips: &mut Vec<ClaimSkip>,
    ) -> Result<(), MemoryError> {
        if let Some(measure) = input.structured_fields.get("measure")
            && let Some(val_str) = input.structured_fields.get("value")
        {
            let unit_str = input
                .structured_fields
                .get("unit")
                .map(|s| s.as_str())
                .unwrap_or("");
            match CanonicalDecimal::parse(val_str) {
                Ok(decimal) => {
                    let mut components = BTreeMap::new();
                    components.insert(
                        "measure".to_string(),
                        NormalizedText::new(measure).to_string(),
                    );
                    components.insert("unit_family".to_string(), unit_str.to_string());
                    let key = ComparisonKey::new(self.schema_ref(), components)?;
                    output.push(ClaimDraftCandidate {
                        schema_ref: self.schema_ref(),
                        subject: input.subject.to_string(),
                        comparison_key: key,
                        qualifiers: BTreeMap::new(),
                        value: ClaimValue::Quantity {
                            value: decimal,
                            unit: CanonicalUnit {
                                family: unit_str.to_string(),
                                symbol: unit_str.to_string(),
                            },
                        },
                        cardinality: ClaimCardinality::SingleValued,
                        observed_at: input.t_ref,
                        valid_from: None,
                        valid_to: None,
                        validity_source: ClaimValiditySource::Explicit,
                        source_lineage: None,
                        source_span: None,
                    });
                }
                Err(_) => {
                    _skips.push(ClaimSkip {
                        reason_code: "invalid_value".to_string(),
                        detail: Some(format!("cannot parse decimal: {val_str}")),
                    });
                }
            }
        }
        Ok(())
    }

    fn policy(&self, _key: &ComparisonKey) -> ClaimPolicy {
        ClaimPolicy {
            cardinality: ClaimCardinality::SingleValued,
        }
    }
}

struct RelationV1;

#[allow(dead_code)]
impl ClaimSchema for RelationV1 {
    fn schema_ref(&self) -> ClaimSchemaRef {
        ClaimSchemaRef {
            family: ClaimSchemaFamily::Relation,
            version: std::num::NonZeroU16::new(1).unwrap(),
        }
    }

    fn project(
        &self,
        input: &ClaimProjectionInput<'_>,
        output: &mut Vec<ClaimDraftCandidate>,
        _skips: &mut Vec<ClaimSkip>,
    ) -> Result<(), MemoryError> {
        if let Some(predicate) = input.structured_fields.get("predicate")
            && let Some(object) = input.structured_fields.get("object")
        {
            let mut components = BTreeMap::new();
            components.insert(
                "predicate".to_string(),
                NormalizedText::new(predicate).to_string(),
            );
            components.insert("object_role".to_string(), "target".to_string());
            let key = ComparisonKey::new(self.schema_ref(), components)?;
            output.push(ClaimDraftCandidate {
                schema_ref: self.schema_ref(),
                subject: input.subject.to_string(),
                comparison_key: key,
                qualifiers: BTreeMap::new(),
                value: ClaimValue::Text(NormalizedText::new(object)),
                cardinality: ClaimCardinality::SetValued,
                observed_at: input.t_ref,
                valid_from: None,
                valid_to: None,
                validity_source: ClaimValiditySource::Explicit,
                source_lineage: None,
                source_span: None,
            });
        }
        Ok(())
    }

    fn policy(&self, _key: &ComparisonKey) -> ClaimPolicy {
        ClaimPolicy {
            cardinality: ClaimCardinality::SetValued,
        }
    }
}

struct CommitmentV1;

#[allow(dead_code)]
impl ClaimSchema for CommitmentV1 {
    fn schema_ref(&self) -> ClaimSchemaRef {
        ClaimSchemaRef {
            family: ClaimSchemaFamily::Commitment,
            version: std::num::NonZeroU16::new(1).unwrap(),
        }
    }

    fn project(
        &self,
        input: &ClaimProjectionInput<'_>,
        output: &mut Vec<ClaimDraftCandidate>,
        _skips: &mut Vec<ClaimSkip>,
    ) -> Result<(), MemoryError> {
        if let Some(action) = input.structured_fields.get("action")
            && let Some(target) = input.structured_fields.get("target")
        {
            let mut components = BTreeMap::new();
            components.insert(
                "action_role".to_string(),
                NormalizedText::new(action).to_string(),
            );
            components.insert(
                "target_role".to_string(),
                NormalizedText::new(target).to_string(),
            );
            let key = ComparisonKey::new(self.schema_ref(), components)?;

            let value = if let Some(status) = input.structured_fields.get("status") {
                ClaimValue::Text(NormalizedText::new(status))
            } else {
                ClaimValue::Boolean(true)
            };

            output.push(ClaimDraftCandidate {
                schema_ref: self.schema_ref(),
                subject: input.subject.to_string(),
                comparison_key: key,
                qualifiers: BTreeMap::new(),
                value,
                cardinality: ClaimCardinality::SingleValued,
                observed_at: input.t_ref,
                valid_from: None,
                valid_to: None,
                validity_source: ClaimValiditySource::Explicit,
                source_lineage: None,
                source_span: None,
            });
        }
        Ok(())
    }

    fn policy(&self, _key: &ComparisonKey) -> ClaimPolicy {
        ClaimPolicy {
            cardinality: ClaimCardinality::SingleValued,
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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

    fn schema_ref(family: ClaimSchemaFamily) -> ClaimSchemaRef {
        ClaimSchemaRef {
            family,
            version: std::num::NonZeroU16::new(1).unwrap(),
        }
    }

    #[test]
    fn attribute_v1_extracts_from_structured_fields() {
        let mut fields = BTreeMap::new();
        fields.insert("dimension".to_string(), "Height".to_string());
        fields.insert("value".to_string(), "180 cm".to_string());
        let input = test_input(fields);

        let schema = AttributeV1;
        let mut output = Vec::new();
        let mut skips = Vec::new();
        schema.project(&input, &mut output, &mut skips).unwrap();

        assert_eq!(output.len(), 1);
        assert_eq!(
            output[0].schema_ref,
            schema_ref(ClaimSchemaFamily::Attribute)
        );
        assert!(matches!(&output[0].value, ClaimValue::Text(t) if t.as_str() == "180 cm"));
        assert!(skips.is_empty());
    }

    #[test]
    fn attribute_v1_skips_without_dimension() {
        let fields = BTreeMap::new();
        let input = test_input(fields);

        let schema = AttributeV1;
        let mut output = Vec::new();
        let mut skips = Vec::new();
        schema.project(&input, &mut output, &mut skips).unwrap();

        assert!(output.is_empty());
    }

    #[test]
    fn quantity_v1_extracts_measure_and_value() {
        let mut fields = BTreeMap::new();
        fields.insert("measure".to_string(), "Temperature".to_string());
        fields.insert("value".to_string(), "36.5".to_string());
        fields.insert("unit".to_string(), "celsius".to_string());
        let input = test_input(fields);

        let schema = QuantityV1;
        let mut output = Vec::new();
        let mut skips = Vec::new();
        schema.project(&input, &mut output, &mut skips).unwrap();

        assert_eq!(output.len(), 1);
        assert_eq!(
            output[0].schema_ref,
            schema_ref(ClaimSchemaFamily::Quantity)
        );
        if let ClaimValue::Quantity { value, unit } = &output[0].value {
            assert_eq!(value.coefficient(), 365);
            assert_eq!(value.scale(), 1);
            assert_eq!(unit.family, "celsius");
        } else {
            panic!("expected Quantity value");
        }
    }

    #[test]
    fn quantity_v1_skips_on_invalid_value() {
        let mut fields = BTreeMap::new();
        fields.insert("measure".to_string(), "Weight".to_string());
        fields.insert("value".to_string(), "not-a-number".to_string());
        let input = test_input(fields);

        let schema = QuantityV1;
        let mut output = Vec::new();
        let mut skips = Vec::new();
        schema.project(&input, &mut output, &mut skips).unwrap();

        assert!(output.is_empty());
        assert_eq!(skips.len(), 1);
        assert_eq!(skips[0].reason_code, "invalid_value");
    }

    #[test]
    fn relation_v1_extracts_predicate_and_object() {
        let mut fields = BTreeMap::new();
        fields.insert("predicate".to_string(), "works_at".to_string());
        fields.insert("object".to_string(), "Acme Corp".to_string());
        let input = test_input(fields);

        let schema = RelationV1;
        let mut output = Vec::new();
        let mut skips = Vec::new();
        schema.project(&input, &mut output, &mut skips).unwrap();

        assert_eq!(output.len(), 1);
        assert_eq!(
            output[0].schema_ref,
            schema_ref(ClaimSchemaFamily::Relation)
        );
        assert_eq!(output[0].cardinality, ClaimCardinality::SetValued);
        if let ClaimValue::Text(t) = &output[0].value {
            assert_eq!(t.as_str(), "acme corp");
        } else {
            panic!("expected Text value");
        }
    }

    #[test]
    fn commitment_v1_extracts_action_and_target() {
        let mut fields = BTreeMap::new();
        fields.insert("action".to_string(), "deliver".to_string());
        fields.insert("target".to_string(), "report".to_string());
        fields.insert("status".to_string(), "pending".to_string());
        let input = test_input(fields);

        let schema = CommitmentV1;
        let mut output = Vec::new();
        let mut skips = Vec::new();
        schema.project(&input, &mut output, &mut skips).unwrap();

        assert_eq!(output.len(), 1);
        assert_eq!(
            output[0].schema_ref,
            schema_ref(ClaimSchemaFamily::Commitment)
        );
        if let ClaimValue::Text(t) = &output[0].value {
            assert_eq!(t.as_str(), "pending");
        } else {
            panic!("expected Text value for status");
        }
    }

    #[test]
    fn commitment_v1_defaults_to_boolean_true_without_status() {
        let mut fields = BTreeMap::new();
        fields.insert("action".to_string(), "review".to_string());
        fields.insert("target".to_string(), "pr".to_string());
        let input = test_input(fields);

        let schema = CommitmentV1;
        let mut output = Vec::new();
        let mut skips = Vec::new();
        schema.project(&input, &mut output, &mut skips).unwrap();

        assert_eq!(output.len(), 1);
        assert!(matches!(&output[0].value, ClaimValue::Boolean(true)));
    }

    #[test]
    fn registry_project_all_dispatches_to_all_schemas() {
        let fp = crate::models::claim::ExtractorFingerprint::compute(1, "test");
        let registry = ClaimSchemaRegistry::built_in(fp);

        let mut fields = BTreeMap::new();
        fields.insert("dimension".to_string(), "Score".to_string());
        fields.insert("value".to_string(), "95".to_string());
        fields.insert("measure".to_string(), "Score".to_string());
        fields.insert("unit".to_string(), "points".to_string());
        fields.insert("predicate".to_string(), "likes".to_string());
        fields.insert("object".to_string(), "pizza".to_string());
        fields.insert("action".to_string(), "eat".to_string());
        fields.insert("target".to_string(), "lunch".to_string());
        let input = test_input(fields);

        let mut output = Vec::new();
        let mut skips = Vec::new();
        registry
            .project_all(&input, &mut output, &mut skips)
            .unwrap();

        // Should have drafts from attribute, quantity, relation, commitment
        assert!(
            output.len() >= 3,
            "expected at least 3 drafts, got {}",
            output.len()
        );
    }

    #[test]
    fn comparison_key_is_deterministic_for_same_inputs() {
        let schema = schema_ref(ClaimSchemaFamily::Attribute);
        let mut c1 = BTreeMap::new();
        c1.insert("dim".to_string(), "height".to_string());
        let mut c2 = BTreeMap::new();
        c2.insert("dim".to_string(), "height".to_string());
        let k1 = ComparisonKey::new(schema, c1).unwrap();
        let k2 = ComparisonKey::new(schema, c2).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn comparison_key_differs_by_schema_family() {
        let k1 = ComparisonKey::new(
            schema_ref(ClaimSchemaFamily::Attribute),
            BTreeMap::from([("dim".to_string(), "x".to_string())]),
        )
        .unwrap();
        let k2 = ComparisonKey::new(
            schema_ref(ClaimSchemaFamily::Quantity),
            BTreeMap::from([("dim".to_string(), "x".to_string())]),
        )
        .unwrap();
        assert_ne!(k1, k2);
    }
}
