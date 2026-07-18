//! Pure reconciliation decision engine.
//!
//! No database, clock, logger, network, or service dependency.
//! All inputs are borrowed; all outputs are value types.

use std::collections::BTreeMap;

use crate::models::ClaimId;
use crate::models::claim::{
    Claim, ClaimCardinality, ClaimRelationEvidence, ClaimRelationOutcome,
    ReconciliationContextFingerprint,
};
use crate::service::claims::schema::ClaimPolicy;

// ─── Types ────────────────────────────────────────────────────────────────────

/// Confirmed alias set for fuzzy alias matching.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub(crate) struct ConfirmedAliasSet {
    aliases: BTreeMap<String, String>,
}

impl ConfirmedAliasSet {
    #[allow(dead_code)]
    pub fn new(aliases: BTreeMap<String, String>) -> Self {
        Self { aliases }
    }

    /// Resolve an alias to its canonical form, if confirmed.
    #[allow(dead_code)]
    pub fn resolve(&self, key: &str) -> Option<&str> {
        self.aliases.get(key).map(|s| s.as_str())
    }
}

/// Evaluator version string.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct EvaluatorVersion(pub String);

/// Bounded reason codes for reconciliation decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub(crate) enum ReconciliationReasonCode {
    NotSameSlot,
    NotComparable,
    Duplicate,
    Correction,
    Contradiction,
    Supersession,
    TemporalAmbiguity,
    SetValuedCoexistence,
    DisjointValidity,
}

impl std::fmt::Display for ReconciliationReasonCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSameSlot => write!(f, "not_same_slot"),
            Self::NotComparable => write!(f, "not_comparable"),
            Self::Duplicate => write!(f, "duplicate"),
            Self::Correction => write!(f, "correction"),
            Self::Contradiction => write!(f, "contradiction"),
            Self::Supersession => write!(f, "supersession"),
            Self::TemporalAmbiguity => write!(f, "temporal_ambiguity"),
            Self::SetValuedCoexistence => write!(f, "set_valued_coexistence"),
            Self::DisjointValidity => write!(f, "disjoint_validity"),
        }
    }
}

/// A persisted relation draft before ID assignment.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct PersistedRelationDraft {
    pub left_claim_id: ClaimId,
    pub right_claim_id: ClaimId,
    pub outcome: ClaimRelationOutcome,
    pub predecessor_claim_id: Option<ClaimId>,
    pub successor_claim_id: Option<ClaimId>,
    pub reason_code: ReconciliationReasonCode,
    pub evidence: ClaimRelationEvidence,
    pub evaluator_version: String,
    pub context_fingerprint: ReconciliationContextFingerprint,
    pub evaluated_at: chrono::DateTime<chrono::Utc>,
}

/// The decision returned by the pure reconciliation engine.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum ReconciliationDecision {
    Persist(Box<PersistedRelationDraft>),
    Skip(ReconciliationReasonCode),
    Coexist(ReconciliationReasonCode),
}

/// Borrowed inputs for reconciliation.
#[allow(dead_code)]
pub(crate) struct ReconciliationInput<'a> {
    pub left: &'a Claim,
    pub right: &'a Claim,
    pub policy: &'a ClaimPolicy,
    pub confirmed_aliases: &'a ConfirmedAliasSet,
    pub evaluator_version: &'a EvaluatorVersion,
    pub context_fingerprint: &'a ReconciliationContextFingerprint,
    pub evaluated_at: chrono::DateTime<chrono::Utc>,
}

// ─── Pure Gate Functions ──────────────────────────────────────────────────────

#[allow(dead_code)]
fn same_exact_slot(left: &Claim, right: &Claim) -> bool {
    left.scope == right.scope
        && left.project_identity == right.project_identity
        && left.access_policy_fingerprint == right.access_policy_fingerprint
        && left.schema_family == right.schema_family
        && left.schema_version == right.schema_version
        && left.subject_key == right.subject_key
        && left.comparison_key_hash == right.comparison_key_hash
}

