//! Context assembly operations.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::cache::{CacheKey, CacheView};
use super::embedding::{cosine_similarity, embedding_from_value};
use super::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessContext, AssembleContextRequest, AssembledContextItem};
use crate::storage::{GraphDirection, json_f64, json_string};

const RECIPROCAL_RANK_FUSION_K: f64 = 10.0;

/// Minimum full-text search score for a fact to be considered a quality match.
/// Facts below this threshold matched on common words only and are not useful.
const MIN_FT_SCORE_THRESHOLD: f64 = 0.0;

/// Multiplier applied to the budget when requesting candidates from the DB.
/// A wider net lets the ranking layer pick the best facts instead of being
/// locked into the DB's coarse FTS ordering.
const CANDIDATE_MULTIPLIER: i32 = 10;

/// Expands a query with temporal synonyms for better FTS recall.
///
/// Converts relative temporal expressions like "last month" or "this month"
/// into concrete month/year strings based on the `as_of` context.
fn expand_temporal_synonyms(
    query: &str,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<String> {
    let mut expansions = vec![query.to_string()];
    let q_lower = query.to_lowercase();

    if let Some(as_of_dt) = as_of {
        // "last month" -> actual month name and year
        if q_lower.contains("last month") {
            let last_month = as_of_dt
                .checked_sub_months(chrono::Months::new(1))
                .unwrap_or(as_of_dt);
            let month_name = last_month.format("%B").to_string();
            let year = last_month.format("%Y").to_string();
            let expanded = query
                .replace("last month", &format!("{} {}", month_name, year))
                .replace("Last month", &format!("{} {}", month_name, year));
            if expanded != query {
                expansions.push(expanded);
            }
        }

        // "this month" -> current month name and year
        if q_lower.contains("this month") {
            let month_name = as_of_dt.format("%B").to_string();
            let year = as_of_dt.format("%Y").to_string();
            let expanded = query
                .replace("this month", &format!("{} {}", month_name, year))
                .replace("This month", &format!("{} {}", month_name, year));
            if expanded != query {
                expansions.push(expanded);
            }
        }
    }

    expansions
}

/// Assemble context for a query.
pub async fn assemble_context(
    service: &crate::service::MemoryService,
    request: AssembleContextRequest,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let access = AccessContext::from_payload(request.access.clone());

    service.logger.log(
        crate::log_event!("assemble_context.start", "success",
            "scope" => &request.scope,
            "query" => &request.query,
            "budget" => request.budget
        ),
        LogLevel::Info,
    );

    service.enforce_rate_limit(access.as_ref())?;

    if request.scope.trim().is_empty() {
        return Err(MemoryError::Validation("scope is required".into()));
    }

    let cutoff = request.as_of.unwrap_or_else(super::query::now);
    let access = access.unwrap_or_else(|| AccessContext {
        allowed_scopes: Some(vec![request.scope.clone()]),
        allowed_tags: None,
        caller_id: None,
        session_vars: None,
        transport: None,
        content_type: None,
        cross_scope_allow: None,
    });

    if !service.is_scope_allowed(&request.scope, &access) {
        return Ok(vec![]);
    }

    let cache_key = CacheKey::new(
        &request.query,
        &request.scope,
        cutoff,
        request.budget,
        CacheView::new(
            request.view_mode.as_deref(),
            request.window_start,
            request.window_end,
        ),
        access.allowed_tags.clone(),
    );

    let cached = {
        let mut cache = service.context_cache.write().await;
        cache.get(&cache_key).cloned()
    };

    if let Some(cached) = cached {
        for item in &cached {
            if !item.fact_id.starts_with("fact:") {
                continue;
            }
            if let Err(err) = service.record_fact_access(&item.fact_id, 1).await {
                service.logger.log(
                    crate::log_error!("assemble_context.access_track_error", &err,
                        "fact_id" => &item.fact_id
                    ),
                    LogLevel::Warn,
                );
            }
        }

        service.logger.log(
            crate::log_event!("assemble_context.cache_hit", "success",
                "scope" => &request.scope,
                "query" => &request.query,
                "count" => cached.len()
            ),
            LogLevel::Info,
        );
        return Ok(cached);
    }

    service.logger.log(
        crate::log_event!("assemble_context.cache_miss", "computing",
            "scope" => &request.scope,
            "query" => &request.query,
            "budget" => request.budget
        ),
        LogLevel::Trace,
    );

    let namespace = service.resolve_namespace_for_scope(&request.scope)?;
    let cutoff_iso = super::normalize_dt(cutoff);
    let cleaned_query = super::preprocess_search_query(&request.query);
    let temporal_expansions = expand_temporal_synonyms(&cleaned_query, request.as_of);

    let query_opt = if cleaned_query.is_empty() {
        None
    } else {
        Some(cleaned_query.as_str())
    };

    let fact_records = select_fact_records_for_query(
        service,
        &namespace,
        &request.scope,
        &cutoff_iso,
        query_opt,
        request.budget,
    )
    .await?;

    let direct_facts = filter_facts_by_policy(fact_records, &access);

    // Alias expansion: search for additional facts using entity aliases
    let mut expanded_facts = Vec::new();
    let mut ranked_facts = if let Some(query) = query_opt {
        let alias_expansions = expand_query_with_aliases(service, query, &namespace).await;
        // Combine temporal and alias expansions for broader recall
        let expanded_queries: Vec<String> = temporal_expansions
            .into_iter()
            .filter(|q| q != query)
            .chain(alias_expansions.into_iter().filter(|q| q != query))
            .collect();
        let direct_fact_ids: HashSet<_> = direct_facts
            .iter()
            .map(|fact| fact.fact_id.clone())
            .collect();

        for expanded_query in &expanded_queries {
            if expanded_query == query {
                continue;
            }
            let extra_records = select_fact_records_for_query(
                service,
                &namespace,
                &request.scope,
                &cutoff_iso,
                Some(expanded_query),
                request.budget,
            )
            .await?;
            for fact in filter_facts_by_policy(extra_records, &access) {
                if !direct_fact_ids.contains(&fact.fact_id) {
                    expanded_facts.push(fact);
                }
            }
        }

        let all_direct_ids: HashSet<_> = direct_facts
            .iter()
            .chain(expanded_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        let community_facts = collect_community_facts(
            service,
            CollectCommunityFactsRequest {
                namespace: &namespace,
                scope: &request.scope,
                cutoff_iso: &cutoff_iso,
                query,
                access: &access,
                direct_fact_ids: &all_direct_ids,
                budget: request.budget,
            },
        )
        .await?;

        let excluded_fact_ids = all_direct_ids
            .iter()
            .cloned()
            .chain(community_facts.iter().map(|(fact, _)| fact.fact_id.clone()))
            .collect::<HashSet<_>>();

        let semantic_facts = collect_semantic_facts(
            service,
            CollectSemanticFactsRequest {
                namespace: &namespace,
                scope: &request.scope,
                cutoff,
                query,
                access: &access,
                excluded_fact_ids: &excluded_fact_ids,
                budget: request.budget,
            },
        )
        .await?;

        // Entity-graph expansion: resolve query entities, walk 1-hop neighbors,
        // and retrieve facts for the full entity set. This surfaces facts that
        // FTS alone would miss for multi-hop queries.
        let entity_expansion_facts = collect_entity_expansion_facts(
            service,
            CollectEntityExpansionFactsRequest {
                namespace: &namespace,
                scope: &request.scope,
                cutoff_iso: &cutoff_iso,
                query,
                access: &access,
                excluded_fact_ids: &excluded_fact_ids,
                budget: request.budget,
            },
        )
        .await?;

        // Merge entity expansion facts with semantic facts for ranking
        let mut combined_semantic = semantic_facts;
        combined_semantic.extend(entity_expansion_facts);

        let mut all_direct = direct_facts;
        all_direct.extend(expanded_facts);

        build_ranked_context_facts(
            all_direct,
            community_facts,
            combined_semantic,
            query_opt,
            &request.scope,
            cutoff,
        )
    } else {
        build_ranked_context_facts(
            direct_facts,
            Vec::new(),
            Vec::new(),
            query_opt,
            &request.scope,
            cutoff,
        )
    };

    apply_time_window(&mut ranked_facts, request.window_start, request.window_end);
    if request.view_mode.as_deref() == Some("timeline") {
        sort_ranked_context_facts_for_timeline(&mut ranked_facts);
    } else {
        sort_ranked_context_facts(&mut ranked_facts);
    }

    // Diversify results: cap the number of facts from any single source_episode
    // so one topic cluster cannot dominate the entire result set.
    diversify_ranked_facts(&mut ranked_facts);

    let mut results: Vec<AssembledContextItem> = ranked_facts
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|ranked| {
            let confidence = super::decayed_confidence(&ranked.fact, cutoff);
            AssembledContextItem {
                fact_id: ranked.fact.fact_id,
                content: ranked.fact.content,
                quote: ranked.fact.quote,
                source_episode: ranked.fact.source_episode,
                confidence,
                provenance: ranked.fact.provenance,
                rationale: ranked.rationale,
                t_ref: ranked.fact.t_ref.or(Some(ranked.fact.t_valid)),
                t_valid: Some(ranked.fact.t_valid),
            }
        })
        .collect();

    append_episode_fallback_items(
        service,
        &namespace,
        &request.scope,
        cleaned_query.as_str(),
        request.budget,
        &access,
        request.window_start,
        request.window_end,
        &mut results,
    )
    .await?;

    for item in &results {
        if !item.fact_id.starts_with("fact:") {
            continue;
        }
        if let Err(err) = service.record_fact_access(&item.fact_id, 1).await {
            service.logger.log(
                crate::log_error!("assemble_context.access_track_error", &err,
                    "fact_id" => &item.fact_id
                ),
                LogLevel::Warn,
            );
        }
    }

    {
        let mut cache = service.context_cache.write().await;
        cache.put(cache_key, results.clone());
    }

    service.logger.log(
        crate::log_event!("assemble_context.cache_set", "success",
            "scope" => &request.scope,
            "query" => &request.query,
            "budget" => request.budget,
            "count" => results.len()
        ),
        LogLevel::Trace,
    );

    Ok(results)
}

/// Filter facts by policy tags.
fn filter_facts_by_policy(records: Vec<Value>, access: &AccessContext) -> Vec<crate::models::Fact> {
    let mut facts = Vec::new();

    for record in records {
        let items: Vec<&Value> = if let Some(arr) = record.get("Array").and_then(|v| v.as_array()) {
            arr.iter().collect()
        } else {
            vec![&record]
        };

        for item in items {
            let fact_item = if let Some(obj) = item.get("Object") {
                obj
            } else {
                item
            };

            if let Some(fact) = super::episode::fact_from_record(fact_item) {
                if !fact_allowed_by_policy(&fact, access) {
                    continue;
                }
                facts.push(fact);
            }
        }
    }

    facts
}

fn fact_allowed_by_policy(fact: &crate::models::Fact, access: &AccessContext) -> bool {
    if fact.policy_tags.is_empty() {
        return true;
    }

    let Some(allowed_tags) = &access.allowed_tags else {
        return true;
    };

    let allowed: HashSet<_> = allowed_tags.iter().collect();
    fact.policy_tags.iter().any(|tag| allowed.contains(tag))
}

fn episode_allowed_by_policy(episode: &crate::models::Episode, access: &AccessContext) -> bool {
    if episode.policy_tags.is_empty() {
        return true;
    }

    let Some(allowed_tags) = &access.allowed_tags else {
        return true;
    };

    let allowed: HashSet<_> = allowed_tags.iter().collect();
    episode.policy_tags.iter().any(|tag| allowed.contains(tag))
}

#[allow(clippy::too_many_arguments)]
async fn append_episode_fallback_items(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cleaned_query: &str,
    budget: i32,
    access: &AccessContext,
    window_start: Option<chrono::DateTime<chrono::Utc>>,
    window_end: Option<chrono::DateTime<chrono::Utc>>,
    results: &mut Vec<AssembledContextItem>,
) -> Result<(), MemoryError> {
    if cleaned_query.trim().is_empty() || results.len() >= budget.max(1) as usize {
        return Ok(());
    }

    let remaining = budget.max(1) as usize - results.len();
    let seen_source_episodes = results
        .iter()
        .map(|item| item.source_episode.clone())
        .collect::<HashSet<_>>();

    let episode_records = service
        .db_client
        .select_episodes_by_content(namespace, scope, cleaned_query, remaining as i32)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    for record in episode_records {
        let Some(map) = record.as_object() else {
            continue;
        };
        let Some(episode) = super::episode::episode_from_record(map) else {
            continue;
        };
        if seen_source_episodes.contains(&episode.episode_id) {
            continue;
        }
        if !service
            .db_client
            .select_facts_by_episode_any(namespace, &episode.episode_id, 1)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?
            .is_empty()
        {
            continue;
        }
        if !episode_allowed_by_policy(&episode, access) {
            continue;
        }
        if window_start.is_some_and(|start| episode.t_ref < start) {
            continue;
        }
        if window_end.is_some_and(|end| episode.t_ref > end) {
            continue;
        }

        results.push(AssembledContextItem {
            fact_id: episode.episode_id.clone(),
            content: episode.content.clone(),
            quote: episode.content.clone(),
            source_episode: episode.episode_id.clone(),
            confidence: 0.4,
            provenance: json!({
                "source_episode": episode.episode_id,
                "source_type": episode.source_type,
                "source_id": episode.source_id,
                "fallback": "episode_content"
            }),
            rationale: format!(
                "matched lexical query=\"{}\" via episode content fallback in scope={}",
                cleaned_query, scope
            ),
            t_ref: Some(episode.t_ref),
            t_valid: Some(episode.t_ref),
        });

        if results.len() >= budget.max(1) as usize {
            break;
        }
    }

    Ok(())
}

fn fact_is_active_at(fact: &crate::models::Fact, cutoff: chrono::DateTime<chrono::Utc>) -> bool {
    if fact.t_valid > cutoff || fact.t_ingested > cutoff {
        return false;
    }

    match (fact.t_invalid, fact.t_invalid_ingested) {
        (None, _) => true,
        (Some(invalidated_at), _) if invalidated_at > cutoff => true,
        (_, Some(invalidated_ingested_at)) if invalidated_ingested_at > cutoff => true,
        _ => false,
    }
}

/// Test-only convenience wrapper around the production comparator below.
///
/// Production code uses `compare_facts_by_recency` directly in composite sorts,
/// while tests keep this helper to assert the standalone ordering contract.
#[cfg(test)]
fn sort_facts_by_recency(facts: &mut [crate::models::Fact]) {
    facts.sort_by(compare_facts_by_recency);
}

fn compare_facts_by_recency(
    left: &crate::models::Fact,
    right: &crate::models::Fact,
) -> std::cmp::Ordering {
    right
        .t_valid
        .cmp(&left.t_valid)
        .then_with(|| left.fact_id.cmp(&right.fact_id))
}

#[derive(Debug)]
struct RankedContextFact {
    fact: crate::models::Fact,
    rationale: String,
    fusion_score: f64,
    source_priority: u8,
    decayed_confidence: f64,
}

fn build_ranked_context_facts(
    direct_facts: Vec<crate::models::Fact>,
    community_facts: Vec<(crate::models::Fact, String)>,
    semantic_facts: Vec<(crate::models::Fact, String)>,
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Vec<RankedContextFact> {
    let mut ranked_by_fact_id = std::collections::HashMap::<String, RankedContextFact>::new();

    // Direct lexical matches get full RRF weight
    let direct_weight = 1.0;
    for (rank, fact) in direct_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = super::decayed_confidence(&fact, cutoff);
        let weighted_rrf = reciprocal_rank(rank) * direct_weight;
        ranked_by_fact_id
            .entry(fact_id)
            .and_modify(|candidate| {
                candidate.fusion_score += weighted_rrf;
                candidate.source_priority = 0;
                candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
            })
            .or_insert_with(|| RankedContextFact {
                rationale: default_direct_rationale(query_opt, scope, cutoff),
                fact,
                fusion_score: weighted_rrf,
                source_priority: 0,
                decayed_confidence: confidence,
            });
    }

    // Community facts get reduced weight (0.7) since they're indirect
    let community_weight = 0.7;
    for (rank, (fact, rationale)) in community_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = super::decayed_confidence(&fact, cutoff);
        let weighted_rrf = reciprocal_rank(rank) * community_weight;
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += weighted_rrf;
            candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
            continue;
        }

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                fact,
                rationale,
                fusion_score: weighted_rrf,
                source_priority: 1,
                decayed_confidence: confidence,
            },
        );
    }

    // Semantic/graph facts get reduced weight (0.5) since they're further indirect
    let semantic_weight = 0.5;
    for (rank, (fact, rationale)) in semantic_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = super::decayed_confidence(&fact, cutoff);
        let weighted_rrf = reciprocal_rank(rank) * semantic_weight;
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += weighted_rrf;
            candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
            continue;
        }

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                fact,
                rationale,
                fusion_score: weighted_rrf,
                source_priority: 2,
                decayed_confidence: confidence,
            },
        );
    }

    ranked_by_fact_id.into_values().collect()
}

