//! Shared structural assertion representation.
//!
//! Parses raw fact content into generic structural assertions once.
//! Schemas consume assertions and never re-parse raw content independently.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::models::claim::NormalizedText;

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

/// Resolve the complete subject phrase in a supported reporting statement.
/// Nested or compound attribution never establishes one comparison subject.
pub(crate) fn resolve_leading_subject<'a>(
    content: &str,
    candidates: &'a [SubjectCandidate],
) -> Result<&'a str, &'static str> {
    let content = NormalizedText::new(content);
    // Match the complete subject phrase of supported reporting constructions.
    // A mere name prefix can select a reporter in a nested attribution, or
    // one member of a compound subject, neither of which identifies the slot.
    let markers = [" reports ", " says ", " will ", " сообщает, что ", "报告"];
    if markers
        .iter()
        .map(|marker| content.as_str().matches(marker).count())
        .sum::<usize>()
        != 1
    {
        return Err("unresolved_subject");
    }
    let subject = markers
        .iter()
        .find_map(|marker| {
            content
                .as_str()
                .split_once(marker)
                .map(|(subject, _)| subject)
        })
        .ok_or("unresolved_subject")?;
    resolve_subject(Some(&NormalizedText::new(subject)), candidates)
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

/// Colon syntax alone does not establish a single-valued schema.
pub(crate) fn has_unstructured_colon_value(content: &str) -> bool {
    content.lines().any(|line| {
        let Some((key, value)) = line.split_once(':').or_else(|| line.split_once('=')) else {
            return false;
        };
        let value = value.trim();
        if key.trim().is_empty() || value.is_empty() || key.contains(" is ") {
            return false;
        }
        true
    })
}

// Priority-1 JSON-scalar leaf extraction is intentionally deferred: no plan
// or ADR mandates the JSON-scalar claim format yet, so this slot always
// returns `None` and parsing falls through to key-value / sentence patterns.
// Re-visit with a plan before treating JSON blobs as a claim source.
// See ADR-0004 (register claim schemas).
fn try_parse_json_scalars(_content: &str) -> Option<Vec<StructuralAssertion>> {
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

pub(super) fn detect_value_and_qualifiers(
    val: &str,
) -> (StructuralValue, BTreeMap<String, String>) {
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

/// Recognize only complete records in the existing built-in schema vocabulary.
/// An arbitrary prose heading never declares a scalar dimension.
pub(crate) fn explicit_schema_fields(content: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for line in content
        .split(['\n', ';'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some((key, value)) = line.split_once(':').or_else(|| line.split_once('=')) else {
            return BTreeMap::new();
        };
        let key = key.trim();
        let value = value.trim();
        if !matches!(
            key,
            "subject"
                | "dimension"
                | "value"
                | "measure"
                | "unit"
                | "predicate"
                | "object"
                | "action"
                | "target"
                | "deadline"
                | "status"
        ) || value.is_empty()
            || fields.insert(key.to_string(), value.to_string()).is_some()
        {
            return BTreeMap::new();
        }
    }
    if (fields.contains_key("dimension") || fields.contains_key("measure"))
        && fields.contains_key("value")
        || fields.contains_key("predicate") && fields.contains_key("object")
        || fields.contains_key("action") && fields.contains_key("target")
    {
        fields
    } else {
        BTreeMap::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
}
