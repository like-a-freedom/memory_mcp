//! Lexical/FTS retrieval for fact and episode records.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::models::Fact;
use crate::service::error::MemoryError;
use crate::service::query::{
    query_hard_anchor_terms, query_term_rarity_weight, query_term_should_be_soft_anchor,
    search_query_terms, unique_query_terms,
};
use crate::service::value_helpers::{json_f64, json_string};
use crate::storage::ContextFactQuery;

use super::filtering::{
    fact_record_matches_project, fact_record_matches_type, raw_array, raw_object,
};
use super::ranking::RetrievalTier;

#[derive(Debug)]
pub(crate) struct LexicalQueryResult {
    pub(crate) records: Vec<Value>,
    pub(crate) retrieval_tier: RetrievalTier,
}

pub(crate) struct FactQueryParams<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff_iso: &'a str,
    pub(crate) query_opt: Option<&'a str>,
    pub(crate) limit: i32,
    pub(crate) project: Option<&'a str>,
    pub(crate) fact_types: &'a [String],
}

pub(crate) async fn select_fact_records_for_query(
    service: &crate::service::service_context::ServiceContext,
    params: FactQueryParams<'_>,
) -> Result<LexicalQueryResult, MemoryError> {
    let query_terms = params.query_opt.map(search_query_terms).unwrap_or_default();
    let candidate_limit = lexical_candidate_limit(params.limit);

    let initial = service
        .context_store()
        .select_facts_filtered(ContextFactQuery {
            namespace: params.namespace,
            scope: params.scope,
            cutoff: params.cutoff_iso,
            query_contains: params.query_opt,
            limit: candidate_limit,
            project: params.project,
            fact_types: params.fact_types,
        })
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
    let initial = rank_lexical_records(initial, &query_terms);

    let Some(_query) = params.query_opt else {
        return Ok(LexicalQueryResult {
            records: initial,
            retrieval_tier: RetrievalTier::Direct,
        });
    };

    let fallback_terms = build_lexical_fallback_queries(&query_terms);

    let mut fallback_records = Vec::new();
    for term in &fallback_terms {
        let term_records = service
            .context_store()
            .select_facts_filtered(ContextFactQuery {
                namespace: params.namespace,
                scope: params.scope,
                cutoff: params.cutoff_iso,
                query_contains: Some(term.as_str()),
                limit: candidate_limit,
                project: params.project,
                fact_types: params.fact_types,
            })
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
        fallback_records.extend(term_records);
    }

    let mut seen_fact_ids = HashSet::new();
    fallback_records.retain(|record| {
        let Some(fact_id) = record
            .get("fact_id")
            .and_then(crate::service::episode::unwrap_record_string)
        else {
            return true;
        };
        seen_fact_ids.insert(fact_id)
    });

    let fallback_records = rank_lexical_records(fallback_records, &query_terms);

    let initial_score = top_query_score(&initial, &query_terms);
    let fallback_score = top_query_score(&fallback_records, &query_terms);
    let best_score = initial_score.max(fallback_score);
    let best_phrase_overlap = top_phrase_overlap(&initial, &query_terms)
        .max(top_phrase_overlap(&fallback_records, &query_terms));

    if query_terms.len() >= 3 && (best_score < query_terms.len().min(4) || best_phrase_overlap == 0)
    {
        let scanned_records =
            scan_fact_records_by_query_terms(service, &params, &query_terms).await?;
        let scanned_score = top_query_score(&scanned_records, &query_terms);
        if (query_terms.len() >= 3 && best_phrase_overlap == 0 && !scanned_records.is_empty())
            || scanned_score > best_score
        {
            return Ok(LexicalQueryResult {
                records: scanned_records,
                retrieval_tier: RetrievalTier::EpisodeFallback,
            });
        }
    }

    if fallback_score > initial_score {
        return Ok(LexicalQueryResult {
            records: fallback_records,
            retrieval_tier: RetrievalTier::EpisodeFallback,
        });
    }

    if !initial.is_empty() {
        return Ok(LexicalQueryResult {
            records: initial,
            retrieval_tier: RetrievalTier::Direct,
        });
    }

    let retrieval_tier = if fallback_records.is_empty() {
        RetrievalTier::Direct
    } else {
        RetrievalTier::EpisodeFallback
    };

    Ok(LexicalQueryResult {
        records: fallback_records,
        retrieval_tier,
    })
}

pub(crate) fn lexical_candidate_limit(limit: i32) -> i32 {
    let base = limit.max(1);
    let cap = base.max(50);
    (base.saturating_mul(5)).clamp(base, cap)
}

#[derive(Debug, Default)]
struct LexicalAnchorProfile {
    hard_anchor_terms: HashSet<String>,
    soft_anchor_terms: HashSet<String>,
    term_weights: HashMap<String, f64>,
}

impl LexicalAnchorProfile {
    fn weight_for(&self, term: &str) -> f64 {
        self.term_weights.get(term).copied().unwrap_or(1.0)
    }

    fn is_hard_anchor(&self, term: &str) -> bool {
        self.hard_anchor_terms.contains(term)
    }

