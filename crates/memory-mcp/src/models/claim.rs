//! Claim value objects, typed values, relation and public reconciliation metadata.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::models::ids::{ClaimId, ClaimJobId, ClaimRelationId, EpisodeId, FactId};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimSchemaFamily {
    Attribute,
    Quantity,
    Relation,
    Commitment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClaimSchemaRef {
    pub family: ClaimSchemaFamily,
    pub version: std::num::NonZeroU16,
}

impl ClaimSchemaRef {
    pub fn new(family: ClaimSchemaFamily, version: u16) -> Self {
        Self {
            family,
            version: std::num::NonZeroU16::new(version).unwrap_or(std::num::NonZeroU16::MIN),
        }
    }
}

// ─── Claim Value ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

impl<'de> Deserialize<'de> for ClaimValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        let kind = raw
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("claim value kind must be a string"))?;
        let value = raw.get("value").cloned().unwrap_or(serde_json::Value::Null);

        fn surreal_scalar(value: serde_json::Value, key: &str) -> serde_json::Value {
            value
                .as_object()
                .and_then(|object| object.get(key).cloned())
                .unwrap_or(value)
        }

        let normalized = match kind {
            "boolean" => serde_json::json!({
                "kind": kind,
                "value": surreal_scalar(value, "Bool")
            }),
            "integer" => serde_json::json!({
                "kind": kind,
                "value": surreal_scalar(value, "Int")
            }),
            _ => serde_json::json!({"kind": kind, "value": value}),
        };

        let raw: RawClaimValue =
            serde_json::from_value(normalized).map_err(serde::de::Error::custom)?;
        Ok(match raw {
            RawClaimValue::Boolean(value) => Self::Boolean(value),
            RawClaimValue::Integer(value) => Self::Integer(value),
            RawClaimValue::Decimal(value) => Self::Decimal(value),
            RawClaimValue::Text(value) => Self::Text(value),
            RawClaimValue::DateTime(value) => Self::DateTime(value),
            RawClaimValue::Duration(value) => Self::Duration(value),
            RawClaimValue::Entity(value) => Self::Entity(value),
            RawClaimValue::Quantity { value, unit } => Self::Quantity { value, unit },
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum RawClaimValue {
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

/// Version of the identity contract used by a persisted claim or slot.
///
/// Missing values are legacy records. New projections always write `v2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClaimIdentityVersion {
    #[default]
    Legacy,
    V2,
}

impl ClaimIdentityVersion {
    #[must_use]
    pub fn is_v2(self) -> bool {
        matches!(self, Self::V2)
    }
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
    /// Legacy policy fingerprint. Kept for compatibility with legacy records and tests.
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

    /// V2 policy fingerprint. The active namespace is the storage boundary;
    /// only policy tags participate in the claim identity.
    pub fn compute_v2(policy_tags: &[String]) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"claim-policy:v2\0");
        let mut sorted_tags = policy_tags.to_vec();
        sorted_tags.sort();
        for tag in &sorted_tags {
            hasher.update(b"tag\0");
            hasher.update(tag.as_bytes());
            hasher.update([0]);
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

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
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

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ─── Claim Slot ───────────────────────────────────────────────────────────────

/// The exact slot a claim occupies.
///
/// V2 identity is schema + subject + comparison key + policy within the
/// process-bound Active Namespace. The namespace is the storage context, not a
/// second identity dimension. Qualifiers remain part of the claim proposition
/// and are evaluated during reconciliation, not candidate-slot identity. The
/// scope/project fields are retained only as legacy metadata. Two claims in
/// different slots are never compared.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClaimSlot {
    #[serde(default)]
    pub identity_version: ClaimIdentityVersion,
    pub namespace: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub project_identity: Option<String>,
    pub access_policy_fingerprint: PolicyFingerprint,
    pub schema_ref: ClaimSchemaRef,
    pub subject_key: String,
    pub comparison_key_hash: ComparisonKeyHash,
    pub qualifier_hash: QualifierHash,
}

impl ClaimSlot {
    /// Compute the v2 slot identity using an optionally canonicalized subject.
    #[must_use]
    pub fn v2_fingerprint(&self) -> String {
        self.v2_fingerprint_for_subject(&self.subject_key)
    }

    /// Compute the v2 slot identity for alias-aware reconciliation.
    #[must_use]
    pub fn v2_fingerprint_for_subject(&self, subject_key: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"claim-slot:v2\0");
        // Active Namespace is already enforced by the bound ClaimStore. Do not
        // include its label in the v2 slot identity: the same semantic slot
        // must retain the same formula if artifacts are compared externally.
        hasher.update((self.schema_ref.family as u8).to_be_bytes());
        hasher.update(self.schema_ref.version.get().to_be_bytes());
        hasher.update([0]);
        hasher.update(subject_key.as_bytes());
        hasher.update([0]);
        hasher.update(self.comparison_key_hash.0.as_bytes());
        hasher.update([0]);
        hasher.update(self.access_policy_fingerprint.0.as_bytes());
        format!("v2:{}", hex::encode(hasher.finalize()))
    }
}

// ─── Claim Draft ──────────────────────────────────────────────────────────────

/// Intermediate representation before building a persisted `Claim`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimDraft {
    pub schema_ref: ClaimSchemaRef,
    pub subject: ClaimSlot,
    pub comparison_key: ComparisonKey,
    pub qualifiers: BTreeMap<String, String>,
    pub value: ClaimValue,
    pub cardinality: ClaimCardinality,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub valid_from: Option<chrono::DateTime<chrono::Utc>>,
    pub valid_to: Option<chrono::DateTime<chrono::Utc>>,
    pub validity_source: ClaimValiditySource,
    pub source_lineage: Option<String>,
}

