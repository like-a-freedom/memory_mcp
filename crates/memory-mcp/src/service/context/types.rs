//! Shared candidate types for the context pipeline.
//!
//! Collectors and rankers depend on these types rather than on each other's
//! modules (Card 7: reduce sideways collector coupling — ranking⇄lexical,
//! ranking⇄scoring, filtering). Keeping the tier enum and the ranked-candidate
//! record here gives every tier one home for the vocabulary of "what tier
//! produced this fact and how it ranks".

use super::graph::GraphTrace;
use crate::models::Fact;

/// Retrieval tier that produced a candidate fact.
///
/// Ordered by pipeline stage; `precedence()` reflects that order for
/// deduplication decisions (a fact surfaced by a higher-precedence tier keeps
/// its provenance over a lower one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetrievalTier {
    Direct,
    AliasExpanded,
    TemporalExpanded,
    GraphExpanded,
    SemanticExpanded,
    EpisodeFallback,
}

impl RetrievalTier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::AliasExpanded => "alias",
            Self::TemporalExpanded => "temporal",
            Self::GraphExpanded => "graph",
            Self::SemanticExpanded => "semantic",
            Self::EpisodeFallback => "fallback",
        }
    }

    pub(crate) fn precedence(self) -> u8 {
        match self {
            Self::Direct => 0,
            Self::EpisodeFallback => 1,
            Self::AliasExpanded => 2,
            Self::TemporalExpanded => 3,
            Self::GraphExpanded => 4,
            Self::SemanticExpanded => 5,
        }
    }
}

/// A ranked candidate fact within the context pipeline.
///
/// Produced by `build_ranked_context_facts` in `ranking` and consumed by the
/// selection/budget tiers; the single record every tier reads and writes.
#[derive(Debug, Clone)]
pub(crate) struct RankedContextFact {
    pub(crate) fact: Fact,
    pub(crate) rationale: String,
    pub(crate) retrieval_tier: RetrievalTier,
    pub(crate) fusion_score: f64,
    pub(crate) source_priority: u8,
    pub(crate) decayed_confidence: f64,
    pub(crate) query_alignment_factor: f64,
    pub(crate) grounding_score: f64,
    pub(crate) semantic_available: bool,
    pub(crate) matched_query_terms: Vec<String>,
    pub(crate) graph_trace: Option<GraphTrace>,
}

impl RankedContextFact {
    /// Merges scoring-related fields into self using element-wise `max`.
    /// This is the common merge path used across all retrieval tiers.
    pub(crate) fn merge_scoring_fields(
        &mut self,
        confidence: f64,
        alignment: f64,
        grounding: f64,
        terms: &[String],
    ) {
        self.decayed_confidence = self.decayed_confidence.max(confidence);
        self.query_alignment_factor = self.query_alignment_factor.max(alignment);
        self.grounding_score = self.grounding_score.max(grounding);
        self.matched_query_terms.extend(terms.iter().cloned());
        self.matched_query_terms.sort();
        self.matched_query_terms.dedup();
    }
}