    fn is_soft_anchor(&self, term: &str) -> bool {
        self.soft_anchor_terms.contains(term)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct LexicalRecordMetrics {
    base_score: usize,
    weighted_overlap: f64,
    hard_anchor_hits: usize,
    hard_anchor_mass: f64,
    soft_anchor_mass: f64,
}

fn build_lexical_fallback_queries(query_terms: &[String]) -> Vec<String> {
    let mut queries = Vec::new();
    let standalone_temporal_terms = standalone_temporal_fallback_terms(query_terms);
    let hard_anchor_terms = query_hard_anchor_terms(query_terms)
        .into_iter()
        .filter(|term| !standalone_temporal_terms.contains(term.as_str()))
        .collect::<HashSet<_>>();
    let enforce_anchor_windows = !hard_anchor_terms.is_empty();

    for width in (2..=3).rev() {
        if query_terms.len() < width {
            continue;
        }
        for window in query_terms.windows(width) {
            if enforce_anchor_windows
                && !window
                    .iter()
                    .any(|term| hard_anchor_terms.contains(term.as_str()))
            {
                continue;
            }
            let query = window.join(" ");
            if !queries.contains(&query) {
                queries.push(query);
            }
        }
    }

    for term in query_terms {
        if standalone_temporal_terms.contains(term.as_str()) {
            continue;
        }
        if enforce_anchor_windows && !hard_anchor_terms.contains(term.as_str()) {
            continue;
        }
        if !queries.contains(term) {
            queries.push(term.clone());
        }
    }

    queries
}

fn standalone_temporal_fallback_terms(query_terms: &[String]) -> HashSet<&str> {
    let mut terms = HashSet::new();

    for window in query_terms.windows(2) {
        let [left, right] = window else {
            continue;
        };

        if (is_calendar_month(left) && is_four_digit_year(right))
            || (is_four_digit_year(left) && is_calendar_month(right))
        {
            terms.insert(left.as_str());
            terms.insert(right.as_str());
        }
    }

    terms
}

fn is_calendar_month(term: &str) -> bool {
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
    )
}

fn is_four_digit_year(term: &str) -> bool {
    term.len() == 4 && term.chars().all(|character| character.is_ascii_digit())
}

async fn scan_fact_records_by_query_terms(
    service: &crate::service::service_context::ServiceContext,
    params: &FactQueryParams<'_>,
    query_terms: &[String],
) -> Result<Vec<Value>, MemoryError> {
    let records = service
        .context_store()
        .select_table("fact", params.namespace)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let filtered = records
        .into_iter()
        .filter(|record| {
            raw_object(record)
                .and_then(|map: &serde_json::Map<String, Value>| map.get("scope"))
                .and_then(json_string)
                .is_some_and(|value| value == params.scope)
        })
        .filter(|record| {
            raw_object(record)
                .and_then(|map: &serde_json::Map<String, Value>| map.get("t_valid"))
                .and_then(json_string)
                .is_some_and(|value| value <= params.cutoff_iso)
        })
        .filter(|record| {
            raw_object(record)
                .and_then(|map: &serde_json::Map<String, Value>| map.get("t_invalid"))
                .and_then(json_string)
                .is_none_or(|value| value > params.cutoff_iso)
        })
        .filter(|record| fact_record_matches_project(record, params.project))
        .filter(|record| fact_record_matches_type(record, params.fact_types))
        .filter(|record| lexical_query_overlap(record, query_terms) > 0)
        .map(|mut record| {
            let score = lexical_query_score(&record, query_terms) as f64;
            if let Some(object) = record.as_object_mut() {
                object.insert("ft_score".to_string(), json!(score));
            } else if let Some(object) = record.get_mut("Object").and_then(Value::as_object_mut) {
                object.insert("ft_score".to_string(), json!(score));
            }
            record
        })
        .collect::<Vec<_>>();

    let filtered = rank_lexical_records(filtered, query_terms);
    let limit = params.limit.max(1) as usize;
    let filtered = filtered.into_iter().take(limit).collect();
    Ok(filtered)
}

pub(crate) fn rank_lexical_records(mut records: Vec<Value>, query_terms: &[String]) -> Vec<Value> {
    if query_terms.is_empty() {
        return records;
    }

    let anchor_profile = build_lexical_anchor_profile(&records, query_terms);

    for record in &mut records {
        let metrics = lexical_record_metrics(record, query_terms, &anchor_profile);
        let soft_anchor_bonus = if metrics.base_score <= 1 {
            metrics.soft_anchor_mass * 0.35
        } else {
            0.0
        };
        let combined_score = dampened_lexical_ft_score(record)
            + metrics.base_score as f64
            + (metrics.hard_anchor_mass * 2.0)
            + soft_anchor_bonus;
        if let Some(object) = record.as_object_mut() {
            object.insert("ft_score".to_string(), json!(combined_score));
        } else if let Some(object) = record.get_mut("Object").and_then(Value::as_object_mut) {
            object.insert("ft_score".to_string(), json!(combined_score));
        }
    }

    records.sort_by(|left, right| {
        let right_metrics = lexical_record_metrics(right, query_terms, &anchor_profile);
        let left_metrics = lexical_record_metrics(left, query_terms, &anchor_profile);

        right_metrics
            .hard_anchor_hits
            .cmp(&left_metrics.hard_anchor_hits)
            .then_with(|| {
                right_metrics
                    .hard_anchor_mass
                    .total_cmp(&left_metrics.hard_anchor_mass)
            })
            .then_with(|| right_metrics.base_score.cmp(&left_metrics.base_score))
            .then_with(|| {
                if right_metrics.base_score <= 1 && left_metrics.base_score <= 1 {
                    right_metrics
                        .weighted_overlap
                        .total_cmp(&left_metrics.weighted_overlap)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| {
                dampened_lexical_ft_score(right).total_cmp(&dampened_lexical_ft_score(left))
            })
            .then_with(|| lexical_t_valid(right).cmp(&lexical_t_valid(left)))
            .then_with(|| {
                right_metrics
                    .weighted_overlap
                    .total_cmp(&left_metrics.weighted_overlap)
            })
            .then_with(|| lexical_fact_id(left).cmp(&lexical_fact_id(right)))
    });

    records
}

fn build_lexical_anchor_profile(records: &[Value], query_terms: &[String]) -> LexicalAnchorProfile {
    let unique_terms = unique_query_terms(query_terms);
    if unique_terms.is_empty() {
        return LexicalAnchorProfile::default();
    }

    let total_records = records.len().max(1);
    let hard_anchor_terms = query_hard_anchor_terms(&unique_terms);
    let mut doc_freq = HashMap::<String, usize>::new();

    for record in records {
        let record_terms = lexical_record_term_set(record);
        for term in &unique_terms {
            if record_terms.contains(term) {
                *doc_freq.entry(term.clone()).or_default() += 1;
            }
        }
    }

    let mut soft_anchor_terms = HashSet::new();
    let mut term_weights = HashMap::new();

    for term in unique_terms {
        let doc_freq_for_term = doc_freq.get(term.as_str()).copied().unwrap_or(0);
        let rarity = query_term_rarity_weight(doc_freq_for_term, total_records);
        let hard_anchor_boost = if hard_anchor_terms.contains(term.as_str()) {
            2.5
        } else {
            1.0
        };
        term_weights.insert(term.clone(), (1.0 + rarity) * hard_anchor_boost);

        if !hard_anchor_terms.contains(term.as_str())
            && query_term_should_be_soft_anchor(&term, doc_freq_for_term, total_records)
        {
            soft_anchor_terms.insert(term);
        }
    }

    LexicalAnchorProfile {
        hard_anchor_terms,
        soft_anchor_terms,
        term_weights,
    }
}

fn lexical_record_metrics(
    record: &Value,
    query_terms: &[String],
    anchor_profile: &LexicalAnchorProfile,
) -> LexicalRecordMetrics {
    let matched_terms = matched_query_terms_for_record(record, query_terms);
    let weighted_overlap = matched_terms
        .iter()
        .map(|term| anchor_profile.weight_for(term))
        .sum::<f64>();
    let hard_anchor_hits = matched_terms
        .iter()
        .filter(|term| anchor_profile.is_hard_anchor(term.as_str()))
        .count();
    let hard_anchor_mass = matched_terms
        .iter()
        .filter(|term| anchor_profile.is_hard_anchor(term.as_str()))
        .map(|term| anchor_profile.weight_for(term))
        .sum::<f64>();
    let soft_anchor_mass = matched_terms
        .iter()
        .filter(|term| anchor_profile.is_soft_anchor(term.as_str()))
        .map(|term| anchor_profile.weight_for(term))
        .sum::<f64>();

    LexicalRecordMetrics {
        base_score: lexical_query_score(record, query_terms),
        weighted_overlap,
        hard_anchor_hits,
        hard_anchor_mass,
        soft_anchor_mass,
    }
}

fn lexical_record_term_set(record: &Value) -> HashSet<String> {
    let mut record_terms = HashSet::<String>::new();
    if let Some(content) = raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("content"))
        .and_then(json_string)
    {
        record_terms.extend(search_query_terms(content));
    }
    if let Some(index_keys) = raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("index_keys"))
        .and_then(raw_array)
    {
        for value in index_keys {
            if let Some(index_key) = json_string(value) {
                record_terms.extend(search_query_terms(index_key));
            }
        }
    }

    record_terms
}

fn matched_query_terms_for_record(record: &Value, query_terms: &[String]) -> Vec<String> {
    if query_terms.is_empty() {
        return Vec::new();
    }

    let mut record_terms = best_matching_content_terms(
        raw_object(record)
            .and_then(|map: &serde_json::Map<String, Value>| map.get("content"))
            .and_then(json_string)
            .unwrap_or_default(),
        query_terms,
    )
    .into_iter()
    .collect::<HashSet<_>>();

    if let Some(index_keys) = raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("index_keys"))
        .and_then(raw_array)
    {
        for value in index_keys {
            if let Some(index_key) = json_string(value) {
                record_terms.extend(search_query_terms(index_key));
            }
        }
    }

