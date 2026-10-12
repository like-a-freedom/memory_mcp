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
    SourceExpanded,
    GraphExpanded,
    SemanticExpanded,
    EpisodeFallback,
}

/// One fact returned by the semantic channel with its original ANN score.
#[derive(Debug, Clone)]
pub(crate) struct SemanticCandidate {
    pub(crate) fact: Fact,
    pub(crate) rationale: String,
    pub(crate) similarity: f64,
}

/// Evidence attached to one candidate identity and carried through fusion.
#[derive(Debug, Clone, Default)]
pub(crate) struct CandidateEvidence {
    pub(crate) reranking: Option<serde_json::Value>,
    /// The candidate passed admission for a query with one explicit calendar
    /// date before reranking. Relative dates and broad calendar periods do not
    /// override the reranker admission floor.
    pub(crate) explicit_date_admitted: bool,
    pub(crate) matched_content_terms: Vec<String>,
    pub(crate) matched_index_key_terms: Vec<String>,
    pub(crate) matched_references: Vec<String>,
    pub(crate) resolved_entity_ids: Vec<String>,
    pub(crate) native_full_text_match: bool,
    pub(crate) semantic_similarity: Option<f64>,
    pub(crate) graph_anchor_origin: Option<GraphAnchorOrigin>,
    pub(crate) hop_count: Option<usize>,
    pub(crate) community: Option<CommunityAdmissionEvidence>,
    pub(crate) repeated_topic: Option<RepeatedTopicAdmissionEvidence>,
    pub(crate) admission_basis: Option<AdmissionBasis>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommunityAdmissionEvidence {
    pub(crate) community_id: String,
    pub(crate) member_entity_id: String,
    pub(crate) summary_terms: Vec<String>,
    pub(crate) matched_summary_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepeatedTopicEvidence {
    pub(crate) term: String,
    pub(crate) supporting_fact_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepeatedTopicAdmissionEvidence {
    pub(crate) matched_query_index_terms: Vec<String>,
    pub(crate) matched_repeated_topics: Vec<String>,
    pub(crate) supporting_fact_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CommunityFactCandidate {
    pub(crate) fact: Fact,
    pub(crate) rationale: String,
    pub(crate) origin_factor: f64,
    pub(crate) evidence: CommunityAdmissionEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionBasis {
    Browse,
    CalendarPeriod,
    CalendarAcquisition,
    Lexical,
    Timeline,
    Reference,
    QualifiedSourceEpisode,
    Entity,
    Semantic,
    GraphRelationship,
    CommunitySummary,
    RepeatedTopic,
}

impl AdmissionBasis {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Browse => "browse",
            Self::CalendarPeriod => "calendar_period",
            Self::CalendarAcquisition => "calendar_acquisition",
            Self::Lexical => "lexical",
            Self::Timeline => "timeline",
            Self::Reference => "reference",
            Self::QualifiedSourceEpisode => "qualified_source_episode",
            Self::Entity => "entity",
            Self::Semantic => "semantic",
            Self::GraphRelationship => "graph_relationship",
            Self::CommunitySummary => "community_summary",
            Self::RepeatedTopic => "repeated_topic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GraphAnchorOrigin {
    QueryEntity { entity_id: String },
    AdmittedSeed { fact_id: String },
}

impl RetrievalTier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::AliasExpanded => "alias",
            Self::TemporalExpanded => "temporal",
            Self::SourceExpanded => "source",
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
            Self::SourceExpanded => 4,
            Self::GraphExpanded => 5,
            Self::SemanticExpanded => 6,
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
    pub(crate) evidence: CandidateEvidence,
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
