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
use crate::knowledge::entity_extraction::EntityExtractor;

/// Refuses extraction inputs that exceed the selected extractor's whole-input limit.
pub fn validate_entity_extraction_input(
    extractor: &(impl EntityExtractor + ?Sized),
    content: &str,
) -> Result<(), MemoryError> {
    let Some(max_bytes) = extractor.max_input_bytes() else {
        return Ok(());
    };
    let actual_bytes = content.len();
    if actual_bytes > max_bytes {
        return Err(MemoryError::Validation(format!(
            "entity extraction input too large: provider={} actual_bytes={} max_bytes={}",
            extractor.provider_name(),
            actual_bytes,
            max_bytes,
        )));
    }
    Ok(())
}

/// Startup metadata reads owned by the knowledge context.
#[async_trait::async_trait]
pub trait FactEmbeddingMetadataReadPort: Send + Sync {
    async fn count_facts(&self) -> Result<usize, MemoryError>;

    async fn sample_stored_embedding_dimensions(
        &self,
        sample_size: usize,
    ) -> Result<Vec<usize>, MemoryError>;
}

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

/// Historical relation read rooted at facts already selected for the caller.
/// `policy_tags` uses the same any-of semantics as retrieval access filtering.
pub(crate) struct TemporalRelationsQuery {
    pub fact_ids: Vec<crate::models::FactId>,
    pub cutoff: chrono::DateTime<chrono::Utc>,
    pub policy_tags: Vec<String>,
    pub counterpart_limit: usize,
    pub max_relation_depth: usize,
}

/// Bounded historical relation projection. Unresolved IDs remain internal so
/// the caller can preserve uncertainty without publishing a denied target.
#[derive(Debug, Clone, Default)]
pub(crate) struct TemporalRelationsBatch {
    pub relations: Vec<crate::knowledge::claims::RelationForFact>,
    pub counterpart_facts: Vec<crate::knowledge::claims::TemporalCounterpartFact>,
    pub claims_by_fact: std::collections::HashMap<crate::models::FactId, Vec<String>>,
    pub unresolved_fact_ids: Vec<crate::models::FactId>,
    /// Visible facts whose relation frontier could not be fully read.
    pub truncated_fact_ids: Vec<crate::models::FactId>,
    /// False when rollout policy withheld all relation evidence.
    pub evidence_available: bool,
    pub truncated: bool,
}

pub(crate) const TEMPORAL_RELATION_MAX_COUNTERPARTS: usize = 32;
pub(crate) const TEMPORAL_RELATION_MAX_DEPTH: usize = 4;
const TEMPORAL_RELATION_MAX_BATCH_ROWS: usize = 256;

fn relation_tags_allowed(record_tags: &[String], allowed_tags: &[String]) -> bool {
    allowed_tags.is_empty()
        || record_tags.is_empty()
        || record_tags
            .iter()
            .any(|tag| allowed_tags.iter().any(|allowed| allowed == tag))
}