    unique_query_terms(query_terms)
        .into_iter()
        .filter(|term| record_terms.contains(term.as_str()))
        .collect()
}

fn top_query_score(records: &[Value], query_terms: &[String]) -> usize {
    records
        .iter()
        .map(|record| lexical_query_score(record, query_terms))
        .max()
        .unwrap_or(0)
}

fn top_phrase_overlap(records: &[Value], query_terms: &[String]) -> usize {
    records
        .iter()
        .map(|record| lexical_phrase_overlap(record, query_terms))
        .max()
        .unwrap_or(0)
}

fn lexical_query_overlap(record: &Value, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }

    let mut record_terms = HashSet::<String>::new();
    if let Some(content) = raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("content"))
        .and_then(json_string)
    {
        record_terms.extend(search_query_terms(content));
    }
    if let Some(index_keys) = raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("index_keys"))
        .and_then(raw_array)
    {
        for value in index_keys {
            if let Some(index_key) = json_string(value) {
                record_terms.extend(search_query_terms(index_key));
            }
        }
    }

    query_terms
        .iter()
        .filter(|term| record_terms.contains(term.as_str()))
        .count()
}

fn lexical_query_score(record: &Value, query_terms: &[String]) -> usize {
    let content_terms = best_matching_content_terms(
        raw_object(record)
            .and_then(|map: &serde_json::Map<String, Value>| map.get("content"))
            .and_then(json_string)
            .unwrap_or_default(),
        query_terms,
    );
    let unigram_overlap = query_term_overlap_for_terms(&content_terms, query_terms)
        + lexical_index_key_overlap(record, query_terms);
    let phrase_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 2)
        + lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);
    let trigram_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

