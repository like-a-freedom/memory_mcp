//! Knowledge policies and queries — public interface.
//!
//! Knowledge owns entities, aliases, facts, claims, triples,
//! communities, extraction/reconciliation and knowledge
//! queries.
//!
//! Reads are expressed as owner-named [`KnowledgeReadScope`]
//! values rather than caller-supplied table names, so an
//! application-facing API cannot reach an arbitrary table.
//! The full canonical fact and entity query surface stays
//! behind the owner infra layer; a cross-owner optimized read
//! is only exposed through an explicit provider contract.

use serde_json::Value;

use crate::error::MemoryError;

/// A read that knowledge is willing to serve, named by owner
/// rather than by table string.
///
/// This is deliberately a closed enum: there is no variant
/// that carries a caller-provided table name, so the previous
/// `select_table(&str)` escape hatch has no representation
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnowledgeReadScope {
    /// Bi-temporally visible canonical facts.
    Facts,
    /// Source episodes, which memory owns but knowledge reads
    /// for provenance assembly.
    Episodes,
}

impl KnowledgeReadScope {
    /// Owning bounded context for this scope.
    pub const fn owner(self) -> &'static str {
        match self {
            Self::Facts => "knowledge",
            Self::Episodes => "memory",
        }
    }
}

/// A completed owner-scoped read.
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeRead {
    pub scope: KnowledgeReadScope,
    pub owner: &'static str,
    pub rows: usize,
}

/// Port supplying the rows for a scope.
///
/// The port is owner-injected: the implementation decides how
/// to read, and the caller cannot name a table.
#[async_trait::async_trait]
pub trait KnowledgeReadPort: Send + Sync {
    async fn read_scope(&self, scope: KnowledgeReadScope) -> Result<Vec<Value>, MemoryError>;
}

/// Serve a scope through an injected port, reporting the owner
/// that answered it.
pub async fn read_through_knowledge_port(
    port: &(impl KnowledgeReadPort + ?Sized),
    scope: KnowledgeReadScope,
) -> Result<KnowledgeRead, MemoryError> {
    let rows = port.read_scope(scope).await?;
    Ok(KnowledgeRead {
        scope,
        owner: scope.owner(),
        rows: rows.len(),
    })
}

/// Owner knowledge of canonical fact reads.
pub async fn owned_fact_scan(
    port: &(impl KnowledgeReadPort + ?Sized),
) -> Result<Vec<Value>, MemoryError> {
    port.read_scope(KnowledgeReadScope::Facts).await
}

/// Owner knowledge of the episode read used for provenance.
pub async fn owned_episode_scan(
    port: &(impl KnowledgeReadPort + ?Sized),
) -> Result<Vec<Value>, MemoryError> {
    port.read_scope(KnowledgeReadScope::Episodes).await
}

// ─── Relation reads ───────────────────────────────────────────────────────────

/// Narrow port for reading the reconciliation relations of a set of facts.
///
/// This is the seam `memory/retrieval` consumes so it never names
/// `ClaimStore` or composes claim SQL: claims and relations belong to
/// `knowledge` (ADR-0058), and one method is all the read path needs.
#[async_trait::async_trait]
pub(crate) trait RelationReadPort: Send + Sync {
    async fn relations_for_facts(
        &self,
        fact_ids: &[crate::models::FactId],
    ) -> Result<crate::knowledge::claims::RelationsByFactResult, MemoryError>;
}

/// Serve the relations of the given facts through an injected port.
pub(crate) async fn relations_for_facts(
    port: &(impl RelationReadPort + ?Sized),
    fact_ids: &[crate::models::FactId],
) -> Result<crate::knowledge::claims::RelationsByFactResult, MemoryError> {
    port.relations_for_facts(fact_ids).await
}

