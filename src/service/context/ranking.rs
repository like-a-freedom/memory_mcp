//! Ranking and MMR-based selection of context facts.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, hash_map::Entry};

use chrono::{DateTime, Utc};

use super::graph::GraphCandidate;
use super::lexical::{lexical_query_overlap_for_fact, lexical_query_score_for_fact};
use super::temporal::TemporalWindow;
use crate::models::{Fact, FactType};
use crate::service::normalize_text;
use crate::service::query::{
    query_hard_anchor_terms, query_term_should_be_soft_anchor, search_query_terms,
    unique_query_terms,
};

// ---------------------------------------------------------------------------
// Scoring and ranking constants
// ---------------------------------------------------------------------------
const RECIPROCAL_RANK_FUSION_K: f64 = 60.0;
const MAX_ITEMS_PER_SOURCE_EPISODE: usize = 2;
const ACCESS_COUNT_NOVELTY_WEIGHT: f64 = 0.08;
const EXPERIENCE_TYPE_RELEVANCE_BOOST: f64 = 1.12;
const FIRST_PERSON_USER_MEMORY_BOOST: f64 = 1.50;
const FIRST_PERSON_ASSISTANT_MEMORY_PENALTY: f64 = 0.65;
const DIRECT_RECALL_HEAD_LIMIT: usize = 3;
const DIRECT_RECALL_HEAD_MIN_RELEVANCE_RATIO: f64 = 0.75;
const MMR_RELEVANCE_WEIGHT: f64 = 0.80;
const REDUNDANCY_INDEX_KEY_WEIGHT: f64 = 0.70;
const REDUNDANCY_TEMPORAL_WEIGHT: f64 = 0.30;
const TEMPORAL_SIMILARITY_WINDOW_DAYS: f64 = 14.0;
const TEMPORAL_ALIGNMENT_WINDOW_DAYS: f64 = 30.0;
const MIN_TEMPORAL_ALIGNMENT_TO_FILL_BUDGET: f64 = 0.50;
const MIN_RANKED_CONFIDENCE: f64 = 0.01;
const MIN_QUERY_GROUNDING_RATIO: f64 = 0.25;
const TWO_HOP_GRAPH_WEIGHT: f64 = 0.72;
const DEEP_GRAPH_WEIGHT: f64 = 0.55;

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
    pub(crate) graph_trace: Option<crate::service::context::graph::GraphTrace>,
}

impl RankedContextFact {
    /// Merges scoring-related fields into self using element-wise `max`.
    /// This is the common merge path used across all retrieval tiers.
    fn merge_scoring_fields(
        &mut self,
        confidence: f64,
        alignment: f64,
        grounding: f64,
        terms: &[String],
    ) {
        self.decayed_confidence = self.decayed_confidence.max(confidence);
        self.query_alignment_factor = self.query_alignment_factor.max(alignment);
        self.grounding_score = self.grounding_score.max(grounding);
        merge_matched_query_terms(&mut self.matched_query_terms, terms);
    }
}

fn merge_matched_query_terms(existing: &mut Vec<String>, incoming: &[String]) {
    existing.extend(incoming.iter().cloned());
    existing.sort();
    existing.dedup();
}

fn merge_graph_trace(
    existing: &mut Option<crate::service::context::graph::GraphTrace>,
    incoming: Option<crate::service::context::graph::GraphTrace>,
) {
    let Some(incoming) = incoming else {
        return;
    };

    let should_replace = match existing.as_ref() {
        None => true,
        Some(current) => {
            incoming.hop_count < current.hop_count
                || (incoming.hop_count == current.hop_count
                    && incoming.anchor_entity_id < current.anchor_entity_id)
        }
    };

    if should_replace {
        *existing = Some(incoming);
    }
}

#[allow(dead_code)]
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

fn protected_direct_recall_fact_ids(
    selected: &[RankedContextFact],
    query_terms: &[String],
    temporal_focus: Option<&TemporalWindow>,
) -> HashSet<String> {
    if selected.len() <= 1 || query_terms.len() < 4 {
        return HashSet::new();
    }

    let max_relevance = selected
        .iter()
        .map(|fact| focused_ranked_relevance_score(fact, temporal_focus))
        .fold(0.0, f64::max);
    let min_relevance = max_relevance * DIRECT_RECALL_HEAD_MIN_RELEVANCE_RATIO;
    let mut protected = HashSet::new();

    for fact in selected {
        if protected.len() >= DIRECT_RECALL_HEAD_LIMIT {
            break;
        }
        if !is_protected_lexical_recall_tier(fact.retrieval_tier) {
            continue;
        }
        if matched_query_terms_for_fact(fact, query_terms).len() < 4 {
            continue;
        }
        if focused_ranked_relevance_score(fact, temporal_focus) + 1e-9 < min_relevance {
            continue;
        }

        protected.insert(fact.fact.fact_id.clone());
    }

    protected
}

fn is_protected_lexical_recall_tier(retrieval_tier: RetrievalTier) -> bool {
    matches!(
        retrieval_tier,
        RetrievalTier::Direct | RetrievalTier::EpisodeFallback
    )
}

fn content_identity_key(fact: &Fact) -> String {
    format!(
        "{}\u{001f}{}",
        fact.source_episode,
        normalize_text(&fact.content)
    )
}

fn should_replace_canonical_fact(existing: &Fact, incoming: &Fact) -> bool {
    if existing.fact_type != FactType::Experience.as_str()
        && incoming.fact_type == FactType::Experience.as_str()
    {
        return true;
    }

    incoming.ft_score > existing.ft_score
        || (incoming.ft_score == existing.ft_score && incoming.t_valid > existing.t_valid)
}

fn merge_ranked_duplicate(existing: &mut RankedContextFact, incoming: RankedContextFact) {
    let incoming_trace = incoming.graph_trace.clone();
    let incoming_terms = incoming.matched_query_terms.clone();
    let incoming_rationale = incoming.rationale.clone();
    let incoming_retrieval_tier = incoming.retrieval_tier;
    existing.fusion_score = existing.fusion_score.max(incoming.fusion_score);
    existing.source_priority = existing.source_priority.min(incoming.source_priority);
    existing.semantic_available = existing.semantic_available || incoming.semantic_available;
    existing.merge_scoring_fields(
        incoming.decayed_confidence,
        incoming.query_alignment_factor,
        incoming.grounding_score,
        &incoming_terms,
    );
    merge_graph_trace(&mut existing.graph_trace, incoming_trace);

    if incoming_retrieval_tier.precedence() > existing.retrieval_tier.precedence() {
        existing.retrieval_tier = incoming_retrieval_tier;
        existing.rationale = incoming_rationale;
    }

    if should_replace_canonical_fact(&existing.fact, &incoming.fact) {
        existing.fact = incoming.fact;
    }
}

pub(crate) struct BuildRankedContextFactsRequest<'a> {
    pub(crate) lexical_facts: Vec<(Fact, RetrievalTier)>,
    pub(crate) graph_facts: Vec<GraphCandidate>,
    pub(crate) community_facts: Vec<(Fact, String, f64)>,
    pub(crate) semantic_facts: Vec<(Fact, String)>,
    pub(crate) query_opt: Option<&'a str>,
    pub(crate) semantic_available: bool,
    pub(crate) scope: &'a str,
    pub(crate) cutoff: DateTime<Utc>,
}