fn reciprocal_rank(rank: usize) -> f64 {
    1.0 / (RECIPROCAL_RANK_FUSION_K + rank as f64 + 1.0)
}

fn default_direct_rationale(
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> String {
    query_opt.map_or_else(
        || {
            format!(
                "matched scope={} and active at {}",
                scope,
                cutoff.date_naive()
            )
        },
        |query| {
            format!(
                "matched lexical query=\"{}\" in scope={} and active at {}",
                query,
                scope,
                cutoff.date_naive()
            )
        },
    )
}

fn sort_ranked_context_facts(facts: &mut [RankedContextFact]) {
    facts.sort_by(|a, b| {
        // Composite score: fusion_score weighted by decayed_confidence,
        // boosted by the DB's full-text search score when available.
        let ft_boost_a = 1.0 + a.fact.ft_score;
        let ft_boost_b = 1.0 + b.fact.ft_score;
        let score_a = a.fusion_score * a.decayed_confidence.max(0.01) * ft_boost_a;
        let score_b = b.fusion_score * b.decayed_confidence.max(0.01) * ft_boost_b;
        score_b
            .total_cmp(&score_a)
            .then_with(|| a.source_priority.cmp(&b.source_priority))
            .then_with(|| b.fact.t_valid.cmp(&a.fact.t_valid))
            .then_with(|| a.fact.fact_id.cmp(&b.fact.fact_id))
    });
}

fn sort_ranked_context_facts_for_timeline(facts: &mut [RankedContextFact]) {
    facts.sort_by(|a, b| {
        a.fact
            .t_valid
            .cmp(&b.fact.t_valid)
            .then_with(|| a.fact.fact_id.cmp(&b.fact.fact_id))
    });
}

fn apply_time_window(
    facts: &mut Vec<RankedContextFact>,
    window_start: Option<chrono::DateTime<chrono::Utc>>,
    window_end: Option<chrono::DateTime<chrono::Utc>>,
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

/// Diversify ranked facts by capping the number of facts from any single
/// source_episode. This prevents one topic cluster from dominating results
/// when a generic query matches many facts from the same episode chain.
///
/// Strategy: greedy pass in score order, keeping at most `max_per_episode`
/// facts per source_episode. Facts from under-represented episodes get priority.
fn diversify_ranked_facts(facts: &mut Vec<RankedContextFact>) {
    // Allow at most 2 facts from the same source_episode in the final set.
    // This is enough to show both a summary and a detail fact from one topic,
    // but prevents 5+ facts from the same email thread from dominating.
    const MAX_PER_EPISODE: usize = 2;

    let mut episode_counts = std::collections::HashMap::<String, usize>::new();
    let mut kept = Vec::with_capacity(facts.len());

    for fact in facts.drain(..) {
        let episode = fact.fact.source_episode.clone();
        let count = episode_counts.entry(episode).or_insert(0);
        if *count < MAX_PER_EPISODE {
            *count += 1;
            kept.push(fact);
        }
        // else: skip this fact — episode already well-represented
    }

    *facts = kept;
}

/// Expands a search query with entity aliases for broader recall.
///
/// Looks up entities whose canonical names appear in the query,
/// and returns additional query terms derived from their aliases.
async fn expand_query_with_aliases(
    service: &crate::service::MemoryService,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }

    // Collect all n-gram phrases and their positions
    let mut phrase_entries: Vec<(String, usize, usize)> = Vec::new();
    for span_len in (1..=terms.len()).rev() {
        for start in 0..=terms.len().saturating_sub(span_len) {
            let end = start + span_len;
            let phrase = terms[start..end].join(" ");
            if phrase.len() >= 2 {
                phrase_entries.push((phrase, start, end));
            }
        }
    }

    // Deduplicate normalized names for batch lookup
    let normalized_names: Vec<String> = phrase_entries
        .iter()
        .map(|(phrase, _, _)| super::normalize_text(phrase))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Add partial name matching: try individual capitalized words as entity names
    let mut partial_names: Vec<String> = Vec::new();
    for term in &terms {
        if term.len() >= 3
            && term
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
        {
            partial_names.push(super::normalize_text(term));
        }
    }

    // Combine with existing phrase lookups, deduplicating
    let all_lookup_names: Vec<String> = normalized_names
        .into_iter()
        .chain(partial_names)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // Single batch query instead of O(N²) individual lookups
    let entities = service
        .db_client
        .select_entities_batch(namespace, &all_lookup_names)
        .await
        .unwrap_or_default();

    // Build lookup map: normalized_name → aliases
    let mut entity_aliases: HashMap<String, Vec<String>> = HashMap::new();
    for entity in &entities {
        let obj = match entity.as_object() {
            Some(obj) => obj,
            None => continue,
        };
        // Use canonical_name_normalized as primary key, fall back to normalizing canonical_name
        let canonical_norm = obj
            .get("canonical_name_normalized")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                obj.get("canonical_name")
                    .and_then(|v| v.as_str())
                    .map(super::normalize_text)
            })
            .unwrap_or_default();
        let aliases: Vec<String> = obj
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !canonical_norm.is_empty() && !aliases.is_empty() {
            entity_aliases.entry(canonical_norm).or_insert(aliases);
        }
    }

    // Expand queries using matched entities
    let mut expanded = HashSet::new();
    for (phrase, start, end) in &phrase_entries {
        let normalized = super::normalize_text(phrase);
        if let Some(aliases) = entity_aliases.get(&normalized) {
            for alias_str in aliases {
                let mut parts: Vec<String> = terms[..*start]
                    .iter()
                    .map(|term| (*term).to_string())
                    .collect();
                parts.push(alias_str.clone());
                parts.extend(terms[*end..].iter().map(|term| (*term).to_string()));
                let alias_expanded = parts.join(" ");

                if alias_expanded != query {
                    expanded.insert(alias_expanded);
                }
            }
        }
    }

    expanded.into_iter().collect()
}