#[allow(dead_code)]
fn values_comparable(left: &Claim, right: &Claim) -> bool {
    use crate::models::claim::ClaimValue;
    match (&left.value, &right.value) {
        (ClaimValue::Boolean(_), ClaimValue::Boolean(_)) => true,
        (ClaimValue::Integer(_), ClaimValue::Integer(_)) => true,
        (ClaimValue::Decimal(_), ClaimValue::Decimal(_)) => true,
        (ClaimValue::Text(_), ClaimValue::Text(_)) => true,
        (ClaimValue::DateTime(_), ClaimValue::DateTime(_)) => true,
        (ClaimValue::Duration(_), ClaimValue::Duration(_)) => true,
        (ClaimValue::Quantity { unit: u1, .. }, ClaimValue::Quantity { unit: u2, .. }) => {
            u1.family == u2.family
        }
        _ => false,
    }
}

#[allow(dead_code)]
fn same_proposition(left: &Claim, right: &Claim) -> bool {
    left.value == right.value && left.qualifier_hash == right.qualifier_hash
}

/// Relationship between validity intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum ValidityRelation {
    /// Both have the same valid_from and valid_to.
    Identical,
    /// Left is strictly contained within right.
    LeftContained,
    /// Right is strictly contained within left.
    RightContained,
    /// Intervals overlap but neither contains the other.
    Overlapping,
    /// Intervals are disjoint.
    Disjoint,
    /// At least one has no explicit bounds.
    Unknown,
}

#[allow(dead_code)]
fn validity_relation(left: &Claim, right: &Claim) -> ValidityRelation {
    match (
        left.valid_from,
        left.valid_to,
        right.valid_from,
        right.valid_to,
    ) {
        (Some(lf), Some(lt), Some(rf), Some(rt)) => {
            if lf == rf && lt == rt {
                ValidityRelation::Identical
            } else if lf >= rf && lt <= rt {
                ValidityRelation::LeftContained
            } else if rf >= lf && rt <= lt {
                ValidityRelation::RightContained
            } else if lf <= rt && rf <= lt {
                ValidityRelation::Overlapping
            } else {
                ValidityRelation::Disjoint
            }
        }
        _ => ValidityRelation::Unknown,
    }
}

#[allow(dead_code)]
fn source_gate(left: &Claim, right: &Claim) -> bool {
    left.source_fact_id != right.source_fact_id
}

#[allow(dead_code)]
fn correction_evidence(left: &Claim, right: &Claim) -> bool {
    left.qualifiers.contains_key("correction")
        || right.qualifiers.contains_key("correction")
        || left.qualifiers.contains_key("replaces")
        || right.qualifiers.contains_key("replaces")
}

#[allow(dead_code)]
fn transition_evidence(left: &Claim, right: &Claim) -> bool {
    left.qualifiers.contains_key("transition")
        || right.qualifiers.contains_key("transition")
        || left.qualifiers.contains_key("supersedes")
        || right.qualifiers.contains_key("supersedes")
}

// ─── Main Reconciliation Entry Point ─────────────────────────────────────────