pub(crate) fn lexical_query_overlap_for_fact(fact: &Fact, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }

    let mut fact_terms = search_query_terms(&fact.content)
        .into_iter()
        .collect::<HashSet<_>>();
    for index_key in &fact.index_keys {
        fact_terms.extend(search_query_terms(index_key));
    }

    query_terms
        .iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .count()
}

pub(crate) fn lexical_query_score_for_text(text: &str, query_terms: &[String]) -> usize {
    let content_terms = best_matching_content_terms(text, query_terms);
    let unigram_overlap = query_term_overlap_for_terms(&content_terms, query_terms);
    let phrase_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 2)
        + lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);
    let trigram_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

pub(crate) fn lexical_query_score_for_fact(fact: &Fact, query_terms: &[String]) -> usize {
    let content_terms = best_matching_content_terms(&fact.content, query_terms);
    let unigram_overlap = query_term_overlap_for_terms(&content_terms, query_terms)
        + lexical_index_key_overlap_for_fact(fact, query_terms);
    let phrase_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 2)
        + lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);
    let trigram_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

fn lexical_phrase_overlap(record: &Value, query_terms: &[String]) -> usize {
    let content_terms = best_matching_content_terms(
        raw_object(record)
            .and_then(|map: &serde_json::Map<String, Value>| map.get("content"))
            .and_then(json_string)
            .unwrap_or_default(),
        query_terms,
    );
    lexical_ngram_overlap_for_terms(&content_terms, query_terms, 2)
        + lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3)
}

fn lexical_ngram_overlap_for_terms(
    content_terms: &[String],
    query_terms: &[String],
    width: usize,
) -> usize {
    if content_terms.len() < width {
        return 0;
    }

    let record_ngrams = content_terms
        .windows(width)
        .map(|window| window.join(" "))
        .collect::<HashSet<_>>();

    query_terms
        .windows(width)
        .filter(|window| record_ngrams.contains(&window.join(" ")))
        .count()
}

fn best_matching_content_terms(text: &str, query_terms: &[String]) -> Vec<String> {
    let fallback_terms = search_query_terms(text);
    if fallback_terms.is_empty() || query_terms.is_empty() {
        return fallback_terms;
    }

    let mut spans = local_content_spans(text).into_iter();
    let Some((mut best_terms, best_span_width)) = spans.next() else {
        return fallback_terms;
    };
    let mut best_score = adjusted_span_score(
        score_content_terms(&best_terms, query_terms),
        best_span_width,
    );
    let mut best_len = best_terms.len();

    for (candidate_terms, span_width) in spans {
        if candidate_terms.is_empty() {
            continue;
        }

        let candidate_score = adjusted_span_score(
            score_content_terms(&candidate_terms, query_terms),
            span_width,
        );
        let candidate_len = candidate_terms.len();
        let should_replace = candidate_score > best_score
            || (candidate_score == best_score && candidate_len < best_len);
        if should_replace {
            best_len = candidate_len;
            best_score = candidate_score;
            best_terms = candidate_terms;
        }
    }

    best_terms
}

fn local_content_spans(text: &str) -> Vec<(Vec<String>, usize)> {
    let sentence_terms = sentence_like_segments(text)
        .into_iter()
        .map(|segment| search_query_terms(&segment))
        .filter(|terms| !terms.is_empty())
        .collect::<Vec<_>>();

    if sentence_terms.is_empty() {
        return vec![(search_query_terms(text), 1)];
    }

    sentence_terms
        .iter()
        .cloned()
        .map(|terms| (terms, 1))
        .collect::<Vec<_>>()
}

