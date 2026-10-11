use std::collections::HashSet;

use chrono::{DateTime, Datelike, Utc};

use crate::models::AccessPayload;
use crate::shared::search::search_query_terms;

use super::filtering::{fact_allowed_by_access, fact_is_active_at};
use super::query_mode::QueryFlags;
use super::types::{AdmissionBasis, RankedContextFact};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionIntent {
    Browse,
    Content,
    Count,
    Entity,
    Personalization,
    Relationship,
    Current,
    Timeline,
    EventTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct QueryEvidence {
    pub(super) content_terms: Vec<String>,
    pub(super) subject_terms: Vec<String>,
    pub(super) explicit_calendar_date: Option<chrono::NaiveDate>,
    pub(super) calendar_period: Option<CalendarPeriod>,
    pub(super) qualified_references: Vec<String>,
    pub(super) resolved_entity_ids: Vec<String>,
    pub(super) repeated_topics: Vec<super::types::RepeatedTopicEvidence>,
    pub(super) mandatory_reference_alternatives: Vec<String>,
    pub(super) scope_reference: Option<String>,
    pub(super) intent: AdmissionIntent,
    pub(super) empty_query: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CalendarPeriod {
    start: chrono::NaiveDate,
    end_exclusive: chrono::NaiveDate,
}

impl CalendarPeriod {
    fn contains(self, date: chrono::NaiveDate) -> bool {
        self.start <= date && date < self.end_exclusive
    }

    fn contains_period(self, other: Self) -> bool {
        self.start <= other.start && other.end_exclusive <= self.end_exclusive
    }

    fn is_single_day(self) -> bool {
        self.end_exclusive == self.start + chrono::Duration::days(1)
    }
}

pub(super) struct AdmissionPolicy<'a> {
    pub(super) version: &'static str,
    pub(super) semantic_threshold: f64,
    pub(super) cutoff: DateTime<Utc>,
    pub(super) access: &'a AccessPayload,
}

impl<'a> AdmissionPolicy<'a> {
    pub(super) fn new(
        semantic_threshold: f64,
        cutoff: DateTime<Utc>,
        access: &'a AccessPayload,
    ) -> Self {
        Self {
            version: "retrieval-admission-v1",
            semantic_threshold,
            cutoff,
            access,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RejectReason {
    AccessOrTime,
    ReferenceConflict,
    InsufficientEvidence,
    NonFiniteScore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionDecision {
    Admit(AdmissionBasis),
    Reject(RejectReason),
}

pub(super) fn build_query_evidence(
    raw_query: Option<&str>,
    resolved_entity_ids: Vec<String>,
    mentioned_names: &[String],
    flags: &QueryFlags,
) -> QueryEvidence {
    let raw = raw_query.unwrap_or_default();
    let without_urls = strip_url_tokens(raw);
    let (cleaned_query, scope_reference) = strip_project_scope(&without_urls);
    let qualified_references = extract_qualified_references(&cleaned_query);
    let reference_terms = reference_component_terms(&cleaned_query);
    let explicit_calendar_date = parse_explicit_calendar_date(&cleaned_query);
    let calendar_period = parse_calendar_period(&cleaned_query);
    let intent = classify_intent(raw, flags, !resolved_entity_ids.is_empty());
    let empty_query = raw.trim().is_empty();
    let content_terms = if empty_query {
        Vec::new()
    } else {
        unique_terms(
            search_query_terms(&cleaned_query)
                .into_iter()
                .filter(|term| {
                    !reference_terms.contains(term)
                        && !calendar_period
                            .is_some_and(|period| is_calendar_period_component(term, period))
                        && !(intent == AdmissionIntent::Timeline && is_timeline_intent_cue(term))
                        && !(intent == AdmissionIntent::Current && is_current_intent_cue(term))
                        && !(intent == AdmissionIntent::Personalization
                            && is_personal_planning_cue(term))
                })
                .collect(),
        )
    };

    QueryEvidence {
        content_terms,
        subject_terms: unique_terms(
            mentioned_names
                .iter()
                .flat_map(|name| search_query_terms(name))
                .collect(),
        ),
        explicit_calendar_date,
        calendar_period,
        mandatory_reference_alternatives: qualified_references.clone(),
        qualified_references,
        resolved_entity_ids,
        repeated_topics: Vec::new(),
        scope_reference,
        intent,
        empty_query,
    }
}

fn is_current_intent_cue(term: &str) -> bool {
    matches!(term, "current" | "currently" | "latest" | "newest")
}

fn is_timeline_intent_cue(term: &str) -> bool {
    matches!(term, "timeline" | "history" | "chronology" | "sequence")
}

fn is_personal_planning_cue(term: &str) -> bool {
    search_query_terms("planning want recommend suggest something options ideas")
        .iter()
        .any(|cue| cue == term)
}

pub(super) fn evaluate_candidate(
    query: &QueryEvidence,
    candidate: &RankedContextFact,
    policy: &AdmissionPolicy<'_>,
) -> AdmissionDecision {
    debug_assert_eq!(policy.version, "retrieval-admission-v1");

    if !fact_is_active_at(&candidate.fact, policy.cutoff)
        || !fact_allowed_by_access(&candidate.fact, policy.access)
    {
        return AdmissionDecision::Reject(RejectReason::AccessOrTime);
    }

    if candidate
        .evidence
        .semantic_similarity
        .is_some_and(|score| !score.is_finite() || !(-1.0..=1.0).contains(&score))
        || (candidate.evidence.native_full_text_match && !candidate.fact.ft_score.is_finite())
    {
        return AdmissionDecision::Reject(RejectReason::NonFiniteScore);
    }

    if query.empty_query || query.intent == AdmissionIntent::Browse {
        return AdmissionDecision::Admit(AdmissionBasis::Browse);
    }

    if negative_project_membership_conflicts(query, &candidate.fact.content) {
        return AdmissionDecision::Reject(RejectReason::ReferenceConflict);
    }

    if query.intent == AdmissionIntent::EventTime
        && !has_topically_grounded_event_time(
            query,
            &candidate.fact.content,
            Some(candidate.fact.t_valid),
            candidate.evidence.semantic_similarity,
            policy.semantic_threshold,
        )
    {
        return AdmissionDecision::Reject(RejectReason::InsufficientEvidence);
    }

    if query.intent != AdmissionIntent::EventTime
        && query.calendar_period.is_some()
        && !has_calendar_period_and_topic_support(
            query,
            &candidate.fact.content,
            Some(candidate.fact.t_valid),
            candidate.evidence.semantic_similarity,
            policy.semantic_threshold,
        )
    {
        return AdmissionDecision::Reject(RejectReason::InsufficientEvidence);
    }

    if !query.mandatory_reference_alternatives.is_empty() {
        let candidate_references = candidate_references(&candidate.fact);
        if query
            .mandatory_reference_alternatives
            .iter()
            .any(|reference| candidate_references.contains(reference))
        {
            if query.content_terms.is_empty()
                || has_sufficient_candidate_support(query, candidate)
                || has_native_single_term_support(query, candidate)
            {
                return AdmissionDecision::Admit(AdmissionBasis::Reference);
            }
            if query
                .resolved_entity_ids
                .iter()
                .any(|entity_id| candidate.fact.entity_links.contains(entity_id))
            {
                return AdmissionDecision::Admit(AdmissionBasis::Reference);
            }
            return AdmissionDecision::Reject(RejectReason::InsufficientEvidence);
        }

        if query
            .mandatory_reference_alternatives
            .iter()
            .any(|expected| {
                candidate_references
                    .iter()
                    .any(|actual| reference_family(expected) == reference_family(actual))
            })
        {
            return AdmissionDecision::Reject(RejectReason::ReferenceConflict);
        }

        return AdmissionDecision::Reject(RejectReason::InsufficientEvidence);
    }

    if query.content_terms.is_empty() && query.calendar_period.is_some() {
        return AdmissionDecision::Admit(AdmissionBasis::CalendarPeriod);
    }

    if query.intent == AdmissionIntent::Timeline
        && query.calendar_period.is_some()
        && has_timeline_topic_support(query, candidate)
    {
        return AdmissionDecision::Admit(AdmissionBasis::Timeline);
    }

    if matches!(
        query.intent,
        AdmissionIntent::Entity | AdmissionIntent::Relationship
    ) && query
        .resolved_entity_ids
        .iter()
        .any(|entity_id| candidate.fact.entity_links.contains(entity_id))
    {
        return AdmissionDecision::Admit(AdmissionBasis::Entity);
    }

    if query.intent == AdmissionIntent::Relationship
        && candidate.evidence.graph_anchor_origin.is_some()
        && candidate.evidence.hop_count.is_some_and(|hops| hops <= 2)
    {
        return AdmissionDecision::Admit(AdmissionBasis::GraphRelationship);
    }

    if query.intent == AdmissionIntent::Entity
        && candidate.evidence.hop_count.is_some_and(|hops| hops <= 1)
        && candidate
            .evidence
            .graph_anchor_origin
            .as_ref()
            .is_some_and(|origin| {
                matches!(origin, super::types::GraphAnchorOrigin::QueryEntity { entity_id }
                if query.resolved_entity_ids.contains(entity_id))
            })
    {
        return AdmissionDecision::Admit(AdmissionBasis::GraphRelationship);
    }

    if !query.content_terms.is_empty() {
        if query.intent == AdmissionIntent::Personalization {
            let content = candidate.fact.content.trim_start().to_lowercase();
            let explicit_self_profile = content.starts_with("current user persona:")
                || content.starts_with("current user profile:");
            if explicit_self_profile {
                let terms = search_query_terms(&content);
                if query.content_terms.iter().any(|term| terms.contains(term)) {
                    return AdmissionDecision::Admit(AdmissionBasis::Lexical);
                }
            }
        }
        if has_sufficient_candidate_support(query, candidate)
            || has_native_single_term_support(query, candidate)
        {
            return AdmissionDecision::Admit(AdmissionBasis::Lexical);
        }
        if candidate
            .evidence
            .semantic_similarity
            .is_some_and(|score| score >= policy.semantic_threshold)
        {
            return AdmissionDecision::Admit(AdmissionBasis::Semantic);
        }
    }

    if matches!(
        query.intent,
        AdmissionIntent::Content | AdmissionIntent::Entity
    ) && candidate
        .evidence
        .community
        .as_ref()
        .is_some_and(|evidence| {
            candidate
                .fact
                .entity_links
                .contains(&evidence.member_entity_id)
                && has_sufficient_term_support(query, &evidence.summary_terms)
        })
    {
        return AdmissionDecision::Admit(AdmissionBasis::CommunitySummary);
    }

    if query.intent == AdmissionIntent::Content
        && repeated_topic_admission_evidence(query, &candidate.fact).is_some()
    {
        return AdmissionDecision::Admit(AdmissionBasis::RepeatedTopic);
    }

    AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
}

fn has_sufficient_candidate_support(query: &QueryEvidence, candidate: &RankedContextFact) -> bool {
    let content_terms = search_query_terms(&strip_url_tokens(&candidate.fact.content));
    let index_key_terms = candidate
        .fact
        .index_keys
        .iter()
        .flat_map(|key| search_query_terms(&strip_url_tokens(key)))
        .collect::<Vec<_>>();
    let matched_terms = query
        .content_terms
        .iter()
        .filter(|term| content_terms.contains(term) || index_key_terms.contains(term))
        .cloned()
        .collect::<Vec<_>>();
    has_sufficient_term_support(query, &matched_terms)
}

fn has_timeline_topic_support(query: &QueryEvidence, candidate: &RankedContextFact) -> bool {
    has_timeline_topic_support_for_text(query, &candidate.fact.content)
        || candidate
            .fact
            .index_keys
            .iter()
            .any(|key| has_timeline_topic_support_for_text(query, key))
}

fn has_timeline_topic_support_for_text(query: &QueryEvidence, content: &str) -> bool {
    let content_terms = search_query_terms(&strip_url_tokens(content))
        .into_iter()
        .collect::<HashSet<_>>();

    query.content_terms.iter().any(|term| {
        !is_timeline_narration_term(term)
            && !query
                .calendar_period
                .is_some_and(|period| is_calendar_period_component(term, period))
            && content_terms.contains(term)
    })
}

fn is_timeline_narration_term(term: &str) -> bool {
    matches!(
        term,
        "timeline"
            | "timelines"
            | "history"
            | "chronology"
            | "chronological"
            | "sequence"
            | "sequences"
            | "change"
            | "changes"
            | "changed"
            | "changing"
            | "update"
            | "updates"
            | "updated"
            | "updating"
            | "event"
            | "events"
            | "occur"
            | "occurred"
            | "happen"
            | "happened"
            | "after"
            | "before"
            | "by"
            | "during"
            | "from"
            | "over"
            | "since"
            | "through"
            | "until"
            | "within"
    )
}

fn has_calendar_period_and_topic_support(
    query: &QueryEvidence,
    content: &str,
    reference_time: Option<DateTime<Utc>>,
    semantic_similarity: Option<f64>,
    semantic_threshold: f64,
) -> bool {
    let Some(period) = query.calendar_period else {
        return true;
    };
    if query.content_terms.is_empty() {
        return candidate_matches_calendar_period(period, content, reference_time);
    }

    let sentences = content
        .split(['.', '!', '?', '\n'])
        .filter(|sentence| !sentence.trim().is_empty())
        .collect::<Vec<_>>();
    let has_explicit_calendar_period = sentences
        .iter()
        .any(|sentence| parse_calendar_period(sentence).is_some());
    let source_speaker_matches_query = content
        .split_once(':')
        .map(|(speaker, _)| search_query_terms(speaker))
        .is_some_and(|speaker_terms| {
            speaker_terms.len() == 1
                && (query.content_terms.contains(&speaker_terms[0])
                    || query.subject_terms.contains(&speaker_terms[0]))
        });

    sentences.into_iter().any(|sentence| {
        if query.intent == AdmissionIntent::Timeline {
            return sentence_refers_to_calendar_period(sentence, period, reference_time)
                && has_timeline_topic_support_for_text(query, sentence);
        }

        if let Some(content_period) = parse_calendar_period(sentence) {
            return period.contains_period(content_period)
                && (has_sufficient_content_support_for_text(query, sentence)
                    || has_semantic_calendar_topic_support(
                        query,
                        sentence,
                        source_speaker_matches_query,
                        semantic_similarity,
                        semantic_threshold,
                    ));
        }

        if has_explicit_calendar_period {
            return false;
        }

        sentence_refers_to_calendar_period(sentence, period, reference_time)
            && (has_sufficient_content_support_for_text(query, sentence)
                || has_semantic_calendar_topic_support(
                    query,
                    sentence,
                    source_speaker_matches_query,
                    semantic_similarity,
                    semantic_threshold,
                ))
    })
}

fn has_semantic_calendar_topic_support(
    query: &QueryEvidence,
    sentence: &str,
    source_speaker_matches_query: bool,
    semantic_similarity: Option<f64>,
    semantic_threshold: f64,
) -> bool {
    if !semantic_similarity.is_some_and(|score| score >= semantic_threshold) {
        return false;
    }

    let sentence_terms = search_query_terms(&strip_url_tokens(sentence));
    let has_lexical_topic_anchor = source_speaker_matches_query
        || query
            .content_terms
            .iter()
            .any(|term| sentence_terms.contains(term));
    const ACQUISITION_INTENT_TERMS: &[&str] = &[
        "acquire",
        "acquired",
        "acquiring",
        "buy",
        "bought",
        "buying",
        "get",
        "gets",
        "got",
        "obtain",
        "obtained",
        "obtaining",
        "purchase",
        "purchased",
        "purchasing",
    ];
    const ACQUISITION_EVIDENCE_TERMS: &[&str] = &[
        "acquire",
        "acquired",
        "acquiring",
        "buy",
        "bought",
        "buying",
        "obtain",
        "obtained",
        "obtaining",
        "purchase",
        "purchased",
        "purchasing",
    ];
    const SALE_TERMS: &[&str] = &["sale", "sell", "selling", "sells", "sold"];
    let query_mentions_acquisition = query
        .content_terms
        .iter()
        .any(|term| ACQUISITION_INTENT_TERMS.contains(&term.as_str()));
    let sentence_mentions_acquisition = sentence_terms
        .iter()
        .any(|term| ACQUISITION_EVIDENCE_TERMS.contains(&term.as_str()))
        || sentence_terms.windows(2).any(|pair| {
            matches!(pair[0].as_str(), "get" | "gets" | "got")
                && matches!(
                    pair[1].as_str(),
                    "a" | "an"
                        | "the"
                        | "my"
                        | "your"
                        | "his"
                        | "her"
                        | "our"
                        | "their"
                        | "this"
                        | "that"
                        | "these"
                        | "those"
                        | "some"
                        | "new"
                        | "another"
                )
        })
        || sentence_terms.windows(3).any(|triple| {
            matches!(triple[0].as_str(), "get" | "gets" | "got")
                && matches!(
                    triple[1].as_str(),
                    "myself" | "yourself" | "himself" | "herself" | "ourselves" | "themselves"
                )
                && matches!(
                    triple[2].as_str(),
                    "a" | "an" | "the" | "this" | "that" | "these" | "those" | "some" | "new"
                )
        });
    let query_mentions_sale = query
        .content_terms
        .iter()
        .any(|term| SALE_TERMS.contains(&term.as_str()));
    let sentence_mentions_sale = sentence_terms
        .iter()
        .any(|term| SALE_TERMS.contains(&term.as_str()));

    if query_mentions_acquisition {
        return sentence_mentions_acquisition;
    }
    if query_mentions_sale {
        return sentence_mentions_sale;
    }

    has_lexical_topic_anchor && has_explicit_event_time(sentence)
}

fn has_native_single_term_support(query: &QueryEvidence, candidate: &RankedContextFact) -> bool {
    query.content_terms.len() == 1 && candidate.evidence.native_full_text_match
}

fn candidate_references(fact: &crate::models::Fact) -> Vec<String> {
    let mut references = extract_qualified_references(&fact.content);
    references.extend(extract_qualified_references(&fact.source_episode));
    for index_key in &fact.index_keys {
        references.extend(extract_qualified_references(index_key));
    }
    unique_terms(references)
}

fn has_sufficient_content_support_for_text(query: &QueryEvidence, content: &str) -> bool {
    let candidate_terms = search_query_terms(&strip_url_tokens(content))
        .into_iter()
        .collect::<HashSet<_>>();
    let matched_terms = query
        .content_terms
        .iter()
        .filter(|term| candidate_terms.contains(*term))
        .cloned()
        .collect::<Vec<_>>();
    has_sufficient_term_support(query, &matched_terms)
}

fn has_sufficient_term_support(query: &QueryEvidence, matched_terms: &[String]) -> bool {
    if query.content_terms.is_empty() {
        return false;
    }

    let matched_terms = matched_terms.iter().collect::<HashSet<_>>();
    let matched_terms = query
        .content_terms
        .iter()
        .filter(|term| matched_terms.contains(term))
        .count();
    let coverage = matched_terms as f64 / query.content_terms.len() as f64;
    let minimum_hits = if query.content_terms.len() == 1 { 1 } else { 2 };
    matched_terms >= minimum_hits && coverage >= 0.25
}

fn repeated_topic_admission_evidence(
    query: &QueryEvidence,
    fact: &crate::models::Fact,
) -> Option<super::types::RepeatedTopicAdmissionEvidence> {
    if fact.fact_type != crate::models::FactType::Experience.as_str() {
        return None;
    }

    let index_terms = fact
        .index_keys
        .iter()
        .flat_map(|index_key| search_query_terms(index_key))
        .collect::<HashSet<_>>();
    let matched_query_index_terms = query
        .content_terms
        .iter()
        .filter(|term| index_terms.contains(*term))
        .cloned()
        .collect::<Vec<_>>();
    if matched_query_index_terms.is_empty() {
        return None;
    }

    let matched_topics = query
        .repeated_topics
        .iter()
        .filter(|topic| topic.supporting_fact_ids.len() >= 2 && index_terms.contains(&topic.term))
        .collect::<Vec<_>>();
    if matched_topics.is_empty() {
        return None;
    }

    let matched_repeated_topics = matched_topics
        .iter()
        .map(|topic| topic.term.clone())
        .collect::<Vec<_>>();
    let mut supporting_fact_ids = matched_topics
        .into_iter()
        .flat_map(|topic| topic.supporting_fact_ids.iter().cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    supporting_fact_ids.sort_unstable();
    if supporting_fact_ids.len() < 2 {
        return None;
    }

    Some(super::types::RepeatedTopicAdmissionEvidence {
        matched_query_index_terms,
        matched_repeated_topics,
        supporting_fact_ids,
    })
}

pub(super) fn episode_candidate_is_admitted(query: &QueryEvidence, content: &str) -> bool {
    episode_candidate_is_admitted_at_reference_time(query, content, None)
}

pub(super) fn episode_candidate_is_admitted_at_reference_time(
    query: &QueryEvidence,
    content: &str,
    reference_time: Option<DateTime<Utc>>,
) -> bool {
    if query.empty_query || query.intent == AdmissionIntent::Browse {
        return true;
    }

    if negative_project_membership_conflicts(query, content) {
        return false;
    }

    if !episode_candidate_passes_temporal_constraints(query, content, reference_time) {
        return false;
    }

    if !query.mandatory_reference_alternatives.is_empty() {
        if negative_project_membership_conflicts(query, content) {
            return false;
        }
        let candidate_references = extract_qualified_references(content);
        let exact_reference_matches = query
            .mandatory_reference_alternatives
            .iter()
            .any(|reference| candidate_references.contains(reference));
        return exact_reference_matches
            && (query.content_terms.is_empty()
                || has_sufficient_content_support_for_text(query, content));
    }

    has_sufficient_content_support_for_text(query, content)
}

pub(super) fn episode_candidate_is_rerankable_at_reference_time(
    query: &QueryEvidence,
    content: &str,
    reference_time: Option<DateTime<Utc>>,
) -> bool {
    if query.empty_query || query.intent == AdmissionIntent::Browse {
        return true;
    }

    if negative_project_membership_conflicts(query, content) {
        return false;
    }

    if !episode_candidate_passes_temporal_constraints(query, content, reference_time) {
        return false;
    }

    if !query.mandatory_reference_alternatives.is_empty() {
        return episode_candidate_is_admitted_at_reference_time(query, content, reference_time);
    }

    let candidate_terms = search_query_terms(&strip_url_tokens(content))
        .into_iter()
        .collect::<HashSet<_>>();
    query
        .content_terms
        .iter()
        .any(|term| candidate_terms.contains(term))
}

fn episode_candidate_passes_temporal_constraints(
    query: &QueryEvidence,
    content: &str,
    reference_time: Option<DateTime<Utc>>,
) -> bool {
    if query.intent == AdmissionIntent::EventTime
        && !has_topically_grounded_event_time(query, content, reference_time, None, 0.0)
    {
        return false;
    }

    if query.intent != AdmissionIntent::EventTime
        && query.calendar_period.is_some()
        && !has_calendar_period_and_topic_support(query, content, reference_time, None, 0.0)
    {
        return false;
    }

    true
}

pub(super) fn admit_ranked_candidates(
    query: &QueryEvidence,
    candidates: Vec<RankedContextFact>,
    policy: &AdmissionPolicy<'_>,
) -> Vec<RankedContextFact> {
    candidates
        .into_iter()
        .filter_map(|mut candidate| {
            if let AdmissionDecision::Admit(basis) = evaluate_candidate(query, &candidate, policy) {
                populate_candidate_evidence(query, &mut candidate);
                candidate.evidence.admission_basis = Some(basis);
                candidate.evidence.explicit_date_admitted = query.intent != AdmissionIntent::Browse
                    && query.explicit_calendar_date.is_some();
                Some(candidate)
            } else {
                None
            }
        })
        .collect()
}

fn populate_candidate_evidence(query: &QueryEvidence, candidate: &mut RankedContextFact) {
    let fact_terms = search_query_terms(&strip_url_tokens(&candidate.fact.content))
        .into_iter()
        .collect::<HashSet<_>>();
    candidate.evidence.matched_content_terms = query
        .content_terms
        .iter()
        .filter(|term| fact_terms.contains(*term))
        .cloned()
        .collect();

    let fact_references = candidate_references(&candidate.fact);
    candidate.evidence.matched_references = query
        .qualified_references
        .iter()
        .filter(|reference| fact_references.contains(reference))
        .cloned()
        .collect();

    let index_key_terms = candidate
        .fact
        .index_keys
        .iter()
        .flat_map(|key| search_query_terms(&strip_url_tokens(key)))
        .collect::<HashSet<_>>();
    candidate.evidence.matched_index_key_terms = query
        .content_terms
        .iter()
        .filter(|term| index_key_terms.contains(*term))
        .cloned()
        .collect();

    let fact_entities = candidate.fact.entity_links.iter().collect::<HashSet<_>>();
    candidate.evidence.resolved_entity_ids = query
        .resolved_entity_ids
        .iter()
        .filter(|entity_id| fact_entities.contains(entity_id))
        .cloned()
        .collect();

    if let Some(community) = &mut candidate.evidence.community {
        let summary_terms = community.summary_terms.iter().collect::<HashSet<_>>();
        community.matched_summary_terms = query
            .content_terms
            .iter()
            .filter(|term| summary_terms.contains(term))
            .cloned()
            .collect();
    }

    candidate.evidence.repeated_topic = repeated_topic_admission_evidence(query, &candidate.fact);
}

fn negative_project_membership_conflicts(query: &QueryEvidence, content: &str) -> bool {
    content
        .split(['.', '!', '?', '\n'])
        .filter(|sentence| !sentence.trim().is_empty())
        .any(|sentence| {
            let lower = sentence.to_lowercase();
            let Some((membership, excluded)) = lower.split_once(", not ") else {
                return false;
            };
            if !membership.contains("belongs to ") && !membership.contains("part of ") {
                return false;
            }
            let excluded_terms = search_query_terms(excluded);
            query
                .content_terms
                .iter()
                .filter(|term| excluded_terms.contains(term))
                .count()
                >= 2
        })
}

fn classify_intent(raw_query: &str, flags: &QueryFlags, has_entities: bool) -> AdmissionIntent {
    let normalized = raw_query.to_lowercase();
    let words = normalized.split_whitespace().collect::<Vec<_>>();

    if raw_query.trim().is_empty() {
        AdmissionIntent::Browse
    } else if asks_for_event_time(&normalized) {
        AdmissionIntent::EventTime
    } else if flags.wants_timeline {
        AdmissionIntent::Timeline
    } else if words.iter().any(|word| {
        matches!(
            word.trim_matches(|c: char| !c.is_alphanumeric()),
            "current" | "currently" | "latest" | "newest"
        )
    }) || normalized.contains("as of")
    {
        AdmissionIntent::Current
    } else if normalized.contains("how many")
        || normalized.contains("how much")
        || normalized.contains("count")
        || normalized.contains("number of")
        || normalized.contains("total number")
    {
        AdmissionIntent::Count
    } else if flags.wants_graph_path {
        AdmissionIntent::Relationship
    } else if flags.is_first_person_memory
        && words.iter().any(|word| {
            matches!(
                word.trim_matches(|c: char| !c.is_alphanumeric()),
                "planning" | "want" | "recommend" | "suggest"
            )
        })
    {
        AdmissionIntent::Personalization
    } else if has_entities
        && (search_query_terms(raw_query).len() <= 2
            || normalized.contains("what do you know about")
            || normalized.contains("tell me about")
            || normalized.contains("profile of")
            || normalized.contains("everything about"))
    {
        AdmissionIntent::Entity
    } else {
        AdmissionIntent::Content
    }
}

fn asks_for_event_time(normalized_query: &str) -> bool {
    let normalized_query =
        normalized_query.trim_start_matches(|character: char| !character.is_alphanumeric());
    normalized_query.starts_with("when ")
        || normalized_query.starts_with("what year ")
        || normalized_query.starts_with("what date ")
        || normalized_query.starts_with("what month ")
        || normalized_query.starts_with("which year ")
        || normalized_query.starts_with("which date ")
        || normalized_query.starts_with("how long ")
        || normalized_query.starts_with("on what date ")
        || ["years", "months", "weeks", "days", "hours", "minutes"]
            .iter()
            .any(|unit| normalized_query.starts_with(&format!("how many {unit} ")))
}

fn parse_explicit_calendar_date(raw: &str) -> Option<chrono::NaiveDate> {
    let tokens = raw
        .split(|character: char| !character.is_alphanumeric() && character != '-')
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();

    for (index, token) in tokens.iter().enumerate() {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(token, "%Y-%m-%d") {
            return Some(date);
        }
        let Some(month) = super::temporal::month_number(token) else {
            continue;
        };
        let day_after_month = tokens
            .get(index + 1)
            .and_then(|day| day.parse::<u32>().ok())
            .zip(
                tokens
                    .get(index + 2)
                    .and_then(|year| year.parse::<i32>().ok()),
            );
        let day_before_month = index
            .checked_sub(1)
            .and_then(|day_index| tokens.get(day_index))
            .and_then(|day| day.parse::<u32>().ok())
            .zip(
                tokens
                    .get(index + 1)
                    .and_then(|year| year.parse::<i32>().ok()),
            );
        let (day, year) = day_after_month.or(day_before_month)?;
        if (1900..=2200).contains(&year) {
            return chrono::NaiveDate::from_ymd_opt(year, month, day);
        }
    }
    None
}

fn parse_calendar_period(raw: &str) -> Option<CalendarPeriod> {
    use chrono::NaiveDate;

    if let Some(date) = parse_explicit_calendar_date(raw) {
        return Some(CalendarPeriod {
            start: date,
            end_exclusive: date.checked_add_days(chrono::Days::new(1))?,
        });
    }

    let tokens = raw
        .split(|character: char| !character.is_alphanumeric() && character != '-')
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    for window in tokens.windows(3) {
        let (Some(first_month), Some(second_month), Ok(year)) = (
            super::temporal::month_number(&window[0]),
            super::temporal::month_number(&window[1]),
            window[2].parse::<i32>(),
        ) else {
            continue;
        };
        if second_month != first_month + 1 || !(1900..=2200).contains(&year) {
            continue;
        }
        let (end_year, end_month) = if second_month == 12 {
            (year.checked_add(1)?, 1)
        } else {
            (year, second_month + 1)
        };
        return Some(CalendarPeriod {
            start: NaiveDate::from_ymd_opt(year, first_month, 1)?,
            end_exclusive: NaiveDate::from_ymd_opt(end_year, end_month, 1)?,
        });
    }
    for (index, token) in tokens.iter().enumerate() {
        if let Some(quarter) = quarter_number(token)
            && let Some(year) = tokens
                .get(index + 1)
                .and_then(|value| value.parse::<i32>().ok())
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|prior| tokens.get(prior))
                        .and_then(|value| value.parse::<i32>().ok())
                })
            && (1900..=2200).contains(&year)
        {
            let start_month = (quarter - 1) * 3 + 1;
            let start = NaiveDate::from_ymd_opt(year, start_month, 1)?;
            let end_month = start_month + 3;
            let (end_year, end_month) = if end_month > 12 {
                (year.checked_add(1)?, end_month - 12)
            } else {
                (year, end_month)
            };
            return Some(CalendarPeriod {
                start,
                end_exclusive: NaiveDate::from_ymd_opt(end_year, end_month, 1)?,
            });
        }

        if let Some(month) = super::temporal::month_number(token)
            && let Some(year) = tokens
                .get(index + 1)
                .and_then(|value| value.parse::<i32>().ok())
            && (1900..=2200).contains(&year)
        {
            let start = NaiveDate::from_ymd_opt(year, month, 1)?;
            let (end_year, end_month) = if month == 12 {
                (year.checked_add(1)?, 1)
            } else {
                (year, month + 1)
            };
            return Some(CalendarPeriod {
                start,
                end_exclusive: NaiveDate::from_ymd_opt(end_year, end_month, 1)?,
            });
        }
    }

    tokens.iter().find_map(|token| {
        let year = token.parse::<i32>().ok()?;
        if token.len() != 4 || !(1900..=2200).contains(&year) {
            return None;
        }
        Some(CalendarPeriod {
            start: NaiveDate::from_ymd_opt(year, 1, 1)?,
            end_exclusive: NaiveDate::from_ymd_opt(year.checked_add(1)?, 1, 1)?,
        })
    })
}

fn quarter_number(token: &str) -> Option<u32> {
    let normalized = token.to_ascii_lowercase();
    let quarter = normalized.strip_prefix('q')?.parse::<u32>().ok()?;
    (1..=4).contains(&quarter).then_some(quarter)
}

fn has_explicit_event_time(content: &str) -> bool {
    let tokens = content
        .split(|character: char| !character.is_alphanumeric() && character != '-')
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();

    if tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "yesterday" | "today" | "tomorrow" | "recently" | "ago" | "earlier" | "previously"
        ) || super::temporal::month_number(token).is_some()
            || matches!(
                token.as_str(),
                "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday"
            )
            || chrono::NaiveDate::parse_from_str(token, "%Y-%m-%d").is_ok()
            || (token.len() == 4
                && token
                    .parse::<i32>()
                    .is_ok_and(|year| (1900..=2200).contains(&year)))
    }) {
        return true;
    }

    for (index, token) in tokens.iter().enumerate() {
        let next = tokens.get(index + 1).map(String::as_str);
        let after_next = tokens.get(index + 2).map(String::as_str);
        if matches!(token.as_str(), "last" | "next" | "this")
            && next.is_some_and(|next| {
                matches!(
                    next,
                    "day"
                        | "week"
                        | "month"
                        | "year"
                        | "morning"
                        | "afternoon"
                        | "evening"
                        | "monday"
                        | "tuesday"
                        | "wednesday"
                        | "thursday"
                        | "friday"
                        | "saturday"
                        | "sunday"
                )
            })
        {
            return true;
        }
        if matches!(token.as_str(), "for" | "during" | "within")
            && next.is_some_and(|next| {
                matches!(
                    next,
                    "a" | "an"
                        | "one"
                        | "two"
                        | "three"
                        | "four"
                        | "five"
                        | "six"
                        | "seven"
                        | "eight"
                        | "nine"
                        | "ten"
                        | "several"
                        | "few"
                ) || next.chars().any(|character| character.is_ascii_digit())
            })
            && after_next.is_some_and(|unit| {
                matches!(
                    unit,
                    "day"
                        | "days"
                        | "week"
                        | "weeks"
                        | "month"
                        | "months"
                        | "year"
                        | "years"
                        | "hour"
                        | "hours"
                        | "minute"
                        | "minutes"
                )
            })
        {
            return true;
        }
        if token == "at" && next.is_some_and(|next| next == "age") && after_next.is_some() {
            return true;
        }
    }

    false
}