/// Pure reconciliation decision engine.
#[allow(dead_code)]
pub(crate) fn reconcile(input: &ReconciliationInput<'_>) -> ReconciliationDecision {
    let left = input.left;
    let right = input.right;

    // Gate 1: Slot mismatch
    if !same_exact_slot(left, right) {
        return ReconciliationDecision::Skip(ReconciliationReasonCode::NotSameSlot);
    }

    // Gate 2: Incompatible types/units
    if !values_comparable(left, right) {
        return ReconciliationDecision::Skip(ReconciliationReasonCode::NotComparable);
    }

    // Gate 3: Same proposition → duplicate
    if same_proposition(left, right) {
        let draft = build_relation_draft(
            left,
            right,
            ClaimRelationOutcome::Consistent,
            ReconciliationReasonCode::Duplicate,
            "Claims are identical",
            input,
        );
        return ReconciliationDecision::Persist(Box::new(draft));
    }

    // Gate 4: Correction evidence (before contradiction check)
    if correction_evidence(left, right) && source_gate(left, right) {
        let (pred, succ) = if left.qualifiers.contains_key("replaces") {
            (Some(left.claim_id.clone()), Some(right.claim_id.clone()))
        } else {
            (Some(right.claim_id.clone()), Some(left.claim_id.clone()))
        };
        let draft = PersistedRelationDraft {
            left_claim_id: canonically_order_ids(&left.claim_id, &right.claim_id).0,
            right_claim_id: canonically_order_ids(&left.claim_id, &right.claim_id).1,
            outcome: ClaimRelationOutcome::Corrects,
            predecessor_claim_id: pred,
            successor_claim_id: succ,
            reason_code: ReconciliationReasonCode::Correction,
            evidence: ClaimRelationEvidence {
                reason_code: "correction".to_string(),
                description: Some("Explicit correction evidence with source gate".to_string()),
            },
            evaluator_version: input.evaluator_version.0.clone(),
            context_fingerprint: input.context_fingerprint.clone(),
            evaluated_at: input.evaluated_at,
        };
        return ReconciliationDecision::Persist(Box::new(draft));
    }

    // Gate 5: Set-valued coexistence (no exclusivity)
    if left.cardinality == ClaimCardinality::SetValued
        || right.cardinality == ClaimCardinality::SetValued
    {
        return ReconciliationDecision::Coexist(ReconciliationReasonCode::SetValuedCoexistence);
    }

    // Gate 6: Mutually exclusive values + overlapping validity → contradiction
    let vr = validity_relation(left, right);
    match vr {
        ValidityRelation::Identical
        | ValidityRelation::LeftContained
        | ValidityRelation::RightContained
        | ValidityRelation::Overlapping => {
            let draft = build_relation_draft(
                left,
                right,
                ClaimRelationOutcome::Contradicts,
                ReconciliationReasonCode::Contradiction,
                "Mutually exclusive values with overlapping validity",
                input,
            );
            return ReconciliationDecision::Persist(Box::new(draft));
        }
        ValidityRelation::Disjoint => {
            return ReconciliationDecision::Coexist(ReconciliationReasonCode::DisjointValidity);
        }
        ValidityRelation::Unknown => {}
    }

    // Gate 7: Transition evidence + source gate → supersession
    if transition_evidence(left, right) && source_gate(left, right) {
        let (pred, succ) = if left.qualifiers.contains_key("supersedes") {
            (Some(left.claim_id.clone()), Some(right.claim_id.clone()))
        } else {
            (Some(right.claim_id.clone()), Some(left.claim_id.clone()))
        };
        let draft = PersistedRelationDraft {
            left_claim_id: canonically_order_ids(&left.claim_id, &right.claim_id).0,
            right_claim_id: canonically_order_ids(&left.claim_id, &right.claim_id).1,
            outcome: ClaimRelationOutcome::Supersedes,
            predecessor_claim_id: pred,
            successor_claim_id: succ,
            reason_code: ReconciliationReasonCode::Supersession,
            evidence: ClaimRelationEvidence {
                reason_code: "supersession".to_string(),
                description: Some("Explicit transition evidence with source gate".to_string()),
            },
            evaluator_version: input.evaluator_version.0.clone(),
            context_fingerprint: input.context_fingerprint.clone(),
            evaluated_at: input.evaluated_at,
        };
        return ReconciliationDecision::Persist(Box::new(draft));
    }

    // Gate 8: Unknown validity + exclusive values → temporal ambiguity
    ReconciliationDecision::Skip(ReconciliationReasonCode::TemporalAmbiguity)
}

#[allow(dead_code)]
fn canonically_order_ids(a: &ClaimId, b: &ClaimId) -> (ClaimId, ClaimId) {
    if a.as_ref() <= b.as_ref() {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    }
}