fn adjusted_span_score(raw_score: usize, span_width: usize) -> usize {
    raw_score.saturating_sub(span_width.saturating_sub(1) * 2)
}

fn sentence_like_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();

    for character in text.trim().chars() {
        current.push(character);
        if matches!(character, '.' | '!' | '?' | ';' | '\n') {
            let segment = current.trim();
            if !segment.is_empty() {
                segments.push(segment.to_string());
            }
            current.clear();
        }
    }

    let trailing = current.trim();
    if !trailing.is_empty() {
        segments.push(trailing.to_string());
    }

    segments
}

fn score_content_terms(content_terms: &[String], query_terms: &[String]) -> usize {
    let unigram_overlap = query_term_overlap_for_terms(content_terms, query_terms);
    let phrase_overlap = lexical_ngram_overlap_for_terms(content_terms, query_terms, 2)
        + lexical_ngram_overlap_for_terms(content_terms, query_terms, 3);
    let trigram_overlap = lexical_ngram_overlap_for_terms(content_terms, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

fn query_term_overlap_for_terms(content_terms: &[String], query_terms: &[String]) -> usize {
    if query_terms.is_empty() || content_terms.is_empty() {
        return 0;
    }

    let content_terms = content_terms.iter().collect::<HashSet<_>>();
    query_terms
        .iter()
        .filter(|term| content_terms.contains(term))
        .count()
}

fn lexical_index_key_overlap(record: &Value, query_terms: &[String]) -> usize {
    raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("index_keys"))
        .and_then(raw_array)
        .map(|index_keys| {
            let terms = index_keys
                .iter()
                .filter_map(json_string)
                .flat_map(search_query_terms)
                .collect::<Vec<_>>();
            query_term_overlap_for_terms(&terms, query_terms)
        })
        .unwrap_or(0)
}

fn lexical_index_key_overlap_for_fact(fact: &Fact, query_terms: &[String]) -> usize {
    let terms = fact
        .index_keys
        .iter()
        .flat_map(|index_key| search_query_terms(index_key))
        .collect::<Vec<_>>();
    query_term_overlap_for_terms(&terms, query_terms)
}

fn lexical_ft_score(record: &Value) -> f64 {
    raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("ft_score"))
        .and_then(json_f64)
        .unwrap_or(0.0)
}

fn dampened_lexical_ft_score(record: &Value) -> f64 {
    lexical_ft_score(record).max(0.0).ln_1p()
}

fn lexical_t_valid(record: &Value) -> String {
    raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("t_valid"))
        .and_then(json_string)
        .unwrap_or_default()
        .to_string()
}

fn lexical_fact_id(record: &Value) -> String {
    raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("fact_id"))
        .and_then(crate::service::episode::unwrap_record_string)
        .unwrap_or_default()
        .to_string()
}