fn has_topically_grounded_event_time(
    query: &QueryEvidence,
    content: &str,
    reference_time: Option<DateTime<Utc>>,
    semantic_similarity: Option<f64>,
    semantic_threshold: f64,
) -> bool {
    content.split(['.', '!', '?', '\n']).any(|sentence| {
        let matches_requested_date = query.calendar_period.is_some_and(|period| {
            sentence_refers_to_calendar_period(sentence, period, reference_time)
        });
        let speaker_terms = sentence
            .split_once(':')
            .map(|(speaker, _)| search_query_terms(speaker))
            .filter(|terms| {
                terms.len() == 1
                    && (query.content_terms.contains(&terms[0])
                        || query.subject_terms.contains(&terms[0]))
            })
            .unwrap_or_default();
        if (query.calendar_period.is_some() && !matches_requested_date)
            || (query.calendar_period.is_none() && !has_explicit_event_time(sentence))
        {
            return false;
        }
        let topic_terms = query
            .content_terms
            .iter()
            .filter(|term| {
                !speaker_terms.contains(term)
                    && !is_event_time_query_cue(term)
                    && !query
                        .calendar_period
                        .is_some_and(|period| is_calendar_period_component(term, period))
            })
            .collect::<Vec<_>>();
        if topic_terms.is_empty() {
            return false;
        }

        let sentence_terms = search_query_terms(sentence)
            .into_iter()
            .collect::<HashSet<_>>();
        let matched_terms = topic_terms
            .iter()
            .filter(|term| sentence_terms.contains(term.as_str()))
            .count();
        if matches_requested_date
            && semantic_similarity.is_some_and(|score| score >= semantic_threshold)
            && matched_terms > 0
        {
            return true;
        }
        let minimum_hits = if topic_terms.len() == 1 { 1 } else { 2 };
        let coverage = matched_terms as f64 / topic_terms.len() as f64;
        if matched_terms >= minimum_hits && coverage >= 0.25 {
            return true;
        }

        !speaker_terms.is_empty()
            && has_anaphoric_event_reference(sentence)
            && topic_terms.iter().any(|query_term| {
                sentence_terms
                    .iter()
                    .any(|sentence_term| event_time_terms_match(query_term, sentence_term))
            })
    })
}

