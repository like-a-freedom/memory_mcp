//! Claim value objects, typed values, relation and public reconciliation metadata.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::service::MemoryError;

// ─── Canonical Decimal ────────────────────────────────────────────────────────

/// A normalized decimal stored as `coefficient * 10^(-scale)`.
/// Always serialized as its normalized decimal string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalDecimal {
    coefficient: i128,
    scale: u32,
}

impl CanonicalDecimal {
    /// Parse a decimal string into a normalized `CanonicalDecimal`.
    ///
    /// Accepts signs, leading zeros, trailing zeros, and optional decimal point.
    /// Normalizes negative zero and trailing zeros.
    pub fn parse(input: &str) -> Result<Self, MemoryError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(MemoryError::Validation("empty decimal string".to_string()));
        }

        let negative = input.starts_with('-');
        let digits_part = if negative || input.starts_with('+') {
            &input[1..]
        } else {
            input
        };

        if digits_part.is_empty() {
            return Err(MemoryError::Validation(
                "no digits in decimal string".to_string(),
            ));
        }

        let (int_part, frac_part) = match digits_part.find('.') {
            Some(pos) => (&digits_part[..pos], &digits_part[pos + 1..]),
            None => (digits_part, ""),
        };

        // Strip leading zeros from integer part
        let int_stripped = int_part.trim_start_matches('0');
        let frac_stripped = frac_part.trim_end_matches('0');

        if int_stripped.is_empty() && frac_stripped.is_empty() {
            // Value is zero
            return Ok(Self {
                coefficient: 0,
                scale: 0,
            });
        }

        let all_digits = format!("{int_stripped}{frac_stripped}");
        let scale = frac_stripped.len() as u32;

        let coefficient: i128 = all_digits
            .parse()
            .map_err(|_| MemoryError::Validation(format!("decimal overflow for input: {input}")))?;

        let coefficient = if negative && coefficient != 0 {
            -coefficient
        } else {
            coefficient
        };

        Ok(Self { coefficient, scale })
    }

    #[must_use]
    pub fn coefficient(&self) -> i128 {
        self.coefficient
    }

    #[must_use]
    pub fn scale(&self) -> u32 {
        self.scale
    }
}

impl fmt::Display for CanonicalDecimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.scale == 0 {
            write!(f, "{}", self.coefficient)
        } else {
            let abs = self.coefficient.unsigned_abs();
            let int_part = abs / 10u128.pow(self.scale);
            let frac_part = abs % 10u128.pow(self.scale);
            let frac_str = format!("{:0width$}", frac_part, width = self.scale as usize);
            let sign = if self.coefficient < 0 { "-" } else { "" };
            write!(f, "{sign}{int_part}.{frac_str}")
        }
    }
}

impl Serialize for CanonicalDecimal {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CanonicalDecimal {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

// ─── Normalized Text ──────────────────────────────────────────────────────────

/// NFC-normalized, case-folded, whitespace-collapsed text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NormalizedText(String);

impl NormalizedText {
    pub fn new(input: &str) -> Self {
        use unicode_normalization::UnicodeNormalization;
        let normalized: String = input.nfc().collect();
        let folded = normalized.to_lowercase();
        let collapsed: String = folded.split_whitespace().collect::<Vec<_>>().join(" ");
        Self(collapsed)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NormalizedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for NormalizedText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NormalizedText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self(s))
    }
}

// ─── Canonical Unit ───────────────────────────────────────────────────────────

/// A known unit within a dimensional family.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CanonicalUnit {
    pub family: String,
    pub symbol: String,
}

// ─── Canonical Duration ───────────────────────────────────────────────────────

/// A duration stored as unsigned seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CanonicalDuration {
    seconds: u64,
}

impl CanonicalDuration {
    pub fn from_seconds(s: u64) -> Self {
        Self { seconds: s }
    }

    #[must_use]
    pub fn seconds(&self) -> u64 {
        self.seconds
    }
}

// ─── Schema Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimSchemaFamily {
    Attribute,
    Quantity,
    Relation,
    Commitment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClaimSchemaRef {
    pub family: ClaimSchemaFamily,
    pub version: std::num::NonZeroU16,
}

// ─── Claim Value ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ClaimValue {
    Boolean(bool),
    Integer(i64),
    Decimal(CanonicalDecimal),
    Text(NormalizedText),
    DateTime(chrono::DateTime<chrono::Utc>),
    Duration(CanonicalDuration),
    Entity(String),
    Quantity {
        value: CanonicalDecimal,
        unit: CanonicalUnit,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimCardinality {
    SetValued,
    SingleValued,
}

// ─── Comparison Key ───────────────────────────────────────────────────────────

/// A structural comparison key: schema reference + ordered components.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ComparisonKey {
    pub schema_ref: ClaimSchemaRef,
    pub components: BTreeMap<String, String>,
}

