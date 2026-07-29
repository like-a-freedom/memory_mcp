//! Shared structural assertion representation.
//!
//! Parses raw fact content into generic structural assertions once.
//! Schemas consume assertions and never re-parse raw content independently.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ops::Range;

use crate::models::claim::{
    CanonicalPayloadHash, ClaimSchemaRef, ComparisonKeyHash, NormalizedText, QualifierHash,
};

// ─── Types ────────────────────────────────────────────────────────────────────

/// A single parsed assertion from source content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructuralAssertion {
    pub subject_hint: Option<NormalizedText>,
    pub predicate: NormalizedText,
    pub value: StructuralValue,
    pub qualifiers: BTreeMap<String, String>,
    pub cardinality_evidence: CardinalityEvidence,
    pub valid_from: Option<chrono::DateTime<chrono::Utc>>,
    pub valid_to: Option<chrono::DateTime<chrono::Utc>>,
    pub source_span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StructuralValue {
    Text(NormalizedText),
    Number { raw: String, unit: Option<String> },
    EntityRef(NormalizedText),
    Boolean(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardinalityEvidence {
    ExplicitScalar,
    ExplicitCollection,
    Unknown,
}

/// A candidate subject resolved from metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubjectCandidate {
    pub entity_id: String,
    pub names: Vec<NormalizedText>,
}

/// A projection identity for deduplication.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProjectionIdentity {
    pub schema: ClaimSchemaRef,
    pub subject: String,
    pub comparison_key_hash: ComparisonKeyHash,
    pub qualifier_hash: QualifierHash,
    pub value_hash: CanonicalPayloadHash,
}

/// Resolve a subject from hint and candidates.
pub(crate) fn resolve_subject<'a>(
    hint: Option<&NormalizedText>,
    candidates: &'a [SubjectCandidate],
) -> Result<&'a str, &'static str> {
    match hint {
        Some(hint) => {
            let mut matches = candidates
                .iter()
                .filter(|candidate| candidate.names.iter().any(|name| name == hint));
            match (matches.next(), matches.next()) {
                (Some(only), None) => Ok(only.entity_id.as_str()),
                _ => Err("unresolved_subject"),
            }
        }
        None if candidates.len() == 1 => Ok(candidates[0].entity_id.as_str()),
        None => Err("unresolved_subject"),
    }
}

/// Parse raw content into structural assertions.
pub(crate) fn parse_assertions(content: &str) -> Vec<StructuralAssertion> {
    let assertions = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    // Priority 1: Try JSON object with scalar leaves
    if let Some(parsed) = try_parse_json_scalars(content) {
        return parsed;
    }

    // Priority 2: Key-value lines
    if let Some(kvs) = try_parse_key_value(&lines) {
        return kvs;
    }

    // Priority 3: Conservative sentence patterns
    if let Some(sentences) = try_parse_sentences(content) {
        return sentences;
    }

    assertions
}

fn try_parse_json_scalars(_content: &str) -> Option<Vec<StructuralAssertion>> {
    // TODO: implement JSON scalar leaf extraction
    None
}

fn try_parse_key_value(lines: &[&str]) -> Option<Vec<StructuralAssertion>> {
    let mut assertions = Vec::new();
    for line in lines {
        let line = line.trim();
        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim();
            let val = val.trim();
            if !key.is_empty() && !val.is_empty() {
                let (value, qualifiers) = detect_value_and_qualifiers(val);
                let cardinality = if val.contains(',') {
                    CardinalityEvidence::ExplicitCollection
                } else {
                    CardinalityEvidence::ExplicitScalar
                };
                assertions.push(StructuralAssertion {
                    subject_hint: None,
                    predicate: NormalizedText::new(key),
                    value,
                    qualifiers,
                    cardinality_evidence: cardinality,
                    valid_from: None,
                    valid_to: None,
                    source_span: 0..line.len(),
                });
            }
        }
    }
    if assertions.is_empty() {
        None
    } else {
        Some(assertions)
    }
}