fn sentence_refers_to_calendar_period(
    sentence: &str,
    period: CalendarPeriod,
    reference_time: Option<DateTime<Utc>>,
) -> bool {
    if let Some(content_period) = parse_calendar_period(sentence) {
        return period.contains_period(content_period);
    }
    let tokens = sentence
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let Some(reference_date) = reference_time.map(|time| time.date_naive()) else {
        return false;
    };
    let relative_days = tokens
        .iter()
        .filter_map(|token| match token.as_str() {
            "yesterday" => Some(-1),
            "today" => Some(0),
            "tomorrow" => Some(1),
            _ => None,
        })
        .map(|offset| reference_date + chrono::Duration::days(offset))
        .collect::<HashSet<_>>();
    if relative_days.len() > 1 {
        return false;
    }
    let relative_week = relative_week_period(&tokens, reference_date);
    let has_unresolved_relative_period = tokens.windows(2).any(|pair| {
        matches!(pair[0].as_str(), "last" | "next" | "this")
            && matches!(
                pair[1].as_str(),
                "day"
                    | "week"
                    | "weekend"
                    | "month"
                    | "year"
                    | "monday"
                    | "tuesday"
                    | "wednesday"
                    | "thursday"
                    | "friday"
                    | "saturday"
                    | "sunday"
            )
            && pair[1] != "week"
    });
    if has_unresolved_relative_period
        || (relative_week.is_none()
            && tokens.windows(2).any(|pair| {
                matches!(pair[0].as_str(), "last" | "next" | "this") && pair[1] == "week"
            }))
    {
        return false;
    }

    match (relative_days.iter().next(), relative_week) {
        (Some(day), Some(week)) => week.contains(*day) && period.contains(*day),
        (Some(day), None) => period.contains(*day),
        (None, Some(week)) => period.contains_period(week),
        (None, None) => period.contains(reference_date),
    }
}

