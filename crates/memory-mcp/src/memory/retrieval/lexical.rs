//! Lexical/FTS retrieval for fact and episode records.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::models::Fact;
use crate::shared::search::{
    query_hard_anchor_terms, query_term_rarity_weight, query_term_should_be_soft_anchor,
    search_query_terms, unique_query_terms,
};
use crate::shared::search_lexical::{is_four_digit_year, record_query_overlap, record_term_set};
use crate::storage::ContextFactQuery;
use crate::storage::value_helpers::{json_f64, json_string};

use super::filtering::{fact_record_matches_type, raw_array, raw_object};
use super::types::RetrievalTier;

#[derive(Debug)]
pub(crate) struct LexicalQueryResult {
    pub(crate) records: Vec<Value>,
    pub(crate) retrieval_tier: RetrievalTier,
}

pub(crate) struct FactQueryParams<'a> {
    pub(crate) cutoff_iso: &'a str,
    pub(crate) query_opt: Option<&'a str>,
    pub(crate) limit: i32,
    pub(crate) fact_types: &'a [String],
}

pub(crate) async fn select_fact_records_for_query(
    service: &crate::memory::retrieval::RetrievalContext,
    params: FactQueryParams<'_>,
) -> Result<LexicalQueryResult, MemoryError> {
    let query_terms = params.query_opt.map(search_query_terms).unwrap_or_default();
    let candidate_limit = lexical_candidate_limit(params.limit);

    let initial = service
        .knowledge_store()
        .select_facts_filtered(ContextFactQuery {
            cutoff: params.cutoff_iso,
            query_contains: params.query_opt,
            limit: candidate_limit,
            fact_types: params.fact_types,
        })
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
    let initial = rank_lexical_candidates(initial, &query_terms);

    let Some(_query) = params.query_opt else {
        return Ok(LexicalQueryResult {
            records: initial.records,
            retrieval_tier: RetrievalTier::Direct,
        });
    };

    let fallback_terms = build_lexical_fallback_queries(&query_terms);

    let term_groups = service
        .knowledge_store()
        .select_facts_for_lexical_terms(
            params.cutoff_iso,
            &fallback_terms,
            candidate_limit,
            params.fact_types,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
    let mut fallback_records = term_groups.into_iter().flatten().collect::<Vec<_>>();

    let mut seen_fact_ids = HashSet::new();
    fallback_records.retain(|record| {
        let Some(fact_id) = record
            .get("fact_id")
            .and_then(crate::storage::value_helpers::unwrap_record_string)
        else {
            return true;
        };
        seen_fact_ids.insert(fact_id)
    });

    let fallback_records = rank_lexical_candidates(fallback_records, &query_terms);

    let initial_score = initial.top_query_score;
    let fallback_score = fallback_records.top_query_score;
    let best_score = initial_score.max(fallback_score);
    let best_phrase_overlap = initial
        .top_phrase_overlap
        .max(fallback_records.top_phrase_overlap);

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
            records: fallback_records.records,
            retrieval_tier: RetrievalTier::EpisodeFallback,
        });
    }

    if !initial.records.is_empty() {
        return Ok(LexicalQueryResult {
            records: initial.records,
            retrieval_tier: RetrievalTier::Direct,
        });
    }

    let retrieval_tier = if fallback_records.records.is_empty() {
        RetrievalTier::Direct
    } else {
        RetrievalTier::EpisodeFallback
    };

    Ok(LexicalQueryResult {
        records: fallback_records.records,
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
    phrase_overlap: usize,
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

    for window in query_terms.windows(3) {
        let date = window.join(" ");
        if ["%B %d %Y", "%d %B %Y", "%Y %m %d"]
            .iter()
            .any(|format| chrono::NaiveDate::parse_from_str(&date, format).is_ok())
        {
            terms.extend(window.iter().map(String::as_str));
        }
    }

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

async fn scan_fact_records_by_query_terms(
    service: &crate::memory::retrieval::RetrievalContext,
    params: &FactQueryParams<'_>,
    query_terms: &[String],
) -> Result<Vec<Value>, MemoryError> {
    let records = service
        .knowledge_store()
        .scan_facts_as_of(params.cutoff_iso)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let filtered = records
        .into_iter()
        .filter(|record| fact_record_matches_type(record, params.fact_types))
        .filter(|record| record_query_overlap(record, query_terms) > 0)
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

struct RankedLexicalCandidates {
    records: Vec<Value>,
    top_query_score: usize,
    top_phrase_overlap: usize,
}

pub(crate) fn rank_lexical_records(records: Vec<Value>, query_terms: &[String]) -> Vec<Value> {
    rank_lexical_candidates(records, query_terms).records
}

fn rank_lexical_candidates(records: Vec<Value>, query_terms: &[String]) -> RankedLexicalCandidates {
    if query_terms.is_empty() {
        return RankedLexicalCandidates {
            records,
            top_query_score: 0,
            top_phrase_overlap: 0,
        };
    }

    let anchor_profile = build_lexical_anchor_profile(&records, query_terms);

    // Metrics depend on immutable text/index keys and this candidate pool, not
    // on the score written below. Compute once rather than tokenize every
    // source again for both sides of every sort comparison.
    let mut ranked = records
        .into_iter()
        .map(|mut record| {
            let metrics = lexical_record_metrics(&record, query_terms, &anchor_profile);
            let soft_anchor_bonus = if metrics.base_score <= 1 {
                metrics.soft_anchor_mass * 0.35
            } else {
                0.0
            };
            let combined_score = dampened_lexical_ft_score(&record)
                + metrics.base_score as f64
                + (metrics.hard_anchor_mass * 2.0)
                + soft_anchor_bonus;
            if let Some(object) = record.as_object_mut() {
                object.insert("ft_score".to_string(), json!(combined_score));
            } else if let Some(object) = record.get_mut("Object").and_then(Value::as_object_mut) {
                object.insert("ft_score".to_string(), json!(combined_score));
            }
            (record, metrics)
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|(left, left_metrics), (right, right_metrics)| {
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

    let top_query_score = ranked
        .iter()
        .map(|(_, metrics)| metrics.base_score)
        .max()
        .unwrap_or(0);
    let top_phrase_overlap = ranked
        .iter()
        .map(|(_, metrics)| metrics.phrase_overlap)
        .max()
        .unwrap_or(0);
    RankedLexicalCandidates {
        records: ranked.into_iter().map(|(record, _)| record).collect(),
        top_query_score,
        top_phrase_overlap,
    }
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
        let record_terms = record_term_set(record);
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
    let content_terms = best_matching_content_terms(
        raw_object(record)
            .and_then(|map| map.get("content"))
            .and_then(json_string)
            .unwrap_or_default(),
        query_terms,
    );
    let index_terms = raw_object(record)
        .and_then(|map| map.get("index_keys"))
        .and_then(raw_array)
        .into_iter()
        .flatten()
        .filter_map(json_string)
        .flat_map(search_query_terms)
        .collect::<Vec<_>>();
    let record_terms = content_terms
        .iter()
        .chain(&index_terms)
        .collect::<HashSet<_>>();
    let matched_terms = unique_query_terms(query_terms)
        .into_iter()
        .filter(|term| record_terms.contains(term))
        .collect::<Vec<_>>();
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

    let (content_score, phrase_overlap) = content_term_scores(&content_terms, query_terms);
    LexicalRecordMetrics {
        base_score: content_score + query_term_overlap_for_terms(&index_terms, query_terms),
        phrase_overlap,
        weighted_overlap,
        hard_anchor_hits,
        hard_anchor_mass,
        soft_anchor_mass,
    }
}

fn top_query_score(records: &[Value], query_terms: &[String]) -> usize {
    records
        .iter()
        .map(|record| lexical_query_score(record, query_terms))
        .max()
        .unwrap_or(0)
}

fn lexical_query_score(record: &Value, query_terms: &[String]) -> usize {
    let content_terms = best_matching_content_terms(
        raw_object(record)
            .and_then(|map: &serde_json::Map<String, Value>| map.get("content"))
            .and_then(json_string)
            .unwrap_or_default(),
        query_terms,
    );
    score_content_terms(&content_terms, query_terms)
        + lexical_index_key_overlap(record, query_terms)
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
    score_content_terms(&content_terms, query_terms)
}

/// Keep immutable text scores outside the stable comparator. A minimum of one
/// implements the historical query-match filter; zero preserves all admissions.
pub(super) fn rank_episodes_by_query(
    episodes: Vec<crate::models::Episode>,
    query_terms: &[String],
    minimum_score: usize,
) -> Vec<crate::models::Episode> {
    let mut scored = episodes
        .into_iter()
        .map(|episode| {
            let score = lexical_query_score_for_text(&episode.content, query_terms);
            (episode, score)
        })
        .filter(|(_, score)| *score >= minimum_score)
        .collect::<Vec<_>>();
    scored.sort_by(|(left, left_score), (right, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| right.t_ref.cmp(&left.t_ref))
            .then_with(|| left.episode_id.cmp(&right.episode_id))
    });
    scored.into_iter().map(|(episode, _)| episode).collect()
}

pub(crate) fn lexical_query_score_for_fact(fact: &Fact, query_terms: &[String]) -> usize {
    let content_terms = best_matching_content_terms(&fact.content, query_terms);
    score_content_terms(&content_terms, query_terms)
        + lexical_index_key_overlap_for_fact(fact, query_terms)
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
    if query_terms.is_empty() {
        return search_query_terms(text);
    }
    if !crate::shared::search::has_search_query_terms(text) {
        return Vec::new();
    }

    let mut spans = local_content_spans(text);
    let Some(mut best_terms) = spans.next() else {
        return search_query_terms(text);
    };
    let mut best_score = score_content_terms(&best_terms, query_terms);
    let mut best_len = best_terms.len();

    for candidate_terms in spans {
        let candidate_score = score_content_terms(&candidate_terms, query_terms);
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

fn local_content_spans(text: &str) -> impl Iterator<Item = Vec<String>> + '_ {
    // Identical sentences have identical scores and lengths. Their later
    // occurrences cannot replace the first winner, so normalize each once.
    // Keep the original order and the full-text fallback's occurrence counts.
    let mut seen = HashSet::new();
    text.trim()
        .split_inclusive(['.', '!', '?', ';', '\n'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .filter(move |segment| seen.insert(*segment))
        .map(search_query_terms)
        .filter(|terms| !terms.is_empty())
}

fn score_content_terms(content_terms: &[String], query_terms: &[String]) -> usize {
    content_term_scores(content_terms, query_terms).0
}

fn content_term_scores(content_terms: &[String], query_terms: &[String]) -> (usize, usize) {
    let unigram_overlap = query_term_overlap_for_terms(content_terms, query_terms);
    let bigram_overlap = lexical_ngram_overlap_for_terms(content_terms, query_terms, 2);
    let trigram_overlap = lexical_ngram_overlap_for_terms(content_terms, query_terms, 3);
    let phrase_overlap = bigram_overlap + trigram_overlap;
    (
        unigram_overlap + (phrase_overlap * 2) + trigram_overlap,
        phrase_overlap,
    )
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
        .and_then(crate::storage::value_helpers::unwrap_record_string)
        .unwrap_or_default()
        .to_string()
}

pub(crate) async fn select_episode_records_for_query(
    service: &crate::memory::retrieval::RetrievalContext,
    cutoff_iso: &str,
    query_opt: Option<&str>,
    limit: i32,
) -> Result<Vec<Value>, MemoryError> {
    let initial = service
        .episode_context_store()
        .select_episodes_by_content(cutoff_iso, query_opt, limit)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let Some(query) = query_opt else {
        return Ok(initial);
    };

    let query_terms = search_query_terms(query);
    let fallback_terms = build_lexical_fallback_queries(&query_terms);
    if fallback_terms.is_empty() {
        return Ok(initial);
    }

    let term_groups = service
        .episode_context_store()
        .select_episodes_for_terms(cutoff_iso, &fallback_terms, limit)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
    let mut records = initial;
    records.extend(term_groups.into_iter().flatten());

    let mut seen_episode_ids = HashSet::new();
    records.retain(|record| {
        let Some(episode_id) = record
            .get("episode_id")
            .and_then(json_string)
            .or_else(|| record.get("id").and_then(json_string))
        else {
            return true;
        };
        seen_episode_ids.insert(episode_id.to_string())
    });

    Ok(records)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::{
        FactQueryParams, LexicalAnchorProfile, build_lexical_fallback_queries,
        lexical_record_metrics, rank_lexical_records, search_query_terms,
        select_fact_records_for_query,
    };

    #[test]
    fn lexical_score_preserves_global_quote_boundary_empty_guard() {
        assert_eq!(
            super::lexical_query_score_for_text(r#"a"1-2.3"b"#, &["a12".into()]),
            0
        );
    }

    #[test]
    fn repeated_identical_sentences_keep_the_single_best_span_score() {
        assert_eq!(
            super::lexical_query_score_for_text(
                &"archive source details. ".repeat(128),
                &["archive".into(), "source".into()]
            ),
            4
        );
    }

    fn reference_text_score(text: &str, query: &[String]) -> usize {
        fn score(terms: &[String], query: &[String]) -> usize {
            let unigram = query.iter().filter(|term| terms.contains(term)).count();
            let bigram = query
                .windows(2)
                .filter(|q| terms.windows(2).any(|c| c == *q))
                .count();
            let trigram = query
                .windows(3)
                .filter(|q| terms.windows(3).any(|c| c == *q))
                .count();
            unigram + 2 * bigram + 3 * trigram
        }
        let fallback = search_query_terms(text);
        if fallback.is_empty() || query.is_empty() {
            return score(&fallback, query);
        }
        // Literal old sentence traversal, independent of the streaming iterator.
        let mut segments = Vec::new();
        let mut current = String::new();
        for ch in text.trim().chars() {
            current.push(ch);
            if matches!(ch, '.' | '!' | '?' | ';' | '\n') {
                segments.push(std::mem::take(&mut current));
            }
        }
        segments.push(current);
        segments
            .iter()
            .map(|segment| search_query_terms(segment.trim()))
            .filter(|terms| !terms.is_empty())
            .map(|terms| score(&terms, query))
            .max()
            .unwrap_or_else(|| score(&fallback, query))
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases:128, failure_persistence:None,
            rng_seed:proptest::test_runner::RngSeed::Fixed(20261126),
            ..proptest::test_runner::Config::default()
        })]
        #[test]
        fn streaming_scores_match_independent_whole_traversal(
            parts in proptest::collection::vec(proptest::sample::select(vec![
                "archive", "source", "details", "архив", "проектами", "R-0712", "episode:abc123",
                "AND", "a\"1-2.3\"b", " ", "\n", ".", "!", "?", ";", "\r\n", "\"", "échelle"
            ]),0..80),
            query in proptest::sample::select(vec!["archive source", "source archive source", "проект архив", "R-0712 details", "", "a12"])
        ) {
            let text = parts.join(" ");
            let terms = search_query_terms(query);
            proptest::prop_assert_eq!(super::lexical_query_score_for_text(&text,&terms), reference_text_score(&text,&terms));
            proptest::prop_assert_eq!(crate::shared::search::has_search_query_terms(&text), !search_query_terms(&text).is_empty());
        }
    }

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
        let query_terms = crate::shared::search::search_query_terms("300k telemetry response");
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
        let query_terms = crate::shared::search::search_query_terms("openshift rollout");
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

    use super::lexical_candidate_limit;
    use crate::error::MemoryError;
    use crate::storage::DbClient;
    use async_trait::async_trait;

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
        let query_terms = crate::shared::search::search_query_terms(
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
    async fn term_order_preserves_dedup_winner() {
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
                _table: crate::storage::table_scope::OwnedTable,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
                _temporal_fields: &[&str],
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
                _temporal_fields: &[&str],
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                sql: &str,
                vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                // The context store now runs the full-text fact retrieval through
                // the core `query` op; serve the canned per-term records here.
                if sql.contains("search::score") {
                    if sql.starts_with("RETURN [") {
                        return Ok(json!([
                            [],
                            [],
                            [],
                            [
                                {
                                    "fact_id": "fact:shared",
                                    "fact_type": "note",
                                    "content": "Atlas launch is scheduled.",
                                    "quote": "Atlas launch is scheduled.",
                                    "source_episode": "episode:first-term",
                                    "t_valid": "2026-01-10T10:30:00Z",
                                    "t_ingested": "2026-01-10T10:30:00Z",
                                    "scope": "org"
                                },
                                {
                                    "fact_id": "fact:atlas-only",
                                    "fact_type": "note",
                                    "content": "Atlas has a risk review.",
                                    "quote": "Atlas has a risk review.",
                                    "source_episode": "episode:2",
                                    "t_valid": "2026-01-09T10:30:00Z",
                                    "t_ingested": "2026-01-09T10:30:00Z",
                                    "scope": "org"
                                }
                            ],
                            [{
                                "fact_id": "fact:shared",
                                "fact_type": "note",
                                "content": "Atlas launch is scheduled.",
                                "quote": "Atlas launch is scheduled.",
                                "source_episode": "episode:later-term",
                                "t_valid": "2026-01-10T10:30:00Z",
                                "t_ingested": "2026-01-10T10:30:00Z",
                                "scope": "org"
                            }, {
                                "fact_id": "fact:launch-only",
                                "fact_type": "note",
                                "content": "Launch checklist is ready.",
                                "quote": "Launch checklist is ready.",
                                "source_episode": "episode:3",
                                "t_valid": "2026-01-08T10:30:00Z",
                                "t_ingested": "2026-01-08T10:30:00Z",
                                "scope": "org"
                            }],
                            [{
                                "fact_id": "fact:launch-only",
                                "fact_type": "note",
                                "content": "Launch checklist is ready.",
                                "quote": "Launch checklist is ready.",
                                "source_episode": "episode:3",
                                "t_valid": "2026-01-08T10:30:00Z",
                                "t_ingested": "2026-01-08T10:30:00Z",
                                "scope": "org"
                            }]
                        ]));
                    }
                    let query = vars.and_then(|vars| vars["query"].as_str().map(str::to_string));
                    return Ok(Value::Array(match query.as_deref() {
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
                    }));
                }
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(DedupFallbackDbClient),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let retrieval = crate::memory::retrieval_deps::AssembleContextDeps::from(&service);
        let lexical_result = select_fact_records_for_query(
            &retrieval,
            FactQueryParams {
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("atlas launch checklist"),
                limit: 10,
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
        let shared = lexical_result
            .records
            .iter()
            .find(|record| record["fact_id"] == "fact:shared")
            .expect("shared fact survives fallback deduplication");
        assert_eq!(
            shared["source_episode"], "episode:first-term",
            "the earliest term group remains the source of a deduplicated record"
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
                _table: crate::storage::table_scope::OwnedTable,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
                _temporal_fields: &[&str],
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
                _temporal_fields: &[&str],
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                sql: &str,
                vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                // The context store now runs the full-text fact retrieval through
                // the core `query` op; serve the canned per-term records here.
                if sql.contains("search::score") {
                    if sql.starts_with("RETURN [") {
                        let support_group = json!({
                            "fact_id": "fact:support-group",
                            "fact_type": "note",
                            "content": "Caroline attended the LGBTQ support group recently.",
                            "quote": "Caroline attended the LGBTQ support group recently.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-09T10:30:00Z",
                            "t_ingested": "2026-01-09T10:30:00Z",
                            "scope": "org",
                            "ft_score": 5.0
                        });
                        let generic_support = json!({
                            "fact_id": "fact:generic-support",
                            "fact_type": "note",
                            "content": "Customer support team added a new channel.",
                            "quote": "Customer support team added a new channel.",
                            "source_episode": "episode:2",
                            "t_valid": "2026-01-08T10:30:00Z",
                            "t_ingested": "2026-01-08T10:30:00Z",
                            "scope": "org",
                            "ft_score": 3.0
                        });
                        let generic_group = json!({
                            "fact_id": "fact:generic-group",
                            "fact_type": "note",
                            "content": "Project group met to finalize the roadmap.",
                            "quote": "Project group met to finalize the roadmap.",
                            "source_episode": "episode:3",
                            "t_valid": "2026-01-07T10:30:00Z",
                            "t_ingested": "2026-01-07T10:30:00Z",
                            "scope": "org",
                            "ft_score": 3.0
                        });
                        return Ok(json!([
                            [support_group.clone()],
                            [support_group.clone()],
                            [support_group],
                            [],
                            [generic_support],
                            [generic_group]
                        ]));
                    }
                    let query = vars.and_then(|vars| vars["query"].as_str().map(str::to_string));
                    return Ok(Value::Array(match query.as_deref() {
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
                    }));
                }
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(FallbackPreferenceDbClient),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let retrieval = crate::memory::retrieval_deps::AssembleContextDeps::from(&service);
        let lexical_result = select_fact_records_for_query(
            &retrieval,
            FactQueryParams {
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("lgbtq support group"),
                limit: 10,
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
    async fn lexical_scan_fallback_observes_bitemporal_ingestion_and_invalidation() {
        let query = "Orion launch mission";
        let cutoff = "2026-01-15T00:00:00Z";
        let fallback_group_count = build_lexical_fallback_queries(&search_query_terms(query)).len();
        let future_ingested = json!({
            "fact_id": "fact:future-ingested",
            "fact_type": "note",
            "content": "Orion launch mission begins soon.",
            "quote": "Orion launch mission begins soon.",
            "t_valid": "2025-12-01T00:00:00Z",
            "t_ingested": "2026-04-01T00:00:00Z",
            "t_invalid": null,
            "t_invalid_ingested": null,
        });
        let invalidation_ingested_later = json!({
            "fact_id": "fact:late-invalidation",
            "fact_type": "note",
            "content": "Orion launch mission remains valid until its invalidation is known.",
            "quote": "Orion launch mission remains valid until its invalidation is known.",
            "t_valid": "2025-12-01T00:00:00Z",
            "t_ingested": "2025-12-01T00:00:00Z",
            "t_invalid": "2026-01-10T00:00:00Z",
            "t_invalid_ingested": "2026-04-01T00:00:00Z",
        });
        let scan_rows = vec![future_ingested, invalidation_ingested_later.clone()];
        let db = crate::service::mock_db::MockDbClient::new().expect_query_with(
            |sql| sql.contains("FROM fact"),
            move |sql, _vars| {
                if sql.starts_with("RETURN [") {
                    return Ok(Value::Array(
                        (0..fallback_group_count)
                            .map(|_| Value::Array(Vec::new()))
                            .collect(),
                    ));
                }
                if sql.contains("search::score") {
                    return Ok(Value::Array(Vec::new()));
                }
                if sql.contains("t_ingested IS NONE")
                    && sql.contains("t_invalid_ingested > type::datetime($cutoff)")
                {
                    return Ok(json!([invalidation_ingested_later]));
                }
                Ok(json!(scan_rows))
            },
        );
        let service = crate::service::MemoryService::new(
            Arc::new(db),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");
        let retrieval = crate::memory::retrieval_deps::AssembleContextDeps::from(&service);

        let result = select_fact_records_for_query(
            &retrieval,
            FactQueryParams {
                cutoff_iso: cutoff,
                query_opt: Some(query),
                limit: 10,
                fact_types: &[],
            },
        )
        .await
        .expect("select temporal lexical fallback");

        let fact_ids = result
            .records
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(fact_ids, vec!["fact:late-invalidation"]);
    }

    #[tokio::test]
    async fn initial_and_fallback_ties_keep_same_tier() {
        let query = "Orion launch July";
        let query_terms = search_query_terms(query);
        let fallback_group_count = build_lexical_fallback_queries(&query_terms).len();
        assert!(fallback_group_count > 0);

        let record = json!({
            "fact_id": "fact:tie",
            "fact_type": "note",
            "content": query,
            "quote": query,
            "source_episode": "episode:tie",
            "t_valid": "2026-01-10T10:30:00Z",
            "t_ingested": "2026-01-10T10:30:00Z",
            "scope": "org",
            "ft_score": 1.0
        });
        let initial_record = record.clone();
        let fallback_record = record;
        let db = crate::service::mock_db::MockDbClient::new()
            .expect_query_with(
                |sql| sql.contains("search::score") && !sql.starts_with("RETURN ["),
                move |_, _| Ok(json!([initial_record.clone()])),
            )
            .expect_query_with(
                |sql| sql.starts_with("RETURN [") && sql.contains("search::score"),
                move |_, _| {
                    Ok(Value::Array(vec![
                        Value::Array(vec![
                            fallback_record.clone()
                        ]);
                        fallback_group_count
                    ]))
                },
            );
        let service = crate::service::MemoryService::new(
            Arc::new(db),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");
        let retrieval = crate::memory::retrieval_deps::AssembleContextDeps::from(&service);

        let result = select_fact_records_for_query(
            &retrieval,
            FactQueryParams {
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some(query),
                limit: 10,
                fact_types: &[],
            },
        )
        .await
        .expect("tied lexical channels should be ranked");

        assert_eq!(result.retrieval_tier, super::RetrievalTier::Direct);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0]["fact_id"], "fact:tie");
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
                _table: crate::storage::table_scope::OwnedTable,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
                _temporal_fields: &[&str],
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
                _temporal_fields: &[&str],
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                sql: &str,
                vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                // The context store now runs the full-text fact retrieval through
                // the core `query` op; serve the canned per-term records here.
                if sql.contains("search::score") {
                    if sql.starts_with("RETURN [") {
                        let answer = json!({
                            "fact_id": "fact:answer",
                            "fact_type": "note",
                            "content": "I will graduate with a degree in Business Administration.",
                            "quote": "I will graduate with a degree in Business Administration.",
                            "source_episode": "episode:1",
                            "t_valid": "2026-01-10T10:30:00Z",
                            "t_ingested": "2026-01-10T10:30:00Z",
                            "scope": "org",
                            "ft_score": 4.0
                        });
                        let generic = json!({
                            "fact_id": "fact:generic",
                            "fact_type": "note",
                            "content": "The degree committee met to review course requirements.",
                            "quote": "The degree committee met to review course requirements.",
                            "source_episode": "episode:2",
                            "t_valid": "2026-01-09T10:30:00Z",
                            "t_ingested": "2026-01-09T10:30:00Z",
                            "scope": "org",
                            "ft_score": 8.0
                        });
                        return Ok(json!([
                            [answer.clone()],
                            [generic, answer.clone()],
                            [answer]
                        ]));
                    }
                    let query = vars.and_then(|vars| vars["query"].as_str().map(str::to_string));
                    return Ok(Value::Array(match query.as_deref() {
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
                    }));
                }
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(ShortQueryFallbackDbClient),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let retrieval = crate::memory::retrieval_deps::AssembleContextDeps::from(&service);
        let lexical_result = select_fact_records_for_query(
            &retrieval,
            FactQueryParams {
                cutoff_iso: "2026-01-15T10:30:00Z",
                query_opt: Some("What degree did I graduate with?"),
                limit: 5,
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