#[cfg(test)]
#[allow(dead_code)]
async fn expand_query_with_aliases_for_test(
    service: &crate::service::MemoryService,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    expand_query_with_aliases(service, query, namespace).await
}

async fn select_fact_records_for_query(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff_iso: &str,
    query_opt: Option<&str>,
    limit: i32,
) -> Result<Vec<Value>, MemoryError> {
    // Request a wider candidate pool so the ranking layer has choices beyond
    // the DB's coarse FTS ordering.
    let candidate_limit = (limit * CANDIDATE_MULTIPLIER).max(50);

    let initial = service
        .db_client
        .select_facts_filtered(namespace, scope, cutoff_iso, query_opt, candidate_limit)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    // Apply a quality gate: only accept facts with meaningful FTS scores.
    let qualified: Vec<_> = initial
        .into_iter()
        .filter(|record| {
            // When no query was provided, all active facts are acceptable.
            if query_opt.is_none() {
                return true;
            }
            let score = record.get("ft_score").and_then(json_f64).unwrap_or(0.0);
            score >= MIN_FT_SCORE_THRESHOLD
        })
        .take(limit as usize)
        .collect();

    if !qualified.is_empty() {
        return Ok(qualified);
    }

    // Fallback: search by individual terms when the full query yields no
    // qualified candidates.
    let Some(query) = query_opt else {
        return Ok(Vec::new());
    };

    let fallback_terms = query
        .split_whitespace()
        .filter(|term| !term.trim().is_empty())
        .collect::<Vec<_>>();
    if fallback_terms.len() < 2 {
        return Ok(Vec::new());
    }

    let mut fallback_records = Vec::new();
    for term in fallback_terms {
        let term_records = service
            .db_client
            .select_facts_filtered(namespace, scope, cutoff_iso, Some(term), candidate_limit)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
        fallback_records.extend(term_records);
    }

    // Deduplicate and apply quality gate to fallback results
    let mut seen_fact_ids = std::collections::HashSet::new();
    fallback_records.retain(|record| {
        let Some(fact_id) = record
            .get("fact_id")
            .and_then(super::episode::unwrap_record_string)
        else {
            return true;
        };
        if !seen_fact_ids.insert(fact_id) {
            return false;
        }
        let score = record.get("ft_score").and_then(json_f64).unwrap_or(0.0);
        score >= MIN_FT_SCORE_THRESHOLD
    });

    Ok(fallback_records.into_iter().take(limit as usize).collect())
}