fn candidate_matches_calendar_period(
    period: CalendarPeriod,
    content: &str,
    reference_time: Option<DateTime<Utc>>,
) -> bool {
    content
        .split(['.', '!', '?', '\n'])
        .filter(|sentence| !sentence.trim().is_empty())
        .any(|sentence| sentence_refers_to_calendar_period(sentence, period, reference_time))
}

fn relative_week_period(
    tokens: &[String],
    reference_date: chrono::NaiveDate,
) -> Option<CalendarPeriod> {
    use chrono::Duration;

    let qualifiers = tokens
        .windows(2)
        .filter(|pair| matches!(pair[0].as_str(), "last" | "this" | "next") && pair[1] == "week")
        .map(|pair| pair[0].as_str())
        .collect::<Vec<_>>();
    let first_qualifier = *qualifiers.first()?;
    let current_week_start =
        reference_date - Duration::days(i64::from(reference_date.weekday().num_days_from_monday()));
    let period_for = |qualifier| {
        let offset = match qualifier {
            "last" => -7,
            "this" => 0,
            "next" => 7,
            _ => return None,
        };
        let start = current_week_start.checked_add_signed(Duration::days(offset))?;
        let end_exclusive = start.checked_add_signed(Duration::days(7))?;
        Some(CalendarPeriod {
            start,
            end_exclusive,
        })
    };
    let first_period = period_for(first_qualifier)?;
    qualifiers
        .iter()
        .all(|qualifier| period_for(qualifier) == Some(first_period))
        .then_some(first_period)
}