pub(crate) async fn select_episode_records_for_query(
    service: &crate::service::service_context::ServiceContext,
    namespace: &str,
    scope: &str,
    cutoff_iso: &str,
    query_opt: Option<&str>,
    limit: i32,
    project: Option<&str>,
) -> Result<Vec<Value>, MemoryError> {
    let initial = service
        .context_store()
        .select_episodes_by_content(namespace, scope, cutoff_iso, query_opt, limit, project)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    if !initial.is_empty() || query_opt.is_none() {
        return Ok(initial);
    }

    let Some(query) = query_opt else {
        return Ok(initial);
    };

    let query_terms = search_query_terms(query);
    let fallback_terms = build_lexical_fallback_queries(&query_terms);
    if fallback_terms.is_empty() {
        return Ok(initial);
    }

    let mut fallback_records = Vec::new();
    for term in fallback_terms {
        let term_records = service
            .context_store()
            .select_episodes_by_content(
                namespace,
                scope,
                cutoff_iso,
                Some(term.as_str()),
                limit,
                project,
            )
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
        fallback_records.extend(term_records);
    }

    let mut seen_episode_ids = HashSet::new();
    fallback_records.retain(|record| {
        let Some(episode_id) = record
            .get("episode_id")
            .and_then(json_string)
            .or_else(|| record.get("id").and_then(json_string))
        else {
            return true;
        };
        seen_episode_ids.insert(episode_id.to_string())
    });

    Ok(fallback_records)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};

    use serde_json::{Value, json};

    use super::{
        LexicalAnchorProfile, build_lexical_fallback_queries, lexical_record_metrics,
        rank_lexical_records,
    };

    fn test_record(fact_id: &str, content: &str, t_valid: &str) -> Value {
        json!({
            "fact_id": fact_id,
            "content": content,
            "index_keys": [],
            "ft_score": 0.0,
            "t_valid": t_valid,
        })
    }

    #[test]
    fn build_lexical_fallback_queries_skips_standalone_temporal_terms_for_month_year_queries() {
        let queries = build_lexical_fallback_queries(&[
            "requirement".to_string(),
            "created".to_string(),
            "july".to_string(),
            "2025".to_string(),
        ]);

        assert!(queries.contains(&"july 2025".to_string()));
        assert!(queries.contains(&"requirement".to_string()));
        assert!(queries.contains(&"created".to_string()));
        assert!(
            !queries.contains(&"july".to_string()),
            "month token should not be used as a standalone fallback when month/year is explicit"
        );
        assert!(
            !queries.contains(&"2025".to_string()),
            "year token should not be used as a standalone fallback when month/year is explicit"
        );
    }

    #[test]
    fn build_lexical_fallback_queries_skips_generic_singletons_when_hard_anchor_exists() {
        let queries = build_lexical_fallback_queries(&[
            "work".to_string(),
            "item".to_string(),
            "9794206".to_string(),
            "requirement".to_string(),
            "product".to_string(),
            "business".to_string(),
            "context".to_string(),
        ]);

        assert!(
            queries.iter().any(|query| query.contains("9794206")),
            "expected anchor-preserving fallback queries, got {queries:?}"
        );
        assert!(queries.contains(&"9794206".to_string()));
        assert!(
            !queries.contains(&"requirement".to_string()),
            "generic singleton fallback should be skipped when a hard anchor exists: {queries:?}"
        );
        assert!(
            !queries.contains(&"product".to_string()),
            "generic singleton fallback should be skipped when a hard anchor exists: {queries:?}"
        );
    }

    #[test]
    fn rank_lexical_records_prioritizes_hard_anchor_matches_over_generic_overlap() {
        let query_terms = crate::service::query::search_query_terms("300k telemetry response");
        let ranked = rank_lexical_records(
            vec![
                test_record(
                    "fact:generic",
                    "Telemetry response workflow updated for the support team.",
                    "2026-04-12T00:00:00Z",
                ),
                test_record(
                    "fact:anchor",
                    "300k telemetry sizing notes were approved for the deal.",
                    "2026-04-01T00:00:00Z",
                ),
            ],
            &query_terms,
        );

        let first_fact_id = ranked[0]
            .get("fact_id")
            .and_then(Value::as_str)
            .expect("fact_id on ranked record");
        assert_eq!(
            first_fact_id, "fact:anchor",
            "hard numeric or mixed-alphanumeric anchors should outrank generic overlap noise"
        );
    }

    #[test]
    fn rank_lexical_records_promotes_soft_anchor_matches_for_rare_lowercase_terms() {
        let query_terms = crate::service::query::search_query_terms("openshift rollout");
        let ranked = rank_lexical_records(
            vec![
                test_record(
                    "fact:generic-1",
                    "Rollout checklist updated for regional launch.",
                    "2026-04-12T00:00:00Z",
                ),
                test_record(
                    "fact:generic-2",
                    "Rollout timeline updated for support workflow.",
                    "2026-04-11T00:00:00Z",
                ),
                test_record(
                    "fact:anchor",
                    "OpenShift migration exception approved for the platform cluster.",
                    "2026-04-01T00:00:00Z",
                ),
            ],
            &query_terms,
        );

        let first_fact_id = ranked[0]
            .get("fact_id")
            .and_then(Value::as_str)
            .expect("fact_id on ranked record");
        assert_eq!(
            first_fact_id, "fact:anchor",
            "rare lower-case product/platform terms should behave like anchors when the candidate pool makes them distinctive"
        );
    }

    #[test]
    fn lexical_record_metrics_are_bitwise_stable_for_identical_input() {
        let query_terms = [
            "anchor01", "anchor02", "anchor03", "soft01", "soft02", "soft03", "soft04", "soft05",
            "soft06", "soft07", "soft08",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        let record = test_record(
            "fact:stable",
            &query_terms.join(" "),
            "2026-04-12T00:00:00Z",
        );
        let anchor_profile = LexicalAnchorProfile {
            hard_anchor_terms: ["anchor01", "anchor02", "anchor03"]
                .into_iter()
                .map(str::to_string)
                .collect::<HashSet<_>>(),
            soft_anchor_terms: [
                "soft01", "soft02", "soft03", "soft04", "soft05", "soft06", "soft07", "soft08",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>(),
            term_weights: HashMap::from([
                ("anchor01".to_string(), 1.0e16),
                ("anchor02".to_string(), 1.0),
                ("anchor03".to_string(), 1.0),
                ("soft01".to_string(), 1.0),
                ("soft02".to_string(), 1.0),
                ("soft03".to_string(), 1.0),
                ("soft04".to_string(), 1.0),
                ("soft05".to_string(), 1.0),
                ("soft06".to_string(), 1.0),
                ("soft07".to_string(), 1.0),
                ("soft08".to_string(), 1.0),
            ]),
        };

        let signatures = (0..256)
            .map(|_| {
                let metrics = lexical_record_metrics(&record, &query_terms, &anchor_profile);
                (
                    metrics.weighted_overlap.to_bits(),
                    metrics.hard_anchor_mass.to_bits(),
                    metrics.soft_anchor_mass.to_bits(),
                )
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(
            signatures.len(),
            1,
            "lexical metrics should be deterministic for identical input, got signatures: {signatures:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Tests relocated from context.rs — rank_lexical_records scenarios and
    // select_fact_records_for_query DB-backed fallback behavior.
    // -----------------------------------------------------------------------

    use super::{FactQueryParams, lexical_candidate_limit, select_fact_records_for_query};
    use crate::service::error::MemoryError;
    use crate::storage::{DbClient, GraphDirection};
    use async_trait::async_trait;
    use std::sync::Arc;

    #[test]
    fn rank_lexical_records_promotes_more_specific_query_overlap() {
        let query_terms = vec![
            "caroline".to_string(),
            "lgbtq".to_string(),
            "support".to_string(),
            "group".to_string(),
        ];

        let ranked = rank_lexical_records(
            vec![
                json!({
                    "fact_id": "fact:generic",
                    "content": "Caroline passed the adoption agency interviews last Friday.",
                    "t_valid": "2026-01-10T10:30:00Z",
                    "ft_score": 20.0
                }),
                json!({
                    "fact_id": "fact:support",
                    "content": "Caroline attended the LGBTQ support group recently.",
                    "t_valid": "2026-01-09T10:30:00Z",
                    "ft_score": 5.0
                }),
            ],
            &query_terms,
        );

        let fact_ids = ranked
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(fact_ids, vec!["fact:support", "fact:generic"]);
    }

    #[test]
    fn rank_lexical_records_prefers_sentence_cohesion_over_cross_sentence_term_soup() {
        let query_terms = crate::service::query::search_query_terms(
            "I recently attended an event where there was a unique blend of modern beats with Pacific sounds.",
        );

        let ranked = rank_lexical_records(
            vec![
                json!({
                    "fact_id": "fact:term-soup",
                    "content": "I recently updated my studio notes after an event planning session. The next experiment used modern beats in a new mix. A Pacific sound library added a unique texture to the blend.",
                    "t_valid": "2026-01-10T10:30:00Z",
                    "ft_score": 18.0
                }),
                json!({
                    "fact_id": "fact:exact-sentence",
                    "content": "I was so thrilled to see that fusion in action! The blend of traditional Pacific sounds with modern beats created a captivating experience that resonated deeply with the audience.",
                    "t_valid": "2026-01-09T10:30:00Z",
                    "ft_score": 8.0
                }),
            ],
            &query_terms,
        );

        let fact_ids = ranked
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            fact_ids,
            vec!["fact:exact-sentence", "fact:term-soup"],
            "exact sentence matches should outrank cross-sentence term soup even when the soup has a stronger raw ft_score"
        );
    }

    #[test]
    fn lexical_candidate_limit_preserves_preexpanded_limits() {
        assert_eq!(lexical_candidate_limit(5), 25);
        assert_eq!(lexical_candidate_limit(50), 50);
        assert_eq!(lexical_candidate_limit(200), 200);
    }

    #[tokio::test]
    async fn select_fact_records_for_query_deduplicates_term_fallback_records() {
        struct DedupFallbackDbClient;

        #[async_trait]
        impl DbClient for DedupFallbackDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("atlas launch checklist") => vec![],
                    Some("atlas") => vec![
                        json!({
                            "fact_id": "fact:shared",
                            "fact_type": "note",
                            "content": "Atlas launch is scheduled.",
                            "quote": "Atlas launch is scheduled.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-10T10:30:00Z",
                            "t_ingested": "2026-01-10T10:30:00Z",
                            "scope": "org"
                        }),
                        json!({
                            "fact_id": "fact:atlas-only",
                            "fact_type": "note",
                            "content": "Atlas has a risk review.",
                            "quote": "Atlas has a risk review.",
                            "source_episode": "episode:2",
                            "t_valid": "2026-01-09T10:30:00Z",
                            "t_ingested": "2026-01-09T10:30:00Z",
                            "scope": "org"
                        }),
                    ],
                    Some("launch") => vec![
                        json!({
                            "fact_id": "fact:shared",
                            "fact_type": "note",
                            "content": "Atlas launch is scheduled.",
                            "quote": "Atlas launch is scheduled.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-10T10:30:00Z",
                            "t_ingested": "2026-01-10T10:30:00Z",
                            "scope": "org"
                        }),
                        json!({
                            "fact_id": "fact:launch-only",
                            "fact_type": "note",
                            "content": "Launch checklist is ready.",
                            "quote": "Launch checklist is ready.",
                            "source_episode": "episode:3",
                            "t_valid": "2026-01-08T10:30:00Z",
                            "t_ingested": "2026-01-08T10:30:00Z",
                            "scope": "org"
                        }),
                    ],
                    Some("checklist") => vec![json!({
                        "fact_id": "fact:launch-only",
                        "fact_type": "note",
                        "content": "Launch checklist is ready.",
                        "quote": "Launch checklist is ready.",
                        "source_episode": "episode:3",
                        "t_valid": "2026-01-08T10:30:00Z",
                        "t_ingested": "2026-01-08T10:30:00Z",
                        "scope": "org"
                    })],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(DedupFallbackDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let lexical_result = select_fact_records_for_query(
            &service.build_context(),
            FactQueryParams {
                namespace: "org",
                scope: "org",
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("atlas launch checklist"),
                limit: 10,
                project: None,
                fact_types: &[],
            },
        )
        .await
        .expect("fallback records");

        let fact_ids = lexical_result
            .records
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            lexical_result.retrieval_tier,
            super::RetrievalTier::EpisodeFallback
        );
        assert_eq!(
            fact_ids,
            vec!["fact:shared", "fact:launch-only", "fact:atlas-only"]
        );
    }

    #[tokio::test]
    async fn select_fact_records_for_query_prefers_term_fallback_with_better_overlap() {
        struct FallbackPreferenceDbClient;

        #[async_trait]
        impl DbClient for FallbackPreferenceDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("lgbtq support group") => vec![json!({
                        "fact_id": "fact:support-group",
                        "fact_type": "note",
                        "content": "Caroline attended the LGBTQ support group recently.",
                        "quote": "Caroline attended the LGBTQ support group recently.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-09T10:30:00Z",
                        "t_ingested": "2026-01-09T10:30:00Z",
                        "scope": "org",
                        "ft_score": 5.0
                    })],
                    Some("support group") => vec![json!({
                        "fact_id": "fact:support-group",
                        "fact_type": "note",
                        "content": "Caroline attended the LGBTQ support group recently.",
                        "quote": "Caroline attended the LGBTQ support group recently.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-09T10:30:00Z",
                        "t_ingested": "2026-01-09T10:30:00Z",
                        "scope": "org",
                        "ft_score": 5.0
                    })],
                    Some("support") => vec![json!({
                        "fact_id": "fact:generic-support",
                        "fact_type": "note",
                        "content": "Customer support team added a new channel.",
                        "quote": "Customer support team added a new channel.",
                        "source_episode": "episode:2",
                        "t_valid": "2026-01-08T10:30:00Z",
                        "t_ingested": "2026-01-08T10:30:00Z",
                        "scope": "org",
                        "ft_score": 3.0
                    })],
                    Some("group") => vec![json!({
                        "fact_id": "fact:generic-group",
                        "fact_type": "note",
                        "content": "Project group met to finalize the roadmap.",
                        "quote": "Project group met to finalize the roadmap.",
                        "source_episode": "episode:3",
                        "t_valid": "2026-01-07T10:30:00Z",
                        "t_ingested": "2026-01-07T10:30:00Z",
                        "scope": "org",
                        "ft_score": 3.0
                    })],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(FallbackPreferenceDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let lexical_result = select_fact_records_for_query(
            &service.build_context(),
            FactQueryParams {
                namespace: "org",
                scope: "org",
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("lgbtq support group"),
                limit: 10,
                project: None,
                fact_types: &[],
            },
        )
        .await
        .expect("fallback preference records");

        let fact_ids = lexical_result
            .records
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(fact_ids.first().copied(), Some("fact:support-group"));
    }

    #[tokio::test]
    async fn select_fact_records_for_short_query_uses_term_fallback() {
        struct ShortQueryFallbackDbClient;

        #[async_trait]
        impl DbClient for ShortQueryFallbackDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(match query_contains {
                    Some("What degree did I graduate with?") => vec![],
                    Some("degree graduate") => vec![json!({
                        "fact_id": "fact:answer",
                        "fact_type": "note",
                        "content": "I will graduate with a degree in Business Administration.",
                        "quote": "I will graduate with a degree in Business Administration.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org",
                        "ft_score": 4.0
                    })],
                    Some("degree") => vec![
                        json!({
                            "fact_id": "fact:generic",
                            "fact_type": "note",
                            "content": "The degree committee met to review course requirements.",
                            "quote": "The degree committee met to review course requirements.",
                            "source_episode": "episode:2",
                            "t_valid": "2026-01-09T10:30:00Z",
                            "t_ingested": "2026-01-09T10:30:00Z",
                            "scope": "org",
                            "ft_score": 8.0
                        }),
                        json!({
                            "fact_id": "fact:answer",
                            "fact_type": "note",
                            "content": "I will graduate with a degree in Business Administration.",
                            "quote": "I will graduate with a degree in Business Administration.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-10T10:30:00Z",
                            "t_ingested": "2026-01-10T10:30:00Z",
                            "scope": "org",
                            "ft_score": 4.0
                        }),
                    ],
                    Some("graduate") => vec![json!({
                        "fact_id": "fact:answer",
                        "fact_type": "note",
                        "content": "I will graduate with a degree in Business Administration.",
                        "quote": "I will graduate with a degree in Business Administration.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org",
                        "ft_score": 4.0
                    })],
                    _ => vec![],
                })
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(ShortQueryFallbackDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let lexical_result = select_fact_records_for_query(
            &service.build_context(),
            FactQueryParams {
                namespace: "org",
                scope: "org",
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("What degree did I graduate with?"),
                limit: 5,
                project: None,
                fact_types: &[],
            },
        )
        .await
        .expect("short-query fallback records");

        let fact_ids = lexical_result
            .records
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            lexical_result.retrieval_tier,
            super::RetrievalTier::EpisodeFallback
        );
        assert_eq!(fact_ids.first().copied(), Some("fact:answer"));
    }
}
