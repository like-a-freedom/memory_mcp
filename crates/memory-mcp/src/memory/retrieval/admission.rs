use std::collections::HashSet;

use chrono::{DateTime, Utc};

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
    Relationship,
    Current,
    Timeline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct QueryEvidence {
    pub(super) content_terms: Vec<String>,
    pub(super) qualified_references: Vec<String>,
    pub(super) resolved_entity_ids: Vec<String>,
    pub(super) repeated_topics: Vec<super::types::RepeatedTopicEvidence>,
    pub(super) mandatory_reference_alternatives: Vec<String>,
    pub(super) scope_reference: Option<String>,
    pub(super) intent: AdmissionIntent,
    pub(super) empty_query: bool,
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
    flags: &QueryFlags,
) -> QueryEvidence {
    let raw = raw_query.unwrap_or_default();
    let without_urls = strip_url_tokens(raw);
    let (cleaned_query, scope_reference) = strip_project_scope(&without_urls);
    let qualified_references = extract_qualified_references(&cleaned_query);
    let reference_terms = reference_component_terms(&cleaned_query);
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
                        && !(intent == AdmissionIntent::Current && is_current_intent_cue(term))
                })
                .collect(),
        )
    };

    QueryEvidence {
        content_terms,
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
                || candidate
                    .evidence
                    .semantic_similarity
                    .is_some_and(|score| score >= policy.semantic_threshold)
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

    if query
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

    if !query.content_terms.is_empty() {
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

    if query.intent == AdmissionIntent::Content
        && candidate
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
    if query.empty_query || query.intent == AdmissionIntent::Browse {
        return true;
    }

    if !query.mandatory_reference_alternatives.is_empty() {
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

fn classify_intent(raw_query: &str, flags: &QueryFlags, has_entities: bool) -> AdmissionIntent {
    let normalized = raw_query.to_lowercase();
    let words = normalized.split_whitespace().collect::<Vec<_>>();

    if raw_query.trim().is_empty() {
        AdmissionIntent::Browse
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
    } else if has_entities {
        AdmissionIntent::Entity
    } else {
        AdmissionIntent::Content
    }
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
        build_query_evidence(Some(raw), Vec::new(), &QueryFlags::default())
    }

    fn evaluate(raw: &str, content: &str, similarity: Option<f64>) -> AdmissionDecision {
        let access = AccessPayload::default();
        let policy = AdmissionPolicy::new(0.7, Utc::now(), &access);
        evaluate_candidate(&query(raw), &candidate(content, similarity), &policy)
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
    fn resolved_entity_link_is_retained_as_candidate_evidence() {
        let mut linked = candidate("A product manager owns the launch plan.", None);
        linked.fact.entity_links = vec!["entity:ana".to_string()];
        let query = build_query_evidence(
            Some("Ana"),
            vec!["entity:ana".to_string()],
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
}