fn has_anaphoric_event_reference(sentence: &str) -> bool {
    sentence
        .split(|character: char| !character.is_alphanumeric())
        .any(|token| {
            matches!(
                token.to_lowercase().as_str(),
                "it" | "this" | "that" | "one"
            )
        })
}

fn is_event_time_query_cue(term: &str) -> bool {
    matches!(
        term,
        "first" | "last" | "year" | "date" | "month" | "week" | "day" | "hour" | "minute" | "long"
    )
}

fn is_calendar_period_component(term: &str, period: CalendarPeriod) -> bool {
    term == period.start.year().to_string()
        || super::temporal::month_number(term).is_some_and(|month| {
            [period.start.year(), period.start.year() + 1]
                .into_iter()
                .filter_map(|year| {
                    let first_day = chrono::NaiveDate::from_ymd_opt(year, month, 1)?;
                    let (next_year, next_month) = if month == 12 {
                        (year + 1, 1)
                    } else {
                        (year, month + 1)
                    };
                    let next_month = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
                    Some((first_day, next_month))
                })
                .any(|(first_day, next_month)| {
                    first_day < period.end_exclusive && next_month > period.start
                })
        })
        || quarter_number(term)
            .is_some_and(|quarter| ((period.start.month() - 1) / 3) + 1 == quarter)
        || (period.is_single_day() && term == period.start.day().to_string())
}

fn event_time_terms_match(query_term: &str, sentence_term: &str) -> bool {
    if query_term == sentence_term {
        return true;
    }

    let sentence_stem = sentence_term
        .strip_suffix("ed")
        .or_else(|| sentence_term.strip_suffix("ing"))
        .unwrap_or(sentence_term);
    sentence_stem == query_term || query_term.strip_suffix('e') == Some(sentence_stem)
}