pub(crate) fn build_ranked_context_facts(
    request: BuildRankedContextFactsRequest<'_>,
    decayed_fn: impl Fn(&Fact, DateTime<Utc>) -> f64,
) -> Vec<RankedContextFact> {
    let BuildRankedContextFactsRequest {
        lexical_facts,
        graph_facts,
        community_facts,
        semantic_facts,
        query_opt,
        semantic_available,
        scope,
        cutoff,
    } = request;

    let mut ranked_by_fact_id = HashMap::<String, RankedContextFact>::new();
    let query_alignment = |fact: &Fact| query_alignment_factor(query_opt, fact);
    let grounding = |fact: &Fact| query_grounding_score(query_opt, fact);
    let lexical_query_terms = query_opt.map(search_query_terms).unwrap_or_default();

    for (rank, (fact, retrieval_tier)) in lexical_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = decayed_fn(&fact, cutoff);
        let lexical_score = lexical_fusion_score(rank, &fact, &lexical_query_terms);
        let query_alignment_factor = query_alignment(&fact);
        let grounding_score = grounding(&fact);
        let matched_terms = matched_terms_for_fact(query_opt, &fact);
        ranked_by_fact_id
            .entry(fact_id)
            .and_modify(|candidate| {
                candidate.fusion_score += lexical_score;
                candidate.source_priority = 0;
                candidate.merge_scoring_fields(
                    confidence,
                    query_alignment_factor,
                    grounding_score,
                    &matched_terms,
                );
                if retrieval_tier.precedence() > candidate.retrieval_tier.precedence() {
                    candidate.retrieval_tier = retrieval_tier;
                    candidate.rationale = build_rationale(
                        retrieval_tier,
                        &fact,
                        candidate.decayed_confidence,
                        candidate.query_alignment_factor,
                        candidate.grounding_score,
                        semantic_available,
                        default_direct_rationale(query_opt, scope, cutoff),
                    );
                }
            })
            .or_insert_with(|| RankedContextFact {
                rationale: build_rationale(
                    retrieval_tier,
                    &fact,
                    confidence,
                    query_alignment_factor,
                    grounding_score,
                    semantic_available,
                    default_direct_rationale(query_opt, scope, cutoff),
                ),
                fact,
                retrieval_tier,
                fusion_score: lexical_score,
                source_priority: 0,
                decayed_confidence: confidence,
                query_alignment_factor,
                grounding_score,
                semantic_available,
                matched_query_terms: matched_terms,
                graph_trace: None,
            });
    }

    for (rank, candidate) in graph_facts.into_iter().enumerate() {
        let fact_id = candidate.fact.fact_id.clone();
        let confidence = decayed_fn(&candidate.fact, cutoff);
        let query_alignment_factor = query_alignment(&candidate.fact);
        let grounding_score = grounding(&candidate.fact);
        let matched_terms = matched_terms_for_fact(query_opt, &candidate.fact);
        let weighted_rank =
            graph_rank_weight(rank, candidate.trace.hop_count, candidate.origin_factor);
        if let Some(existing) = ranked_by_fact_id.get_mut(&fact_id) {
            existing.fusion_score += weighted_rank;
            existing.merge_scoring_fields(
                confidence,
                query_alignment_factor,
                grounding_score,
                &matched_terms,
            );
            merge_graph_trace(&mut existing.graph_trace, Some(candidate.trace));
            continue;
        }

        let rationale = build_rationale(
            RetrievalTier::GraphExpanded,
            &candidate.fact,
            confidence,
            query_alignment_factor,
            grounding_score,
            semantic_available,
            candidate.rationale,
        );

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                fact: candidate.fact,
                rationale,
                retrieval_tier: RetrievalTier::GraphExpanded,
                fusion_score: weighted_rank,
                source_priority: 1,
                decayed_confidence: confidence,
                query_alignment_factor,
                grounding_score,
                semantic_available,
                matched_query_terms: matched_terms,
                graph_trace: Some(candidate.trace),
            },
        );
    }

    for (rank, (fact, rationale, graph_origin_factor)) in community_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = decayed_fn(&fact, cutoff);
        let query_alignment_factor = query_alignment(&fact);
        let grounding_score = grounding(&fact);
        let matched_terms = matched_terms_for_fact(query_opt, &fact);
        let weighted_rank = reciprocal_rank(rank) * graph_origin_factor.clamp(0.0, 1.0);
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += weighted_rank;
            candidate.merge_scoring_fields(
                confidence,
                query_alignment_factor,
                grounding_score,
                &matched_terms,
            );
            continue;
        }

        let rationale = build_rationale(
            RetrievalTier::GraphExpanded,
            &fact,
            confidence,
            query_alignment_factor,
            grounding_score,
            semantic_available,
            rationale,
        );

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                fact,
                rationale,
                retrieval_tier: RetrievalTier::GraphExpanded,
                fusion_score: weighted_rank,
                source_priority: 1,
                decayed_confidence: confidence,
                query_alignment_factor,
                grounding_score,
                semantic_available,
                matched_query_terms: matched_terms,
                graph_trace: None,
            },
        );
    }

    for (rank, (fact, rationale)) in semantic_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = decayed_fn(&fact, cutoff);
        let query_alignment_factor = query_alignment(&fact);
        let grounding_score = grounding(&fact);
        let matched_terms = matched_terms_for_fact(query_opt, &fact);
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += reciprocal_rank(rank);
            candidate.merge_scoring_fields(
                confidence,
                query_alignment_factor,
                grounding_score,
                &matched_terms,
            );
            continue;
        }

        let rationale = build_rationale(
            RetrievalTier::SemanticExpanded,
            &fact,
            confidence,
            query_alignment_factor,
            grounding_score,
            semantic_available,
            rationale,
        );

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                fact,
                rationale,
                retrieval_tier: RetrievalTier::SemanticExpanded,
                fusion_score: reciprocal_rank(rank),
                source_priority: 2,
                decayed_confidence: confidence,
                query_alignment_factor,
                grounding_score,
                semantic_available,
                matched_query_terms: matched_terms,
                graph_trace: None,
            },
        );
    }

    let mut ranked_by_content = HashMap::<String, RankedContextFact>::new();
    for ranked in ranked_by_fact_id.into_values() {
        let key = content_identity_key(&ranked.fact);
        match ranked_by_content.entry(key) {
            Entry::Occupied(mut existing) => {
                merge_ranked_duplicate(existing.get_mut(), ranked);
            }
            Entry::Vacant(slot) => {
                slot.insert(ranked);
            }
        }
    }

    ranked_by_content.into_values().collect()
}

fn lexical_fusion_score(rank: usize, fact: &Fact, query_terms: &[String]) -> f64 {
    let dampened_ft_score = fact.ft_score.max(0.0).ln_1p();
    let lexical_query_score = lexical_query_score_for_fact(fact, query_terms) as f64;
    reciprocal_rank(rank) * (1.0 + dampened_ft_score + lexical_query_score)
}

fn reciprocal_rank(rank: usize) -> f64 {
    1.0 / (RECIPROCAL_RANK_FUSION_K + rank as f64 + 1.0)
}

fn graph_rank_weight(rank: usize, hop_count: usize, origin_factor: f64) -> f64 {
    let hop_weight = match hop_count {
        0 | 1 => 1.0,
        2 => TWO_HOP_GRAPH_WEIGHT,
        _ => DEEP_GRAPH_WEIGHT,
    };
    reciprocal_rank(rank) * hop_weight * origin_factor.clamp(0.0, 1.0)
}

pub(crate) fn default_direct_rationale(
    query_opt: Option<&str>,
    scope: &str,
    cutoff: DateTime<Utc>,
) -> String {
    query_opt.map_or_else(
        || {
            format!(
                "matched scope={scope} and active at {}",
                cutoff.date_naive()
            )
        },
        |query| {
            format!(
                "matched lexical query=\"{query}\" in scope={scope} and active at {}",
                cutoff.date_naive()
            )
        },
    )
}

