use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::error::MemoryError;
use crate::models::claim::{ClaimRelation, ClaimRelationOutcome};
use crate::storage::DbClient;
use crate::storage::claims::ClaimStore;
use crate::storage::claims::{RelationsForFactsQuery, SurrealClaimStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedRelation {
    pub relation_id: String,
    pub left_fact_id: String,
    pub right_fact_id: String,
    pub outcome: ClaimRelationOutcome,
    pub reason_code: String,
    /// The Active Namespace associated with this eval relation.
    pub namespace: String,
    /// Active policy identity derived from policy tags.
    pub policy_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLineage {
    pub episode_id: String,
    pub fact_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceLineageMap {
    pub by_source_id: BTreeMap<String, SourceLineage>,
}

impl SourceLineageMap {
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, Vec<String>)>) -> Self {
        let mut map = BTreeMap::new();
        for (source_id, fact_ids) in pairs {
            map.insert(
                source_id,
                SourceLineage {
                    episode_id: String::new(),
                    fact_ids: fact_ids.into_iter().collect(),
                },
            );
        }
        Self { by_source_id: map }
    }

    pub fn fact_ids(&self, source_id: &str) -> &BTreeSet<String> {
        static EMPTY: BTreeSet<String> = BTreeSet::new();
        self.by_source_id
            .get(source_id)
            .map(|l| &l.fact_ids)
            .unwrap_or(&EMPTY)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationBoundary {
    /// The process-bound Active Namespace is the storage isolation boundary.
    pub namespace: String,
    /// Policy/tag identity is evaluated independently of namespace isolation.
    pub policy_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationViolation {
    pub relation_id: String,
    pub boundary_field: String,
    pub expected: String,
    pub actual: String,
}

pub fn classify_isolation_violation(
    relation: &EvaluatedRelation,
    expected: &IsolationBoundary,
) -> Option<IsolationViolation> {
    if relation.namespace != expected.namespace {
        return Some(IsolationViolation {
            relation_id: relation.relation_id.clone(),
            boundary_field: "namespace".into(),
            expected: expected.namespace.clone(),
            actual: relation.namespace.clone(),
        });
    }
    if relation.policy_fingerprint != expected.policy_fingerprint {
        return Some(IsolationViolation {
            relation_id: relation.relation_id.clone(),
            boundary_field: "policy_fingerprint".into(),
            expected: expected.policy_fingerprint.clone(),
            actual: relation.policy_fingerprint.clone(),
        });
    }
    None
}

/// Read-only, feature-gated evidence seam over persisted claim relations
/// (eval-v2-closure Task 2). Exposes immutable evaluation views only — no
/// SurrealDB queries, no mutation, and never reachable through MCP.
pub struct ClaimEvidenceReader {
    store: SurrealClaimStore,
}

/// Persisted evidence for a set of fact IDs.
#[derive(Debug, Clone, Default)]
pub struct PersistedClaimEvidence {
    pub relations: Vec<EvaluatedRelation>,
}

impl ClaimEvidenceReader {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            store: SurrealClaimStore::new(db, namespace),
        }
    }

    /// Loads active persisted relations touching any of `fact_ids`.
    pub async fn for_fact_ids(
        &self,
        fact_ids: &[String],
    ) -> Result<PersistedClaimEvidence, MemoryError> {
        let ids: Vec<crate::models::FactId> = fact_ids
            .iter()
            .map(|f| crate::models::FactId::from(f.as_str()))
            .collect();
        if ids.is_empty() {
            return Ok(PersistedClaimEvidence::default());
        }
        let relations = self
            .store
            .select_relations_for_facts(RelationsForFactsQuery { fact_ids: &ids })
            .await?;
        Ok(PersistedClaimEvidence {
            relations: relations.iter().filter_map(evaluated_relation).collect(),
        })
    }
}

/// Projects a persisted relation onto the evaluation view. Relations without
/// both source fact IDs (pre-migration rows) are skipped.
fn evaluated_relation(rel: &ClaimRelation) -> Option<EvaluatedRelation> {
    let left_fact_id = rel.left_fact_id.as_ref()?.as_ref().to_string();
    let right_fact_id = rel.right_fact_id.as_ref()?.as_ref().to_string();
    Some(EvaluatedRelation {
        relation_id: rel.claim_relation_id.as_ref().to_string(),
        left_fact_id,
        right_fact_id,
        outcome: rel.outcome,
        reason_code: rel.reason_code.clone(),
        namespace: String::new(),
        policy_fingerprint: rel.policy_tags.join(","),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_boundary() -> IsolationBoundary {
        IsolationBoundary {
            namespace: "main".into(),
            policy_fingerprint: "fp-1".into(),
        }
    }

    fn relation(left: &str, right: &str, outcome: ClaimRelationOutcome) -> EvaluatedRelation {
        EvaluatedRelation {
            relation_id: format!("rel:{left}:{right}"),
            left_fact_id: left.into(),
            right_fact_id: right.into(),
            outcome,
            reason_code: "test".into(),
            namespace: "main".into(),
            policy_fingerprint: "fp-1".into(),
        }
    }

    #[test]
    fn different_fact_ids_are_not_an_isolation_violation() {
        let rel = relation("fact:old", "fact:new", ClaimRelationOutcome::Contradiction);
        assert_eq!(classify_isolation_violation(&rel, &same_boundary()), None);
    }

    #[test]
    fn namespace_mismatch_is_an_isolation_violation() {
        let mut rel = relation("f1", "f2", ClaimRelationOutcome::Contradiction);
        rel.namespace = "other".into();
        let violation = classify_isolation_violation(&rel, &same_boundary());
        assert_eq!(
            violation.as_ref().map(|v| v.boundary_field.as_str()),
            Some("namespace")
        );
    }

    #[test]
    fn different_policy_fingerprint_is_reported_separately_from_namespace() {
        let mut rel = relation("f1", "f2", ClaimRelationOutcome::Contradiction);
        rel.policy_fingerprint = "fp-2".into();
        let violation = classify_isolation_violation(&rel, &same_boundary());
        assert!(violation.is_some());
        assert_eq!(violation.unwrap().boundary_field, "policy_fingerprint");
    }

    #[test]
    fn source_lineage_map_returns_fact_ids() {
        let map = SourceLineageMap::from_pairs([
            ("setup-1".into(), vec!["fact:old".into()]),
            ("source-1".into(), vec!["fact:new".into()]),
        ]);
        assert_eq!(map.fact_ids("setup-1").len(), 1);
        assert!(map.fact_ids("setup-1").contains("fact:old"));
        assert!(map.fact_ids("nonexistent").is_empty());
    }
}
