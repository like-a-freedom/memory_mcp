//! Ranking and MMR-based selection of context facts.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, hash_map::Entry};

use chrono::{DateTime, Utc};

use super::lexical::{lexical_query_overlap_for_fact, lexical_query_score_for_fact};
use crate::models::{Fact, FactType};
use crate::service::query::{
    query_hard_anchor_terms, query_term_should_be_soft_anchor, search_query_terms,
    unique_query_terms,
};

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
use super::temporal::TemporalWindow;
use crate::service::normalize_text;

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
    existing.fusion_score = existing.fusion_score.max(incoming.fusion_score);
    existing.decayed_confidence = existing.decayed_confidence.max(incoming.decayed_confidence);
    existing.source_priority = existing.source_priority.min(incoming.source_priority);
    existing.query_alignment_factor = existing
        .query_alignment_factor
        .max(incoming.query_alignment_factor);
    existing.grounding_score = existing.grounding_score.max(incoming.grounding_score);
    existing.semantic_available = existing.semantic_available || incoming.semantic_available;

    if incoming.retrieval_tier.precedence() > existing.retrieval_tier.precedence() {
        existing.retrieval_tier = incoming.retrieval_tier;
        existing.rationale = incoming.rationale.clone();
    }

    if should_replace_canonical_fact(&existing.fact, &incoming.fact) {
        existing.fact = incoming.fact;
    }
}

pub(crate) struct BuildRankedContextFactsRequest<'a> {
    pub(crate) lexical_facts: Vec<(Fact, RetrievalTier)>,
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
        ranked_by_fact_id
            .entry(fact_id)
            .and_modify(|candidate| {
                candidate.fusion_score += lexical_score;
                candidate.source_priority = 0;
                candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
                candidate.query_alignment_factor =
                    candidate.query_alignment_factor.max(query_alignment_factor);
                candidate.grounding_score = candidate.grounding_score.max(grounding_score);
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
            });
    }

    for (rank, (fact, rationale, graph_origin_factor)) in community_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = decayed_fn(&fact, cutoff);
        let query_alignment_factor = query_alignment(&fact);
        let grounding_score = grounding(&fact);
        let weighted_rank = reciprocal_rank(rank) * graph_origin_factor.clamp(0.0, 1.0);
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += weighted_rank;
            candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
            candidate.query_alignment_factor =
                candidate.query_alignment_factor.max(query_alignment_factor);
            candidate.grounding_score = candidate.grounding_score.max(grounding_score);
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
            },
        );
    }

    for (rank, (fact, rationale)) in semantic_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = decayed_fn(&fact, cutoff);
        let query_alignment_factor = query_alignment(&fact);
        let grounding_score = grounding(&fact);
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += reciprocal_rank(rank);
            candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
            candidate.query_alignment_factor =
                candidate.query_alignment_factor.max(query_alignment_factor);
            candidate.grounding_score = candidate.grounding_score.max(grounding_score);
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
