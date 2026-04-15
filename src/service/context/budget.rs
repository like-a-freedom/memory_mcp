use crate::models::AssembledContextItem;

use super::filtering;
use super::lexical;
use super::params::DefaultContextParams;
use super::ranking::{RankedContextFact, RetrievalTier, default_episode_fallback_rationale};
use super::scoring::selected_fact_query_term_coverage;
use crate::service::MemoryService;
use crate::service::error::MemoryError;

fn matched_query_terms_for_text(text: &str, query_terms: &[String]) -> Vec<String> {
    if query_terms.is_empty() {
        return Vec::new();
    }

    let content_terms = crate::service::query::search_query_terms(text)
        .into_iter()
        .collect::<std::collections::HashSet<_>>();

    query_terms
        .iter()
        .filter(|term| content_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

pub(super) fn should_prefer_episode_content(
    selected_facts: &[RankedContextFact],
    episode_items: &[AssembledContextItem],
    query_terms: &[String],
) -> bool {
    if episode_items.is_empty() {
        return false;
    }

    if selected_facts
        .iter()
        .any(|fact| fact.retrieval_tier == RetrievalTier::GraphExpanded)
    {
        return false;
    }

    let best_fact_overlap = selected_facts
        .iter()
        .map(|fact| lexical::lexical_query_score_for_fact(&fact.fact, query_terms))
        .max()
        .unwrap_or(0);

    let Some(best_episode_item) = episode_items
        .iter()
        .max_by_key(|item| lexical::lexical_query_score_for_text(&item.content, query_terms))
    else {
        return false;
    };

    let best_episode_overlap =
        lexical::lexical_query_score_for_text(&best_episode_item.content, query_terms);

    if best_episode_overlap <= best_fact_overlap {
        return false;
    }

    let best_episode_term_coverage =
        matched_query_terms_for_text(&best_episode_item.content, query_terms).len();
    let selected_fact_term_coverage =
        selected_fact_query_term_coverage(selected_facts, query_terms);

    best_episode_term_coverage > selected_fact_term_coverage
}

pub(super) async fn collect_episode_fallback_items(
    service: &MemoryService,
    params: &DefaultContextParams<'_>,
    query: &str,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let episode_records = lexical::select_episode_records_for_query(
        service,
        params.namespace,
        params.scope,
        params.cutoff_iso,
        Some(query),
        params.budget,
        params.project_opt,
    )
    .await?;

    let query_terms = crate::service::query::search_query_terms(query);
    let mut episodes = filtering::filter_episodes_by_constraints(
        episode_records,
        params.access,
        params.project_opt,
    );

    episodes.sort_by(|left, right| {
        lexical::lexical_query_score_for_text(&right.content, &query_terms)
            .cmp(&lexical::lexical_query_score_for_text(
                &left.content,
                &query_terms,
            ))
            .then_with(|| right.t_ref.cmp(&left.t_ref))
            .then_with(|| left.episode_id.cmp(&right.episode_id))
    });

    use super::views::{EpisodeFallbackParams, build_episode_fallback_items};

    Ok(build_episode_fallback_items(EpisodeFallbackParams {
        episodes,
        query_opt: Some(query),
        semantic_available: service.embedding_provider.is_enabled(),
        scope: params.scope,
        cutoff: params.cutoff,
        window_start: params.window_start,
        window_end: params.window_end,
        timeline_mode: params.view_mode == Some("timeline"),
        budget: params.budget,
        fallback_rationale_fn: default_episode_fallback_rationale,
    }))
}