pub(crate) fn default_episode_fallback_rationale(
    query_opt: Option<&str>,
    scope: &str,
    cutoff: DateTime<Utc>,
) -> String {
    query_opt.map_or_else(
        || {
            format!(
                "matched episode content in scope={scope} and active at {}",
                cutoff.date_naive()
            )
        },
        |query| {
            format!(
                "matched episode content query=\"{query}\" in scope={scope} and active at {}",
                cutoff.date_naive()
            )
        },
    )
}

fn build_rationale(
    retrieval_tier: RetrievalTier,
    fact: &Fact,
    confidence: f64,
    query_alignment_factor: f64,
    grounding_score: f64,
    semantic_available: bool,
    detail: String,
) -> String {
    format!(
        "tier={} fts={:.2} access_count={} confidence={:.2} relevance={:.2} grounding={:.2} alignment={:.2} semantic={} {detail}",
        retrieval_tier.as_str(),
        fact.ft_score.max(0.0),
        fact.access_count,
        confidence,
        confidence,
        grounding_score,
        query_alignment_factor,
        if semantic_available {
            "enabled"
        } else {
            "disabled"
        },
    )
}

fn matched_terms_for_fact(query_opt: Option<&str>, fact: &Fact) -> Vec<String> {
    let Some(query) = query_opt else {
        return Vec::new();
    };

    let query_terms = unique_query_terms(&search_query_terms(query));
    let fact_terms = fact_term_set(fact);
    query_terms
        .into_iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .collect()
}

fn novelty_factor(access_count: i64) -> f64 {
    let access_count = access_count.max(0) as f64;
    1.0 / (1.0 + access_count.ln_1p() * ACCESS_COUNT_NOVELTY_WEIGHT)
}

fn fact_type_relevance_factor(fact: &Fact) -> f64 {
    if fact.fact_type == FactType::Experience.as_str() {
        EXPERIENCE_TYPE_RELEVANCE_BOOST
    } else {
        1.0
    }
}

pub(crate) fn ranked_relevance_score(fact: &RankedContextFact) -> f64 {
    fact.fusion_score
        * fact.decayed_confidence.max(MIN_RANKED_CONFIDENCE)
        * novelty_factor(fact.fact.access_count)
        * fact_type_relevance_factor(&fact.fact)
        * fact.query_alignment_factor
}

pub(crate) fn normalized_relevance_score(fact: &RankedContextFact) -> f64 {
    let raw = ranked_relevance_score(fact).max(0.0);
    (raw / (1.0 + raw)).clamp(0.0, 1.0)
}

fn query_alignment_factor(query_opt: Option<&str>, fact: &Fact) -> f64 {
    lexical_query_alignment_factor(query_opt, fact)
        * first_person_memory_alignment_factor(query_opt, &fact.content)
}

fn lexical_query_alignment_factor(query_opt: Option<&str>, fact: &Fact) -> f64 {
    0.85 + (0.30 * query_grounding_score(query_opt, fact))
}

pub(crate) fn query_grounding_score(query_opt: Option<&str>, fact: &Fact) -> f64 {
    let Some(query) = query_opt else {
        return 1.0;
    };

    let query_terms = unique_query_terms(&search_query_terms(query));
    if query_terms.is_empty() {
        return 1.0;
    }

    let lexical_matches = lexical_query_overlap_for_fact(fact, &query_terms) as f64;
    let coverage = (lexical_matches / query_terms.len() as f64).clamp(0.0, 1.0);
    let lexical_score = lexical_query_score_for_fact(fact, &query_terms) as f64;
    let phrase_support =
        ((lexical_score - lexical_matches).max(0.0) / query_terms.len() as f64).clamp(0.0, 1.0);
    let anchor_terms = query_hard_anchor_terms(&query_terms);
    let anchor_hits = if anchor_terms.is_empty() {
        0.0
    } else {
        let fact_terms = fact_term_set(fact);
        (anchor_terms
            .iter()
            .filter(|term| fact_terms.contains(term.as_str()))
            .count() as f64
            / anchor_terms.len() as f64)
            .clamp(0.0, 1.0)
    };

    (0.55 * coverage + 0.20 * phrase_support + 0.25 * anchor_hits).clamp(0.0, 1.0)
}

fn first_person_memory_alignment_factor(query_opt: Option<&str>, content: &str) -> f64 {
    let Some(query) = query_opt else {
        return 1.0;
    };
    if !query_is_first_person_memory(query) {
        return 1.0;
    }

    let trimmed = content.trim_start();
    if trimmed.starts_with("User:") {
        FIRST_PERSON_USER_MEMORY_BOOST
    } else if trimmed.starts_with("Assistant:") {
        FIRST_PERSON_ASSISTANT_MEMORY_PENALTY
    } else {
        1.0
    }
}

pub(crate) fn query_is_first_person_memory(query: &str) -> bool {
    let normalized = normalize_text(query);
    let terms = normalized
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.trim().is_empty())
        .collect::<HashSet<_>>();

    terms.contains("i") || terms.contains("me") || terms.contains("my") || terms.contains("mine")
}

fn temporal_alignment_factor(fact_time: DateTime<Utc>, temporal_focus: &TemporalWindow) -> f64 {
    let distance_days = if fact_time < temporal_focus.start {
        (temporal_focus.start - fact_time).num_seconds().abs() as f64 / 86_400.0
    } else if fact_time > temporal_focus.end {
        (fact_time - temporal_focus.end).num_seconds().abs() as f64 / 86_400.0
    } else {
        0.0
    };

    if distance_days <= 0.0 {
        1.0
    } else {
        1.0 / (1.0 + distance_days / TEMPORAL_ALIGNMENT_WINDOW_DAYS)
    }
}

fn candidate_temporal_alignment(
    fact: &RankedContextFact,
    temporal_focus: Option<&TemporalWindow>,
) -> f64 {
    temporal_focus
        .map(|focus| temporal_alignment_factor(fact.fact.t_valid, focus))
        .unwrap_or(1.0)
}

fn fact_is_within_temporal_focus(
    fact: &RankedContextFact,
    temporal_focus: &TemporalWindow,
) -> bool {
    fact.fact.t_valid >= temporal_focus.start && fact.fact.t_valid <= temporal_focus.end
}

fn temporal_query_terms(query_terms: &[String]) -> Vec<String> {
    query_terms
        .iter()
        .filter(|term| is_temporal_query_term(term))
        .cloned()
        .collect()
}

fn is_temporal_query_term(term: &str) -> bool {
    matches!(
        term,
        "january"
            | "february"
            | "march"
            | "april"
            | "may"
            | "june"
            | "july"
            | "august"
            | "september"
            | "october"
            | "november"
            | "december"
            | "monday"
            | "tuesday"
            | "wednesday"
            | "thursday"
            | "friday"
            | "saturday"
            | "sunday"
            | "today"
            | "yesterday"
            | "tomorrow"
            | "week"
            | "quarter"
            | "q1"
            | "q2"
            | "q3"
            | "q4"
    ) || (term.len() == 4 && term.chars().all(|character| character.is_ascii_digit()))
}

fn fact_matches_all_query_terms(fact: &RankedContextFact, required_terms: &[String]) -> bool {
    if required_terms.is_empty() {
        return false;
    }

    let matched_terms = matched_query_terms_for_fact(fact, required_terms);
    required_terms
        .iter()
        .all(|term| matched_terms.contains(term.as_str()))
}