// ─── Claim Derivation ─────────────────────────────────────────────────────────

/// How a claim was derived from source data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimDerivation {
    pub source_fact_id: FactId,
    pub source_episode_id: EpisodeId,
    pub extractor_fingerprint: ExtractorFingerprint,
}

// ─── Validity Source ──────────────────────────────────────────────────────────

/// Where the validity window of a claim came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimValiditySource {
    /// Explicitly stated in the source.
    Explicit,
    /// Inherited from the episode timestamp.
    EpisodeTimestamp,
    /// Inferred from ordering of claims.
    Inferred,
}

// ─── Persisted Claim ──────────────────────────────────────────────────────────

/// A persisted, immutable-after-creation claim record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    #[serde(default)]
    pub identity_version: ClaimIdentityVersion,
    pub claim_id: ClaimId,
    pub namespace: String,
    pub source_fact_id: FactId,
    pub source_episode_id: EpisodeId,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub project_identity: Option<String>,
    pub policy_tags: Vec<String>,
    pub access_policy_fingerprint: PolicyFingerprint,
    pub schema_family: ClaimSchemaFamily,
    pub schema_version: u16,
    pub subject: ClaimSlot,
    pub subject_key: String,
    pub comparison_key: ComparisonKey,
    pub comparison_key_hash: ComparisonKeyHash,
    pub qualifiers: BTreeMap<String, String>,
    pub qualifier_hash: QualifierHash,
    pub slot_fingerprint: String,
    pub value: ClaimValue,
    pub cardinality: ClaimCardinality,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub valid_from: Option<chrono::DateTime<chrono::Utc>>,
    pub valid_to: Option<chrono::DateTime<chrono::Utc>>,
    pub validity_source: ClaimValiditySource,
    pub source_lineage: Option<String>,
    pub derivation: ClaimDerivation,
    pub extractor_fingerprint: ExtractorFingerprint,
    pub t_ingested: chrono::DateTime<chrono::Utc>,
    pub t_invalid_ingested: Option<chrono::DateTime<chrono::Utc>>,
}

// ─── Claim Relation ───────────────────────────────────────────────────────────