fn try_parse_sentences(content: &str) -> Option<Vec<StructuralAssertion>> {
    let lower = content.to_lowercase();

    // Pattern: "The X is Y" or "X is Y"
    if let Some((subject, predicate, value_text)) = extract_is_sentence(&lower) {
        let (value, qualifiers) = detect_value_and_qualifiers(&value_text);
        return Some(vec![StructuralAssertion {
            subject_hint: Some(NormalizedText::new(&subject)),
            predicate: NormalizedText::new(&predicate),
            value,
            qualifiers,
            cardinality_evidence: CardinalityEvidence::Unknown,
            valid_from: None,
            valid_to: None,
            source_span: 0..content.len(),
        }]);
    }

    None
}

fn extract_is_sentence(text: &str) -> Option<(String, String, String)> {
    // Match: "the X is Y" or "X is Y"
    let text = text.trim();
    let after_the = if let Some(t) = text.strip_prefix("the ") {
        t
    } else {
        text
    };

    if let Some(pos) = after_the.find(" is ") {
        let subject = after_the[..pos].trim().to_string();
        let after_is = after_the[pos + 4..].trim();
        // Predicate is the subject itself (dimension name)
        // Value is everything after "is"
        Some((subject.clone(), subject, after_is.to_string()))
    } else {
        None
    }
}

fn detect_value_and_qualifiers(val: &str) -> (StructuralValue, BTreeMap<String, String>) {
    let val = val.trim();
    let mut qualifiers = BTreeMap::new();

    // Correction/transition markers
    if val.eq_ignore_ascii_case("corrected")
        || val.eq_ignore_ascii_case("correction")
        || val.eq_ignore_ascii_case("replaces")
    {
        qualifiers.insert("correction".to_string(), val.to_lowercase());
    }
    if val.eq_ignore_ascii_case("supersedes") || val.eq_ignore_ascii_case("transition") {
        qualifiers.insert("transition".to_string(), val.to_lowercase());
    }

    // Boolean detection
    if let Some(b) = parse_bool(val) {
        return (StructuralValue::Boolean(b), qualifiers);
    }

    // Number detection (with optional unit)
    if let Some((num, unit)) = split_number_unit(val) {
        return (StructuralValue::Number { raw: num, unit }, qualifiers);
    }

    // Text
    (StructuralValue::Text(NormalizedText::new(val)), qualifiers)
}