struct CollectCommunityFactsRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff_iso: &'a str,
    query: &'a str,
    access: &'a AccessContext,
    direct_fact_ids: &'a std::collections::HashSet<String>,
    budget: i32,
}

struct CollectSemanticFactsRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff: chrono::DateTime<chrono::Utc>,
    query: &'a str,
    access: &'a AccessContext,
    excluded_fact_ids: &'a HashSet<String>,
    budget: i32,
}

#[derive(Debug, Clone)]
struct CommunityMatch {
    rank: usize,
    community_id: String,
    summary: String,
}

async fn collect_community_facts(
    service: &crate::service::MemoryService,
    request: CollectCommunityFactsRequest<'_>,
) -> Result<Vec<(crate::models::Fact, String)>, MemoryError> {
    let matched_communities =
        find_matching_communities(service, request.namespace, request.query).await?;
    if matched_communities.is_empty() {
        return Ok(Vec::new());
    }

    let member_ids = matched_communities
        .iter()
        .flat_map(|community| community.member_entities.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let fallback_records = service
        .db_client
        .select_facts_by_entity_links(
            request.namespace,
            request.scope,
            request.cutoff_iso,
            &member_ids,
            request.budget.max(1),
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let community_summary_by_member = matched_communities
        .iter()
        .enumerate()
        .flat_map(|(rank, community)| {
            community
                .member_entities
                .iter()
                .cloned()
                .map(move |entity_id| {
                    (
                        entity_id,
                        CommunityMatch {
                            rank,
                            community_id: community.community_id.clone(),
                            summary: community.summary.clone(),
                        },
                    )
                })
        })
        .collect::<std::collections::HashMap<_, _>>();

    let mut facts = filter_facts_by_policy(fallback_records, request.access)
        .into_iter()
        .filter(|fact| !request.direct_fact_ids.contains(&fact.fact_id))
        .filter(|fact| {
            fact.entity_links
                .iter()
                .any(|entity_id| member_ids.iter().any(|member_id| member_id == entity_id))
        })
        .collect::<Vec<_>>();
    facts.sort_by(|left, right| {
        let left_rank = best_community_match(left, &community_summary_by_member)
            .map(|matched| matched.rank)
            .unwrap_or(usize::MAX);
        let right_rank = best_community_match(right, &community_summary_by_member)
            .map(|matched| matched.rank)
            .unwrap_or(usize::MAX);

        left_rank
            .cmp(&right_rank)
            .then_with(|| compare_facts_by_recency(left, right))
    });

    Ok(facts
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|fact| {
            let rationale = best_community_match(&fact, &community_summary_by_member).map_or_else(
                || format!("matched community summary for query=\"{}\"", request.query),
                |matched| {
                    format!(
                        "matched community summary for query=\"{}\" via {}: {}",
                        request.query, matched.community_id, matched.summary
                    )
                },
            );
            (fact, rationale)
        })
        .collect())
}

async fn collect_semantic_facts(
    service: &crate::service::MemoryService,
    request: CollectSemanticFactsRequest<'_>,
) -> Result<Vec<(crate::models::Fact, String)>, MemoryError> {
    let query_embedding = match service.generate_embedding(request.query).await {
        Ok(Some(embedding)) => embedding,
        Ok(None) => return Ok(Vec::new()),
        Err(err) => {
            service.logger.log(
                std::collections::HashMap::from([
                    ("op".to_string(), json!("embedding.query_skipped")),
                    (
                        "provider".to_string(),
                        json!(service.embedding_provider.provider_name()),
                    ),
                    ("error".to_string(), json!(err.to_string())),
                ]),
                LogLevel::Warn,
            );
            return Ok(Vec::new());
        }
    };

    if query_embedding.is_empty() {
        return Ok(Vec::new());
    }

    // Request more candidates than budget since HNSW results may be filtered
    // by temporal/scope constraints post-search
    let search_limit = request.budget.max(1) * 4;

    let fact_records = service
        .db_client
        .select_facts_ann(
            request.namespace,
            request.scope,
            &super::normalize_dt(request.cutoff),
            &query_embedding,
            search_limit,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut ranked_facts = Vec::new();
    for record in fact_records {
        let Some(fact) = super::episode::fact_from_record(&record) else {
            continue;
        };

        if fact.scope != request.scope
            || request.excluded_fact_ids.contains(&fact.fact_id)
            || !fact_allowed_by_policy(&fact, request.access)
            || !fact_is_active_at(&fact, request.cutoff)
        {
            continue;
        }

        // Use DB-computed sem_score if available, otherwise compute in Rust
        let similarity = record
            .as_object()
            .and_then(|map| map.get("sem_score"))
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| {
                let embedding = record
                    .as_object()
                    .and_then(|map| map.get("embedding"))
                    .and_then(embedding_from_value);
                match embedding {
                    Some(ref emb) if emb.len() == query_embedding.len() => {
                        cosine_similarity(&query_embedding, emb)
                    }
                    _ => 0.0,
                }
            });

        if similarity < service.embedding_similarity_threshold {
            continue;
        }

        ranked_facts.push((similarity, fact));
    }

    ranked_facts.sort_by(
        |(left_similarity, left_fact), (right_similarity, right_fact)| {
            right_similarity
                .total_cmp(left_similarity)
                .then_with(|| compare_facts_by_recency(left_fact, right_fact))
        },
    );

    Ok(ranked_facts
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|(similarity, fact)| {
            (
                fact,
                format!(
                    "matched semantic similarity={similarity:.3} for query=\"{}\"",
                    request.query
                ),
            )
        })
        .collect())
}