/// The outcome of a relation evaluation between two claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClaimRelationOutcome {
    /// The claim is a duplicate of another (same proposition, compatible validity).
    Duplicate,
    /// The right claim supersedes the left.
    Supersession,
    /// The right claim was explicitly stated as a correction of the left.
    Correction,
    /// The claims contradict each other.
    Contradiction,
    /// Claims cannot be compared due to insufficient temporal information.
    TemporalAmbiguity,
}

impl std::fmt::Display for ClaimRelationOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate => write!(f, "duplicate"),
            Self::Supersession => write!(f, "supersession"),
            Self::Correction => write!(f, "correction"),
            Self::Contradiction => write!(f, "contradiction"),
            Self::TemporalAmbiguity => write!(f, "temporal_ambiguity"),
        }
    }
}

/// Evidence supporting a relation evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRelationEvidence {
    pub reason_code: String,
    pub description: Option<String>,
}

/// A persisted relation between two claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimRelation {
    pub claim_relation_id: ClaimRelationId,
    pub left_claim_id: ClaimId,
    pub right_claim_id: ClaimId,
    pub pair_fingerprint: String,
    pub outcome: ClaimRelationOutcome,
    pub predecessor_claim_id: Option<ClaimId>,
    pub successor_claim_id: Option<ClaimId>,
    pub reason_code: String,
    pub evidence: ClaimRelationEvidence,
    pub evaluator_version: String,
    pub context_fingerprint: ReconciliationContextFingerprint,
    pub evaluated_at: chrono::DateTime<chrono::Utc>,
    pub supersedes_relation_id: Option<ClaimRelationId>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    pub policy_tags: Vec<String>,
    pub t_ingested: chrono::DateTime<chrono::Utc>,
    pub t_invalid_ingested: Option<chrono::DateTime<chrono::Utc>>,
    /// Migration 028: schema family for lookup/metrics.
    #[serde(default)]
    pub schema_family: Option<ClaimSchemaFamily>,
    /// Migration 028: schema version for lookup/metrics.
    #[serde(default)]
    pub schema_version: Option<u16>,
    /// Migration 028: left source fact for cross-fact relation queries.
    #[serde(default)]
    pub left_fact_id: Option<FactId>,
    /// Migration 028: right source fact for cross-fact relation queries.
    #[serde(default)]
    pub right_fact_id: Option<FactId>,
}

// ─── Claim Job ────────────────────────────────────────────────────────────────

/// The kind of claim job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimJobKind {
    /// Extract claims from a fact.
    Extract,
    /// Evaluate relations between claims.
    Reconcile,
    /// Discover facts lacking the current extractor fingerprint.
    Backfill,
}

/// The state of a claim job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimJobState {
    Pending,
    Leased,
    Running,
    Completed,
    Failed,
}

/// A persisted claim processing job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimJob {
    pub job_id: ClaimJobId,
    pub kind: ClaimJobKind,
    pub namespace: String,
    pub source_fact_id: Option<FactId>,
    pub claim_id: Option<ClaimId>,
    pub extractor_fingerprint: ExtractorFingerprint,
    pub evaluator_fingerprint: Option<String>,
    pub status: ClaimJobState,
    pub cursor: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub processed: u64,
    pub succeeded: u64,
    pub skipped: u64,
    pub failed: u64,
    pub retry_count: u32,
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ─── Claim Build Input & Constructor ──────────────────────────────────────────

/// Borrowed inputs for constructing a persisted `Claim`.
pub struct ClaimBuildInput<'a> {
    pub namespace: &'a str,
    pub source_fact_id: &'a FactId,
    pub source_episode_id: &'a EpisodeId,
    pub policy_tags: &'a [String],
    pub draft: ClaimDraft,
    pub extractor_fingerprint: &'a ExtractorFingerprint,
    pub t_ingested: chrono::DateTime<chrono::Utc>,
}