#[allow(dead_code)]
fn build_relation_draft(
    left: &Claim,
    right: &Claim,
    outcome: ClaimRelationOutcome,
    reason_code: ReconciliationReasonCode,
    description: &str,
    input: &ReconciliationInput<'_>,
) -> PersistedRelationDraft {
    let (left_id, right_id) = canonically_order_ids(&left.claim_id, &right.claim_id);
    PersistedRelationDraft {
        left_claim_id: left_id,
        right_claim_id: right_id,
        outcome,
        predecessor_claim_id: None,
        successor_claim_id: None,
        reason_code,
        evidence: ClaimRelationEvidence {
            reason_code: reason_code.to_string(),
            description: Some(description.to_string()),
        },
        evaluator_version: input.evaluator_version.0.clone(),
        context_fingerprint: input.context_fingerprint.clone(),
        evaluated_at: input.evaluated_at,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::claim::{
        ClaimCardinality, ClaimSchemaFamily, ClaimSchemaRef, ClaimSlot, ClaimValiditySource,
        ClaimValue, ComparisonKey, ComparisonKeyHash, ExtractorFingerprint, NormalizedText,
        PolicyFingerprint, QualifierHash,
    };
    use crate::models::{EpisodeId, FactId};
    use std::collections::BTreeMap;

    fn make_claim(
        id: &str,
        value: ClaimValue,
        scope: &str,
        project: Option<&str>,
        qualifiers: BTreeMap<String, String>,
        valid_from: Option<chrono::DateTime<chrono::Utc>>,
        valid_to: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Claim {
        make_claim_with_fact(
            id,
            value,
            scope,
            project,
            qualifiers,
            valid_from,
            valid_to,
            "fact:test",
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn make_claim_with_fact(
        id: &str,
        value: ClaimValue,
        scope: &str,
        project: Option<&str>,
        qualifiers: BTreeMap<String, String>,
        valid_from: Option<chrono::DateTime<chrono::Utc>>,
        valid_to: Option<chrono::DateTime<chrono::Utc>>,
        fact_id: &str,
    ) -> Claim {
        let schema = ClaimSchemaRef {
            family: ClaimSchemaFamily::Attribute,
            version: std::num::NonZeroU16::new(1).unwrap(),
        };
        let mut components = BTreeMap::new();
        components.insert("dim".to_string(), "test".to_string());
        let key = ComparisonKey::new(schema, components).unwrap();
        let comparison_key_hash = ComparisonKeyHash::compute(&key);
        let qualifier_hash = QualifierHash::compute(&qualifiers);
        let access_policy_fingerprint = PolicyFingerprint::compute(scope, project, &[]);
        let project_identity = project.unwrap_or("__none__").to_string();

        Claim {
            claim_id: ClaimId::from_raw(format!("claim:{id}")),
            namespace: "test".to_string(),
            source_fact_id: FactId::from(fact_id),
            source_episode_id: EpisodeId::from("ep:test"),
            scope: scope.to_string(),
            project: project.map(String::from),
            project_identity,
            policy_tags: vec![],
            access_policy_fingerprint: access_policy_fingerprint.clone(),
            schema_family: ClaimSchemaFamily::Attribute,
            schema_version: 1,
            subject: ClaimSlot {
                namespace: "test".to_string(),
                scope: scope.to_string(),
                project_identity: project.unwrap_or("__none__").to_string(),
                access_policy_fingerprint: access_policy_fingerprint.clone(),
                schema_ref: schema,
                subject_key: "entity:s1".to_string(),
                comparison_key_hash: comparison_key_hash.clone(),
                qualifier_hash: qualifier_hash.clone(),
            },
            subject_key: "entity:s1".to_string(),
            comparison_key: key,
            comparison_key_hash,
            qualifiers,
            qualifier_hash,
            slot_fingerprint: format!("test:{scope}:{}:1:entity:s1", project.unwrap_or("__none__")),
            value,
            cardinality: ClaimCardinality::SingleValued,
            observed_at: chrono::Utc::now(),
            valid_from,
            valid_to,
            validity_source: ClaimValiditySource::Explicit,
            source_lineage: None,
            derivation: crate::models::claim::ClaimDerivation {
                source_fact_id: FactId::from("fact:test"),
                source_episode_id: EpisodeId::from("ep:test"),
                extractor_fingerprint: ExtractorFingerprint::compute(1, "test"),
            },
            extractor_fingerprint: ExtractorFingerprint::compute(1, "test"),
            t_ingested: chrono::Utc::now(),
            t_invalid_ingested: None,
        }
    }

    fn default_input<'a>(left: &'a Claim, right: &'a Claim) -> ReconciliationInput<'a> {
        static DEFAULT_ALIASES: std::sync::OnceLock<ConfirmedAliasSet> = std::sync::OnceLock::new();
        let aliases = DEFAULT_ALIASES.get_or_init(ConfirmedAliasSet::default);
        static DEFAULT_POLICY: std::sync::OnceLock<ClaimPolicy> = std::sync::OnceLock::new();
        let policy = DEFAULT_POLICY.get_or_init(|| ClaimPolicy {
            cardinality: ClaimCardinality::SingleValued,
        });
        static DEFAULT_EVAL_VERSION: std::sync::OnceLock<EvaluatorVersion> =
            std::sync::OnceLock::new();
        let eval_version =
            DEFAULT_EVAL_VERSION.get_or_init(|| EvaluatorVersion("test-1.0".to_string()));
        static DEFAULT_CTX_FP: std::sync::OnceLock<ReconciliationContextFingerprint> =
            std::sync::OnceLock::new();
        let ctx_fp = DEFAULT_CTX_FP.get_or_init(|| {
            ReconciliationContextFingerprint::compute(
                "test-1.0",
                "attribute",
                "alias_hash",
                "policy_hash",
            )
        });
        ReconciliationInput {
            left,
            right,
            policy,
            confirmed_aliases: aliases,
            evaluator_version: eval_version,
            context_fingerprint: ctx_fp,
            evaluated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn slot_mismatch_yields_not_same_slot() {
        let left = make_claim(
            "a",
            ClaimValue::Boolean(true),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let right = make_claim(
            "b",
            ClaimValue::Boolean(true),
            "team",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let input = default_input(&left, &right);
        assert!(matches!(
            reconcile(&input),
            ReconciliationDecision::Skip(ReconciliationReasonCode::NotSameSlot)
        ));
    }

    #[test]
    fn incompatible_types_yields_not_comparable() {
        let left = make_claim(
            "a",
            ClaimValue::Boolean(true),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let right = make_claim(
            "b",
            ClaimValue::Integer(42),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let input = default_input(&left, &right);
        assert!(matches!(
            reconcile(&input),
            ReconciliationDecision::Skip(ReconciliationReasonCode::NotComparable)
        ));
    }

    #[test]
    fn same_proposition_yields_duplicate() {
        let left = make_claim(
            "a",
            ClaimValue::Boolean(true),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let right = make_claim(
            "b",
            ClaimValue::Boolean(true),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let input = default_input(&left, &right);
        match reconcile(&input) {
            ReconciliationDecision::Persist(draft) => {
                assert_eq!(draft.outcome, ClaimRelationOutcome::Consistent);
                assert_eq!(draft.reason_code, ReconciliationReasonCode::Duplicate);
            }
            other => panic!("expected Persist(Duplicate), got {:?}", other),
        }
    }

    #[test]
    fn set_valued_coexistence() {
        let left = make_claim(
            "a",
            ClaimValue::Text(NormalizedText::new("x")),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let mut right = make_claim(
            "b",
            ClaimValue::Text(NormalizedText::new("y")),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        right.cardinality = ClaimCardinality::SetValued;
        let input = default_input(&left, &right);
        assert!(matches!(
            reconcile(&input),
            ReconciliationDecision::Coexist(ReconciliationReasonCode::SetValuedCoexistence)
        ));
    }

    #[test]
    fn disjoint_validity_coexists() {
        let t1 = chrono::Utc::now() - chrono::Duration::days(30);
        let t2 = chrono::Utc::now() - chrono::Duration::days(20);
        let t3 = chrono::Utc::now() - chrono::Duration::days(10);
        let t4 = chrono::Utc::now();
        let left = make_claim(
            "a",
            ClaimValue::Boolean(true),
            "personal",
            None,
            BTreeMap::new(),
            Some(t1),
            Some(t2),
        );
        let right = make_claim(
            "b",
            ClaimValue::Boolean(false),
            "personal",
            None,
            BTreeMap::new(),
            Some(t3),
            Some(t4),
        );
        let input = default_input(&left, &right);
        assert!(matches!(
            reconcile(&input),
            ReconciliationDecision::Coexist(ReconciliationReasonCode::DisjointValidity)
        ));
    }

    #[test]
    fn overlapping_validity_yields_contradiction() {
        let t1 = chrono::Utc::now() - chrono::Duration::days(30);
        let t2 = chrono::Utc::now();
        let left = make_claim(
            "a",
            ClaimValue::Boolean(true),
            "personal",
            None,
            BTreeMap::new(),
            Some(t1),
            Some(t2),
        );
        let right = make_claim(
            "b",
            ClaimValue::Boolean(false),
            "personal",
            None,
            BTreeMap::new(),
            Some(t1),
            Some(t2),
        );
        let input = default_input(&left, &right);
        match reconcile(&input) {
            ReconciliationDecision::Persist(draft) => {
                assert_eq!(draft.outcome, ClaimRelationOutcome::Contradicts);
                assert_eq!(draft.reason_code, ReconciliationReasonCode::Contradiction);
            }
            other => panic!("expected Persist(Contradiction), got {:?}", other),
        }
    }

    #[test]
    fn correction_with_source_gate() {
        let mut quals_left = BTreeMap::new();
        quals_left.insert("correction".to_string(), "true".to_string());
        let left = make_claim_with_fact(
            "a",
            ClaimValue::Boolean(false),
            "personal",
            None,
            quals_left,
            None,
            None,
            "fact:old",
        );
        let right = make_claim_with_fact(
            "b",
            ClaimValue::Boolean(true),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
            "fact:new",
        );
        let input = default_input(&left, &right);
        match reconcile(&input) {
            ReconciliationDecision::Persist(draft) => {
                assert_eq!(draft.outcome, ClaimRelationOutcome::Corrects);
                assert_eq!(draft.reason_code, ReconciliationReasonCode::Correction);
            }
            other => panic!("expected Persist(Correction), got {:?}", other),
        }
    }

    #[test]
    fn temporal_ambiguity_when_unknown_validity() {
        let left = make_claim(
            "a",
            ClaimValue::Boolean(true),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let right = make_claim(
            "b",
            ClaimValue::Boolean(false),
            "personal",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let input = default_input(&left, &right);
        assert!(matches!(
            reconcile(&input),
            ReconciliationDecision::Skip(ReconciliationReasonCode::TemporalAmbiguity)
        ));
    }

    #[test]
    fn no_correction_without_source_gate() {
        let mut quals = BTreeMap::new();
        quals.insert("correction".to_string(), "true".to_string());
        let left = make_claim(
            "a",
            ClaimValue::Boolean(false),
            "personal",
            None,
            quals.clone(),
            None,
            None,
        );
        let right = make_claim(
            "b",
            ClaimValue::Boolean(true),
            "personal",
            None,
            quals,
            None,
            None,
        );
        // Same source_fact_id → source_gate fails → no correction
        // Unknown validity → temporal_ambiguity
        let input = default_input(&left, &right);
        assert!(matches!(
            reconcile(&input),
            ReconciliationDecision::Skip(ReconciliationReasonCode::TemporalAmbiguity)
        ));
    }

    #[test]
    fn canonically_order_ids_is_consistent() {
        let a = ClaimId::from_raw("claim:aaa".to_string());
        let b = ClaimId::from_raw("claim:bbb".to_string());
        assert_eq!(canonically_order_ids(&a, &b), canonically_order_ids(&b, &a));
    }
}
