//! Experience fact retrieval and appending.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::models::{AccessContext, AssembledContextItem, Fact, FactType};
use crate::service::error::MemoryError;
use crate::service::query::search_query_terms;
use crate::service::query::{decayed_confidence, normalize_dt};

use super::filtering::{compare_facts_by_recency, fact_is_active_at, filter_facts_by_constraints};
use super::lexical::{lexical_query_overlap_for_fact, lexical_query_score_for_fact};

const REPEATED_TOPIC_MATCH_BOOST: f64 = 5.0;

pub(crate) struct RecentExperienceRequest<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) project: Option<&'a str>,
    pub(crate) access: &'a AccessContext,
    pub(crate) budget: i32,
    pub(crate) fact_types: &'a [String],
}

pub(crate) fn expand_experience_query_terms(
    query_terms: &[String],
    direct_facts: &[Fact],
) -> Vec<String> {
    let mut expanded = Vec::new();
    let mut seen_terms = HashSet::new();
    for term in query_terms {
        if seen_terms.insert(term.clone()) {
            expanded.push(term.clone());
        }
    }

    let mut term_frequency = HashMap::<String, usize>::new();
    for fact in direct_facts.iter().take(5) {
        let mut fact_terms = search_query_terms(&fact.content)
            .into_iter()
            .collect::<HashSet<_>>();
        for index_key in &fact.index_keys {
            fact_terms.extend(search_query_terms(index_key));
        }
        for term in fact_terms {
            *term_frequency.entry(term).or_default() += 1;
        }
    }

    let mut repeated_terms = term_frequency
        .into_iter()
        .filter(|(term, count)| *count >= 2 && !seen_terms.contains(term))
        .collect::<Vec<_>>();
    repeated_terms.sort_by(|(left_term, left_count), (right_term, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_term.cmp(right_term))
    });

    for (term, _) in repeated_terms.into_iter().take(6) {
        if seen_terms.insert(term.clone()) {
            expanded.push(term);
        }
    }

    expanded
}

pub(crate) async fn collect_recent_experience_facts(
    service: &crate::service::MemoryService,
    request: RecentExperienceRequest<'_>,
    query_terms: &[String],
    topical_terms: &[String],
    excluded_fact_ids: &HashSet<String>,
) -> Result<Vec<Fact>, MemoryError> {
    if !request.fact_types.is_empty()
        && !request
            .fact_types
            .iter()
            .any(|fact_type| fact_type == FactType::Experience.as_str())
    {
        return Ok(Vec::new());
    }

    let records = service
        .db_client
        .select_active_facts(request.namespace, 500)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
    let experience_filter = vec![FactType::Experience.as_str().to_string()];
    let mut facts =
        filter_facts_by_constraints(records, request.access, request.project, &experience_filter)
            .into_iter()
            .filter(|fact| fact.scope == request.scope)
            .filter(|fact| fact_is_active_at(fact, request.cutoff))
            .filter(|fact| !excluded_fact_ids.contains(&fact.fact_id))
            .filter(|fact| {
                let topical_overlap = lexical_query_overlap_for_fact(fact, topical_terms);
                let query_overlap = lexical_query_overlap_for_fact(fact, query_terms);
                if !topical_terms.is_empty() {
                    topical_overlap > 0 || query_overlap > 0
                } else {
                    query_terms.is_empty() || query_overlap > 0
                }
            })
            .collect::<Vec<_>>();

    for fact in &mut facts {
        if !query_terms.is_empty() {
            fact.ft_score = lexical_query_score_for_fact(fact, query_terms) as f64;
            let topical_overlap = lexical_query_overlap_for_fact(fact, topical_terms);
            if topical_overlap > 0 {
                fact.ft_score += topical_overlap as f64 * REPEATED_TOPIC_MATCH_BOOST;
            }
        }
    }

    facts.sort_by(|left, right| {
        if query_terms.is_empty() {
            right
                .t_ingested
                .cmp(&left.t_ingested)
                .then_with(|| compare_facts_by_recency(left, right))
        } else {
            right
                .ft_score
                .total_cmp(&left.ft_score)
                .then_with(|| right.t_ingested.cmp(&left.t_ingested))
                .then_with(|| compare_facts_by_recency(left, right))
        }
    });

    let limit = if query_terms.is_empty() {
        request.budget.max(1) as usize
    } else {
        (request.budget.max(1) as usize).saturating_mul(4)
    };
    facts.truncate(limit);
    Ok(facts)
}

pub(crate) async fn append_recent_experience_items(
    results: &mut Vec<AssembledContextItem>,
    service: &crate::service::MemoryService,
    request: RecentExperienceRequest<'_>,
) -> Result<usize, MemoryError> {
    let budget = request.budget.max(1) as usize;
    let cutoff = request.cutoff;
    if results.len() >= budget {
        return Ok(0);
    }

    let facts =
        collect_recent_experience_facts(service, request, &[], &[], &HashSet::new()).await?;

    let mut seen_fact_ids = results
        .iter()
        .map(|item| item.fact_id.clone())
        .collect::<HashSet<_>>();
    let mut appended = 0;

    for fact in facts {
        if results.len() >= budget || !seen_fact_ids.insert(fact.fact_id.clone()) {
            continue;
        }

        let confidence = decayed_confidence(&fact, cutoff);

        results.push(AssembledContextItem {
            fact_id: fact.fact_id,
            content: fact.content,
            quote: fact.quote,
            source_episode: fact.source_episode,
            confidence,
            semantic_available: Some(service.embedding_provider.is_enabled()),
            provenance: fact.provenance,
            rationale: format!(
                "supplemental experience recent_t_ingested={}",
                normalize_dt(fact.t_ingested)
            ),
            retrieval_tier: None,
            ..Default::default()
        });
        appended += 1;
    }

    Ok(appended)
}