/// Deterministic claim ID from content hashes.
pub fn claim_id(
    schema_ref: &ClaimSchemaRef,
    extractor_fingerprint: &ExtractorFingerprint,
    source_fact_id: &FactId,
    canonical_payload: &CanonicalPayloadHash,
) -> ClaimId {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(schema_ref.version.get().to_be_bytes());
    hasher.update((schema_ref.family as u8).to_be_bytes());
    hasher.update(extractor_fingerprint.as_str().as_bytes());
    hasher.update(source_fact_id.as_ref().as_bytes());
    hasher.update(canonical_payload.as_str().as_bytes());
    let hash = hex::encode(hasher.finalize());
    ClaimId::from_raw(format!("claim:{hash}"))
}

/// Deterministic relation ID for an unordered pair of claim IDs.
pub fn relation_id(left: &ClaimId, right: &ClaimId) -> ClaimRelationId {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let (a, b) = if left.as_ref() <= right.as_ref() {
        (left.as_ref(), right.as_ref())
    } else {
        (right.as_ref(), left.as_ref())
    };
    hasher.update(a.as_bytes());
    hasher.update(b.as_bytes());
    let hash = hex::encode(hasher.finalize());
    ClaimRelationId::from_raw(format!("claim_relation:{hash}"))
}

/// Build a persisted `Claim` from borrowed inputs.
pub fn build_claim(input: ClaimBuildInput<'_>) -> Result<Claim, MemoryError> {
    let draft = &input.draft;

    let canonical_payload = CanonicalPayloadHash::compute(&draft.value, &draft.qualifiers);
    let claim_id = claim_id(
        &draft.schema_ref,
        input.extractor_fingerprint,
        input.source_fact_id,
        &canonical_payload,
    );

    let comparison_key_hash = ComparisonKeyHash::compute(&draft.comparison_key);
    let qualifier_hash = QualifierHash::compute(&draft.qualifiers);
    let access_policy_fingerprint = PolicyFingerprint::compute_v2(input.policy_tags);
    let mut subject = draft.subject.clone();
    subject.identity_version = ClaimIdentityVersion::V2;
    subject.namespace = input.namespace.to_string();
    subject.access_policy_fingerprint = access_policy_fingerprint.clone();
    subject.comparison_key_hash = comparison_key_hash.clone();
    subject.qualifier_hash = qualifier_hash.clone();
    let slot_fingerprint = subject.v2_fingerprint();

    Ok(Claim {
        identity_version: ClaimIdentityVersion::V2,
        claim_id,
        namespace: input.namespace.to_string(),
        source_fact_id: input.source_fact_id.clone(),
        source_episode_id: input.source_episode_id.clone(),
        scope: None,
        project: None,
        project_identity: None,
        policy_tags: input.policy_tags.to_vec(),
        access_policy_fingerprint,
        schema_family: draft.schema_ref.family,
        schema_version: draft.schema_ref.version.get(),
        subject,
        subject_key: draft.subject.subject_key.clone(),
        comparison_key: draft.comparison_key.clone(),
        comparison_key_hash,
        qualifiers: draft.qualifiers.clone(),
        qualifier_hash,
        slot_fingerprint,
        value: draft.value.clone(),
        cardinality: draft.cardinality,
        observed_at: draft.observed_at,
        // When the source did not carry an explicit validity start, the claim
        // is valid from the moment it was observed (the fact's `t_valid`).
        // This keeps two contradictory values observed at the same instant
        // from landing in the Unknown branch of `validity_relation`, which
        // would silently drop the contradiction.
        valid_from: draft.valid_from.or(Some(draft.observed_at)),
        valid_to: draft.valid_to,
        validity_source: draft.validity_source,
        source_lineage: draft.source_lineage.clone(),
        derivation: ClaimDerivation {
            source_fact_id: input.source_fact_id.clone(),
            source_episode_id: input.source_episode_id.clone(),
            extractor_fingerprint: input.extractor_fingerprint.clone(),
        },
        extractor_fingerprint: input.extractor_fingerprint.clone(),
        t_ingested: input.t_ingested,
        t_invalid_ingested: None,
    })
}

