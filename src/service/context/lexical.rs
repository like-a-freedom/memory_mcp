//! Lexical/FTS retrieval for fact and episode records.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::models::Fact;
use crate::service::error::MemoryError;
use crate::service::query::search_query_terms;
use crate::service::value_helpers::{json_f64, json_string};

use super::RetrievalTier;
use super::filtering::{
    fact_record_matches_project, fact_record_matches_type, raw_array, raw_object,
};

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

pub(crate) struct FactFilterParams<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff: chrono::DateTime<chrono::Utc>,
    pub(crate) project: Option<&'a str>,
    pub(crate) fact_types: &'a [String],
    pub(crate) access: &'a crate::models::AccessContext,
}

pub(crate) async fn select_fact_records_for_query(
    service: &crate::service::MemoryService,
    params: FactQueryParams<'_>,
) -> Result<LexicalQueryResult, MemoryError> {
    let query_terms = params.query_opt.map(search_query_terms).unwrap_or_default();
    let candidate_limit = lexical_candidate_limit(params.limit);

    let initial = service
        .db_client
        .select_facts_filtered_advanced(
            params.namespace,
            params.scope,
            params.cutoff_iso,
            params.query_opt,
            candidate_limit,
            params.project,
            params.fact_types,
        )
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
            .db_client
            .select_facts_filtered_advanced(
                params.namespace,
                params.scope,
                params.cutoff_iso,
                Some(term.as_str()),
                candidate_limit,
                params.project,
                params.fact_types,
            )
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

fn build_lexical_fallback_queries(query_terms: &[String]) -> Vec<String> {
    let mut queries = Vec::new();

    for width in (2..=3).rev() {
        if query_terms.len() < width {
            continue;
        }
        for window in query_terms.windows(width) {
            let query = window.join(" ");
            if !queries.contains(&query) {
                queries.push(query);
            }
        }
    }

    for term in query_terms {
        if !queries.contains(term) {
            queries.push(term.clone());
        }
    }

    queries
}

async fn scan_fact_records_by_query_terms(
    service: &crate::service::MemoryService,
    params: &FactQueryParams<'_>,
    query_terms: &[String],
) -> Result<Vec<Value>, MemoryError> {
    let records = service
        .db_client
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

    for record in &mut records {
        let combined_score =
            lexical_ft_score(record) + lexical_query_score(record, query_terms) as f64;
        if let Some(object) = record.as_object_mut() {
            object.insert("ft_score".to_string(), json!(combined_score));
        } else if let Some(object) = record.get_mut("Object").and_then(Value::as_object_mut) {
            object.insert("ft_score".to_string(), json!(combined_score));
        }
    }

    records.sort_by(|left, right| {
        lexical_query_score(right, query_terms)
            .cmp(&lexical_query_score(left, query_terms))
            .then_with(|| lexical_ft_score(right).total_cmp(&lexical_ft_score(left)))
            .then_with(|| lexical_t_valid(right).cmp(&lexical_t_valid(left)))
            .then_with(|| lexical_fact_id(left).cmp(&lexical_fact_id(right)))
    });

    records
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
    let unigram_overlap = lexical_query_overlap(record, query_terms);
    let phrase_overlap = lexical_phrase_overlap(record, query_terms);
    let trigram_overlap = lexical_ngram_overlap(record, query_terms, 3);

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

pub(crate) fn lexical_query_score_for_fact(fact: &Fact, query_terms: &[String]) -> usize {
    let content_terms = search_query_terms(&fact.content);
    let unigram_overlap = lexical_query_overlap_for_fact(fact, query_terms);
    let phrase_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 2)
        + lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);
    let trigram_overlap = lexical_ngram_overlap_for_terms(&content_terms, query_terms, 3);

    unigram_overlap + (phrase_overlap * 2) + trigram_overlap
}

fn lexical_phrase_overlap(record: &Value, query_terms: &[String]) -> usize {
    lexical_ngram_overlap(record, query_terms, 2) + lexical_ngram_overlap(record, query_terms, 3)
}

fn lexical_ngram_overlap(record: &Value, query_terms: &[String], width: usize) -> usize {
    if query_terms.len() < width {
        return 0;
    }

    let content_terms = lexical_record_terms(record);
    lexical_ngram_overlap_for_terms(&content_terms, query_terms, width)
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

fn lexical_record_terms(record: &Value) -> Vec<String> {
    raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("content"))
        .and_then(json_string)
        .map(search_query_terms)
        .unwrap_or_default()
}

fn lexical_ft_score(record: &Value) -> f64 {
    raw_object(record)
        .and_then(|map: &serde_json::Map<String, Value>| map.get("ft_score"))
        .and_then(json_f64)
        .unwrap_or(0.0)
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
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff_iso: &str,
    query_opt: Option<&str>,
    limit: i32,
    project: Option<&str>,
) -> Result<Vec<Value>, MemoryError> {
    let initial = service
        .db_client
        .select_episodes_by_content_advanced(
            namespace, scope, cutoff_iso, query_opt, limit, project,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    if !initial.is_empty() || query_opt.is_none() {
        return Ok(initial);
    }

    let Some(query) = query_opt else {
        return Ok(initial);
    };

    let fallback_terms = query
        .split_whitespace()
        .filter(|term| !term.trim().is_empty())
        .collect::<Vec<_>>();
    if fallback_terms.len() < 2 {
        return Ok(initial);
    }

    let mut fallback_records = Vec::new();
    for term in fallback_terms {
        let term_records = service
            .db_client
            .select_episodes_by_content_advanced(
                namespace,
                scope,
                cutoff_iso,
                Some(term),
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