/// Knowledge use case for historical, access-filtered relation traversal.
/// Storage reads stay set-based behind the narrow knowledge-owned port.
pub(crate) async fn temporal_relations_from_store(
    store: &(impl crate::knowledge::claims::TemporalRelationReadStore + ?Sized),
    query: TemporalRelationsQuery,
) -> Result<TemporalRelationsBatch, MemoryError> {
    use crate::knowledge::claims::{RelationForFact, RelationParticipant};
    use crate::shared::temporal::normalize_dt;

    if query.fact_ids.is_empty() || query.counterpart_limit == 0 || query.max_relation_depth == 0 {
        return Ok(TemporalRelationsBatch::default());
    }

    let counterpart_limit = query
        .counterpart_limit
        .min(TEMPORAL_RELATION_MAX_COUNTERPARTS);
    let depth_limit = query.max_relation_depth.min(TEMPORAL_RELATION_MAX_DEPTH);
    let cutoff = normalize_dt(query.cutoff);
    let request_truncated =
        query.counterpart_limit > counterpart_limit || query.max_relation_depth > depth_limit;
    let mut batch = TemporalRelationsBatch {
        evidence_available: true,
        truncated: request_truncated,
        ..TemporalRelationsBatch::default()
    };
    let mut visible_fact_ids = query
        .fact_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let initial_fact_ids = visible_fact_ids.clone();
    let mut frontier = query.fact_ids;
    let mut truncated_frontiers = if request_truncated {
        initial_fact_ids.clone()
    } else {
        std::collections::HashSet::new()
    };
    let mut seen_relation_ids = std::collections::HashSet::new();
    let mut unresolved = std::collections::HashSet::new();

    for depth in 0..depth_limit {
        if frontier.is_empty() {
            break;
        }

        let claims = store
            .claims_by_facts_at(&frontier, &cutoff, TEMPORAL_RELATION_MAX_BATCH_ROWS)
            .await?;
        if claims.len() >= TEMPORAL_RELATION_MAX_BATCH_ROWS {
            batch.truncated = true;
            truncated_frontiers.extend(frontier.iter().cloned());
        }
        let claims = claims
            .into_iter()
            .filter(|claim| relation_tags_allowed(&claim.policy_tags, &query.policy_tags))
            .collect::<Vec<_>>();
        for claim in &claims {
            if initial_fact_ids.contains(&claim.source_fact_id) {
                batch
                    .claims_by_fact
                    .entry(claim.source_fact_id.clone())
                    .or_default()
                    .push(claim.claim_id.to_string());
            }
        }
        if claims.is_empty() {
            continue;
        }

        let frontier_claim_ids = claims
            .iter()
            .map(|claim| claim.claim_id.to_string())
            .collect::<Vec<_>>();
        let relations = store
            .relations_by_claim_ids_at(
                &frontier_claim_ids,
                &cutoff,
                TEMPORAL_RELATION_MAX_BATCH_ROWS,
            )
            .await?;
        if relations.len() >= TEMPORAL_RELATION_MAX_BATCH_ROWS {
            batch.truncated = true;
            truncated_frontiers.extend(frontier.iter().cloned());
        }
        let relations = relations
            .into_iter()
            .filter(|relation| relation_tags_allowed(&relation.policy_tags, &query.policy_tags))
            .collect::<Vec<_>>();
        if relations.is_empty() {
            continue;
        }

        let mut relation_claim_ids = relations
            .iter()
            .flat_map(|relation| {
                [
                    relation.left_claim_id.to_string(),
                    relation.right_claim_id.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        relation_claim_ids.sort();
        relation_claim_ids.dedup();
        let related_claims = store
            .claims_by_ids_at(
                &relation_claim_ids,
                &cutoff,
                TEMPORAL_RELATION_MAX_BATCH_ROWS,
            )
            .await?;
        if related_claims.len() >= TEMPORAL_RELATION_MAX_BATCH_ROWS {
            batch.truncated = true;
            truncated_frontiers.extend(frontier.iter().cloned());
        }
        let allowed_claim_ids = related_claims
            .iter()
            .filter(|claim| relation_tags_allowed(&claim.policy_tags, &query.policy_tags))
            .map(|claim| claim.claim_id.to_string())
            .collect::<std::collections::HashSet<_>>();
        let claim_by_id = related_claims
            .iter()
            .map(|claim| (claim.claim_id.to_string(), claim))
            .collect::<std::collections::HashMap<_, _>>();

        let mut candidate_fact_ids = related_claims
            .iter()
            .filter(|claim| allowed_claim_ids.contains(claim.claim_id.as_ref()))
            .map(|claim| claim.source_fact_id.to_string())
            .filter(|fact_id| {
                !visible_fact_ids.contains(&crate::models::FactId::from(fact_id.clone()))
            })
            .collect::<Vec<_>>();
        candidate_fact_ids.sort();
        candidate_fact_ids.dedup();

        let remaining = counterpart_limit.saturating_sub(batch.counterpart_facts.len());
        let fact_fetch_limit = remaining.saturating_add(1);
        let mut newly_visible = Vec::new();
        if !candidate_fact_ids.is_empty() && remaining > 0 {
            let facts = store
                .facts_by_ids_at(&candidate_fact_ids, &cutoff, fact_fetch_limit)
                .await?;
            let mut allowed_facts = facts
                .into_iter()
                .filter(|fact| relation_tags_allowed(&fact.policy_tags, &query.policy_tags))
                .collect::<Vec<_>>();
            allowed_facts.sort_by_key(|fact| fact.fact_id.to_string());
            if allowed_facts.len() > remaining {
                batch.truncated = true;
                truncated_frontiers.extend(frontier.iter().cloned());
                allowed_facts.truncate(remaining);
            }
            for fact in allowed_facts {
                let fact_id = fact.fact_id.clone();
                if visible_fact_ids.insert(fact_id) {
                    newly_visible.push(fact.fact_id.clone());
                    batch.counterpart_facts.push(fact);
                }
            }
        } else if !candidate_fact_ids.is_empty() {
            batch.truncated = true;
            truncated_frontiers.extend(frontier.iter().cloned());
        }

        for relation in relations {
            if !seen_relation_ids.insert(relation.claim_relation_id.to_string()) {
                continue;
            }
            let side_claims = [&relation.left_claim_id, &relation.right_claim_id]
                .into_iter()
                .filter_map(|claim_id| claim_by_id.get(claim_id.as_ref()).copied())
                .filter(|claim| allowed_claim_ids.contains(claim.claim_id.as_ref()))
                .filter(|claim| visible_fact_ids.contains(&claim.source_fact_id))
                .collect::<Vec<_>>();
            if side_claims.is_empty() {
                continue;
            }
            let mut seen_participants = std::collections::HashSet::new();
            let participants = side_claims
                .iter()
                .filter(|claim| seen_participants.insert(claim.source_fact_id.clone()))
                .map(|claim| RelationParticipant {
                    fact_id: claim.source_fact_id.clone(),
                    source_episode_id: claim.source_episode_id.clone(),
                })
                .collect::<Vec<_>>();
            for claim_id in [&relation.left_claim_id, &relation.right_claim_id] {
                if let Some(claim) = claim_by_id.get(claim_id.as_ref())
                    && (!allowed_claim_ids.contains(claim.claim_id.as_ref())
                        || !visible_fact_ids.contains(&claim.source_fact_id))
                {
                    unresolved.insert(claim.source_fact_id.clone());
                }
            }
            let fact_for_claim = |id: &Option<crate::models::ClaimId>| {
                id.as_ref()
                    .and_then(|claim_id| claim_by_id.get(claim_id.as_ref()))
                    .filter(|claim| allowed_claim_ids.contains(claim.claim_id.as_ref()))
                    .filter(|claim| visible_fact_ids.contains(&claim.source_fact_id))
                    .map(|claim| claim.source_fact_id.clone())
            };
            let (predecessor_fact_id, successor_fact_id) = if relation.outcome.has_direction() {
                (
                    fact_for_claim(&relation.predecessor_claim_id),
                    fact_for_claim(&relation.successor_claim_id),
                )
            } else {
                (None, None)
            };
            batch.relations.push(RelationForFact {
                relation_id: relation.claim_relation_id.to_string(),
                outcome: relation.outcome,
                reason_code: relation.reason_code,
                evaluator_version: relation.evaluator_version,
                predecessor_fact_id,
                successor_fact_id,
                participants,
            });
        }

        if depth + 1 == depth_limit && !newly_visible.is_empty() {
            batch.truncated = true;
            truncated_frontiers.extend(newly_visible.iter().cloned());
        }
        frontier = newly_visible;
    }

    batch.unresolved_fact_ids = unresolved.into_iter().collect();
    batch.unresolved_fact_ids.sort_by_key(ToString::to_string);
    batch.truncated_fact_ids = truncated_frontiers.into_iter().collect();
    batch.truncated_fact_ids.sort_by_key(ToString::to_string);
    for claim_ids in batch.claims_by_fact.values_mut() {
        claim_ids.sort();
        claim_ids.dedup();
    }
    batch
        .counterpart_facts
        .sort_by_key(|fact| fact.fact_id.to_string());
    batch
        .relations
        .sort_by(|left, right| left.relation_id.cmp(&right.relation_id));
    Ok(batch)
}

/// Project a temporal batch onto the caller's already-selected fact set. This
/// keeps a hydrated but unselected counterpart from becoming public metadata.
pub(crate) fn visible_temporal_relations_for_facts(
    batch: &TemporalRelationsBatch,
    fact_ids: &[crate::models::FactId],
) -> crate::knowledge::claims::RelationsByFactResult {
    let visible = fact_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let relations = batch
        .relations
        .iter()
        .filter_map(|relation| {
            let participants = relation
                .participants
                .iter()
                .filter(|participant| visible.contains(&participant.fact_id))
                .cloned()
                .collect::<Vec<_>>();
            if participants.is_empty() {
                return None;
            }
            let mut projected = relation.clone();
            projected.participants = participants;
            projected.predecessor_fact_id = projected
                .predecessor_fact_id
                .filter(|fact_id| visible.contains(fact_id));
            projected.successor_fact_id = projected
                .successor_fact_id
                .filter(|fact_id| visible.contains(fact_id));
            Some(projected)
        })
        .collect();
    let claims_by_fact = batch
        .claims_by_fact
        .iter()
        .filter(|(fact_id, _)| visible.contains(*fact_id))
        .map(|(fact_id, claim_ids)| (fact_id.clone(), claim_ids.clone()))
        .collect();

    crate::knowledge::claims::RelationsByFactResult {
        relations,
        claims_by_fact,
    }
}

/// Narrow port for reading the reconciliation relations of a set of facts.
///
/// This is the seam `memory/retrieval` consumes so it never names
/// `ClaimStore` or composes claim SQL: claims and relations belong to
/// `knowledge` (ADR-0058), and the two reads here are the only exposed shapes.
#[async_trait::async_trait]
pub(crate) trait RelationReadPort: Send + Sync {
    async fn temporal_relations(
        &self,
        query: TemporalRelationsQuery,
    ) -> Result<TemporalRelationsBatch, MemoryError>;
}

/// Read bounded relation evidence and visible counterpart facts through the
/// knowledge-owned port. Callers must not expose `unresolved_fact_ids`.
pub(crate) async fn temporal_relations(
    port: &(impl RelationReadPort + ?Sized),
    query: TemporalRelationsQuery,
) -> Result<TemporalRelationsBatch, MemoryError> {
    port.temporal_relations(query).await
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
        let directed = relation.outcome.has_direction();

        for own in &relation.participants {
            let counterpart = relation
                .participants
                .iter()
                .find(|participant| participant.fact_id != own.fact_id);
            let superseded_by_fact_id =
                if directed && relation.predecessor_fact_id.as_ref() == Some(&own.fact_id) {
                    relation
                        .successor_fact_id
                        .as_ref()
                        .filter(|successor| {
                            relation
                                .participants
                                .iter()
                                .any(|participant| &participant.fact_id == *successor)
                        })
                        .map(ToString::to_string)
                } else {
                    None
                };

            let summary = ClaimRelationSummary {
                relation_id: relation.relation_id.clone(),
                outcome: relation.outcome,
                counterpart_source_episode_id: counterpart
                    .map(|participant| participant.source_episode_id.to_string()),
                // Only the losing side names its replacement; see the doc comment.
                superseded_by_fact_id,
                reason_code: relation.reason_code.clone(),
                evaluator_version: relation.evaluator_version.clone(),
            };

            let entry = by_fact.entry(own.fact_id.to_string()).or_insert_with(|| {
                ClaimReconciliationMetadata {
                    claim_ids: result
                        .claims_by_fact
                        .get(&own.fact_id)
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

/// Returns only persisted Duplicate groups whose participants are all in the
/// caller's visible fact set. This keeps an unselected or inaccessible
/// counterpart from becoming a deduplication hint.
pub(crate) fn duplicate_fact_groups(
    result: &crate::knowledge::claims::RelationsByFactResult,
) -> Vec<std::collections::BTreeSet<String>> {
    result
        .relations
        .iter()
        .filter(|relation| {
            relation.outcome == crate::models::claim::ClaimRelationOutcome::Duplicate
        })
        .map(|relation| {
            relation
                .participants
                .iter()
                .map(|participant| participant.fact_id.to_string())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .filter(|group| group.len() > 1)
        .collect()
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
    async fn temporal_relations(
        &self,
        query: TemporalRelationsQuery,
    ) -> Result<TemporalRelationsBatch, MemoryError> {
        if !self.exposes_evidence {
            return Ok(TemporalRelationsBatch::default());
        }
        temporal_relations_from_store(&self.store, query).await
    }
}