// ─── Test Module ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_value_accepts_surreal_boolean_wrapper() {
        let value: ClaimValue = serde_json::from_value(serde_json::json!({
            "kind": "boolean",
            "value": {"Bool": true}
        }))
        .unwrap();
        assert_eq!(value, ClaimValue::Boolean(true));
    }
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
    fn new_claim_identity_is_v2_and_ignores_legacy_partition_fields() {
        let tags = vec!["private".to_string(), "source:chat".to_string()];
        let personal = build_identity_test_claim("personal", Some("project-a"), &tags);
        let team = build_identity_test_claim("team", Some("project-b"), &tags);

        assert_eq!(
            personal.access_policy_fingerprint,
            team.access_policy_fingerprint
        );
        assert_eq!(personal.slot_fingerprint, team.slot_fingerprint);
        assert_eq!(
            serde_json::to_value(&personal).unwrap()["identity_version"],
            "v2"
        );
        assert_eq!(
            serde_json::to_value(&personal.subject).unwrap()["identity_version"],
            "v2"
        );
    }

    #[test]
    fn v2_slot_identity_ignores_storage_label_and_qualifiers() {
        let schema = ClaimSchemaRef::new(ClaimSchemaFamily::Attribute, 1);
        let mut components = BTreeMap::new();
        components.insert("dim".to_string(), "status".to_string());
        let comparison_key = ComparisonKey::new(schema, components).unwrap();
        let mut first_qualifiers = BTreeMap::new();
        first_qualifiers.insert("source".to_string(), "email".to_string());
        let mut second_qualifiers = BTreeMap::new();
        second_qualifiers.insert("source".to_string(), "chat".to_string());
        let base = ClaimSlot {
            identity_version: ClaimIdentityVersion::V2,
            namespace: "main".to_string(),
            scope: None,
            project_identity: None,
            access_policy_fingerprint: PolicyFingerprint::compute_v2(&[]),
            schema_ref: schema,
            subject_key: "entity:abc".to_string(),
            comparison_key_hash: ComparisonKeyHash::compute(&comparison_key),
            qualifier_hash: QualifierHash::compute(&first_qualifiers),
        };
        let mut other = base.clone();
        other.namespace = "work".to_string();
        other.qualifier_hash = QualifierHash::compute(&second_qualifiers);

        assert_eq!(base.v2_fingerprint(), other.v2_fingerprint());
    }

    #[test]
    fn v2_slot_identity_changes_with_policy_tags() {
        let tags_a = vec!["private".to_string()];
        let tags_b = vec!["public".to_string()];
        let claim_a = build_identity_test_claim("personal", Some("project-a"), &tags_a);
        let claim_b = build_identity_test_claim("team", Some("project-b"), &tags_b);

        assert_ne!(
            claim_a.access_policy_fingerprint,
            claim_b.access_policy_fingerprint
        );
        assert_ne!(claim_a.slot_fingerprint, claim_b.slot_fingerprint);
    }

    #[test]
    fn legacy_claim_fields_are_optional_and_identity_defaults_to_legacy() {
        let claim = build_identity_test_claim("personal", None, &[]);
        let mut raw = serde_json::to_value(&claim).unwrap();
        let object = raw.as_object_mut().unwrap();
        object.remove("identity_version");
        object.remove("scope");
        object.remove("project");
        let subject = object.get_mut("subject").unwrap().as_object_mut().unwrap();
        subject.remove("identity_version");

        let decoded: Claim = serde_json::from_value(raw).unwrap();
        let encoded = serde_json::to_value(decoded).unwrap();
        assert_eq!(encoded["identity_version"], "legacy");
        assert_eq!(encoded["scope"], serde_json::Value::Null);
        assert_eq!(encoded["project"], serde_json::Value::Null);
        assert_eq!(encoded["subject"]["identity_version"], "legacy");
    }

    fn build_identity_test_claim(
        scope: &str,
        project: Option<&str>,
        policy_tags: &[String],
    ) -> Claim {
        let schema = ClaimSchemaRef::new(ClaimSchemaFamily::Attribute, 1);
        let mut components = BTreeMap::new();
        components.insert("dim".to_string(), "status".to_string());
        let key = ComparisonKey::new(schema, components).unwrap();
        let qualifiers = BTreeMap::new();
        let subject = ClaimSlot {
            identity_version: ClaimIdentityVersion::Legacy,
            namespace: "ns".to_string(),
            scope: Some(scope.to_string()),
            project_identity: project.map(str::to_string),
            access_policy_fingerprint: PolicyFingerprint::compute(scope, project, policy_tags),
            schema_ref: schema,
            subject_key: "entity:abc".to_string(),
            comparison_key_hash: ComparisonKeyHash::compute(&key),
            qualifier_hash: QualifierHash::compute(&qualifiers),
        };
        let fact_id = FactId::from("fact:identity-test");
        let episode_id = EpisodeId::from("episode:identity-test");
        build_claim(ClaimBuildInput {
            namespace: "ns",
            source_fact_id: &fact_id,
            source_episode_id: &episode_id,
            policy_tags,
            draft: ClaimDraft {
                schema_ref: schema,
                subject,
                comparison_key: key,
                qualifiers,
                value: ClaimValue::Boolean(true),
                cardinality: ClaimCardinality::SingleValued,
                observed_at: chrono::Utc::now(),
                valid_from: None,
                valid_to: None,
                validity_source: ClaimValiditySource::Explicit,
                source_lineage: None,
            },
            extractor_fingerprint: &ExtractorFingerprint::compute(1, "test"),
            t_ingested: chrono::Utc::now(),
        })
        .unwrap()
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

    #[test]
    fn claim_id_rejects_wrong_prefix() {
        assert!(ClaimId::new("wrong:abc").is_err());
    }

    #[test]
    fn claim_id_rejects_empty_body() {
        assert!(ClaimId::new("claim:").is_err());
    }

    #[test]
    fn claim_id_accepts_valid() {
        let id = ClaimId::new("claim:abc123").unwrap();
        assert_eq!(id.body(), "abc123");
        assert_eq!(id.to_string(), "claim:abc123");
    }

    #[test]
    fn claim_relation_id_rejects_wrong_prefix() {
        assert!(ClaimRelationId::new("wrong:abc").is_err());
    }

    #[test]
    fn claim_relation_id_accepts_valid() {
        let id = ClaimRelationId::new("claim_relation:abc123").unwrap();
        assert_eq!(id.body(), "abc123");
    }

    #[test]
    fn claim_job_id_rejects_wrong_prefix() {
        assert!(ClaimJobId::new("wrong:abc").is_err());
    }

    #[test]
    fn claim_job_id_accepts_valid() {
        let id = ClaimJobId::new("claim_job:abc123").unwrap();
        assert_eq!(id.body(), "abc123");
    }

    #[test]
    fn relation_id_is_symmetric_for_unordered_pair() {
        let a = ClaimId::new("claim:aaa").unwrap();
        let b = ClaimId::new("claim:bbb").unwrap();
        let fwd = relation_id(&a, &b);
        let rev = relation_id(&b, &a);
        assert_eq!(fwd, rev);
    }

    #[test]
    fn claim_id_changes_when_schema_version_changes() {
        use sha2::{Digest, Sha256};
        let mut h1 = Sha256::new();
        h1.update(1u16.to_be_bytes());
        h1.update(0u8.to_be_bytes());
        h1.update("ext");
        h1.update("fact");
        h1.update("payload");
        let id1 = hex::encode(h1.finalize());

        let mut h2 = Sha256::new();
        h2.update(2u16.to_be_bytes());
        h2.update(0u8.to_be_bytes());
        h2.update("ext");
        h2.update("fact");
        h2.update("payload");
        let id2 = hex::encode(h2.finalize());

        assert_ne!(id1, id2);
    }

    #[test]
    fn canonical_payload_hash_changes_with_value() {
        let v1 = ClaimValue::Boolean(true);
        let v2 = ClaimValue::Boolean(false);
        let q = BTreeMap::new();
        assert_ne!(
            CanonicalPayloadHash::compute(&v1, &q),
            CanonicalPayloadHash::compute(&v2, &q)
        );
    }

    #[test]
    fn unresolved_subject_cannot_construct_comparable_slot() {
        let schema = ClaimSchemaRef {
            family: ClaimSchemaFamily::Attribute,
            version: std::num::NonZeroU16::new(1).unwrap(),
        };
        let mut components = BTreeMap::new();
        components.insert("dim".to_string(), "height".to_string());
        let key = ComparisonKey::new(schema, components).unwrap();
        let hash = ComparisonKeyHash::compute(&key);
        // An empty subject_key signals unresolved subject
        let slot = ClaimSlot {
            identity_version: ClaimIdentityVersion::Legacy,
            namespace: "ns".to_string(),
            scope: Some("personal".to_string()),
            project_identity: Some("__none__".to_string()),
            access_policy_fingerprint: PolicyFingerprint::compute("personal", None, &[]),
            schema_ref: schema,
            subject_key: String::new(),
            comparison_key_hash: hash,
            qualifier_hash: QualifierHash::compute(&BTreeMap::new()),
        };
        assert!(slot.subject_key.is_empty());
    }

    #[test]
    fn build_claim_produces_deterministic_id() {
        let schema = ClaimSchemaRef {
            family: ClaimSchemaFamily::Attribute,
            version: std::num::NonZeroU16::new(1).unwrap(),
        };
        let mut components = BTreeMap::new();
        components.insert("dim".to_string(), "height".to_string());
        let key = ComparisonKey::new(schema, components).unwrap();
        let hash = ComparisonKeyHash::compute(&key);

        let subject = ClaimSlot {
            identity_version: ClaimIdentityVersion::Legacy,
            namespace: "ns".to_string(),
            scope: Some("personal".to_string()),
            project_identity: Some("__none__".to_string()),
            access_policy_fingerprint: PolicyFingerprint::compute("personal", None, &[]),
            schema_ref: schema,
            subject_key: "entity:abc".to_string(),
            comparison_key_hash: hash,
            qualifier_hash: QualifierHash::compute(&BTreeMap::new()),
        };

        let draft = ClaimDraft {
            schema_ref: schema,
            subject,
            comparison_key: key,
            qualifiers: BTreeMap::new(),
            value: ClaimValue::Boolean(true),
            cardinality: ClaimCardinality::SingleValued,
            observed_at: chrono::Utc::now(),
            valid_from: None,
            valid_to: None,
            validity_source: ClaimValiditySource::Explicit,
            source_lineage: None,
        };

        let fact_id = FactId::from("fact:test1");
        let episode_id = EpisodeId::from("ep:test1");
        let ext_fp = ExtractorFingerprint::compute(1, "test");

        let input = ClaimBuildInput {
            namespace: "ns",
            source_fact_id: &fact_id,
            source_episode_id: &episode_id,
            policy_tags: &[],
            draft,
            extractor_fingerprint: &ext_fp,
            t_ingested: chrono::Utc::now(),
        };

        let c1 = build_claim(input).unwrap();
        // Rebuild with same inputs — same ID
        let fact_id2 = FactId::from("fact:test1");
        let episode_id2 = EpisodeId::from("ep:test1");
        let ext_fp2 = ExtractorFingerprint::compute(1, "test");
        let input2 = ClaimBuildInput {
            namespace: "ns",
            source_fact_id: &fact_id2,
            source_episode_id: &episode_id2,
            policy_tags: &[],
            draft: ClaimDraft {
                schema_ref: schema,
                subject: c1.subject.clone(),
                comparison_key: c1.comparison_key.clone(),
                qualifiers: c1.qualifiers.clone(),
                value: c1.value.clone(),
                cardinality: c1.cardinality,
                observed_at: c1.observed_at,
                valid_from: c1.valid_from,
                valid_to: c1.valid_to,
                validity_source: c1.validity_source,
                source_lineage: c1.source_lineage.clone(),
            },
            extractor_fingerprint: &ext_fp2,
            t_ingested: c1.t_ingested,
        };
        let c2 = build_claim(input2).unwrap();
        assert_eq!(c1.claim_id, c2.claim_id);
    }
}