fn strip_url_tokens(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|token| {
            let trimmed = token.trim_matches(|character: char| {
                matches!(
                    character,
                    '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
                )
            });
            let lowered = trimmed.to_ascii_lowercase();
            !lowered.starts_with("https://") && !lowered.starts_with("http://")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_project_scope(raw: &str) -> (String, Option<String>) {
    let words = raw.split_whitespace().collect::<Vec<_>>();
    let Some(project_index) = words.iter().position(|word| {
        clean_word(word).is_some_and(|cleaned| cleaned == "project" || cleaned == "projects")
    }) else {
        return (raw.to_string(), None);
    };

    let start = project_index.saturating_sub(8);
    let marker = (start..project_index).rev().find(|index| {
        clean_word(words[*index]).is_some_and(|word| word == "in" || word == "within")
    });
    let Some(marker_index) = marker else {
        return (raw.to_string(), None);
    };

    let scope_words = words[marker_index + 1..=project_index]
        .iter()
        .filter_map(|word| clean_word(word))
        .collect::<Vec<_>>();
    let mut kept_words = words;
    kept_words.drain(marker_index..=project_index);
    (kept_words.join(" "), Some(scope_words.join(" ")))
}

fn clean_word(raw: &str) -> Option<String> {
    let cleaned = raw
        .trim_matches(|character: char| !character.is_alphanumeric() && character != '_')
        .to_lowercase();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn extract_qualified_references(raw: &str) -> Vec<String> {
    let words = raw
        .split_whitespace()
        .filter_map(|word| {
            let cleaned = word
                .chars()
                .filter(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
                .collect::<String>();
            (!cleaned.is_empty()).then_some(cleaned)
        })
        .collect::<Vec<_>>();
    let mut references = Vec::new();

    for (index, word) in words.iter().enumerate() {
        if let Some(reference) = canonical_reference(word) {
            references.push(reference);
        }
        if looks_like_reference_prefix(word)
            && words
                .get(index + 1)
                .is_some_and(|next| next.chars().all(|character| character.is_ascii_digit()))
        {
            references.push(format!("{}{}", collapse_identifier(word), words[index + 1]));
        }
    }

    unique_terms(references)
}

fn canonical_reference(raw: &str) -> Option<String> {
    if chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").is_ok() {
        return None;
    }
    let has_separator = raw.chars().any(|character| matches!(character, '_' | '-'));
    let has_digit = raw.chars().any(|character| character.is_ascii_digit());
    let has_alpha = raw.chars().any(char::is_alphabetic);
    if !(has_separator && (has_digit || raw.contains('_')) || has_digit && has_alpha) {
        return None;
    }
    let collapsed = collapse_identifier(raw);
    (collapsed.len() >= 3).then_some(collapsed)
}

fn collapse_identifier(raw: &str) -> String {
    raw.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn reference_component_terms(raw: &str) -> HashSet<String> {
    let mut terms = HashSet::new();
    let words = raw.split_whitespace().collect::<Vec<_>>();
    for (index, word) in words.iter().enumerate() {
        let cleaned = word
            .chars()
            .filter(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
            .collect::<String>();
        if canonical_reference(&cleaned).is_some() {
            for part in cleaned.split(['_', '-']) {
                if !part.is_empty() {
                    terms.insert(part.to_lowercase());
                }
            }
        }
        if looks_like_reference_prefix(&cleaned)
            && words
                .get(index + 1)
                .is_some_and(|next| next.chars().all(|character| character.is_ascii_digit()))
        {
            terms.insert(cleaned.to_lowercase());
            terms.insert(words[index + 1].to_string());
        }
    }
    terms
}

fn reference_family(reference: &str) -> String {
    reference
        .chars()
        .take_while(|character| !character.is_ascii_digit())
        .collect()
}

fn looks_like_reference_prefix(word: &str) -> bool {
    if !word.chars().all(char::is_alphabetic) {
        return false;
    }
    let normalized = word.to_lowercase();
    if super::temporal::month_number(&normalized).is_some() {
        return false;
    }
    matches!(
        normalized.as_str(),
        "task" | "issue" | "ticket" | "req" | "requirement" | "wi" | "bug" | "case" | "pr"
    ) || word.chars().next().is_some_and(char::is_uppercase)
}

fn unique_terms(terms: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    terms
        .into_iter()
        .filter(|term| seen.insert(term.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::retrieval::query_mode::QueryFlags;
    use crate::memory::retrieval::types::{CandidateEvidence, RetrievalTier};
    use crate::models::{Fact, Provenance};

    fn candidate(content: &str, similarity: Option<f64>) -> RankedContextFact {
        let t_valid = Utc::now() - chrono::Duration::minutes(1);
        RankedContextFact {
            fact: Fact {
                fact_id: "fact:component".to_string(),
                fact_type: "note".to_string(),
                content: content.to_string(),
                quote: content.to_string(),
                source_episode: "episode:component".to_string(),
                t_valid,
                t_ingested: t_valid,
                t_invalid: None,
                t_invalid_ingested: None,
                confidence: 0.9,
                index_keys: Vec::new(),
                access_count: 0,
                last_accessed: None,
                entity_links: Vec::new(),
                scope: String::new(),
                policy_tags: Vec::new(),
                provenance: Provenance::manual(),
                ft_score: 0.0,
            },
            evidence: CandidateEvidence {
                semantic_similarity: similarity,
                ..CandidateEvidence::default()
            },
            rationale: String::new(),
            retrieval_tier: if similarity.is_some() {
                RetrievalTier::SemanticExpanded
            } else {
                RetrievalTier::Direct
            },
            fusion_score: 0.5,
            source_priority: 0,
            decayed_confidence: 0.9,
            query_alignment_factor: 0.9,
            grounding_score: 0.0,
            semantic_available: true,
            matched_query_terms: Vec::new(),
            graph_trace: None,
        }
    }

    fn query(raw: &str) -> QueryEvidence {
        build_query_evidence(Some(raw), Vec::new(), &[], &QueryFlags::default())
    }

    fn timeline_query(raw: &str) -> QueryEvidence {
        let flags = QueryFlags {
            wants_timeline: true,
            ..QueryFlags::default()
        };
        build_query_evidence(Some(raw), Vec::new(), &[], &flags)
    }

    #[test]
    fn calendar_month_and_day_are_not_mandatory_identifiers() {
        assert!(
            query("Which activity was James pursuing on March 16, 2022?")
                .mandatory_reference_alternatives
                .is_empty()
        );
    }

    #[test]
    fn exact_calendar_date_admits_semantic_topic_paraphrase_with_subject_anchor() {
        let query = query("Which recreational activity was James pursuing on March 16, 2022?");
        let mut fact = candidate(
            "James: By the way, yesterday I went bowling and got 2 strikes. I love bowling!",
            Some(0.8),
        );
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2022-03-17T15:47:00Z")
            .expect("valid source timestamp")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(
            0.7,
            chrono::DateTime::parse_from_rfc3339("2022-03-18T00:00:00Z")
                .expect("valid cutoff")
                .with_timezone(&Utc),
            &access,
        );

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(AdmissionBasis::Semantic)
        );
    }

    #[test]
    fn exact_calendar_date_semantic_match_does_not_cross_subjects() {
        let query = query("Which recreational activity was James pursuing on March 16, 2022?");
        let mut fact = candidate(
            "Bob: By the way, yesterday I went bowling and got 2 strikes. I love bowling!",
            Some(0.99),
        );
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2022-03-17T15:47:00Z")
            .expect("valid source timestamp")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(
            0.7,
            chrono::DateTime::parse_from_rfc3339("2022-03-18T00:00:00Z")
                .expect("valid cutoff")
                .with_timezone(&Utc),
            &access,
        );

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn exact_calendar_date_semantics_do_not_substitute_for_the_requested_action() {
        let query = query("What did Calvin buy on March 21, 2023?");
        let mut fact = candidate(
            "Calvin: On March 21, 2023, the music festival was my favorite.",
            Some(0.99),
        );
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-03-21T12:00:00Z")
            .expect("valid source timestamp")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(
            0.7,
            chrono::DateTime::parse_from_rfc3339("2023-03-22T00:00:00Z")
                .expect("valid cutoff")
                .with_timezone(&Utc),
            &access,
        );

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn calendar_purchase_query_rejects_get_to_experience_as_an_acquisition() {
        let query = query("What items did Calvin buy in March 2023?");
        let mut fact = candidate(
            "Calvin: Yeah, it's always a dream to see your favorites doing their thing! So glad you got to experience that.",
            Some(0.99),
        );
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-03-14T12:00:00Z")
            .expect("valid source timestamp")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(
            0.7,
            chrono::DateTime::parse_from_rfc3339("2023-04-01T00:00:00Z")
                .expect("valid cutoff")
                .with_timezone(&Utc),
            &access,
        );

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn calendar_purchase_query_accepts_direct_get_object_evidence() {
        let query = query("What items did Calvin buy in March 2023?");
        let mut fact = candidate("Calvin: I got a new car in March!", Some(0.99));
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-03-14T12:00:00Z")
            .expect("valid source timestamp")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(
            0.7,
            chrono::DateTime::parse_from_rfc3339("2023-04-01T00:00:00Z")
                .expect("valid cutoff")
                .with_timezone(&Utc),
            &access,
        );

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(AdmissionBasis::Semantic)
        );
    }

    #[test]
    fn calendar_month_and_year_are_not_mandatory_identifiers() {
        assert!(
            query("What items did Calvin buy in March 2023?")
                .mandatory_reference_alternatives
                .is_empty()
        );
    }

    #[test]
    fn quarter_period_is_not_mistaken_for_an_identifier() {
        let query = query("Q1 2026");

        assert!(query.mandatory_reference_alternatives.is_empty());
        assert_eq!(
            query.calendar_period,
            Some(CalendarPeriod {
                start: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid start"),
                end_exclusive: chrono::NaiveDate::from_ymd_opt(2026, 4, 1).expect("valid end"),
            })
        );
    }

    #[test]
    fn fourth_quarter_period_ends_at_the_next_year_boundary() {
        let query = query("Q4 2026");

        assert_eq!(
            query.calendar_period,
            Some(CalendarPeriod {
                start: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).expect("valid start"),
                end_exclusive: chrono::NaiveDate::from_ymd_opt(2027, 1, 1).expect("valid end"),
            })
        );
    }

    #[test]
    fn quarter_period_accepts_the_year_before_the_quarter() {
        let query = query("2026 Q2");

        assert_eq!(
            query.calendar_period,
            Some(CalendarPeriod {
                start: chrono::NaiveDate::from_ymd_opt(2026, 4, 1).expect("valid start"),
                end_exclusive: chrono::NaiveDate::from_ymd_opt(2026, 7, 1).expect("valid end"),
            })
        );
    }

    #[test]
    fn quarter_period_accepts_the_year_before_the_quarter_with_trailing_topic_terms() {
        let query = query("timeline of 2026 Q2 Atlas changes");

        assert_eq!(
            query.calendar_period,
            Some(CalendarPeriod {
                start: chrono::NaiveDate::from_ymd_opt(2026, 4, 1).expect("valid start"),
                end_exclusive: chrono::NaiveDate::from_ymd_opt(2026, 7, 1).expect("valid end"),
            })
        );
    }

    #[test]
    fn timeline_admits_a_topically_supported_fact_inside_the_requested_quarter() {
        let query = timeline_query("timeline of Atlas changes in Q1 2026");
        let mut fact = candidate("Atlas planning started.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-01-05T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert!(query.mandatory_reference_alternatives.is_empty());
        assert!(matches!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(_)
        ));
    }

    #[test]
    fn timeline_rejects_a_topically_supported_fact_outside_the_requested_quarter() {
        let query = timeline_query("timeline of Atlas changes in Q1 2026");
        let mut fact = candidate("Atlas planning started.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-04-01T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn timeline_rejects_an_unrelated_fact_inside_the_requested_quarter() {
        let query = timeline_query("timeline of Atlas changes in Q1 2026");
        let mut fact = candidate("Northwind planning started.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-01-05T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn timeline_rejects_an_unrelated_project_sharing_only_the_generic_change_term() {
        let query = timeline_query("timeline of Atlas changes in Q1 2026");
        let mut fact = candidate("Trackly changes were approved.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-01-05T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn timeline_rejects_a_cross_project_release_note_with_shared_temporal_language() {
        let query = timeline_query("Show the timeline of Atlas changes during Q1 2026.");
        let mut fact = candidate(
            "Trackly route editor release note, 31 March 2026: During Q1 2026, the import queue moved from an in-memory list to IndexedDB. The change shipped on 24 March.",
            Some(0.85),
        );
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-03-31T16:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn timeline_rejects_a_conflicting_quarter_even_when_the_content_ends_with_punctuation() {
        let query = timeline_query("timeline of Atlas changes in Q1 2026");
        let mut fact = candidate("Atlas release completed in Q2 2026.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-01-05T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn calendar_month_rejects_a_relative_event_outside_the_requested_month() {
        let query = query("What bicycle did Mara buy in April 2023?");
        let mut fact = candidate("Mara just bought a bicycle yesterday.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-04-01T12:00:00Z")
            .expect("valid fact reference time")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, fact.fact.t_valid, &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn calendar_month_admits_a_relative_event_inside_the_requested_month() {
        let query = query("What bicycle did Mara buy in March 2023?");
        let mut fact = candidate("Mara just bought a bicycle yesterday.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-04-01T12:00:00Z")
            .expect("valid fact reference time")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, fact.fact.t_valid, &access);

        assert!(matches!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(_)
        ));
    }

    #[test]
    fn calendar_month_rejects_an_event_described_as_last_week_outside_the_requested_month() {
        let query = query("What bicycle did Mara buy in April 2023?");
        let mut fact = candidate("Mara just bought a bicycle last week.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-04-01T12:00:00Z")
            .expect("valid fact reference time")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, fact.fact.t_valid, &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn calendar_month_admits_an_event_described_as_last_week_inside_the_requested_month() {
        let query = query("What bicycle did Mara buy in March 2023?");
        let mut fact = candidate("Mara just bought a bicycle last week.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-04-01T12:00:00Z")
            .expect("valid fact reference time")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, fact.fact.t_valid, &access);

        assert!(matches!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(_)
        ));
    }

    #[test]
    fn calendar_month_rejects_conflicting_relative_days() {
        let query = query("What bicycle did Mara buy in April 2023?");
        let mut fact = candidate("Mara bought a bicycle yesterday and today.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-04-01T12:00:00Z")
            .expect("valid fact reference time")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, fact.fact.t_valid, &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn iso_date_is_not_a_mandatory_identifier() {
        assert!(
            query("Which activity was James pursuing on 2022-03-16?")
                .mandatory_reference_alternatives
                .is_empty()
        );
    }

    fn evaluate(raw: &str, content: &str, similarity: Option<f64>) -> AdmissionDecision {
        let fact = candidate(content, similarity);
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);
        evaluate_candidate(&query(raw), &fact, &policy)
    }

    #[test]
    fn zero_grounding_paraphrase_is_admitted_by_semantic_evidence() {
        assert_eq!(
            evaluate(
                "How does the service find conceptually related recollections?",
                "HNSW vectors retrieve stored facts by cosine similarity.",
                Some(0.9),
            ),
            AdmissionDecision::Admit(AdmissionBasis::Semantic)
        );
    }

    #[test]
    fn task_reference_conflict_beats_high_semantic_similarity() {
        assert_eq!(
            evaluate(
                "Task 16 licenses",
                "Task 17 tracks sixteen active licenses.",
                Some(0.99),
            ),
            AdmissionDecision::Reject(RejectReason::ReferenceConflict)
        );
    }

    #[test]
    fn explicit_negative_project_membership_beats_a_matching_task_number() {
        assert_eq!(
            evaluate(
                "HTTP bounded memory Task 16 diagnostics",
                "Task 16 validates touch gestures. It belongs to Trackly, not Memory MCP HTTP.",
                Some(0.99)
            ),
            AdmissionDecision::Reject(RejectReason::ReferenceConflict)
        );
    }

    #[test]
    fn explicit_negative_project_membership_excludes_unqualified_content_queries() {
        assert_eq!(
            evaluate(
                "Which body measurements and diagnostics does the HTTP bounded memory plan require?",
                "Task 16 checks KML point import and touch gestures. It belongs to Trackly, not Memory MCP HTTP.",
                Some(0.99)
            ),
            AdmissionDecision::Reject(RejectReason::ReferenceConflict)
        );
    }

    #[test]
    fn a_qualified_identifier_needs_topical_support_when_query_has_topic_terms() {
        assert_eq!(
            evaluate(
                "Task 16 in the Trackly mobile route editor plan",
                "Memory MCP HTTP bounded memory Task 16 checks body preflight diagnostics and refusal metrics.",
                Some(0.99)
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn event_time_question_rejects_a_topically_related_fact_without_event_time() {
        assert_eq!(
            evaluate(
                "When did Caroline go to the LGBTQ support group?",
                "Caroline chose an adoption agency because it helps LGBTQ+ people. Its inclusivity and support spoke to her.",
                Some(0.99)
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn event_time_question_rejects_a_different_topically_related_event_with_a_date() {
        assert_eq!(
            evaluate(
                "When did Caroline go to the LGBTQ support group?",
                "Melanie: Hey Caroline, since we last chatted, I ran a charity race for mental health last Saturday.",
                Some(0.99)
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn event_time_must_share_a_sentence_with_topical_evidence() {
        assert_eq!(
            evaluate(
                "When did Caroline go to the LGBTQ support group?",
                "Caroline went to an LGBTQ support group. She ran a charity race last Saturday.",
                Some(0.99)
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn event_time_rejects_a_different_event_even_when_the_queried_person_said_it() {
        assert_eq!(
            evaluate(
                "When did Caroline go to the LGBTQ support group?",
                "Caroline: I ran a charity race for mental health last Saturday.",
                Some(0.99)
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn calendar_period_qualifier_does_not_change_a_content_question_to_event_time() {
        let query = query("What items did Calvin buy in March 2023?");
        assert_eq!(query.intent, AdmissionIntent::Content);
        assert!(query.content_terms.iter().any(|term| term == "buy"));
        assert!(!query.content_terms.iter().any(|term| term == "2023"));
    }

    #[test]
    fn month_year_content_query_admits_a_paraphrased_acquisition_in_that_period() {
        let query = query("What items did Calvin buy in March 2023?");
        let mut fact = candidate("Calvin: I just got a new car.", Some(0.99));
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-03-26T12:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(AdmissionBasis::Semantic)
        );
    }

    #[test]
    fn calendar_period_only_query_admits_facts_whose_valid_time_is_in_period() {
        let query = query("March 2026");
        let mut fact = candidate("Quarterly launch review finalized.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-03-15T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(AdmissionBasis::CalendarPeriod)
        );
    }

    #[test]
    fn calendar_period_only_query_rejects_facts_outside_the_period() {
        let query = query("March 2026");
        let mut fact = candidate("Quarterly launch review finalized.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-04-15T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn calendar_period_and_topical_support_must_come_from_the_same_sentence() {
        let query = query("Atlas release in March 2026");
        let mut fact = candidate(
            "Atlas release happened in April 2026. Trackly release happened in March 2026.",
            None,
        );
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-04-15T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn episode_fallback_rejects_a_matching_topic_with_an_explicit_conflicting_period() {
        let query = query("Atlas cache updates in March 2026");
        let recorded_in_march = chrono::DateTime::parse_from_rfc3339("2026-03-15T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);

        assert!(!episode_candidate_is_admitted_at_reference_time(
            &query,
            "Atlas cache updates completed in April 2026.",
            Some(recorded_in_march),
        ));
    }

    #[test]
    fn episode_fallback_keeps_an_explicit_matching_event_date_after_later_ingestion() {
        let query = query("Atlas cache updates in March 2026");
        let recorded_in_april = chrono::DateTime::parse_from_rfc3339("2026-04-15T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);

        assert!(episode_candidate_is_admitted_at_reference_time(
            &query,
            "Atlas cache updates completed in March 2026.",
            Some(recorded_in_april),
        ));
    }

    #[test]
    fn relative_week_is_resolved_inside_a_month_year_query() {
        let query = query("What engineering project did Jolene finish in January 2023?");
        let mut fact = candidate(
            "Jolene: I finished an electrical engineering project last week.",
            Some(0.99),
        );
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-01-23T12:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(AdmissionBasis::Lexical)
        );
    }

    #[test]
    fn a_relative_week_does_not_establish_an_exact_calendar_date() {
        let query = query("What engineering project did Jolene finish on January 20, 2023?");
        let mut fact = candidate(
            "Jolene: I finished an electrical engineering project last week.",
            Some(0.99),
        );
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-01-23T12:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn explicit_date_and_matching_speaker_do_not_admit_an_unrelated_event() {
        let query = query("What did Calvin buy on March 21, 2023?");
        let mut fact = candidate("Calvin: The music festival was my favorite.", Some(0.99));
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-03-21T12:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn explicit_date_and_matching_subject_do_not_admit_the_opposite_transaction() {
        let query = query("What did Mara sell on March 31, 2023?");
        let mut fact = candidate("Mara: I bought a bicycle yesterday.", Some(0.99));
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2023-04-01T12:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn event_time_admits_an_elliptical_answer_with_a_matching_speaker_and_action() {
        assert_eq!(
            evaluate(
                "When did Morgan donate the family vehicle to a local charity?",
                "Morgan: I donated it yesterday.",
                Some(0.99)
            ),
            AdmissionDecision::Admit(AdmissionBasis::Semantic)
        );
    }

    #[test]
    fn event_time_question_admits_a_fact_with_an_explicit_event_time() {
        assert_eq!(
            evaluate(
                "When did Caroline go to the LGBTQ support group?",
                "Caroline went to an LGBTQ support group yesterday.",
                Some(0.99)
            ),
            AdmissionDecision::Admit(AdmissionBasis::Lexical)
        );
    }

    #[test]
    fn explicit_calendar_date_question_admits_relative_event_time_at_the_matching_reference_date() {
        let query = query("When did James go bowling on March 16, 2022?");
        assert_eq!(query.intent, AdmissionIntent::EventTime);
        let mut fact = candidate("James: Yesterday I went bowling.", Some(0.99));
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2022-03-17T12:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(AdmissionBasis::Lexical)
        );
    }

    #[test]
    fn explicit_calendar_date_question_rejects_relative_event_resolved_to_another_day() {
        let query = query("When did James go bowling on March 16, 2022?");
        let mut fact = candidate("James: Yesterday I went bowling.", Some(0.99));
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2022-03-18T12:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn event_time_episode_fallback_rejects_a_different_dated_event() {
        let evidence = query("When did Joanna first watch Eternal Sunshine of the Spotless Mind?");
        assert!(!episode_candidate_is_admitted(
            &evidence,
            "Joanna: Woo! I finally finished my first full screenplay and printed it last Tuesday."
        ));
    }

    #[test]
    fn event_time_episode_fallback_admits_the_directly_supported_event() {
        let evidence = query("When did Joanna first watch Eternal Sunshine of the Spotless Mind?");
        assert!(episode_candidate_is_admitted(
            &evidence,
            "Joanna: I first watched Eternal Sunshine of the Spotless Mind last Tuesday."
        ));
    }

    #[test]
    fn negative_project_membership_does_not_exclude_the_positive_project() {
        assert_eq!(
            evaluate(
                "Trackly Task 16 touch gestures",
                "Task 16 validates touch gestures. It belongs to Trackly, not Memory MCP HTTP.",
                Some(0.99)
            ),
            AdmissionDecision::Admit(AdmissionBasis::Reference)
        );
    }

    #[test]
    fn negative_logging_rule_is_not_negative_project_membership() {
        assert_eq!(
            evaluate(
                "HTTP bounded memory Task 16 diagnostics",
                "Task 16 HTTP memory diagnostics must not log raw request content.",
                Some(0.99)
            ),
            AdmissionDecision::Admit(AdmissionBasis::Reference)
        );
    }

    #[test]
    fn missing_specific_reference_is_not_admitted() {
        assert_eq!(
            evaluate(
                "Task 99999999 licenses",
                "The memory service manages customer licenses.",
                Some(0.99),
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn exact_technical_identifier_survives() {
        assert_eq!(
            evaluate(
                "memory_mcp",
                "The memory_mcp binary starts the service.",
                None
            ),
            AdmissionDecision::Admit(AdmissionBasis::Reference)
        );
    }

    #[test]
    fn exact_task_reference_alone_does_not_supply_topical_support() {
        assert_eq!(
            evaluate("Task 16 licensing", "Task 16 is important.", None),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
        assert_eq!(
            evaluate("Task 16 licenses", "Task 16 grants licenses.", None),
            AdmissionDecision::Admit(AdmissionBasis::Reference)
        );
    }

    #[test]
    fn url_scheme_alone_is_not_content_evidence() {
        assert_eq!(
            evaluate(
                "https://docs.example.com/guide",
                "https://docs.example.com/guide",
                None
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn short_exact_query_is_admitted_without_global_bypass() {
        assert_eq!(
            evaluate("tracing", "Tracing records stage durations.", None),
            AdmissionDecision::Admit(AdmissionBasis::Lexical)
        );
        assert_eq!(
            evaluate("tracing", "The team uses a memory server.", None),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn native_full_text_match_admits_a_single_stemmed_term() {
        let mut gazprom = candidate("Иван работает в Газпроме.", None);
        gazprom.evidence.native_full_text_match = true;
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query("Газпром"), &gazprom, &policy),
            AdmissionDecision::Admit(AdmissionBasis::Lexical)
        );
    }

    #[test]
    fn exact_index_key_terms_are_lexical_admission_evidence() {
        let mut person = candidate("Quarterly launch review finalized.", None);
        person.fact.index_keys = vec!["Alice Smith".to_string()];
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query("Alice Smith"), &person, &policy),
            AdmissionDecision::Admit(AdmissionBasis::Lexical)
        );
    }

    #[test]
    fn exact_source_episode_reference_matches_candidate_identity() {
        let mut episode_fact = candidate(
            "Checklist entry: cert rotation scheduled, token refresh in progress, ports 5223 and 443 open",
            None,
        );
        episode_fact.fact.source_episode = "episode:fts_test_2".to_string();
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(
                &query("mobile certs tokens ports episode:fts_test_2"),
                &episode_fact,
                &policy,
            ),
            AdmissionDecision::Admit(AdmissionBasis::Reference)
        );
    }

    #[test]
    fn multi_term_lexical_admission_requires_at_least_two_content_hits() {
        assert_eq!(
            evaluate(
                "tracing database recovery policy",
                "Tracing records spans.",
                None,
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
        assert_eq!(
            evaluate(
                "tracing database recovery policy",
                "Tracing recovery emits spans.",
                None,
            ),
            AdmissionDecision::Admit(AdmissionBasis::Lexical)
        );
    }

    #[test]
    fn bare_count_number_is_not_promoted_to_a_task_reference() {
        let evidence = query("How many 16 licenses are active?");
        assert_eq!(evidence.intent, AdmissionIntent::Count);
        assert!(evidence.mandatory_reference_alternatives.is_empty());
    }

    #[test]
    fn project_alias_is_not_used_as_content_support() {
        assert_eq!(
            evaluate(
                "In the Memory MCP project, why does tracing fail?",
                "Tracing fails when the HNSW index is unavailable.",
                None,
            ),
            AdmissionDecision::Admit(AdmissionBasis::Lexical)
        );
        assert_eq!(
            evaluate(
                "In the Memory MCP project, why does tracing fail?",
                "The Memory MCP project maintains glossary records.",
                None,
            ),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn empty_query_preserves_browse_behavior() {
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);
        assert_eq!(
            evaluate_candidate(&query("  "), &candidate("Any stored fact.", None), &policy),
            AdmissionDecision::Admit(AdmissionBasis::Browse)
        );
    }

    #[test]
    fn unicode_qualified_reference_requires_an_exact_target() {
        assert_eq!(
            evaluate("Задача-42 доступ", "Задача-42: доступ включён.", None),
            AdmissionDecision::Admit(AdmissionBasis::Reference)
        );
        assert_eq!(
            evaluate("Задача-42 доступ", "Задача-43: доступ включён.", Some(0.99)),
            AdmissionDecision::Reject(RejectReason::ReferenceConflict)
        );
    }

    #[test]
    fn stale_and_denied_candidates_are_rejected_before_relevance() {
        let mut stale = candidate("tracing records stage durations", Some(0.99));
        stale.fact.t_valid = Utc::now() + chrono::Duration::days(1);
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);
        assert_eq!(
            evaluate_candidate(&query("tracing"), &stale, &policy),
            AdmissionDecision::Reject(RejectReason::AccessOrTime)
        );

        let mut denied = candidate("tracing records stage durations", Some(0.99));
        denied.fact.policy_tags = vec!["secret".to_string()];
        let restricted = AccessPayload {
            allowed_tags: Some(vec!["public".to_string()]),
            ..AccessPayload::default()
        };
        let restricted_policy = AdmissionPolicy::new(0.7, Utc::now(), &restricted);
        assert_eq!(
            evaluate_candidate(&query("tracing"), &denied, &restricted_policy),
            AdmissionDecision::Reject(RejectReason::AccessOrTime)
        );
    }

    #[test]
    fn graph_expansion_alone_is_not_admission_evidence() {
        let mut fact = candidate("An unrelated stored note.", None);
        fact.retrieval_tier = RetrievalTier::GraphExpanded;
        fact.graph_trace = Some(crate::memory::retrieval::graph::GraphTrace {
            anchor_entity_id: "entity:tracing".to_string(),
            anchor_origin: crate::memory::retrieval::types::GraphAnchorOrigin::QueryEntity {
                entity_id: "entity:tracing".to_string(),
            },
            anchor_canonical_name: "Tracing".to_string(),
            hop_count: 1,
            path: vec!["entity:tracing".to_string(), "entity:other".to_string()],
        });
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);
        assert_eq!(
            evaluate_candidate(&query("tracing reliability"), &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn non_finite_semantic_scores_are_never_admitted() {
        assert_eq!(
            evaluate(
                "tracing reliability",
                "A relevant tracing note.",
                Some(f64::NAN)
            ),
            AdmissionDecision::Reject(RejectReason::NonFiniteScore)
        );
    }

    #[test]
    fn entity_match_does_not_admit_an_unrelated_attribute_question() {
        let mut fact = candidate("Mara is a product manager for the launch.", None);
        fact.fact.entity_links = vec!["entity:mara".into()];
        let evidence = build_query_evidence(
            Some("What is Mara's favourite cuisine?"),
            vec!["entity:mara".into()],
            &[],
            &QueryFlags::default(),
        );
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);
        assert_eq!(
            evaluate_candidate(&evidence, &fact, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn resolved_entity_link_is_retained_as_candidate_evidence() {
        let mut linked = candidate("A product manager owns the launch plan.", None);
        linked.fact.entity_links = vec!["entity:ana".to_string()];
        let query = build_query_evidence(
            Some("Ana"),
            vec!["entity:ana".to_string()],
            &[],
            &QueryFlags::default(),
        );
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);
        assert_eq!(
            evaluate_candidate(&query, &linked, &policy),
            AdmissionDecision::Admit(AdmissionBasis::Entity)
        );

        let admitted = admit_ranked_candidates(&query, vec![linked], &policy);
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].evidence.resolved_entity_ids, vec!["entity:ana"]);
        assert_eq!(
            admitted[0].evidence.admission_basis,
            Some(AdmissionBasis::Entity)
        );
    }

    #[test]
    fn relationship_intent_accepts_graph_evidence_with_known_anchor_origin() {
        let query = build_query_evidence(
            Some("What is the connection path between Alice and Atlas?"),
            Vec::new(),
            &[],
            &QueryFlags {
                wants_graph_path: true,
                ..QueryFlags::default()
            },
        );
        assert_eq!(query.intent, AdmissionIntent::Relationship);

        let mut graph_candidate = candidate("An unrelated stored note.", None);
        graph_candidate.evidence.graph_anchor_origin = Some(
            crate::memory::retrieval::types::GraphAnchorOrigin::QueryEntity {
                entity_id: "entity:alice".to_string(),
            },
        );
        graph_candidate.evidence.hop_count = Some(2);
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query, &graph_candidate, &policy),
            AdmissionDecision::Admit(AdmissionBasis::GraphRelationship)
        );
    }

    #[test]
    fn graph_seed_limit_is_32_and_keeps_stable_admitted_order() {
        let query = query("alpha beta context");
        let mut facts = vec![candidate("alpha only", None).fact];
        for index in 1..=40 {
            let mut fact = candidate(&format!("alpha beta context evidence {index}"), None).fact;
            fact.fact_id = format!("fact:seed-{index:02}");
            facts.push(fact);
        }

        let selected = crate::memory::retrieval::graph::admitted_graph_seed_facts(&query, &facts);

        assert_eq!(selected.len(), 32);
        let selected_ids = selected
            .iter()
            .map(|fact| fact.fact_id.as_str())
            .collect::<Vec<_>>();
        let expected_ids = (1..=32)
            .map(|index| format!("fact:seed-{index:02}"))
            .collect::<Vec<_>>();
        assert_eq!(
            selected_ids,
            expected_ids.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn graph_evidence_does_not_admit_general_content_queries() {
        let mut graph_candidate = candidate("An unrelated stored note.", None);
        graph_candidate.evidence.graph_anchor_origin = Some(
            crate::memory::retrieval::types::GraphAnchorOrigin::QueryEntity {
                entity_id: "entity:alice".to_string(),
            },
        );
        graph_candidate.evidence.hop_count = Some(1);
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query("tracing reliability"), &graph_candidate, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn matched_community_summary_admits_its_linked_member_fact() {
        let mut linked = candidate("An unrelated stored note.", None);
        linked.fact.entity_links = vec!["entity:alpha".to_string()];
        linked.evidence.community = Some(super::super::types::CommunityAdmissionEvidence {
            community_id: "community:alpha".to_string(),
            member_entity_id: "entity:alpha".to_string(),
            summary_terms: vec![
                "alpha".to_string(),
                "launch".to_string(),
                "workstream".to_string(),
            ],
            matched_summary_terms: vec!["launch".to_string(), "workstream".to_string()],
        });
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query("launch workstream"), &linked, &policy),
            AdmissionDecision::Admit(AdmissionBasis::CommunitySummary)
        );
    }

    #[test]
    fn community_summary_cannot_admit_a_fact_outside_its_membership() {
        let mut unlinked = candidate("An unrelated stored note.", None);
        unlinked.fact.entity_links = vec!["entity:beta".to_string()];
        unlinked.evidence.community = Some(super::super::types::CommunityAdmissionEvidence {
            community_id: "community:alpha".to_string(),
            member_entity_id: "entity:alpha".to_string(),
            summary_terms: vec![
                "alpha".to_string(),
                "launch".to_string(),
                "workstream".to_string(),
            ],
            matched_summary_terms: vec!["launch".to_string(), "workstream".to_string()],
        });
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query("launch workstream"), &unlinked, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn community_summary_must_meet_the_query_content_floor() {
        let mut linked = candidate("An unrelated stored note.", None);
        linked.fact.entity_links = vec!["entity:alpha".to_string()];
        linked.evidence.community = Some(super::super::types::CommunityAdmissionEvidence {
            community_id: "community:alpha".to_string(),
            member_entity_id: "entity:alpha".to_string(),
            summary_terms: vec!["alpha".to_string(), "launch".to_string()],
            matched_summary_terms: vec!["launch".to_string()],
        });
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query("launch workstream"), &linked, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn repeated_query_topic_admits_experience_with_matching_index_keys() {
        let mut experience = candidate("I prefer quiet hotels away from nightlife.", None);
        experience.fact.fact_type = "experience".to_string();
        experience.fact.index_keys = vec!["conference".to_string(), "hotel".to_string()];
        let mut query = query("conference venue");
        query.repeated_topics = vec![super::super::types::RepeatedTopicEvidence {
            term: "hotel".to_string(),
            supporting_fact_ids: vec![
                "fact:conference-one".to_string(),
                "fact:conference-two".to_string(),
            ],
        }];
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query, &experience, &policy),
            AdmissionDecision::Admit(AdmissionBasis::RepeatedTopic)
        );
    }

    #[test]
    fn one_fact_cannot_establish_a_repeated_topic_experience_path() {
        let mut experience = candidate("I prefer quiet hotels away from nightlife.", None);
        experience.fact.fact_type = "experience".to_string();
        experience.fact.index_keys = vec!["conference".to_string(), "hotel".to_string()];
        let mut query = query("conference venue");
        query.repeated_topics = vec![super::super::types::RepeatedTopicEvidence {
            term: "hotel".to_string(),
            supporting_fact_ids: vec!["fact:conference-one".to_string()],
        }];
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query, &experience, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn relationship_intent_rejects_graph_evidence_without_anchor_origin() {
        let query = build_query_evidence(
            Some("What is the connection path between Alice and Atlas?"),
            Vec::new(),
            &[],
            &QueryFlags {
                wants_graph_path: true,
                ..QueryFlags::default()
            },
        );
        let mut graph_candidate = candidate("An unrelated stored note.", None);
        graph_candidate.evidence.hop_count = Some(2);
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);

        assert_eq!(
            evaluate_candidate(&query, &graph_candidate, &policy),
            AdmissionDecision::Reject(RejectReason::InsufficientEvidence)
        );
    }

    #[test]
    fn adjacent_months_with_shared_year_form_one_calendar_period() {
        let period = parse_calendar_period("march april 2026 atlas beacon decisions")
            .expect("calendar range");
        assert_eq!(
            period.start,
            chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
        );
        assert_eq!(
            period.end_exclusive,
            chrono::NaiveDate::from_ymd_opt(2026, 5, 1).unwrap()
        );
    }

    #[test]
    fn adjacent_month_range_crosses_year_boundary() {
        let period =
            parse_calendar_period("November December 2026").expect("year-crossing calendar period");
        assert_eq!(
            period.start,
            chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
        );
        assert_eq!(
            period.end_exclusive,
            chrono::NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()
        );
        let query = query("November December 2026");
        let mut fact = candidate("The annual review was approved.", None);
        fact.fact.t_valid = chrono::DateTime::parse_from_rfc3339("2026-11-15T09:00:00Z")
            .expect("valid reference date")
            .with_timezone(&Utc);
        fact.fact.t_ingested = fact.fact.t_valid;
        let access = AccessPayload::default();
        let policy =
            AdmissionPolicy::new(0.7, fact.fact.t_valid + chrono::Duration::days(1), &access);

        assert_eq!(
            evaluate_candidate(&query, &fact, &policy),
            AdmissionDecision::Admit(AdmissionBasis::CalendarPeriod)
        );
    }
}