impl ComparisonKey {
    pub fn new(
        schema_ref: ClaimSchemaRef,
        components: BTreeMap<String, String>,
    ) -> Result<Self, MemoryError> {
        for (k, v) in &components {
            if k.is_empty() {
                return Err(MemoryError::Validation(
                    "empty component name in comparison key".to_string(),
                ));
            }
            if v.is_empty() {
                return Err(MemoryError::Validation(format!(
                    "empty component value for key '{k}'"
                )));
            }
        }
        if components.is_empty() {
            return Err(MemoryError::Validation(
                "comparison key must have at least one component".to_string(),
            ));
        }
        Ok(Self {
            schema_ref,
            components,
        })
    }
}

// ─── Hash Newtypes ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ComparisonKeyHash(String);

impl ComparisonKeyHash {
    pub fn compute(key: &ComparisonKey) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(key.schema_ref.version.get().to_be_bytes());
        hasher.update((key.schema_ref.family as u8).to_be_bytes());
        for (k, v) in &key.components {
            hasher.update(k.as_bytes());
            hasher.update(v.as_bytes());
        }
        Self(hex::encode(hasher.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct QualifierHash(String);

impl QualifierHash {
    pub fn compute(qualifiers: &BTreeMap<String, String>) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for (k, v) in qualifiers {
            hasher.update(k.as_bytes());
            hasher.update(v.as_bytes());
        }
        Self(hex::encode(hasher.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PolicyFingerprint(String);

impl PolicyFingerprint {
    pub fn compute(scope: &str, project: Option<&str>, policy_tags: &[String]) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(scope.as_bytes());
        if let Some(p) = project {
            hasher.update(b"\x00project\x00");
            hasher.update(p.as_bytes());
        }
        let mut sorted_tags = policy_tags.to_vec();
        sorted_tags.sort();
        for tag in &sorted_tags {
            hasher.update(b"\x00tag\x00");
            hasher.update(tag.as_bytes());
        }
        Self(hex::encode(hasher.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExtractorFingerprint(String);

impl ExtractorFingerprint {
    pub fn compute(schema_version: u16, extractor_name: &str) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(schema_version.to_be_bytes());
        hasher.update(extractor_name.as_bytes());
        Self(hex::encode(hasher.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReconciliationContextFingerprint(String);

impl ReconciliationContextFingerprint {
    pub fn compute(
        evaluator_version: &str,
        schema_family: &str,
        alias_hash: &str,
        policy_hash: &str,
    ) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(evaluator_version.as_bytes());
        hasher.update(schema_family.as_bytes());
        hasher.update(alias_hash.as_bytes());
        hasher.update(policy_hash.as_bytes());
        Self(hex::encode(hasher.finalize()))
    }
}

// ─── Canonical Payload Hash ───────────────────────────────────────────────────

/// Deterministic hash of a claim's canonical payload (value + qualifiers).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CanonicalPayloadHash(String);

impl CanonicalPayloadHash {
    pub fn compute(value: &ClaimValue, qualifiers: &BTreeMap<String, String>) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        // Serialize value deterministically via BTreeMap-ordered JSON
        let value_str = serde_json::to_string(value).unwrap_or_default();
        hasher.update(value_str.as_bytes());
        for (k, v) in qualifiers {
            hasher.update(k.as_bytes());
            hasher.update(v.as_bytes());
        }
        Self(hex::encode(hasher.finalize()))
    }
}

// ─── Claim Slot ───────────────────────────────────────────────────────────────

/// The exact slot a claim occupies: namespace + scope + project + policy + schema + subject + comparison key.
/// Two claims in different slots are never compared.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClaimSlot {
    pub namespace: String,
    pub scope: String,
    pub project_identity: String,
    pub access_policy_fingerprint: PolicyFingerprint,
    pub schema_ref: ClaimSchemaRef,
    pub subject_key: String,
    pub comparison_key_hash: ComparisonKeyHash,
    pub qualifier_hash: QualifierHash,
}

// ─── Test Module ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn canonical_decimal_parse_normalizes() {
        let d = CanonicalDecimal::parse("00120.5000").unwrap();
        assert_eq!(d.coefficient(), 1205);
        assert_eq!(d.scale(), 1);
    }

    #[test]
    fn canonical_decimal_parse_integer() {
        let d = CanonicalDecimal::parse("42").unwrap();
        assert_eq!(d.coefficient(), 42);
        assert_eq!(d.scale(), 0);
    }

    #[test]
    fn canonical_decimal_parse_negative() {
        let d = CanonicalDecimal::parse("-3.14").unwrap();
        assert_eq!(d.coefficient(), -314);
        assert_eq!(d.scale(), 2);
    }

    #[test]
    fn canonical_decimal_parse_negative_zero() {
        let d = CanonicalDecimal::parse("-0.0").unwrap();
        assert_eq!(d.coefficient(), 0);
        assert_eq!(d.scale(), 0);
    }

    #[test]
    fn canonical_decimal_overflow_returns_validation_error() {
        let big = "99999999999999999999999999999999999999999999999999999999999999999";
        let result = CanonicalDecimal::parse(big);
        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    #[test]
    fn canonical_decimal_empty_returns_validation_error() {
        assert!(matches!(
            CanonicalDecimal::parse(""),
            Err(MemoryError::Validation(_))
        ));
    }

    #[test]
    fn canonical_decimal_display_roundtrip() {
        let d = CanonicalDecimal::parse("00120.5000").unwrap();
        assert_eq!(d.to_string(), "120.5");
    }

    #[test]
    fn normalized_text_nfc_idempotent() {
        let input = "caf\u{00e9}"; // already NFC
        let t1 = NormalizedText::new(input);
        let t2 = NormalizedText::new(t1.as_str());
        assert_eq!(t1, t2);
    }

    #[test]
    fn normalized_text_case_fold() {
        let t = NormalizedText::new("Hello WORLD");
        assert_eq!(t.as_str(), "hello world");
    }

    #[test]
    fn normalized_text_whitespace_collapse() {
        let t = NormalizedText::new("  hello   world  ");
        assert_eq!(t.as_str(), "hello world");
    }

    #[test]
    fn qualifier_hash_order_invariant() {
        let mut forward = BTreeMap::new();
        forward.insert("a".to_string(), "1".to_string());
        forward.insert("b".to_string(), "2".to_string());
        forward.insert("c".to_string(), "3".to_string());

        let reverse: BTreeMap<String, String> = forward
            .iter()
            .rev()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        assert_eq!(
            QualifierHash::compute(&forward),
            QualifierHash::compute(&reverse)
        );
    }

    #[test]
    fn policy_fingerprint_order_invariant() {
        let tags_a = vec!["z".to_string(), "a".to_string(), "m".to_string()];
        let tags_b = vec!["a".to_string(), "m".to_string(), "z".to_string()];
        assert_eq!(
            PolicyFingerprint::compute("org", Some("proj"), &tags_a),
            PolicyFingerprint::compute("org", Some("proj"), &tags_b)
        );
    }

    #[test]
    fn project_none_differs_from_some() {
        let fp_none = PolicyFingerprint::compute("org", None, &[]);
        let fp_some = PolicyFingerprint::compute("org", Some("proj"), &[]);
        assert_ne!(fp_none, fp_some);
    }

    #[test]
    fn comparison_key_rejects_empty_name() {
        let schema = ClaimSchemaRef {
            family: ClaimSchemaFamily::Attribute,
            version: std::num::NonZeroU16::new(1).unwrap(),
        };
        let mut components = BTreeMap::new();
        components.insert("".to_string(), "value".to_string());
        assert!(ComparisonKey::new(schema, components).is_err());
    }

    #[test]
    fn comparison_key_rejects_empty_value() {
        let schema = ClaimSchemaRef {
            family: ClaimSchemaFamily::Attribute,
            version: std::num::NonZeroU16::new(1).unwrap(),
        };
        let mut components = BTreeMap::new();
        components.insert("key".to_string(), "".to_string());
        assert!(ComparisonKey::new(schema, components).is_err());
    }

    #[test]
    fn comparison_key_rejects_empty_components() {
        let schema = ClaimSchemaRef {
            family: ClaimSchemaFamily::Attribute,
            version: std::num::NonZeroU16::new(1).unwrap(),
        };
        assert!(ComparisonKey::new(schema, BTreeMap::new()).is_err());
    }

    #[test]
    fn canonical_duration_from_seconds() {
        let d = CanonicalDuration::from_seconds(3600);
        assert_eq!(d.seconds(), 3600);
    }

    proptest::proptest! {
        #[test]
        fn qualifier_hash_is_order_invariant(
            entries in prop::collection::btree_map(
                "[a-z]{1,12}",
                "[a-z0-9 ]{0,24}",
                0..12
            )
        ) {
            let forward = entries.clone();
            let reverse: BTreeMap<String, String> = entries.into_iter().rev().collect();
            prop_assert_eq!(QualifierHash::compute(&forward), QualifierHash::compute(&reverse));
        }
    }
}