fn supports_explicit_temporal_focus(
    fact: &RankedContextFact,
    temporal_focus: &TemporalWindow,
    query_terms: &[String],
) -> bool {
    if fact_is_within_temporal_focus(fact, temporal_focus) {
        return true;
    }

    let required_temporal_terms = temporal_query_terms(query_terms);
    fact_matches_all_query_terms(fact, &required_temporal_terms)
}

fn focused_ranked_relevance_score(
    fact: &RankedContextFact,
    temporal_focus: Option<&TemporalWindow>,
) -> f64 {
    let temporal_factor = candidate_temporal_alignment(fact, temporal_focus);
    ranked_relevance_score(fact) * temporal_factor
}

fn compare_ranked_context_facts_with_focus(
    a: &RankedContextFact,
    b: &RankedContextFact,
    temporal_focus: Option<&TemporalWindow>,
) -> Ordering {
    let score_a = focused_ranked_relevance_score(a, temporal_focus);
    let score_b = focused_ranked_relevance_score(b, temporal_focus);
    score_b
        .total_cmp(&score_a)
        .then_with(|| a.source_priority.cmp(&b.source_priority))
        .then_with(|| b.fact.ft_score.total_cmp(&a.fact.ft_score))
        .then_with(|| b.fact.t_valid.cmp(&a.fact.t_valid))
        .then_with(|| a.fact.fact_id.cmp(&b.fact.fact_id))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn compare_ranked_context_facts(
    a: &RankedContextFact,
    b: &RankedContextFact,
) -> Ordering {
    compare_ranked_context_facts_with_focus(a, b, None)
}

fn source_episode_selection_cap(budget: usize) -> usize {
    MAX_ITEMS_PER_SOURCE_EPISODE.min(budget.max(1))
}

fn temporal_similarity(left: DateTime<Utc>, right: DateTime<Utc>) -> f64 {
    let diff_days = (left - right).num_seconds().abs() as f64 / 86_400.0;
    1.0 / (1.0 + diff_days / TEMPORAL_SIMILARITY_WINDOW_DAYS)
}

fn index_key_jaccard_similarity(left: &[String], right: &[String]) -> f64 {
    let left = left
        .iter()
        .map(|key| normalize_text(key))
        .filter(|key| !key.is_empty())
        .collect::<HashSet<_>>();
    let right = right
        .iter()
        .map(|key| normalize_text(key))
        .filter(|key| !key.is_empty())
        .collect::<HashSet<_>>();

    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let intersection = left.intersection(&right).count() as f64;
    let union = left.union(&right).count() as f64;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn matched_query_terms_for_fact(
    fact: &RankedContextFact,
    query_terms: &[String],
) -> HashSet<String> {
    if query_terms.is_empty() {
        return HashSet::new();
    }

    let fact_terms = fact_term_set(&fact.fact);

    query_terms
        .iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

fn fact_term_set(fact: &Fact) -> HashSet<String> {
    let mut fact_terms = search_query_terms(&fact.content)
        .into_iter()
        .collect::<HashSet<_>>();
    for index_key in &fact.index_keys {
        fact_terms.extend(search_query_terms(index_key));
    }
    fact_terms
}

fn derive_query_anchor_terms(
    facts: &[RankedContextFact],
    query_terms: &[String],
) -> HashSet<String> {
    let unique_terms = unique_query_terms(query_terms);
    if unique_terms.is_empty() {
        return HashSet::new();
    }

    let mut anchor_terms = query_hard_anchor_terms(&unique_terms);
    let mut doc_freq = HashMap::<String, usize>::new();
    let total_docs = facts.len().max(1);

    for fact in facts {
        let matched_terms = matched_query_terms_for_fact(fact, &unique_terms);
        for term in matched_terms {
            *doc_freq.entry(term).or_default() += 1;
        }
    }

    for term in unique_terms {
        let term_doc_freq = doc_freq.get(term.as_str()).copied().unwrap_or(0);
        if !anchor_terms.contains(term.as_str())
            && query_term_should_be_soft_anchor(&term, term_doc_freq, total_docs)
        {
            anchor_terms.insert(term);
        }
    }

    anchor_terms
}

fn anchor_term_hits_for_fact(fact: &RankedContextFact, anchor_terms: &HashSet<String>) -> usize {
    if anchor_terms.is_empty() {
        return 0;
    }

    let fact_terms = fact_term_set(&fact.fact);
    anchor_terms
        .iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .count()
}

fn anchor_support_factor(fact: &RankedContextFact, anchor_terms: &HashSet<String>) -> f64 {
    if anchor_terms.is_empty() {
        return 1.0;
    }

    let hits = anchor_term_hits_for_fact(fact, anchor_terms);
    if hits == 0 {
        0.75
    } else {
        1.0 + (hits as f64 / anchor_terms.len() as f64)
    }
}

fn anchor_adjusted_relevance_score(
    fact: &RankedContextFact,
    temporal_focus: Option<&TemporalWindow>,
    anchor_terms: &HashSet<String>,
) -> f64 {
    focused_ranked_relevance_score(fact, temporal_focus) * anchor_support_factor(fact, anchor_terms)
}

fn query_term_set_similarity(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let intersection = left.intersection(right).count() as f64;
    let union = left.union(right).count() as f64;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

pub(crate) fn prune_redundant_selected_facts(
    mut selected: Vec<RankedContextFact>,
    query_terms: &[String],
    temporal_focus: Option<&TemporalWindow>,
) -> Vec<RankedContextFact> {
    const REDUNDANT_SUPPORT_SIMILARITY: f64 = 0.40;
    if selected.len() <= 1 || query_terms.len() < 4 {
        return selected;
    }

    let protected_fact_ids =
        protected_direct_recall_fact_ids(&selected, query_terms, temporal_focus);

    loop {
        let matched_terms = selected
            .iter()
            .map(|fact| matched_query_terms_for_fact(fact, query_terms))
            .collect::<Vec<_>>();
        let mut term_frequency = HashMap::<String, usize>::new();
        for terms in &matched_terms {
            for term in terms {
                *term_frequency.entry(term.clone()).or_default() += 1;
            }
        }
        let informative_terms = matched_terms
            .iter()
            .map(|terms| {
                terms
                    .iter()
                    .filter(|term| {
                        term_frequency.get(term.as_str()).copied().unwrap_or(0) < selected.len()
                    })
                    .cloned()
                    .collect::<HashSet<_>>()
            })
            .collect::<Vec<_>>();

        let mut removal_idx = None;
        let mut removal_support_count = 0usize;
        let mut removal_score = f64::INFINITY;

        for idx in 0..selected.len() {
            if protected_fact_ids.contains(selected[idx].fact.fact_id.as_str()) {
                continue;
            }
            if informative_terms[idx].len() < 4 {
                continue;
            }

            let mut support_count = 0usize;
            let mut similarity_count = 0usize;

            for other_idx in 0..selected.len() {
                if idx == other_idx {
                    continue;
                }

                let similarity = query_term_set_similarity(
                    &informative_terms[idx],
                    &informative_terms[other_idx],
                );
                if similarity >= REDUNDANT_SUPPORT_SIMILARITY {
                    support_count += 1;
                }
                similarity_count += 1;
            }

            if support_count < 2 || similarity_count == 0 {
                continue;
            }

            let score = focused_ranked_relevance_score(&selected[idx], temporal_focus);
            let should_remove = removal_idx.is_none()
                || support_count > removal_support_count
                || (support_count == removal_support_count && score < removal_score);
            if should_remove {
                removal_idx = Some(idx);
                removal_support_count = support_count;
                removal_score = score;
            }
        }

        let Some(removal_idx) = removal_idx else {
            break;
        };
        selected.remove(removal_idx);
    }

    selected
}

fn candidate_redundancy(candidate: &RankedContextFact, selected: &RankedContextFact) -> f64 {
    let index_key_similarity =
        index_key_jaccard_similarity(&candidate.fact.index_keys, &selected.fact.index_keys);
    let temporal_overlap = temporal_similarity(candidate.fact.t_valid, selected.fact.t_valid);
    ((REDUNDANCY_INDEX_KEY_WEIGHT * index_key_similarity)
        + (REDUNDANCY_TEMPORAL_WEIGHT * temporal_overlap))
        .clamp(0.0, 1.0)
}

fn mmr_selection_score(
    candidate: &RankedContextFact,
    selected: &[RankedContextFact],
    max_relevance: f64,
    temporal_focus: Option<&TemporalWindow>,
    anchor_terms: &HashSet<String>,
) -> f64 {
    let relevance = (anchor_adjusted_relevance_score(candidate, temporal_focus, anchor_terms)
        / max_relevance.max(MIN_RANKED_CONFIDENCE))
    .clamp(0.0, 1.0);
    if selected.is_empty() {
        return relevance;
    }

    let redundancy = selected
        .iter()
        .map(|picked| candidate_redundancy(candidate, picked))
        .fold(0.0, f64::max);

    (MMR_RELEVANCE_WEIGHT * relevance) - ((1.0 - MMR_RELEVANCE_WEIGHT) * redundancy)
}

#[allow(clippy::too_many_arguments)]
fn seed_direct_recall_head(
    facts: &mut Vec<RankedContextFact>,
    selected: &mut Vec<RankedContextFact>,
    source_counts: &mut HashMap<String, usize>,
    budget: usize,
    per_source_episode_cap: usize,
    max_relevance: f64,
    temporal_focus: Option<&TemporalWindow>,
    query_terms: &[String],
    anchor_terms: &HashSet<String>,
) {
    if query_terms.len() < 4 || budget <= 1 {
        return;
    }

    let head_limit = DIRECT_RECALL_HEAD_LIMIT.min(budget);
    let min_relevance = max_relevance * DIRECT_RECALL_HEAD_MIN_RELEVANCE_RATIO;
    let mut selected_indices = Vec::new();

    for (idx, candidate) in facts.iter().enumerate() {
        if !is_protected_lexical_recall_tier(candidate.retrieval_tier) {
            continue;
        }

        let source_count = source_counts
            .get(candidate.fact.source_episode.as_str())
            .copied()
            .unwrap_or(0);
        if source_count >= per_source_episode_cap {
            continue;
        }

        let relevance = anchor_adjusted_relevance_score(candidate, temporal_focus, anchor_terms);
        if relevance + 1e-9 < min_relevance {
            continue;
        }

        selected_indices.push(idx);
        if selected_indices.len() >= head_limit {
            break;
        }
    }

    let mut seeded = Vec::with_capacity(selected_indices.len());
    for idx in selected_indices.into_iter().rev() {
        seeded.push(facts.remove(idx));
    }
    seeded.reverse();

    for chosen in seeded {
        *source_counts
            .entry(chosen.fact.source_episode.clone())
            .or_default() += 1;
        selected.push(chosen);
    }
}

pub(crate) fn select_ranked_context_facts(
    mut facts: Vec<RankedContextFact>,
    budget: usize,
    temporal_focus: Option<TemporalWindow>,
    query_terms: Vec<String>,
) -> Vec<RankedContextFact> {
    if facts.is_empty() || budget == 0 {
        return Vec::new();
    }

    let temporal_focus_ref = temporal_focus.as_ref();
    let anchor_terms = derive_query_anchor_terms(&facts, &query_terms);
    if let Some(temporal_focus) = temporal_focus_ref {
        facts.retain(|candidate| {
            supports_explicit_temporal_focus(candidate, temporal_focus, &query_terms)
        });

        if facts.is_empty() {
            return Vec::new();
        }
    }

    facts.sort_by(|left, right| {
        anchor_adjusted_relevance_score(right, temporal_focus_ref, &anchor_terms)
            .total_cmp(&anchor_adjusted_relevance_score(
                left,
                temporal_focus_ref,
                &anchor_terms,
            ))
            .then_with(|| compare_ranked_context_facts_with_focus(left, right, temporal_focus_ref))
    });

    let max_relevance = facts
        .first()
        .map(|fact| anchor_adjusted_relevance_score(fact, temporal_focus_ref, &anchor_terms))
        .unwrap_or(1.0)
        .max(MIN_RANKED_CONFIDENCE);
    let per_source_episode_cap = source_episode_selection_cap(budget);
    let mut source_counts = HashMap::<String, usize>::new();
    let mut selected = Vec::with_capacity(budget.min(facts.len()));

    seed_direct_recall_head(
        &mut facts,
        &mut selected,
        &mut source_counts,
        budget,
        per_source_episode_cap,
        max_relevance,
        temporal_focus_ref,
        &query_terms,
        &anchor_terms,
    );

    while selected.len() < budget && !facts.is_empty() {
        let enforce_temporal_alignment = temporal_focus_ref.is_some()
            && facts.iter().any(|candidate| {
                candidate_temporal_alignment(candidate, temporal_focus_ref)
                    >= MIN_TEMPORAL_ALIGNMENT_TO_FILL_BUDGET
            });
        let enforce_cap = facts.iter().any(|candidate| {
            source_counts
                .get(candidate.fact.source_episode.as_str())
                .copied()
                .unwrap_or(0)
                < per_source_episode_cap
        });

        let mut best_idx = None;
        let mut best_score = f64::NEG_INFINITY;
        let mut best_alignment = 1.0;
        for (idx, candidate) in facts.iter().enumerate() {
            let source_count = source_counts
                .get(candidate.fact.source_episode.as_str())
                .copied()
                .unwrap_or(0);
            if enforce_cap && source_count >= per_source_episode_cap {
                continue;
            }

            let temporal_alignment = candidate_temporal_alignment(candidate, temporal_focus_ref);
            if enforce_temporal_alignment
                && temporal_alignment < MIN_TEMPORAL_ALIGNMENT_TO_FILL_BUDGET
            {
                continue;
            }

            let score = mmr_selection_score(
                candidate,
                &selected,
                max_relevance,
                temporal_focus_ref,
                &anchor_terms,
            );
            let is_better = match best_idx {
                None => true,
                Some(_) if score > best_score + 1e-9 => true,
                Some(current_best_idx)
                    if (score - best_score).abs() <= 1e-9
                        && compare_ranked_context_facts_with_focus(
                            candidate,
                            &facts[current_best_idx],
                            temporal_focus_ref,
                        ) == Ordering::Less =>
                {
                    true
                }
                _ => false,
            };

            if is_better {
                best_idx = Some(idx);
                best_score = score;
                best_alignment = temporal_alignment;
            }
        }

        let Some(best_idx) = best_idx else {
            break;
        };
        if !selected.is_empty() && best_alignment < MIN_TEMPORAL_ALIGNMENT_TO_FILL_BUDGET {
            break;
        }
        let chosen = facts.remove(best_idx);
        *source_counts
            .entry(chosen.fact.source_episode.clone())
            .or_default() += 1;
        selected.push(chosen);
    }

    let selected = prune_redundant_selected_facts(selected, &query_terms, temporal_focus_ref);
    if !selected_results_meet_grounding_floor(&selected, &query_terms, &anchor_terms) {
        return Vec::new();
    }

    selected
}

fn selected_results_meet_grounding_floor(
    selected: &[RankedContextFact],
    query_terms: &[String],
    anchor_terms: &HashSet<String>,
) -> bool {
    if selected.is_empty() || query_terms.len() < 4 {
        return !selected.is_empty();
    }

    if selected.iter().any(|fact| {
        matches!(
            fact.retrieval_tier,
            RetrievalTier::GraphExpanded | RetrievalTier::SemanticExpanded
        )
    }) {
        return true;
    }

    let first_person_query = query_terms
        .iter()
        .any(|term| matches!(term.as_str(), "i" | "me" | "my" | "mine"));
    if first_person_query
        && selected
            .iter()
            .any(|fact| fact.fact.fact_type == FactType::Experience.as_str())
    {
        return true;
    }

    let mut matched_terms = HashSet::new();
    for fact in selected {
        matched_terms.extend(matched_query_terms_for_fact(fact, query_terms));
    }

    let coverage = matched_terms.len() as f64 / query_terms.len() as f64;
    if !anchor_terms.is_empty()
        && selected
            .iter()
            .any(|fact| anchor_term_hits_for_fact(fact, anchor_terms) > 0)
    {
        return true;
    }

    let min_terms = if query_terms.len() >= 8 { 3 } else { 2 };
    matched_terms.len() >= min_terms && coverage >= MIN_QUERY_GROUNDING_RATIO
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn sort_ranked_context_facts(facts: &mut [RankedContextFact]) {
    facts.sort_by(compare_ranked_context_facts);
}

pub(crate) fn sort_ranked_context_facts_for_timeline(facts: &mut [RankedContextFact]) {
    facts.sort_by(|a, b| {
        a.fact
            .t_valid
            .cmp(&b.fact.t_valid)
            .then_with(|| a.fact.fact_id.cmp(&b.fact.fact_id))
    });
}

pub(crate) fn apply_time_window(
    facts: &mut Vec<RankedContextFact>,
    window_start: Option<DateTime<Utc>>,
    window_end: Option<DateTime<Utc>>,
) {
    if window_start.is_none() && window_end.is_none() {
        return;
    }

    facts.retain(|ranked| {
        let after_start = window_start.is_none_or(|start| ranked.fact.t_valid >= start);
        let before_end = window_end.is_none_or(|end| ranked.fact.t_valid <= end);
        after_start && before_end
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_rank_weight_penalizes_deeper_matches() {
        let one_hop = graph_rank_weight(0, 1, 1.0);
        let two_hop = graph_rank_weight(0, 2, 1.0);
        let weak_one_hop = graph_rank_weight(3, 1, 0.5);

        assert!(one_hop > two_hop);
        assert!(one_hop > weak_one_hop);
    }

    // -----------------------------------------------------------------------
    // Tests relocated from context.rs — build_ranked_context_facts,
    // ranked_relevance_score, select_ranked_context_facts, and
    // prune_redundant_selected_facts scenario coverage.
    // -----------------------------------------------------------------------

    use crate::service::context::temporal::infer_temporal_window;

    fn create_test_fact(fact_id: &str, t_valid: DateTime<Utc>) -> Fact {
        Fact {
            fact_id: fact_id.to_string(),
            fact_type: "note".to_string(),
            content: "Test content".to_string(),
            quote: "Test quote".to_string(),
            source_episode: "episode:123".to_string(),
            t_valid,
            t_ingested: t_valid,
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: crate::models::Provenance::manual(),
            ft_score: 0.0,
        }
    }

    fn create_ranked_test_fact(
        fact_id: &str,
        source_episode: &str,
        t_valid: DateTime<Utc>,
        fusion_score: f64,
        ft_score: f64,
        access_count: i64,
        index_keys: &[&str],
    ) -> RankedContextFact {
        let mut fact = create_test_fact(fact_id, t_valid);
        fact.source_episode = source_episode.to_string();
        fact.ft_score = ft_score;
        fact.access_count = access_count;
        fact.index_keys = index_keys.iter().map(|key| (*key).to_string()).collect();

        RankedContextFact {
            fact,
            rationale: "test rationale".to_string(),
            retrieval_tier: RetrievalTier::Direct,
            fusion_score,
            source_priority: 0,
            decayed_confidence: 1.0,
            query_alignment_factor: 1.0,
            grounding_score: 1.0,
            semantic_available: false,
            matched_query_terms: Vec::new(),
            graph_trace: None,
        }
    }

    fn fixed_temporal_cutoff() -> DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-04-08T12:00:00Z")
            .expect("cutoff")
            .with_timezone(&Utc)
    }

    #[test]
    fn query_is_first_person_memory_recognizes_contractions() {
        assert!(query_is_first_person_memory(
            "I'm planning a weekend getaway and want something creatively fulfilling"
        ));
        assert!(query_is_first_person_memory(
            "I've decided to focus on original music projects"
        ));
        assert!(query_is_first_person_memory(
            "My reviews felt less authentic over time"
        ));
        assert!(!query_is_first_person_memory(
            "What activities feel creatively fulfilling for a music lover?"
        ));
    }

    #[test]
    fn build_ranked_context_facts_promotes_temporal_tier_over_direct() {
        let cutoff = Utc::now();
        let fact = create_test_fact("fact:temporal", cutoff - chrono::Duration::days(1));

        let ranked = build_ranked_context_facts(
            BuildRankedContextFactsRequest {
                lexical_facts: vec![
                    (fact.clone(), RetrievalTier::Direct),
                    (fact, RetrievalTier::TemporalExpanded),
                ],
                graph_facts: Vec::new(),
                community_facts: Vec::new(),
                semantic_facts: Vec::new(),
                query_opt: Some("march 2026 launch review"),
                semantic_available: false,
                scope: "org",
                cutoff,
            },
            crate::service::decayed_confidence,
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].retrieval_tier, RetrievalTier::TemporalExpanded);
        assert!(ranked[0].rationale.contains("tier=temporal"));
    }

    #[test]
    fn build_ranked_context_facts_weights_graph_results_by_origin_factor() {
        let cutoff = Utc::now();

        let mut inferred = create_test_fact("fact:inferred", cutoff - chrono::Duration::days(1));
        inferred.content = "Inferred fact content from beta community".to_string();
        let mut extracted = create_test_fact("fact:extracted", cutoff - chrono::Duration::days(1));
        extracted.content = "Extracted fact content from alpha community".to_string();

        let mut ranked = build_ranked_context_facts(
            BuildRankedContextFactsRequest {
                lexical_facts: Vec::new(),
                graph_facts: Vec::new(),
                community_facts: vec![
                    (
                        inferred,
                        "matched community summary via community:beta".to_string(),
                        0.2,
                    ),
                    (
                        extracted,
                        "matched community summary via community:alpha".to_string(),
                        1.0,
                    ),
                ],
                semantic_facts: Vec::new(),
                query_opt: Some("launch workstream"),
                semantic_available: false,
                scope: "org",
                cutoff,
            },
            crate::service::decayed_confidence,
        );
        sort_ranked_context_facts(&mut ranked);

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].fact.fact_id, "fact:extracted");
        assert_eq!(ranked[1].fact.fact_id, "fact:inferred");
    }

    #[test]
    fn ranked_relevance_score_softly_penalizes_frequently_accessed_facts() {
        let cutoff = Utc::now();
        let cold = create_ranked_test_fact(
            "fact:cold",
            "episode:cold",
            cutoff,
            10.0,
            5.0,
            0,
            &["alpha"],
        );
        let hot =
            create_ranked_test_fact("fact:hot", "episode:hot", cutoff, 10.0, 5.0, 50, &["alpha"]);

        assert!(ranked_relevance_score(&cold) > ranked_relevance_score(&hot));
    }

    #[test]
    fn ranked_relevance_score_prefers_experience_facts_when_other_signals_tie() {
        let cutoff = Utc::now();
        let mut note = create_ranked_test_fact(
            "fact:note",
            "episode:shared",
            cutoff,
            10.0,
            5.0,
            0,
            &["hotel", "quiet"],
        );
        note.fact.fact_type = "note".to_string();

        let mut experience = create_ranked_test_fact(
            "fact:experience",
            "episode:shared",
            cutoff,
            10.0,
            5.0,
            0,
            &["hotel", "quiet"],
        );
        experience.fact.fact_type = "experience".to_string();

        assert!(
            ranked_relevance_score(&experience) > ranked_relevance_score(&note),
            "experience memories should beat otherwise identical generic notes"
        );
    }

    #[test]
    fn build_ranked_context_facts_prefers_user_memories_for_first_person_queries() {
        let cutoff = Utc::now();

        let mut user_fact = create_test_fact("fact:user", cutoff);
        user_fact.content =
            "User: I was thrilled to hear modern beats blended with Pacific sounds live."
                .to_string();
        user_fact.quote = user_fact.content.clone();
        user_fact.ft_score = 4.0;

        let mut assistant_fact = create_test_fact("fact:assistant", cutoff);
        assistant_fact.content =
            "Assistant: It sounds like live music gives you a strong sense of cultural connection."
                .to_string();
        assistant_fact.quote = assistant_fact.content.clone();
        assistant_fact.ft_score = 4.0;

        let mut ranked = build_ranked_context_facts(
            BuildRankedContextFactsRequest {
                lexical_facts: vec![
                    (assistant_fact, RetrievalTier::Direct),
                    (user_fact, RetrievalTier::Direct),
                ],
                graph_facts: Vec::new(),
                community_facts: Vec::new(),
                semantic_facts: Vec::new(),
                query_opt: Some(
                    "I recently attended an event where there was a unique blend of modern beats with Pacific sounds.",
                ),
                semantic_available: false,
                scope: "org",
                cutoff,
            },
            crate::service::decayed_confidence,
        );
        sort_ranked_context_facts(&mut ranked);

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].fact.fact_id, "fact:user");
        assert_eq!(ranked[1].fact.fact_id, "fact:assistant");
    }

    #[test]
    fn select_ranked_context_facts_filters_out_of_window_candidates_without_temporal_support() {
        let temporal_focus =
            infer_temporal_window("july 2025", fixed_temporal_cutoff()).expect("temporal focus");
        let query_terms =
            crate::service::query::search_query_terms("platform planning notes july 2025");

        let july_candidate_time = chrono::DateTime::parse_from_rfc3339("2025-07-10T10:00:00Z")
            .expect("july candidate timestamp")
            .with_timezone(&Utc);
        let october_candidate_time = chrono::DateTime::parse_from_rfc3339("2025-10-13T10:00:00Z")
            .expect("october candidate timestamp")
            .with_timezone(&Utc);

        let july_candidate = RankedContextFact {
            fact: Fact {
                content: "Platform planning notes were finalized in July 2025.".to_string(),
                ..create_ranked_test_fact(
                    "fact:july",
                    "episode:july",
                    july_candidate_time,
                    2.0,
                    6.0,
                    0,
                    &[],
                )
                .fact
            },
            retrieval_tier: RetrievalTier::Direct,
            ..create_ranked_test_fact(
                "fact:july",
                "episode:july",
                july_candidate_time,
                2.0,
                6.0,
                0,
                &[],
            )
        };

        let october_semantic_candidate = RankedContextFact {
            fact: Fact {
                content: "October 2025 summary: Platform 2.3 patch release updates.".to_string(),
                ..create_ranked_test_fact(
                    "fact:october",
                    "episode:october",
                    october_candidate_time,
                    1.8,
                    5.0,
                    0,
                    &[],
                )
                .fact
            },
            retrieval_tier: RetrievalTier::SemanticExpanded,
            ..create_ranked_test_fact(
                "fact:october",
                "episode:october",
                october_candidate_time,
                1.8,
                5.0,
                0,
                &[],
            )
        };

        let selected = select_ranked_context_facts(
            vec![october_semantic_candidate, july_candidate],
            5,
            Some(temporal_focus),
            query_terms,
        );

        let fact_ids = selected
            .iter()
            .map(|fact| fact.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:july"]);
    }

    #[test]
    fn select_ranked_context_facts_caps_source_episode_before_budget_fill() {
        let cutoff = Utc::now();
        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:a1",
                    "episode:alpha",
                    cutoff,
                    12.0,
                    10.0,
                    0,
                    &["alpha", "shared"],
                ),
                create_ranked_test_fact(
                    "fact:a2",
                    "episode:alpha",
                    cutoff - chrono::Duration::days(1),
                    11.0,
                    9.0,
                    0,
                    &["alpha", "shared"],
                ),
                create_ranked_test_fact(
                    "fact:a3",
                    "episode:alpha",
                    cutoff - chrono::Duration::days(2),
                    10.5,
                    8.0,
                    0,
                    &["alpha", "shared"],
                ),
                create_ranked_test_fact(
                    "fact:b1",
                    "episode:beta",
                    cutoff - chrono::Duration::days(3),
                    9.5,
                    8.0,
                    0,
                    &["beta"],
                ),
                create_ranked_test_fact(
                    "fact:c1",
                    "episode:gamma",
                    cutoff - chrono::Duration::days(4),
                    9.0,
                    8.0,
                    0,
                    &["gamma"],
                ),
            ],
            4,
            None,
            vec![],
        );

        assert_eq!(selected.len(), 4);
        assert_eq!(
            selected
                .iter()
                .filter(|item| item.fact.source_episode == "episode:alpha")
                .count(),
            2
        );
        assert!(
            selected
                .iter()
                .any(|item| item.fact.source_episode == "episode:beta")
        );
        assert!(
            selected
                .iter()
                .any(|item| item.fact.source_episode == "episode:gamma")
        );
    }

    #[test]
    fn select_ranked_context_facts_prefers_novel_index_keys_when_scores_are_close() {
        let cutoff = Utc::now();
        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:anchor",
                    "episode:anchor",
                    cutoff,
                    10.0,
                    10.0,
                    0,
                    &["alpha", "beta"],
                ),
                create_ranked_test_fact(
                    "fact:redundant",
                    "episode:redundant",
                    cutoff - chrono::Duration::days(1),
                    9.9,
                    9.0,
                    0,
                    &["alpha", "beta"],
                ),
                create_ranked_test_fact(
                    "fact:diverse",
                    "episode:diverse",
                    cutoff - chrono::Duration::days(1),
                    9.7,
                    9.0,
                    0,
                    &["gamma", "delta"],
                ),
            ],
            2,
            None,
            vec![],
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:anchor", "fact:diverse"]);
    }

    #[test]
    fn select_ranked_context_facts_prefers_temporal_spread_for_tied_candidates() {
        let anchor_time = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
            .expect("anchor time")
            .with_timezone(&Utc);
        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:anchor",
                    "episode:anchor",
                    anchor_time,
                    10.0,
                    10.0,
                    0,
                    &[],
                ),
                create_ranked_test_fact(
                    "fact:nearby",
                    "episode:nearby",
                    anchor_time + chrono::Duration::days(1),
                    9.5,
                    9.0,
                    0,
                    &[],
                ),
                create_ranked_test_fact(
                    "fact:distant",
                    "episode:distant",
                    anchor_time + chrono::Duration::days(60),
                    9.5,
                    9.0,
                    0,
                    &[],
                ),
            ],
            2,
            None,
            vec![],
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:anchor", "fact:distant"]);
    }

    #[test]
    fn select_ranked_context_facts_prefers_in_window_items_over_stale_out_of_window_digests() {
        let anchor_time = chrono::DateTime::parse_from_rfc3339("2026-03-10T12:00:00Z")
            .expect("anchor time")
            .with_timezone(&Utc);
        let temporal_focus = infer_temporal_window(
            "march april 2026 alpha suite decisions",
            fixed_temporal_cutoff(),
        );

        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:stale-digest",
                    "episode:stale-digest",
                    chrono::DateTime::parse_from_rfc3339("2025-10-14T09:00:00Z")
                        .expect("stale time")
                        .with_timezone(&Utc),
                    12.0,
                    11.0,
                    0,
                    &["alpha", "suite", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:in-window",
                    "episode:in-window",
                    anchor_time,
                    10.5,
                    9.0,
                    0,
                    &["alpha", "suite", "decisions"],
                ),
            ],
            1,
            temporal_focus,
            vec![
                "march".to_string(),
                "april".to_string(),
                "2026".to_string(),
                "alpha".to_string(),
                "suite".to_string(),
                "decision".to_string(),
            ],
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:in-window"]);
    }

    #[test]
    fn select_ranked_context_facts_stops_before_budget_for_far_out_of_window_tail() {
        let temporal_focus = infer_temporal_window(
            "march april 2026 alpha suite delta control signal monitor orbit portal decisions",
            fixed_temporal_cutoff(),
        );
        let selected = select_ranked_context_facts(
            vec![
                create_ranked_test_fact(
                    "fact:alpha",
                    "episode:alpha",
                    chrono::DateTime::parse_from_rfc3339("2026-03-10T09:00:00Z")
                        .expect("alpha time")
                        .with_timezone(&Utc),
                    11.0,
                    10.0,
                    0,
                    &["alpha", "suite", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:delta",
                    "episode:delta",
                    chrono::DateTime::parse_from_rfc3339("2026-03-11T09:00:00Z")
                        .expect("delta time")
                        .with_timezone(&Utc),
                    10.5,
                    9.5,
                    0,
                    &["delta", "control", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:signal",
                    "episode:signal",
                    chrono::DateTime::parse_from_rfc3339("2026-04-02T09:00:00Z")
                        .expect("signal time")
                        .with_timezone(&Utc),
                    10.0,
                    9.0,
                    0,
                    &["signal", "monitor", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:orbit",
                    "episode:orbit",
                    chrono::DateTime::parse_from_rfc3339("2026-04-03T09:00:00Z")
                        .expect("orbit time")
                        .with_timezone(&Utc),
                    9.8,
                    9.0,
                    0,
                    &["orbit", "portal", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:stale-1",
                    "episode:stale-1",
                    chrono::DateTime::parse_from_rfc3339("2025-10-14T09:00:00Z")
                        .expect("stale 1 time")
                        .with_timezone(&Utc),
                    12.0,
                    11.0,
                    0,
                    &["alpha", "suite", "decisions"],
                ),
                create_ranked_test_fact(
                    "fact:stale-2",
                    "episode:stale-2",
                    chrono::DateTime::parse_from_rfc3339("2025-10-13T09:00:00Z")
                        .expect("stale 2 time")
                        .with_timezone(&Utc),
                    11.5,
                    10.5,
                    0,
                    &["orbit", "portal", "decisions"],
                ),
            ],
            6,
            temporal_focus,
            vec![
                "march".to_string(),
                "april".to_string(),
                "2026".to_string(),
                "alpha".to_string(),
                "suite".to_string(),
                "delta".to_string(),
                "control".to_string(),
                "signal".to_string(),
                "monitor".to_string(),
                "orbit".to_string(),
                "portal".to_string(),
                "decision".to_string(),
            ],
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            fact_ids,
            vec!["fact:alpha", "fact:delta", "fact:signal", "fact:orbit"]
        );
    }

    #[test]
    fn prune_redundant_selected_facts_removes_broad_umbrella_summaries() {
        let selected = prune_redundant_selected_facts(
            vec![
                // Specific facts first — highest relevance, they fill the protected set.
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:atlas",
                        "episode:atlas",
                        chrono::DateTime::parse_from_rfc3339("2026-03-15T09:00:00Z")
                            .expect("atlas time")
                            .with_timezone(&Utc),
                        10.0,
                        9.0,
                        0,
                        &[],
                    );
                    fact.fact.content = "March 2026 Atlas blocker: legal signoff is still missing for the reseller appendix.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:beacon",
                        "episode:beacon",
                        chrono::DateTime::parse_from_rfc3339("2026-03-16T09:00:00Z")
                            .expect("beacon time")
                            .with_timezone(&Utc),
                        9.9,
                        9.0,
                        0,
                        &[],
                    );
                    fact.fact.content =
                        "March 2026 Beacon blocker and decision: finance approved the revised launch budget after the blocker was resolved."
                            .to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:atlas-april",
                        "episode:atlas-april",
                        chrono::DateTime::parse_from_rfc3339("2026-04-05T09:00:00Z")
                            .expect("atlas april time")
                            .with_timezone(&Utc),
                        9.8,
                        9.0,
                        0,
                        &[],
                    );
                    fact.fact.content = "April 2026 Atlas decision: partner onboarding moved to the managed rollout path.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:beacon-april",
                        "episode:beacon-april",
                        chrono::DateTime::parse_from_rfc3339("2026-04-06T09:00:00Z")
                            .expect("beacon april time")
                            .with_timezone(&Utc),
                        9.7,
                        9.0,
                        0,
                        &[],
                    );
                    fact.fact.content = "April 2026 Beacon blocker: the migration depends on the final tax mapping table.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                // Broad umbrella summaries — lower scores, NOT protected.
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:digest-a",
                        "episode:digest-a",
                        chrono::DateTime::parse_from_rfc3339("2026-04-07T09:00:00Z")
                            .expect("digest a time")
                            .with_timezone(&Utc),
                        8.5,
                        7.5,
                        0,
                        &[],
                    );
                    fact.fact.content = "Quarterly digest for Atlas and Beacon repeated blockers and decisions keywords across March and April 2026 without resolving any specific item.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
                {
                    let mut fact = create_ranked_test_fact(
                        "fact:digest-b",
                        "episode:digest-b",
                        chrono::DateTime::parse_from_rfc3339("2026-04-07T10:00:00Z")
                            .expect("digest b time")
                            .with_timezone(&Utc),
                        8.3,
                        7.3,
                        0,
                        &[],
                    );
                    fact.fact.content = "Combined Atlas and Beacon digest covering March and April 2026: blocker updates, decision summaries, and launch progress across both workstreams.".to_string();
                    fact.fact.quote = fact.fact.content.clone();
                    fact
                },
            ],
            &[
                "march".to_string(),
                "april".to_string(),
                "2026".to_string(),
                "atlas".to_string(),
                "beacon".to_string(),
                "blocker".to_string(),
                "decision".to_string(),
            ],
            None,
        );

        let fact_ids = selected
            .iter()
            .map(|item| item.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            fact_ids,
            vec![
                "fact:atlas",
                "fact:beacon",
                "fact:atlas-april",
                "fact:beacon-april"
            ]
        );
    }
}