/// Project a relations read onto the public per-fact metadata shape.
///
/// The result is keyed by fact id, so the read path assigns it without
/// knowing anything about relations. Each fact that participates in a
/// relation gets that relation attached, addressed from *its own* side:
/// `counterpart_source_episode_id` is the other side's episode.
///
/// `superseded_by_fact_id` is set only on the **predecessor's** entry — the
/// fact whose claim lost. A successor carrying `Some(itself)` would let a
/// reader demote an item below itself, so the successor's entry deliberately
/// holds `None`: it still learns the relation exists, but names no target.
pub(crate) fn reconciliation_metadata_by_fact(
    result: &crate::knowledge::claims::RelationsByFactResult,
) -> std::collections::HashMap<String, crate::models::ClaimReconciliationMetadata> {
    use crate::models::{ClaimReconciliationMetadata, ClaimRelationSummary};

    let mut by_fact: std::collections::HashMap<String, ClaimReconciliationMetadata> =
        std::collections::HashMap::new();

    for relation in &result.relations {
        let directed = matches!(
            relation.outcome,
            crate::models::claim::ClaimRelationOutcome::Supersession
                | crate::models::claim::ClaimRelationOutcome::Correction
        );

        // (fact whose claim lost, its episode, fact whose claim won, its episode)
        let sides: [(
            Option<&crate::models::FactId>,
            Option<&crate::models::EpisodeId>,
        ); 2] = [
            (
                relation.predecessor_fact_id.as_ref(),
                relation.predecessor_source_episode_id.as_ref(),
            ),
            (
                relation.successor_fact_id.as_ref(),
                relation.successor_source_episode_id.as_ref(),
            ),
        ];

        for (side_index, (own_fact, _own_episode)) in sides.into_iter().enumerate() {
            let Some(own_fact) = own_fact else { continue };
            let counterpart_fact = sides[1 - side_index].0;
            let counterpart_episode = sides[1 - side_index].1;

            let summary = ClaimRelationSummary {
                relation_id: relation.relation_id.clone(),
                outcome: relation.outcome,
                counterpart_source_episode_id: counterpart_episode
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                // Only the losing side names its replacement; see the doc comment.
                superseded_by_fact_id: if directed && side_index == 0 {
                    counterpart_fact.map(ToString::to_string)
                } else {
                    None
                },
                reason_code: relation.reason_code.clone(),
                evaluator_version: relation.evaluator_version.clone(),
            };

            let entry = by_fact.entry(own_fact.to_string()).or_insert_with(|| {
                ClaimReconciliationMetadata {
                    claim_ids: result
                        .claims_by_fact
                        .get(own_fact)
                        .cloned()
                        .unwrap_or_default(),
                    relations: Vec::new(),
                }
            });
            entry.relations.push(summary);
        }
    }

    by_fact
}

/// SurrealDB adapter behind [`RelationReadPort`], built the same way as its
/// sibling store clients — from a bound database client plus a namespace.
///
/// The adapter carries the rollout policy as well as the store: relations are
/// served only when the claim rollout stage exposes evidence. Keeping the gate
/// here means `memory/retrieval` never learns the stage exists, and a
/// deployment at `shadow` or `relations` cannot receive relation rows at all —
/// which is what `docs/evals/CLAIM_RECONCILIATION.md` promises. Because
/// `demote_superseded` reads its input from those same rows, one gate covers
/// both the metadata and the reordering: neither can happen while the stage
/// forbids disclosure.
pub(crate) struct SurrealRelationReader {
    store: crate::knowledge::claims::SurrealClaimStore,
    exposes_evidence: bool,
}

impl SurrealRelationReader {
    pub(crate) fn new(
        db: std::sync::Arc<dyn crate::storage::DbClient>,
        namespace: impl Into<String>,
        exposes_evidence: bool,
    ) -> Self {
        Self {
            store: crate::knowledge::claims::SurrealClaimStore::new(db, namespace),
            exposes_evidence,
        }
    }
}

#[async_trait::async_trait]
impl RelationReadPort for SurrealRelationReader {
    async fn relations_for_facts(
        &self,
        fact_ids: &[crate::models::FactId],
    ) -> Result<crate::knowledge::claims::RelationsByFactResult, MemoryError> {
        if !self.exposes_evidence {
            return Ok(crate::knowledge::claims::RelationsByFactResult::default());
        }
        use crate::knowledge::claims::ClaimStore;
        self.store
            .select_relations_by_fact(crate::knowledge::claims::RelationsByFactQuery { fact_ids })
            .await
    }
}