fn parse_bool(val: &str) -> Option<bool> {
    match val.to_lowercase().as_str() {
        "true" | "yes" | "active" | "on" | "enabled" => Some(true),
        "false" | "no" | "inactive" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn split_number_unit(val: &str) -> Option<(String, Option<String>)> {
    let val = val.trim();
    // Try to split into number + optional unit suffix
    let num_end = val.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != '+');
    match num_end {
        Some(pos) if pos > 0 => {
            let num_part = &val[..pos];
            let rest = val[pos..].trim();
            if !num_part.is_empty() && !rest.is_empty() {
                Some((num_part.to_string(), Some(rest.to_string())))
            } else if !num_part.is_empty() {
                Some((num_part.to_string(), None))
            } else {
                None
            }
        }
        _ => {
            if !val.is_empty()
                && val
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+')
            {
                Some((val.to_string(), None))
            } else {
                None
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::claim::ClaimSchemaFamily;

    #[test]
    fn parse_kv_measure_line() {
        let assertions = parse_assertions("height: 180 cm");
        assert_eq!(assertions.len(), 1);
        let a = &assertions[0];
        assert_eq!(a.predicate, NormalizedText::new("height"));
        assert!(matches!(a.value, StructuralValue::Number { ref raw, .. } if raw == "180"));
    }

    #[test]
    fn parse_kv_boolean_line() {
        let assertions = parse_assertions("active: true");
        assert_eq!(assertions.len(), 1);
        let a = &assertions[0];
        assert!(matches!(a.value, StructuralValue::Boolean(true)));
    }

    #[test]
    fn parse_is_sentence_scalar() {
        let assertions = parse_assertions("The temperature is 36.5 celsius");
        assert_eq!(assertions.len(), 1);
        let a = &assertions[0];
        assert!(matches!(a.value, StructuralValue::Number { .. }));
    }

    #[test]
    fn parse_is_sentence_boolean() {
        let assertions = parse_assertions("status is active");
        assert_eq!(assertions.len(), 1);
        let a = &assertions[0];
        assert!(matches!(a.value, StructuralValue::Boolean(true)));
    }

    #[test]
    fn resolve_subject_exact_match() {
        let candidates = vec![SubjectCandidate {
            entity_id: "entity:alice".to_string(),
            names: vec![NormalizedText::new("alice")],
        }];
        let hint = NormalizedText::new("alice");
        let result = resolve_subject(Some(&hint), &candidates);
        assert_eq!(result, Ok("entity:alice"));
    }

    #[test]
    fn resolve_subject_ambiguous_returns_err() {
        let candidates = vec![
            SubjectCandidate {
                entity_id: "entity:alice".to_string(),
                names: vec![NormalizedText::new("alice")],
            },
            SubjectCandidate {
                entity_id: "entity:bob".to_string(),
                names: vec![NormalizedText::new("bob")],
            },
        ];
        let hint = NormalizedText::new("unknown");
        let result = resolve_subject(Some(&hint), &candidates);
        assert_eq!(result, Err("unresolved_subject"));
    }

    #[test]
    fn resolve_subject_single_candidate_without_hint() {
        let candidates = vec![SubjectCandidate {
            entity_id: "entity:alice".to_string(),
            names: vec![NormalizedText::new("alice")],
        }];
        let result = resolve_subject(None, &candidates);
        assert_eq!(result, Ok("entity:alice"));
    }

    #[test]
    fn resolve_subject_multi_without_hint_returns_err() {
        let candidates = vec![
            SubjectCandidate {
                entity_id: "entity:alice".to_string(),
                names: vec![NormalizedText::new("alice")],
            },
            SubjectCandidate {
                entity_id: "entity:bob".to_string(),
                names: vec![NormalizedText::new("bob")],
            },
        ];
        let result = resolve_subject(None, &candidates);
        assert_eq!(result, Err("unresolved_subject"));
    }

    #[test]
    fn detect_correction_qualifier() {
        let (_, quals) = detect_value_and_qualifiers("corrected");
        assert!(quals.contains_key("correction"));
    }

    #[test]
    fn detect_transition_qualifier() {
        let (_, quals) = detect_value_and_qualifiers("supersedes");
        assert!(quals.contains_key("transition"));
    }

    #[test]
    fn two_kv_lines_produce_two_assertions() {
        let assertions = parse_assertions("height: 180 cm\nweight: 75 kg");
        assert_eq!(assertions.len(), 2);
    }

    proptest::proptest! {
        #[test]
        fn projection_identity_changes_when_schema_or_key_changes(
            key_a in "[a-z]{1,16}",
            key_b in "[a-z]{1,16}",
        ) {
            proptest::prop_assume!(key_a != key_b);
            let schema = ClaimSchemaRef {
                family: ClaimSchemaFamily::Attribute,
                version: std::num::NonZeroU16::new(1).unwrap(),
            };
            let ck_a = ComparisonKeyHash::compute(&crate::models::claim::ComparisonKey::new(
                schema,
                std::collections::BTreeMap::from([("dim".to_string(), key_a)]),
            ).unwrap());
            let ck_b = ComparisonKeyHash::compute(&crate::models::claim::ComparisonKey::new(
                schema,
                std::collections::BTreeMap::from([("dim".to_string(), key_b)]),
            ).unwrap());
            let vh = CanonicalPayloadHash::compute(
                &crate::models::claim::ClaimValue::Text(NormalizedText::new("same")),
                &BTreeMap::new(),
            );
            let qh = QualifierHash::compute(&BTreeMap::new());
            let a = ProjectionIdentity {
                schema,
                subject: "entity:s".to_string(),
                comparison_key_hash: ck_a,
                qualifier_hash: qh.clone(),
                value_hash: vh.clone(),
            };
            let b = ProjectionIdentity {
                schema,
                subject: "entity:s".to_string(),
                comparison_key_hash: ck_b,
                qualifier_hash: qh,
                value_hash: vh,
            };
            proptest::prop_assert_ne!(a, b);
        }
    }
}