/// Request parameters for entity-graph expansion fact collection.
struct CollectEntityExpansionFactsRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff_iso: &'a str,
    query: &'a str,
    access: &'a AccessContext,
    excluded_fact_ids: &'a HashSet<String>,
    budget: i32,
}

/// Detects queries that likely require multi-hop graph traversal.
fn is_relationship_query(query: &str) -> bool {
    let q_lower = query.to_lowercase();
    q_lower.contains("who knows")
        || q_lower.contains("introduce")
        || q_lower.contains("connected to")
        || q_lower.contains("relationship")
        || q_lower.contains("knows who")
        || q_lower.contains("mutual")
        || q_lower.contains("connection")
}

/// Retrieves facts linked to entities resolved from the query, plus their 1-hop graph neighbors.
///
/// Pipeline: NER extract → entity batch lookup → edge neighbor expansion → fact retrieval.
/// This enables the server to surface facts about related entities that FTS alone would miss.
/// For relationship-shaped queries, expands to 2-hop neighbors to find indirect connections.
async fn collect_entity_expansion_facts(
    service: &crate::service::MemoryService,
    request: CollectEntityExpansionFactsRequest<'_>,
) -> Result<Vec<(crate::models::Fact, String)>, MemoryError> {
    // Step A: extract candidate entity names from query text via NER
    let entity_names = service
        .entity_extractor
        .extract_candidates(request.query)
        .await
        .unwrap_or_default();

    if entity_names.is_empty() {
        return Ok(Vec::new());
    }

    // Step B: resolve names to entity_ids (batch lookup)
    let normalized: Vec<String> = entity_names
        .iter()
        .map(|e| super::normalize_text(&e.canonical_name))
        .collect();

    let seed_entities = service
        .db_client
        .select_entities_batch(request.namespace, &normalized)
        .await
        .unwrap_or_default();

    if seed_entities.is_empty() {
        return Ok(Vec::new());
    }

    // Step C: expand to 1-hop neighbors via entity graph edges
    let mut all_entity_ids: Vec<String> = seed_entities
        .iter()
        .filter_map(|v| {
            v.get("entity_id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
        .collect();

    for entity in &seed_entities {
        let Some(entity_id) = entity.get("entity_id").and_then(|v| v.as_str()) else {
            continue;
        };
        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            let neighbors = service
                .db_client
                .select_edge_neighbors(request.namespace, entity_id, request.cutoff_iso, direction)
                .await
                .unwrap_or_default();

            for neighbor in neighbors {
                if let Some(neighbor_id) = neighbor.get("neighbor_id").and_then(|v| v.as_str()) {
                    let neighbor_str = neighbor_id.to_string();
                    if !all_entity_ids.contains(&neighbor_str) {
                        all_entity_ids.push(neighbor_str);
                    }
                }
            }
        }
    }

    // 2-hop expansion for relationship queries
    if is_relationship_query(request.query) {
        let first_hop_ids: Vec<String> = all_entity_ids.clone();
        for entity_id in &first_hop_ids {
            for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
                let neighbors = service
                    .db_client
                    .select_edge_neighbors(
                        request.namespace,
                        entity_id,
                        request.cutoff_iso,
                        direction,
                    )
                    .await
                    .unwrap_or_default();

                for neighbor in neighbors {
                    if let Some(neighbor_id) = neighbor.get("neighbor_id").and_then(|v| v.as_str())
                    {
                        let neighbor_str = neighbor_id.to_string();
                        if !all_entity_ids.contains(&neighbor_str) {
                            all_entity_ids.push(neighbor_str);
                        }
                    }
                }
            }
        }
    }

    if all_entity_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Step D: retrieve facts for the full entity set
    let search_limit = request.budget.max(1) * 3;
    let fact_records = service
        .db_client
        .select_facts_by_entity_links(
            request.namespace,
            request.scope,
            request.cutoff_iso,
            &all_entity_ids,
            search_limit,
        )
        .await
        .unwrap_or_default();

    Ok(fact_records
        .into_iter()
        .filter_map(|record| {
            let fact = super::episode::fact_from_record(&record)?;
            if fact.scope != request.scope
                || request.excluded_fact_ids.contains(&fact.fact_id)
                || !fact_allowed_by_policy(&fact, request.access)
                || !fact_is_active_at(&fact, chrono::Utc::now())
            {
                return None;
            }
            Some((fact, "matched entity-graph expansion".to_string()))
        })
        .collect())
}

#[derive(Debug)]
struct StoredCommunitySummary {
    community_id: String,
    summary: String,
    member_entities: Vec<String>,
    ft_score: f64,
}

async fn find_matching_communities(
    service: &crate::service::MemoryService,
    namespace: &str,
    query: &str,
) -> Result<Vec<StoredCommunitySummary>, MemoryError> {
    let communities = service
        .db_client
        .select_communities_matching_summary(namespace, query)
        .await?;

    let mut matched = communities
        .iter()
        .filter_map(stored_community_summary_from_value)
        .collect::<Vec<_>>();
    matched.sort_by(|left, right| {
        right
            .ft_score
            .total_cmp(&left.ft_score)
            .then_with(|| left.community_id.cmp(&right.community_id))
    });

    Ok(matched)
}

fn stored_community_summary_from_value(value: &Value) -> Option<StoredCommunitySummary> {
    let map = value.as_object()?;
    let community_id = map
        .get("community_id")
        .and_then(json_string)
        .or_else(|| map.get("id").and_then(json_string))?
        .to_string();
    let summary = map
        .get("summary")
        .and_then(json_string)
        .unwrap_or_default()
        .to_string();
    let member_entities = map
        .get("member_entities")
        .and_then(unwrap_context_array)
        .map(|values| {
            values
                .iter()
                .filter_map(json_string)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let ft_score = map.get("ft_score").and_then(json_f64).unwrap_or(0.0);

    if summary.is_empty() || member_entities.is_empty() {
        return None;
    }

    Some(StoredCommunitySummary {
        community_id,
        summary,
        member_entities,
        ft_score,
    })
}

fn unwrap_context_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        Some(array)
    } else if let Some(object) = value.as_object() {
        object.get("Array").and_then(Value::as_array)
    } else {
        None
    }
}

fn best_community_match<'a>(
    fact: &crate::models::Fact,
    matches_by_entity: &'a std::collections::HashMap<String, CommunityMatch>,
) -> Option<&'a CommunityMatch> {
    fact.entity_links
        .iter()
        .filter_map(|entity_id| matches_by_entity.get(entity_id))
        .min_by(|left, right| left.rank.cmp(&right.rank))
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_EMBEDDING_DIMENSION;
    use crate::service::EmbeddingProvider;
    use crate::service::test_support::MockDb;
    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn create_test_fact(fact_id: &str, t_valid: chrono::DateTime<Utc>) -> crate::models::Fact {
        crate::models::Fact {
            fact_id: fact_id.to_string(),
            fact_type: "note".to_string(),
            content: "Test content".to_string(),
            quote: "Test quote".to_string(),
            source_episode: "episode:123".to_string(),
            t_ref: Some(t_valid),
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
            provenance: json!({}),
            ft_score: 0.0,
        }
    }

    #[test]
    fn sort_facts_by_recency_orders_by_date_desc() {
        let t1 = Utc::now();
        let t2 = t1 - chrono::Duration::hours(1);
        let t3 = t1 - chrono::Duration::hours(2);

        let mut facts = vec![
            create_test_fact("fact:3", t3),
            create_test_fact("fact:1", t1),
            create_test_fact("fact:2", t2),
        ];

        sort_facts_by_recency(&mut facts);

        assert_eq!(facts[0].fact_id, "fact:1");
        assert_eq!(facts[1].fact_id, "fact:2");
        assert_eq!(facts[2].fact_id, "fact:3");
    }

    #[test]
    fn sort_facts_by_recency_breaks_ties_with_id() {
        let t = Utc::now();

        let mut facts = vec![
            create_test_fact("fact:b", t),
            create_test_fact("fact:a", t),
            create_test_fact("fact:c", t),
        ];

        sort_facts_by_recency(&mut facts);

        assert_eq!(facts[0].fact_id, "fact:a");
        assert_eq!(facts[1].fact_id, "fact:b");
        assert_eq!(facts[2].fact_id, "fact:c");
    }

    #[tokio::test]
    async fn select_fact_records_for_query_deduplicates_term_fallback_records() {
        let mut db_client = MockDb::default();
        db_client.select_facts_filtered_fn = Box::new(|_, _, _, query_contains, _| {
            Ok(match query_contains {
                Some("atlas launch") => vec![],
                Some("atlas") => vec![
                    json!({
                        "fact_id": "fact:shared",
                        "fact_type": "note",
                        "content": "Atlas launch is scheduled.",
                        "quote": "Atlas launch is scheduled.",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-10T10:30:00Z",
                        "t_ingested": "2026-01-10T10:30:00Z",
                        "scope": "org",
                        "ft_score": 2.5
                    }),
                    json!({
                        "fact_id": "fact:atlas-only",
                        "fact_type": "note",
                        "content": "Atlas has a risk review.",
                        "quote": "Atlas has a risk review.",
                        "source_episode": "episode:2",
                        "t_valid": "2026-01-09T10:30:00Z",
                        "t_ingested": "2026-01-09T10:30:00Z",
                        "scope": "org",
                        "ft_score": 1.8
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
                        "scope": "org",
                        "ft_score": 2.5
                    }),
                    json!({
                        "fact_id": "fact:launch-only",
                        "fact_type": "note",
                        "content": "Launch checklist is ready.",
                        "quote": "Launch checklist is ready.",
                        "source_episode": "episode:3",
                        "t_valid": "2026-01-08T10:30:00Z",
                        "t_ingested": "2026-01-08T10:30:00Z",
                        "scope": "org",
                        "ft_score": 1.5
                    }),
                ],
                _ => vec![],
            })
        });

        let service = crate::service::MemoryService::new(
            Arc::new(db_client),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let records = select_fact_records_for_query(
            &service,
            "org",
            "org",
            "2026-01-15T10:30:00Z",
            Some("atlas launch"),
            10,
        )
        .await
        .expect("fallback records");

        let fact_ids = records
            .iter()
            .filter_map(|record| record.get("fact_id").and_then(Value::as_str))
            .collect::<Vec<_>>();

        assert_eq!(
            fact_ids,
            vec!["fact:shared", "fact:atlas-only", "fact:launch-only"]
        );
    }

    #[test]
    fn sort_facts_by_recency_handles_empty() {
        let mut facts: Vec<crate::models::Fact> = vec![];
        sort_facts_by_recency(&mut facts);
        assert!(facts.is_empty());
    }

    #[test]
    fn sort_facts_by_recency_handles_single() {
        let mut facts = vec![create_test_fact("fact:1", Utc::now())];
        sort_facts_by_recency(&mut facts);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_returns_empty_for_empty_input() {
        let access = AccessContext::default();
        let result = filter_facts_by_policy(vec![], &access);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_facts_by_policy_skips_invalid_records() {
        let access = AccessContext::default();
        let records = vec![json!({"invalid": "data"})];
        let result = filter_facts_by_policy(records, &access);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_facts_by_policy_filters_by_allowed_tags() {
        let mut fact1 = create_test_fact("fact:1", Utc::now());
        fact1.policy_tags = vec!["allowed".to_string(), "other".to_string()];

        let mut fact2 = create_test_fact("fact:2", Utc::now());
        fact2.policy_tags = vec!["blocked".to_string()];

        let access = AccessContext {
            allowed_scopes: None,
            allowed_tags: Some(vec!["allowed".to_string()]),
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };

        let records = vec![
            json!({
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["allowed", "other"]
            }),
            json!({
                "fact_id": "fact:2",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["blocked"]
            }),
        ];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_allows_all_when_no_tags_specified() {
        let access = AccessContext {
            allowed_scopes: None,
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };

        let records = vec![
            json!({
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["tag1"]
            }),
            json!({
                "fact_id": "fact:2",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org"
            }),
        ];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_facts_by_policy_handles_wrapped_objects() {
        let access = AccessContext::default();

        let records = vec![json!({
            "Object": {
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org"
            }
        })];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_handles_array_wrapped_objects() {
        let access = AccessContext::default();

        let records = vec![json!({
            "Array": [
                {
                    "Object": {
                        "fact_id": "fact:1",
                        "fact_type": "note",
                        "content": "Test",
                        "quote": "Quote",
                        "source_episode": "episode:1",
                        "t_valid": "2024-01-15T10:30:00Z",
                        "scope": "org"
                    }
                },
                {
                    "Object": {
                        "fact_id": "fact:2",
                        "fact_type": "note",
                        "content": "Test2",
                        "quote": "Quote2",
                        "source_episode": "episode:2",
                        "t_valid": "2024-01-15T10:30:00Z",
                        "scope": "org"
                    }
                }
            ]
        })];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn assemble_context_uses_db_side_community_lookup_for_summary_matches() {
        let community_lookup_calls = Arc::new(AtomicUsize::new(0));
        let entity_link_fact_calls = Arc::new(AtomicUsize::new(0));
        let mut db_client = MockDb::default();
        db_client.select_table_fn = Box::new(|table, _| {
            assert_eq!(table, "fact");
            Ok(vec![])
        });
        db_client.select_facts_filtered_fn = Box::new(|_, _, _, query_contains, _| {
            if query_contains.is_some() {
                Ok(vec![])
            } else {
                panic!(
                    "community fact expansion should not use unfiltered select_facts_filtered fallback"
                )
            }
        });
        db_client.select_facts_by_entity_links_fn = Box::new({
            let entity_link_fact_calls = Arc::clone(&entity_link_fact_calls);
            move |_, _, _, entity_links, _| {
                entity_link_fact_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(entity_links, &["entity:alice".to_string()]);

                Ok(vec![
                    json!({
                        "fact_id": "fact:community",
                        "fact_type": "note",
                        "content": "Alice works on project Atlas",
                        "quote": "Alice works on project Atlas",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-15T10:30:00Z",
                        "t_ingested": "2026-01-15T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:alice"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:1"}
                    }),
                    json!({
                        "fact_id": "fact:other",
                        "fact_type": "note",
                        "content": "Mallory works elsewhere",
                        "quote": "Mallory works elsewhere",
                        "source_episode": "episode:2",
                        "t_valid": "2026-01-15T10:30:00Z",
                        "t_ingested": "2026-01-15T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:mallory"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:2"}
                    }),
                ])
            }
        });
        db_client.select_communities_matching_summary_fn = Box::new({
            let community_lookup_calls = Arc::clone(&community_lookup_calls);
            move |_, query| {
                community_lookup_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(query, "alice atlas");

                Ok(vec![json!({
                    "community_id": "community:atlas",
                    "summary": "Alice and the Atlas project team",
                    "member_entities": ["entity:alice"]
                })])
            }
        });

        let db_client = Arc::new(db_client);
        let service = crate::service::MemoryService::new(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "alice atlas".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(community_lookup_calls.load(Ordering::SeqCst), 1);
        assert_eq!(entity_link_fact_calls.load(Ordering::SeqCst), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:community");
        assert!(results[0].rationale.contains("community:atlas"));
    }

    #[tokio::test]
    async fn assemble_context_without_lexical_or_graph_matches_returns_empty() {
        let service = crate::service::MemoryService::new(
            Arc::new(MockDb::default()),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "alice platform".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert!(
            results.is_empty(),
            "without lexical or graph matches, assemble_context should return no results"
        );
    }

    #[tokio::test]
    async fn assemble_context_prefers_direct_lexical_matches_over_newer_community_expansion() {
        let mut db_client = MockDb::default();
        db_client.select_facts_filtered_fn = Box::new(|_, _, _, query_contains, _| {
            assert_eq!(query_contains, Some("atlas launch"));
            Ok(vec![json!({
                "fact_id": "fact:direct",
                "fact_type": "note",
                "content": "Atlas launch checklist is blocked on DNS cutover.",
                "quote": "Atlas launch checklist is blocked on DNS cutover.",
                "source_episode": "episode:direct",
                "t_valid": "2026-01-10T10:30:00Z",
                "t_ingested": "2026-01-10T10:30:00Z",
                "scope": "org",
                "entity_links": ["entity:atlas"],
                "policy_tags": [],
                "provenance": {"source_episode": "episode:direct"},
                "ft_score": 100.0
            })])
        });
        db_client.select_facts_by_entity_links_fn = Box::new(|_, _, _, entity_links, _| {
            assert_eq!(entity_links, &["entity:atlas".to_string()]);
            Ok(vec![json!({
                "fact_id": "fact:community",
                "fact_type": "note",
                "content": "Atlas team sync moved to Friday.",
                "quote": "Atlas team sync moved to Friday.",
                "source_episode": "episode:community",
                "t_valid": "2026-01-15T10:30:00Z",
                "t_ingested": "2026-01-15T10:30:00Z",
                "scope": "org",
                "entity_links": ["entity:atlas"],
                "policy_tags": [],
                "provenance": {"source_episode": "episode:community"}
            })])
        });
        db_client.select_communities_matching_summary_fn = Box::new(|_, query| {
            assert_eq!(query, "atlas launch");
            Ok(vec![json!({
                "community_id": "community:atlas",
                "summary": "Atlas launch workstream",
                "member_entities": ["entity:atlas"]
            })])
        });

        let service = crate::service::MemoryService::new(
            Arc::new(db_client),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "atlas launch".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].fact_id, "fact:direct");
        assert!(
            results[0].rationale.contains("lexical"),
            "direct lexical result should explain itself as a lexical match, got: {}",
            results[0].rationale
        );
        assert_eq!(results[1].fact_id, "fact:community");
        assert!(results[1].rationale.contains("community:atlas"));
    }

    #[tokio::test]
    async fn assemble_context_orders_community_facts_by_matching_summary_relevance() {
        let mut db_client = MockDb::default();
        db_client.select_facts_by_entity_links_fn = Box::new(|_, _, _, entity_links, _| {
            assert_eq!(
                entity_links,
                &["entity:alpha".to_string(), "entity:beta".to_string()]
            );

            Ok(vec![
                json!({
                    "fact_id": "fact:beta",
                    "fact_type": "note",
                    "content": "Beta launch note.",
                    "quote": "Beta launch note.",
                    "source_episode": "episode:beta",
                    "t_valid": "2026-01-20T10:30:00Z",
                    "t_ingested": "2026-01-20T10:30:00Z",
                    "scope": "org",
                    "entity_links": ["entity:beta"],
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:beta"}
                }),
                json!({
                    "fact_id": "fact:alpha",
                    "fact_type": "note",
                    "content": "Alpha launch note.",
                    "quote": "Alpha launch note.",
                    "source_episode": "episode:alpha",
                    "t_valid": "2026-01-10T10:30:00Z",
                    "t_ingested": "2026-01-10T10:30:00Z",
                    "scope": "org",
                    "entity_links": ["entity:alpha"],
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:alpha"}
                }),
            ])
        });
        db_client.select_communities_matching_summary_fn = Box::new(|_, query| {
            assert_eq!(query, "launch workstream");
            Ok(vec![
                json!({
                    "community_id": "community:alpha",
                    "summary": "Alpha launch workstream",
                    "member_entities": ["entity:alpha"],
                    "ft_score": 20.0
                }),
                json!({
                    "community_id": "community:beta",
                    "summary": "Beta launch workstream",
                    "member_entities": ["entity:beta"],
                    "ft_score": 10.0
                }),
            ])
        });

        let service = crate::service::MemoryService::new(
            Arc::new(db_client),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "launch workstream".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].fact_id, "fact:alpha");
        assert!(results[0].rationale.contains("community:alpha"));
        assert_eq!(results[1].fact_id, "fact:beta");
    }

    #[tokio::test]
    async fn assemble_context_uses_provider_backed_semantic_similarity() {
        let mut db_client = MockDb::default();
        db_client.select_table_fn =
            Box::new(|_, _| panic!("semantic retrieval should not scan the full fact table"));
        db_client.select_facts_ann_fn = Box::new(|_, _, _, _, _| {
            let mut embedding = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
            embedding[0] = 1.0;
            Ok(vec![json!({
                "fact_id": "fact:semantic",
                "fact_type": "note",
                "content": "Compensation increase approved for the engineering team",
                "quote": "Compensation increase approved",
                "source_episode": "episode:semantic",
                "t_valid": "2026-01-15T10:30:00Z",
                "t_ingested": "2026-01-15T10:30:00Z",
                "scope": "org",
                "entity_links": [],
                "policy_tags": [],
                "confidence": 0.9,
                "provenance": {},
                "embedding": embedding,
                "sem_score": 0.99,
            })])
        });

        struct SemanticEmbeddingProvider;

        #[async_trait]
        impl EmbeddingProvider for SemanticEmbeddingProvider {
            fn is_enabled(&self) -> bool {
                true
            }

            fn provider_name(&self) -> &'static str {
                "test"
            }

            fn dimension(&self) -> usize {
                DEFAULT_EMBEDDING_DIMENSION
            }

            async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
                let mut embedding = vec![0.0; DEFAULT_EMBEDDING_DIMENSION];
                embedding[0] = 1.0;
                Ok(embedding)
            }
        }

        let service = crate::service::MemoryService::new_with_embedding_provider(
            Arc::new(db_client),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
            Arc::new(SemanticEmbeddingProvider),
            crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "salary raise".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:semantic");
        assert!(results[0].rationale.contains("semantic similarity"));
    }

    #[test]
    fn stored_community_summary_from_value_handles_wrapped_ft_score_number() {
        let summary = stored_community_summary_from_value(&json!({
            "community_id": "community:atlas",
            "summary": "Atlas workstream",
            "member_entities": ["entity:atlas"],
            "ft_score": {"Number": 42.5}
        }))
        .expect("community summary");

        assert_eq!(summary.ft_score, 42.5);
    }

    #[tokio::test]
    async fn expand_query_with_aliases_supports_multi_word_entities() {
        let mut db_client = MockDb::default();
        db_client.select_entity_lookup_fn = Box::new(|_, normalized_name| {
            if normalized_name == "alice smith" {
                return Ok(Some(json!({
                    "entity_id": "entity:alice_smith",
                    "aliases": ["alice s."]
                })));
            }

            Ok(None)
        });
        db_client.select_entities_batch_fn = Box::new(|_, names| {
            let mut results = Vec::new();
            for name in names {
                if name == "alice smith" {
                    results.push(json!({
                        "entity_id": "entity:alice_smith",
                        "canonical_name_normalized": "alice smith",
                        "aliases": ["alice s."]
                    }));
                }
            }
            Ok(results)
        });

        let service = crate::service::MemoryService::new(
            Arc::new(db_client),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let expanded =
            expand_query_with_aliases_for_test(&service, "alice smith atlas", "org").await;

        assert!(
            expanded.iter().any(|query| query == "alice s. atlas"),
            "multi-word entity alias should expand the full phrase, got: {expanded:?}"
        );
    }

    #[tokio::test]
    async fn community_expansion_returns_empty_when_no_entity_links_match() {
        let mut db_client = MockDb::default();
        db_client.select_communities_matching_summary_fn = Box::new(|_, _| {
            Ok(vec![json!({
                "community_id": "community:orphan",
                "summary": "Orphan community with no facts",
                "member_entities": ["entity:nobody"],
                "ft_score": 1.0
            })])
        });

        let service = crate::service::MemoryService::new(
            Arc::new(db_client),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "orphan community query".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect("assemble context should not panic on empty community expansion");

        assert!(
            results.is_empty(),
            "community expansion with no matching entity_links should produce no results, got {}",
            results.len()
        );
    }

    #[tokio::test]
    async fn assemble_context_rejects_unknown_scope_instead_of_searching_default_namespace() {
        let service = crate::service::MemoryService::new(
            Arc::new(MockDb::default()),
            vec!["org".to_string(), "personal".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let err = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "catalog entries".to_string(),
                scope: "team".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            },
        )
        .await
        .expect_err("unknown scope should be rejected");

        assert!(
            matches!(err, MemoryError::Validation(ref message) if message.contains("unknown scope")),
            "expected validation error for unknown scope, got {err:?}"
        );
    }
}
