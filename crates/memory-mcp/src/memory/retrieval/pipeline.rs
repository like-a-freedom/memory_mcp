use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::memory::retrieval::RetrievalContext;
use crate::models::{
    AccessPayload, AssembleContextRequest, AssembledContextItem, RetrievalEvidence,
};
use crate::platform::log_event::log_event;

use super::alias_expansion::expand_query_with_aliases;
use super::budget::{collect_episode_fallback_items, should_prefer_episode_content};
use super::community::{CollectCommunityFactsRequest, collect_community_facts};
use super::experience::{
    RecentExperienceRequest, collect_recent_experience_facts, expand_experience_query_terms,
};
use super::filtering::filter_facts_by_constraints;
use super::graph::{CollectGraphFactsRequest, collect_graph_facts};
use super::lexical::{FactQueryParams, select_fact_records_for_query};
use super::params::DefaultContextParams;
use super::query_mode;
use super::ranking::{
    BuildRankedContextFactsRequest, RetrievalTier, apply_time_window, build_ranked_context_facts,
    select_ranked_context_facts, sort_ranked_context_facts_for_timeline,
};
use super::rescue::{
    build_episode_rescue_log_result, maybe_append_first_person_episode_item,
    maybe_append_first_person_ranked_fact_item,
};
use super::scoring::{ranked_fact_to_item, selected_fact_matched_terms};
use super::semantic::{CollectSemanticFactsRequest, collect_semantic_facts};
use super::temporal::{CollectTemporalFactsRequest, collect_temporal_facts, infer_temporal_window};
use super::triple::collect_triple_facts;
use super::types::RankedContextFact;
use crate::platform::context_cache::{CacheGeneration, CacheInsertOutcome, ContextCacheLookup};
use crate::platform::context_cache_key::{CacheCutoff, CacheKey, CacheView};

const DIRECT_RETRIEVAL_CANDIDATE_FLOOR: i32 = 5;

fn retrieval_candidate_limit(service: &RetrievalContext, response_budget: i32) -> i32 {
    super::reranking::candidate_pool_limit(
        response_budget.max(1) as usize,
        service.embedding_provider.reranking_signature().is_some(),
    )
    .max(DIRECT_RETRIEVAL_CANDIDATE_FLOOR as usize)
    .min(i32::MAX as usize) as i32
}

// ─── Parameter preparation and cache operations ──────────────────────────

/// Prepared parameters for context assembly, extracted from the request
/// and access payload. Avoids re-parsing in the main orchestrator.
pub(super) struct PreparedContextParams {
    pub access: AccessPayload,
    pub namespace: String,
    pub cutoff: DateTime<Utc>,
    pub cutoff_iso: String,
    pub cleaned_query: String,
    pub fact_types: Vec<String>,
    pub query_terms: Vec<String>,
    pub resolved_view_mode_opt: Option<String>,
    pub query_flags: super::query_mode::QueryFlags,
    pub cache_key: CacheKey,
}

/// Parses the request and prepares all parameters needed for context assembly.
pub(super) async fn prepare_context_params(
    service: &RetrievalContext,
    request: &AssembleContextRequest,
    access_opt: Option<AccessPayload>,
) -> Result<PreparedContextParams, MemoryError> {
    let cutoff = request.as_of.unwrap_or_else(crate::shared::temporal::now);
    let cache_cutoff = match request.as_of {
        Some(cutoff) => CacheCutoff::HistoricalExact(cutoff),
        None => CacheCutoff::CurrentBucket(cutoff),
    };
    let access = access_opt.unwrap_or(AccessPayload {
        allowed_tags: None,
        caller_id: None,
        session_vars: None,
        transport: None,
        content_type: None,
    });

    let namespace = service.active_namespace.clone();
    let cutoff_iso = crate::shared::temporal::normalize_dt(cutoff);
    let cleaned_query = crate::shared::search::preprocess_search_query(&request.query);

    let requested_view_mode = request
        .view_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let fact_types = request
        .fact_types
        .iter()
        .filter_map(|fact_type| {
            let trimmed = fact_type.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect::<Vec<_>>();
    let query_terms = if cleaned_query.is_empty() {
        Vec::new()
    } else {
        crate::shared::search::search_query_terms(&cleaned_query)
    };
    let raw_query_opt = if request.query.trim().is_empty() {
        None
    } else {
        Some(request.query.as_str())
    };
    let (resolved_view_mode, query_flags) = query_mode::resolve_view_mode(
        requested_view_mode.as_deref(),
        raw_query_opt,
        &query_terms,
        cutoff,
    );
    let resolved_view_mode_opt = resolved_view_mode.as_option_str().map(str::to_string);
    let cache_key = CacheKey::new(
        &request.query,
        cache_cutoff,
        request.budget,
        &request.fact_types,
        CacheView::new_with_cutoff(
            resolved_view_mode.as_option_str(),
            request.window_start,
            request.window_end,
            cache_cutoff,
        ),
        access.allowed_tags.clone(),
    );

    Ok(PreparedContextParams {
        access,
        namespace,
        cutoff,
        cutoff_iso,
        cleaned_query,
        fact_types,
        query_terms,
        resolved_view_mode_opt,
        query_flags,
        cache_key,
    })
}

/// Checks the context cache for a hit. Returns cached items if found.
pub(super) async fn check_cache(
    service: &RetrievalContext,
    cache_key: &CacheKey,
) -> ContextCacheLookup {
    let mut cache = service.context_cache.write().await;
    cache.lookup(cache_key)
}

/// Stores results in the context cache.
pub(super) async fn store_cache(
    service: &RetrievalContext,
    generation: CacheGeneration,
    cache_key: CacheKey,
    results: &[AssembledContextItem],
) -> CacheInsertOutcome {
    let mut cache = service.context_cache.write().await;
    cache.insert(generation, cache_key, results)
}

/// Logs the start of context assembly.
pub(super) fn log_context_start(
    service: &RetrievalContext,
    request: &AssembleContextRequest,
    access: Option<&AccessPayload>,
) {
    service.logger.log(
        log_event(
            "assemble_context.start",
            json!({"query": request.query, "budget": request.budget}),
            json!({}),
            access,
            None,
            None,
        ),
        LogLevel::Info,
    );
}

/// Executes the full multi-tier retrieval pipeline for the default (non-view-mode) path.
///
/// Tiers: lexical BM25 → temporal → alias expansion → experience → community → semantic ANN.
/// Falls back to episode search if no facts match.
/// The tier-fallback outcome for the default context pipeline.
///
/// When the episode-content rescue tier and the ranked-fact tiers compete for
/// the response, this records which side wins. `assemble_default_context`
/// returns episode items on [`FallbackDecision::UseEpisodes`] and otherwise
/// builds the response from the ranked facts (with episodes appended only when
/// they earn a slot via the first-person heuristics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FallbackDecision {
    /// Episode content replaces the ranked-fact tiers.
    UseEpisodes,
    /// Ranked facts win; episode content may still be appended opportunistically.
    UseRanked,
}

/// Strategy object for the episode-rescue tier fallback.
///
/// Encapsulates the decision `assemble_default_context` makes when the
/// episode-content tier should replace the ranked-fact tiers. The episode pack
/// must preserve every query term represented by the selected fact pack;
/// comparable same-model reranker evidence or stronger lexical coverage then
/// decides the winner (see [`super::budget::should_prefer_episode_content`]).
/// The companion decision — ranking produced nothing for a query — is guarded
/// in the pipeline before selection; both branches of the fallback are covered
/// by this strategy's unit tests.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct EpisodeFallbackStrategy;

impl EpisodeFallbackStrategy {
    /// Decides whether episode content should replace the ranked facts.
    pub(super) fn decide(
        &self,
        selected_ranked: &[RankedContextFact],
        episode_fallback_items: &[AssembledContextItem],
        query_terms: &[String],
    ) -> FallbackDecision {
        if should_prefer_episode_content(selected_ranked, episode_fallback_items, query_terms) {
            FallbackDecision::UseEpisodes
        } else {
            FallbackDecision::UseRanked
        }
    }
}

pub(super) async fn assemble_default_context(
    service: &RetrievalContext,
    params: DefaultContextParams<'_>,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    // Task 14 decomposition: the assembly pipeline is split
    // into three private phase functions. Each phase takes
    // the inputs it needs and returns the outputs the next
    // phase expects. The phase boundaries are documented
    // here so future changes can target a single phase
    // without disturbing the others.
    let channels = retrieve_fact_channels(service, &params).await?;
    let (subject_ids, mentioned_names) = super::alias_expansion::resolve_entity_subject(
        service,
        params.raw_query_opt.unwrap_or_default(),
    )
    .await?;
    let mut query_evidence = super::admission::build_query_evidence(
        params.raw_query_opt,
        subject_ids,
        &mentioned_names,
        params.query_flags,
    );
    if !query_evidence.resolved_entity_ids.is_empty()
        && query_evidence.intent == super::admission::AdmissionIntent::Content
    {
        query_evidence.intent = super::admission::AdmissionIntent::Entity;
    }
    if query_evidence.intent == super::admission::AdmissionIntent::Content {
        // A mentioned person's name identifies the subject, but does not
        // supply evidence for the attribute asked about that person.
        let subject_terms = mentioned_names
            .iter()
            .flat_map(|name| crate::shared::search::search_query_terms(name))
            .collect::<HashSet<_>>();
        query_evidence
            .content_terms
            .retain(|term| !subject_terms.contains(term));
    }
    query_evidence.repeated_topics = super::experience::repeated_topic_evidence_for_query(
        &query_evidence,
        &channels.direct_facts,
    );
    let ranked = rank_fact_channels(service, &params, channels).await?;
    let admission_policy = super::admission::AdmissionPolicy::new(
        service.embedding_service.embedding_similarity_threshold(),
        params.cutoff,
        params.access,
    );
    let ranked =
        super::admission::admit_ranked_candidates(&query_evidence, ranked, &admission_policy);
    let ranked: Vec<RankedContextFact> = ranked
        .into_iter()
        .take(retrieval_candidate_limit(service, params.budget) as usize)
        .collect();
    // A source episode can be a useful discourse anchor even when its
    // standalone text is a weak answer to the query. Preserve anchors after
    // evidence admission but before the cross-encoder prunes candidates; the
    // source-question resolver still requires a matching question, a unique
    // adjacent turn, access/time eligibility, and a matching speaker.
    let source_question_candidates = ranked
        .iter()
        .map(|candidate| candidate.fact.source_episode.clone())
        .collect::<Vec<_>>();
    let ranked = super::reranking::filter_candidates(service, &params, ranked).await?;
    let current_query = query_evidence.intent == super::admission::AdmissionIntent::Current;
    let ranked = project_current_candidates(service, &params, &query_evidence, ranked).await?;
    let selection = select_context_facts(service, &params, ranked, current_query).await?;
    let mut source_question_candidates = source_question_candidates;
    for episode_id in &selection.episode_fallback_source_anchors {
        if !source_question_candidates.contains(episode_id) {
            source_question_candidates.push(episode_id.clone());
        }
    }
    let selected = finalize_with_first_person_appenders(&params, selection).await?;
    append_source_answers(
        service,
        &params,
        current_query,
        &source_question_candidates,
        selected,
    )
    .await
}

/// Short answers can omit the words in the user's query. Use a matching
/// question in the same producer-declared stream to retain its response and,
/// for a follow-up question, the preceding answer from the same speaker.
async fn append_source_answers(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
    current_query: bool,
    candidate_anchors: &[String],
    mut items: Vec<AssembledContextItem>,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    if params.query_opt.is_none() || current_query || params.query_flags.wants_timeline {
        return Ok(items);
    }
    let query = params.raw_query_opt.unwrap_or_default();
    let query_evidence =
        super::admission::build_query_evidence(Some(query), Vec::new(), &[], params.query_flags);
    let subject_terms = query_evidence.subject_terms.clone();
    let selected_anchor_ids = items
        .iter()
        .map(|item| item.source_episode.clone())
        .collect::<HashSet<_>>();
    let mut anchors = items
        .iter()
        .filter(|item| {
            item.provenance
                .get("episode_fallback")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
                || item
                    .provenance
                    .get("source_context")
                    .is_some_and(serde_json::Value::is_object)
        })
        .map(|item| item.source_episode.clone())
        .collect::<Vec<_>>();
    for episode_id in candidate_anchors {
        if !anchors.contains(episode_id) {
            anchors.push(episode_id.clone());
        }
    }
    let window_start = params
        .window_start
        .as_ref()
        .map(|date| crate::shared::temporal::normalize_dt(*date));
    let window_end = params
        .window_end
        .as_ref()
        .map(|date| crate::shared::temporal::normalize_dt(*date));
    let neighborhoods = super::filtering::filter_episodes_by_constraints(
        service
            .episode_context_store()
            .select_source_neighborhoods_for_episodes(
                &anchors,
                params.cutoff_iso,
                window_start.as_deref(),
                window_end.as_deref(),
            )
            .await?,
        params.access,
    );
    let mut directly_answered_questions = HashSet::new();
    let mut directly_answered_evidence = HashSet::new();
    let mut linked_source_evidence = HashMap::<String, HashSet<String>>::new();
    for anchor_id in anchors {
        let Some(anchor) = neighborhoods
            .iter()
            .find(|episode| episode.episode_id == anchor_id)
        else {
            continue;
        };
        let Some(source_context) = &anchor.source_context else {
            continue;
        };
        let neighbors = neighborhoods
            .iter()
            .filter(|episode| {
                episode.source_context.as_ref().is_some_and(|context| {
                    context.group == source_context.group
                        && source_context.position.abs_diff(context.position)
                            <= crate::memory::episode_context_store::SOURCE_CONTEXT_RADIUS as u64
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut supplemental = Vec::new();
        for related_episode in &neighbors {
            if related_episode.source_context.is_none() {
                continue;
            }
            if related_episode.episode_id == anchor.episode_id
                || !super::admission::episode_candidate_matches_query_subject(
                    &query_evidence,
                    &related_episode.content,
                )
            {
                continue;
            }
            let has_explicit_link = super::reranking::source_context_has_explicit_additive_link(
                query,
                &anchor.content,
                &related_episode.content,
            )
                || super::reranking::source_context_has_explicit_additive_link(
                    query,
                    &related_episode.content,
                    &anchor.content,
                );
            let speakers_are_requested =
                episode_speaker_matches_subject(&anchor.content, &subject_terms)
                    && episode_speaker_matches_subject(&related_episode.content, &subject_terms);
            let related_to_anchor = has_explicit_link
                && query_requests_multiple_evidence(query)
                && !same_turn_speaker(&anchor.content, &related_episode.content)
                && subject_terms.len() >= 2
                && speakers_are_requested;
            let related_typed_answer = query_requests_multiple_evidence(query)
                && super::reranking::typed_answer_slot_support(
                    query,
                    &related_episode.content,
                    &subject_terms,
                ) == Some(true)
                && same_turn_speaker(&anchor.content, &related_episode.content)
                && episode_speaker_matches_subject(&related_episode.content, &subject_terms);
            if related_to_anchor || related_typed_answer {
                supplemental.push((
                    related_episode.clone(),
                    None,
                    (source_context.group.clone(), anchor.episode_id.clone()),
                    SourceContextRelation::QueryRelatedConversationTurn,
                    anchor.episode_id.clone(),
                ));
            } else if related_episode
                .source_context
                .as_ref()
                .is_some_and(|context| {
                    context.group == source_context.group
                        && context.position < source_context.position
                })
                && same_turn_speaker(&anchor.content, &related_episode.content)
                && super::reranking::source_context_supports_requested_transition(
                    query,
                    &anchor.content,
                    &related_episode.content,
                )
            {
                supplemental.push((
                    related_episode.clone(),
                    None,
                    (source_context.group.clone(), anchor.episode_id.clone()),
                    SourceContextRelation::QueryRelatedAntecedent,
                    anchor.episode_id.clone(),
                ));
            }
        }
        for question in &neighbors {
            let Some(question_context) = &question.source_context else {
                continue;
            };
            let query_matched_question =
                super::reranking::source_question_matches_query_via_context(
                    &question.content,
                    query,
                    &subject_terms,
                    &anchor.content,
                );
            let context_only_follow_up =
                super::reranking::source_question_is_context_only_follow_up(
                    &question.content,
                    query,
                    &subject_terms,
                );
            let question_key = (question_context.group.clone(), question.episode_id.clone());
            let question_position = question_context.position;
            let response = question_position
                .checked_add(1)
                .and_then(|position| unique_source_episode_at(&neighbors, position));
            let travel_event_follow_up = response.as_ref().is_some_and(|response| {
                same_turn_speaker(&anchor.content, &response.content)
                    && episode_speaker_matches_subject(&response.content, &subject_terms)
                    && neighbors.iter().any(|antecedent| {
                        antecedent.source_context.as_ref().is_some_and(|context| {
                            context.group == question_key.0
                                && context.position < question_position
                                && question_position.abs_diff(context.position)
                                    <= crate::memory::episode_context_store::SOURCE_CONTEXT_RADIUS
                                        as u64
                        }) && !antecedent.content.contains('?')
                            && same_turn_speaker(&antecedent.content, &response.content)
                            && super::reranking::source_context_is_travel_event_antecedent(
                                query,
                                &question.content,
                                &antecedent.content,
                            )
                    })
            });
            let query_matched_question = query_matched_question || travel_event_follow_up;
            let anaphoric_follow_up = selected_anchor_ids.contains(&anchor.episode_id)
                && super::reranking::source_question_contains_anaphoric_reference(
                    &question.content,
                )
                && super::reranking::source_question_is_supported_by_assertion(
                    &question.content,
                    query,
                    &anchor.content,
                    &subject_terms,
                )
                && response.as_ref().is_some_and(|response| {
                    same_turn_speaker(&anchor.content, &response.content)
                        && episode_speaker_matches_subject(&response.content, &subject_terms)
                        && super::reranking::anaphoric_response_asserts_requested_acquisition(
                            query,
                            &question.content,
                            &response.content,
                        )
                });
            if !query_matched_question && !anaphoric_follow_up && !context_only_follow_up {
                continue;
            }
            if context_only_follow_up && !query_matched_question && !anaphoric_follow_up {
                if question_position.checked_add(1) == Some(source_context.position) {
                    for antecedent in neighbors.iter().filter(|episode| {
                        episode.source_context.as_ref().is_some_and(|context| {
                            context.group == question_key.0
                                && context.position < question_position
                                && question_position.abs_diff(context.position)
                                    <= crate::memory::episode_context_store::SOURCE_CONTEXT_RADIUS
                                        as u64
                        }) && !episode.content.contains('?')
                            && same_turn_speaker(&anchor.content, &episode.content)
                            && super::reranking::source_context_is_related_support(
                                query,
                                &anchor.content,
                                &episode.content,
                            )
                    }) {
                        supplemental.push((
                            antecedent.clone(),
                            None,
                            question_key.clone(),
                            SourceContextRelation::QueryRelatedAntecedent,
                            anchor.episode_id.clone(),
                        ));
                    }
                }
                continue;
            }
            if anaphoric_follow_up && let Some(response) = response.as_ref() {
                supplemental.push((
                    (**response).clone(),
                    None,
                    question_key.clone(),
                    SourceContextRelation::AnaphoricFollowUpAnswer,
                    anchor.episode_id.clone(),
                ));
            }

            if question_position == source_context.position
                && let Some(response) = response
                && episode_speaker_matches_subject(&response.content, &subject_terms)
            {
                supplemental.push((
                    response.clone(),
                    Some(anchor.episode_id.clone()),
                    question_key.clone(),
                    SourceContextRelation::AdjacentQuestionAnswer,
                    response.episode_id.clone(),
                ));
                for antecedent in neighbors.iter().filter(|episode| {
                    episode.source_context.as_ref().is_some_and(|context| {
                        context.group == question_key.0
                            && context.position < question_position
                            && question_position.abs_diff(context.position)
                                <= crate::memory::episode_context_store::SOURCE_CONTEXT_RADIUS
                                    as u64
                    }) && !episode.content.contains('?')
                        && same_turn_speaker(&episode.content, &response.content)
                        && (super::reranking::source_context_is_related_support(
                            query,
                            &response.content,
                            &episode.content,
                        ) || super::reranking::source_question_is_supported_by_assertion(
                            &question.content,
                            query,
                            &episode.content,
                            &subject_terms,
                        ))
                }) {
                    supplemental.push((
                        antecedent.clone(),
                        None,
                        question_key.clone(),
                        SourceContextRelation::QueryRelatedAntecedent,
                        response.episode_id.clone(),
                    ));
                }
            }

            if question_position.checked_sub(1) == Some(source_context.position)
                && let Some(response) = response
                && same_turn_speaker(&anchor.content, &response.content)
                && episode_speaker_matches_subject(&response.content, &subject_terms)
            {
                supplemental.push((
                    response.clone(),
                    None,
                    question_key.clone(),
                    SourceContextRelation::AdjacentQuestionAnswer,
                    response.episode_id.clone(),
                ));
            }

            if question_position.checked_add(1) == Some(source_context.position) {
                // The anchor is already the unique response immediately after
                // a query-matching source question. Treat it as direct evidence
                // and keep the bundle within this producer-declared group;
                // an earlier same-speaker turn is not automatically relevant
                // just because it repeats one query word.
                directly_answered_questions.insert(question_key.clone());
                directly_answered_evidence
                    .insert((question_key.0.clone(), anchor.episode_id.clone()));
                for antecedent in neighbors.iter().filter(|episode| {
                    episode.source_context.as_ref().is_some_and(|context| {
                        context.group == question_key.0
                            && context.position < question_position
                            && question_position.abs_diff(context.position)
                                <= crate::memory::episode_context_store::SOURCE_CONTEXT_RADIUS
                                    as u64
                    }) && !episode.content.contains('?')
                        && same_turn_speaker(&episode.content, &anchor.content)
                        && (super::reranking::source_context_is_related_support(
                            query,
                            &anchor.content,
                            &episode.content,
                        ) || super::reranking::source_context_is_travel_event_antecedent(
                            query,
                            &question.content,
                            &episode.content,
                        ))
                }) {
                    supplemental.push((
                        antecedent.clone(),
                        None,
                        question_key.clone(),
                        SourceContextRelation::QueryRelatedAntecedent,
                        anchor.episode_id.clone(),
                    ));
                }
            }
        }

        let mut source_fact_episode_seen = HashSet::new();
        let source_fact_episode_ids = supplemental
            .iter()
            .filter(|&(episode, _, _, _, _)| {
                !super::admission::source_episode_has_hard_query_conflict(
                    &query_evidence,
                    &episode.content,
                ) && !params
                    .window_start
                    .is_some_and(|start| episode.t_ref < start)
                    && !params.window_end.is_some_and(|end| episode.t_ref > end)
                    && !items
                        .iter()
                        .any(|item| item.source_episode == episode.episode_id)
                    && source_fact_episode_seen.insert(episode.episode_id.clone())
            })
            .map(|(episode, _, _, _, _)| episode.episode_id.clone())
            .collect::<Vec<_>>();
        let source_fact_groups = service
            .graph_store
            .select_active_facts_by_episodes(
                &source_fact_episode_ids,
                params.cutoff_iso,
                retrieval_candidate_limit(service, params.budget),
            )
            .await?;
        let source_facts_by_episode = source_fact_episode_ids
            .into_iter()
            .zip(source_fact_groups)
            .collect::<HashMap<_, _>>();

        for (episode, replaces_episode_id, question_key, relation, relevance_anchor_id) in
            supplemental
        {
            let directly_answers_question = replaces_episode_id.is_some();
            if super::admission::source_episode_has_hard_query_conflict(
                &query_evidence,
                &episode.content,
            ) {
                continue;
            }
            if params
                .window_start
                .is_some_and(|start| episode.t_ref < start)
                || params.window_end.is_some_and(|end| episode.t_ref > end)
            {
                continue;
            }
            if items
                .iter()
                .any(|item| item.source_episode == episode.episode_id)
            {
                if matches!(
                    relation,
                    SourceContextRelation::QueryRelatedAntecedent
                        | SourceContextRelation::QueryRelatedConversationTurn
                        | SourceContextRelation::AnaphoricFollowUpAnswer
                ) && let Some(item) = items
                    .iter_mut()
                    .find(|item| item.source_episode == episode.episode_id)
                {
                    annotate_source_context_admission(
                        item,
                        &question_key.1,
                        &source_context.group,
                        source_context.position,
                        episode
                            .source_context
                            .as_ref()
                            .map(|context| context.position),
                        relation,
                    );
                    linked_source_evidence
                        .entry(episode.episode_id.clone())
                        .or_default()
                        .insert(relevance_anchor_id.clone());
                }
                if directly_answers_question {
                    directly_answered_evidence
                        .insert((question_key.0.clone(), episode.episode_id.clone()));
                    directly_answered_questions.insert(question_key);
                }
                continue;
            }
            let answer_facts = source_facts_by_episode
                .get(&episode.episode_id)
                .cloned()
                .ok_or_else(|| {
                    MemoryError::Storage("batched source-answer fact group is missing".to_string())
                })?;
            let answer_facts =
                filter_facts_by_constraints(answer_facts, params.access, params.fact_types);
            let mut answer_items = answer_facts
                .into_iter()
                .map(|fact| source_answer_fact_item(fact, params.cutoff, service))
                .collect::<Vec<_>>();
            if answer_items.is_empty() {
                answer_items = super::views::build_episode_fallback_items(
                    super::views::EpisodeFallbackParams {
                        episodes: vec![episode.clone()],
                        query_opt: params.raw_query_opt,
                        semantic_available: service.embedding_provider.is_enabled(),
                        cutoff: params.cutoff,
                        window_start: params.window_start,
                        window_end: params.window_end,
                        timeline_mode: false,
                        budget: 1,
                        fallback_rationale_fn: source_answer_rationale,
                    },
                );
            }
            for mut answer_item in answer_items {
                if super::admission::source_episode_has_hard_query_conflict(
                    &query_evidence,
                    &answer_item.content,
                ) {
                    continue;
                }
                if items.iter().any(|item| item.fact_id == answer_item.fact_id) {
                    continue;
                }
                annotate_source_context_admission(
                    &mut answer_item,
                    &question_key.1,
                    &source_context.group,
                    source_context.position,
                    episode
                        .source_context
                        .as_ref()
                        .map(|context| context.position),
                    relation,
                );
                if let Some(question_episode_id) = replaces_episode_id.as_ref()
                    && let Some(index) = items.iter().position(|item| {
                        item.source_episode.as_str() == question_episode_id.as_str()
                    })
                {
                    items[index] = answer_item;
                    if directly_answers_question {
                        directly_answered_evidence
                            .insert((question_key.0.clone(), episode.episode_id.clone()));
                        directly_answered_questions.insert(question_key.clone());
                    }
                    continue;
                }
                if items.len() < params.budget.max(1) as usize
                    || service.embedding_provider.reranking_signature().is_some()
                {
                    items.push(answer_item);
                    if matches!(
                        relation,
                        SourceContextRelation::QueryRelatedAntecedent
                            | SourceContextRelation::QueryRelatedConversationTurn
                            | SourceContextRelation::AnaphoricFollowUpAnswer
                    ) {
                        linked_source_evidence
                            .entry(episode.episode_id.clone())
                            .or_default()
                            .insert(relevance_anchor_id.clone());
                    }
                    if directly_answers_question {
                        directly_answered_evidence
                            .insert((question_key.0.clone(), episode.episode_id.clone()));
                        directly_answered_questions.insert(question_key.clone());
                    }
                }
            }
        }
    }
    let multi_evidence_query = query_requests_multiple_evidence(query);
    if !directly_answered_questions.is_empty() {
        // A matched source question anchors its response and validated
        // same-speaker antecedents within that producer-declared group. Keep
        // other source groups only when the query asks for multiple values.
        let directly_answered_groups = directly_answered_questions
            .iter()
            .map(|(group, _)| group.clone())
            .collect::<HashSet<_>>();
        items.retain(|item| {
            let source_group = neighborhoods
                .iter()
                .find(|episode| episode.episode_id == item.source_episode)
                .and_then(|episode| episode.source_context.as_ref())
                .map(|context| context.group.clone())
                .or_else(|| {
                    item.provenance
                        .get("source_context")
                        .and_then(serde_json::Value::as_object)
                        .and_then(|context| context.get("group"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                });
            let Some(source_group) = source_group else {
                return true;
            };
            if !multi_evidence_query && !directly_answered_groups.contains(&source_group) {
                return false;
            }
            !directly_answered_groups.contains(&source_group)
                || directly_answered_evidence.contains(&(source_group, item.source_episode.clone()))
                || linked_source_evidence.contains_key(&item.source_episode)
        });
    }
    let items = append_source_episode_fact_candidates(service, params, items).await?;
    let filtered = super::reranking::filter_items(
        service,
        params.raw_query_opt.unwrap_or_default(),
        items,
        params.budget.max(1) as usize,
        params.resolved_view_mode == Some("timeline"),
    )
    .await?;
    if !multi_evidence_query && params.resolved_view_mode != Some("timeline") {
        let source_group_by_episode = neighborhoods
            .iter()
            .filter_map(|episode| {
                episode
                    .source_context
                    .as_ref()
                    .map(|context| (episode.episode_id.clone(), context.group.clone()))
            })
            .collect::<HashMap<_, _>>();
        return Ok(retain_best_source_group_items(
            filtered,
            &source_group_by_episode,
            &directly_answered_evidence,
            &linked_source_evidence,
        ));
    }
    Ok(filtered)
}

fn query_requests_multiple_evidence(query: &str) -> bool {
    let lower_query = query.to_lowercase();
    let tokens = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| {
            (
                token.to_lowercase(),
                token.chars().next().is_some_and(char::is_uppercase),
            )
        })
        .collect::<Vec<_>>();
    if tokens
        .iter()
        .any(|token| matches!(token.0.as_str(), "both" | "each" | "several" | "multiple"))
    {
        return true;
    }
    if ["which year", "what year"]
        .iter()
        .any(|phrase| lower_query.starts_with(phrase))
    {
        return false;
    }
    if ["types of", "kinds of", "sorts of"]
        .iter()
        .any(|phrase| lower_query.contains(phrase))
    {
        return true;
    }
    if ["type of", "kind of", "sort of"]
        .iter()
        .any(|phrase| lower_query.contains(phrase))
    {
        return false;
    }
    let invariant_or_singular_s = [
        "analysis", "business", "focus", "news", "physics", "politics", "series", "species",
        "status", "this",
    ];
    tokens.iter().any(|(token, proper_name)| {
        !proper_name
            && token.len() >= 4
            && !invariant_or_singular_s.contains(&token.as_str())
            && (token.ends_with("ies") || token.ends_with('s') && !token.ends_with("ss"))
    })
}

fn retain_best_source_group_items(
    items: Vec<AssembledContextItem>,
    source_group_by_episode: &HashMap<String, String>,
    directly_answered_evidence: &HashSet<(String, String)>,
    linked_source_evidence: &HashMap<String, HashSet<String>>,
) -> Vec<AssembledContextItem> {
    let directly_matched_groups = items
        .iter()
        .filter(|item| super::reranking::item_has_direct_lexical_evidence(item))
        .filter_map(|item| source_group_by_episode.get(&item.source_episode).cloned())
        .collect::<HashSet<_>>();
    let directly_answered_groups = directly_answered_evidence
        .iter()
        .map(|(group, _)| group.as_str())
        .collect::<HashSet<_>>();
    let mut retained_groups = HashSet::new();
    let mut retained_directly_matched_groups = HashSet::new();
    items
        .into_iter()
        .filter(|item| {
            let Some(group) = source_group_by_episode.get(&item.source_episode) else {
                return true;
            };
            if directly_answered_evidence.contains(&(group.clone(), item.source_episode.clone())) {
                return true;
            }
            if linked_source_evidence.contains_key(&item.source_episode) {
                return true;
            }
            if directly_answered_groups.contains(group.as_str()) {
                return false;
            }
            if directly_matched_groups.contains(group) {
                return super::reranking::item_has_direct_lexical_evidence(item)
                    && retained_directly_matched_groups.insert(group.clone());
            }
            retained_groups.insert(group.clone())
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceContextRelation {
    AdjacentQuestionAnswer,
    QueryRelatedAntecedent,
    QueryRelatedConversationTurn,
    AnaphoricFollowUpAnswer,
}

impl SourceContextRelation {
    fn as_str(self) -> &'static str {
        match self {
            Self::AdjacentQuestionAnswer => "adjacent_question_answer",
            Self::QueryRelatedAntecedent => "query_related_antecedent",
            Self::QueryRelatedConversationTurn => "query_related_conversation_turn",
            Self::AnaphoricFollowUpAnswer => "anaphoric_follow_up_answer",
        }
    }
}

fn episode_speaker_matches_subject(content: &str, subject_terms: &[String]) -> bool {
    if subject_terms.is_empty() {
        return true;
    }
    let Some((speaker, _)) = content.split_once(':') else {
        return false;
    };
    let speaker_terms = crate::shared::search::search_query_terms(speaker.trim());
    subject_terms
        .iter()
        .any(|term| speaker_terms.contains(term))
}

fn annotate_source_context_admission(
    item: &mut AssembledContextItem,
    related_episode_id: &str,
    group: &str,
    question_position: u64,
    answer_position: Option<u64>,
    relation: SourceContextRelation,
) {
    if !item.provenance.is_object() {
        item.provenance = serde_json::json!({});
    }
    item.provenance["retrieval_admission"] = serde_json::json!({
        "policy_version": "retrieval-admission-v1",
        "basis": "qualified_source_episode",
        "source_context_relation": relation.as_str(),
        "related_episode_id": related_episode_id,
        "source_context_group": group,
        "source_context_position": question_position,
    });
    if let Some(position) = answer_position {
        item.provenance["source_context"] = json!({"group": group, "position": position});
    }
}

fn source_answer_fact_item(
    fact: crate::models::Fact,
    cutoff: DateTime<Utc>,
    service: &RetrievalContext,
) -> AssembledContextItem {
    let mut provenance = fact.provenance.to_json_value();
    if !provenance.is_object() {
        provenance = json!({});
    }
    provenance["reference_time"] = json!(fact.t_valid.to_rfc3339());
    provenance["source_episode"] = json!(fact.source_episode);
    provenance["source_episode_id"] = json!(fact.source_episode);
    let confidence = super::fact_decayed_confidence(&fact, cutoff);
    AssembledContextItem {
        fact_id: fact.fact_id,
        content: fact.content,
        quote: fact.quote,
        source_episode: fact.source_episode,
        confidence,
        semantic_available: Some(service.embedding_provider.is_enabled()),
        provenance,
        rationale: "tier=source_expanded source-ordered answer to a matching question".into(),
        retrieval_tier: Some("source_expanded".into()),
        retrieval_evidence: Some(RetrievalEvidence {
            policy_version: "retrieval-admission-v1".into(),
            admission_basis: "qualified_source_episode".into(),
            semantic_similarity: None,
            graph_anchor_origin: None,
            hop_count: None,
        }),
        ..Default::default()
    }
}

async fn append_source_episode_fact_candidates(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
    mut items: Vec<AssembledContextItem>,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    if service.embedding_provider.reranking_signature().is_none() || items.is_empty() {
        return Ok(items);
    }

    let mut seen_episode_ids = HashSet::new();
    let episode_ids = items
        .iter()
        .filter(|item| {
            item.provenance
                .get("episode_fallback")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        })
        .map(|item| item.source_episode.clone())
        .filter(|episode_id| !episode_id.is_empty() && seen_episode_ids.insert(episode_id.clone()))
        .collect::<Vec<_>>();
    if episode_ids.is_empty() {
        return Ok(items);
    }

    let candidate_limit =
        super::reranking::candidate_pool_limit(params.budget.max(1) as usize, true);
    let candidate_slots = candidate_limit.saturating_sub(items.len());
    if candidate_slots == 0 {
        return Ok(items);
    }
    let per_episode_limit = (candidate_slots / episode_ids.len()).max(1);
    let mut seen_fact_ids = items
        .iter()
        .map(|item| item.fact_id.clone())
        .collect::<HashSet<_>>();
    let mut siblings = Vec::new();

    let fact_groups = service
        .graph_store
        .select_active_facts_by_episodes(
            &episode_ids,
            params.cutoff_iso,
            per_episode_limit.min(i32::MAX as usize) as i32,
        )
        .await?;
    for records in fact_groups {
        let facts = super::filtering::filter_facts_by_constraints(
            records,
            params.access,
            params.fact_types,
        )
        .into_iter()
        .filter(|fact| {
            params
                .window_start
                .is_none_or(|start| fact.t_valid >= start)
                && params.window_end.is_none_or(|end| fact.t_valid <= end)
        })
        .collect::<Vec<_>>();
        for fact in facts {
            if !seen_fact_ids.insert(fact.fact_id.clone()) {
                continue;
            }
            siblings.push(source_episode_fact_candidate_item(
                fact,
                params.cutoff,
                service,
            ));
        }
    }

    items.extend(siblings);
    Ok(items)
}

fn source_episode_fact_candidate_item(
    fact: crate::models::Fact,
    cutoff: DateTime<Utc>,
    service: &RetrievalContext,
) -> AssembledContextItem {
    let mut item = source_answer_fact_item(fact, cutoff, service);
    item.retrieval_evidence = None;
    item.rationale = "tier=source_expanded sibling fact from a selected source episode".into();
    item
}

fn unique_source_episode_at(
    episodes: &[crate::models::Episode],
    position: u64,
) -> Option<&crate::models::Episode> {
    let mut matches = episodes.iter().filter(|episode| {
        episode
            .source_context
            .as_ref()
            .is_some_and(|context| context.position == position)
    });
    let episode = matches.next()?;
    matches.next().is_none().then_some(episode)
}

fn same_turn_speaker(anchor: &str, answer: &str) -> bool {
    fn speaker(content: &str) -> Option<&str> {
        content
            .split_once(':')
            .map(|(name, _)| name.trim())
            .filter(|name| !name.is_empty() && name.len() <= 60 && !name.contains('\n'))
    }
    match (speaker(anchor), speaker(answer)) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        _ => false,
    }
}

fn source_answer_rationale(_: Option<&str>, _: chrono::DateTime<Utc>) -> String {
    "source-ordered context for adjacent matching question".into()
}

async fn project_current_candidates(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
    query: &super::admission::QueryEvidence,
    ranked: Vec<RankedContextFact>,
) -> Result<Vec<RankedContextFact>, MemoryError> {
    if query.intent != super::admission::AdmissionIntent::Current || ranked.is_empty() {
        return Ok(ranked);
    }

    let fact_ids = ranked
        .iter()
        .map(|candidate| crate::models::FactId::from(candidate.fact.fact_id.as_str()))
        .collect::<Vec<_>>();
    let relations = crate::knowledge::api::temporal_relations(
        &*service.relation_read,
        crate::knowledge::api::TemporalRelationsQuery {
            fact_ids: fact_ids.clone(),
            cutoff: params.cutoff,
            policy_tags: params.access.allowed_tags.clone().unwrap_or_default(),
            counterpart_limit: crate::knowledge::api::TEMPORAL_RELATION_MAX_COUNTERPARTS,
            max_relation_depth: crate::knowledge::api::TEMPORAL_RELATION_MAX_DEPTH,
        },
    )
    .await?;
    let projection = super::currency::project_current_candidates(query, &fact_ids, &relations);
    let current_ids = projection
        .fact_ids
        .iter()
        .map(ToString::to_string)
        .collect::<HashSet<_>>();
    let retained = ranked
        .into_iter()
        .filter(|candidate| current_ids.contains(&candidate.fact.fact_id))
        .collect::<Vec<_>>();

    if projection.currency_unknown {
        service.logger.log(
            log_event(
                "assemble_context.current_projection",
                json!({"namespace": params.namespace}),
                json!({
                    "currency": "unknown",
                    "retained_candidates": retained.len(),
                    "truncated": relations.truncated,
                }),
                Some(params.access),
                None,
                None,
            ),
            LogLevel::Debug,
        );
    }

    Ok(retained)
}

/// The four fact channels produced by `retrieve_fact_channels`
/// (Task 14, step 1). The fields mirror the inputs
/// `build_ranked_context_facts` consumes; the
/// `direct_retrieval_tier` records which tier produced the
/// initial lexical hit so `rank_fact_channels` can attach
/// the right tier label to the merged stream.
struct RetrievedFactChannels {
    direct_facts: Vec<crate::models::Fact>,
    temporal_facts: Vec<crate::models::Fact>,
    expanded_facts: Vec<crate::models::Fact>,
    source_facts: Vec<crate::models::Fact>,
    graph_facts: Vec<crate::memory::retrieval::graph::GraphCandidate>,
    community_facts: Vec<super::types::CommunityFactCandidate>,
    semantic_facts: Vec<super::types::SemanticCandidate>,
    direct_retrieval_tier: RetrievalTier,
}

/// The ranked + selected output of the selection phase
/// (Task 14, step 1). `selected` is the budgeted list that
/// survives timeline/relevance selection;
/// `episode_fallback_items` is the empty-fact rescue list
/// that the orchestrator appends or substitutes. The clone
/// of the pre-selection ranked list lives in
/// `ranked_candidates` so the first-person appender can
/// reference facts that did not survive selection.
struct SelectedContext {
    selected: Vec<RankedContextFact>,
    ranked_candidates: Vec<RankedContextFact>,
    episode_fallback_items: Vec<AssembledContextItem>,
    episode_fallback_source_anchors: Vec<String>,
    prefer_episode_content: bool,
}

/// Phase 1: collect fact records from every retrieval channel.
/// The orchestrator's first half is preserved verbatim, just
/// hoisted into a function. The lexical, temporal, alias,
/// experience, triple, graph, community, and semantic
/// channels are populated here; semantic is excluded from
/// the direct-fact set so it does not double-count.
async fn retrieve_fact_channels(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
) -> Result<RetrievedFactChannels, MemoryError> {
    let candidate_limit = retrieval_candidate_limit(service, params.budget);
    let lexical_result = select_fact_records_for_query(
        service,
        FactQueryParams {
            cutoff_iso: params.cutoff_iso,
            query_opt: params.query_opt,
            limit: candidate_limit,
            fact_types: params.fact_types,
        },
    )
    .await?;

    let direct_retrieval_tier = lexical_result.retrieval_tier;
    let mut direct_facts =
        filter_facts_by_constraints(lexical_result.records, params.access, params.fact_types);

    let mut expanded_facts = Vec::new();
    let mut source_facts = Vec::<crate::models::Fact>::new();
    let mut temporal_facts = Vec::new();
    let mut graph_facts = Vec::new();
    let mut community_facts = Vec::new();
    let mut semantic_facts = Vec::new();

    if let Some(query) = params.query_opt {
        temporal_facts = collect_temporal_facts(
            service,
            CollectTemporalFactsRequest {
                cutoff_iso: params.cutoff_iso,
                cutoff: params.cutoff,
                query,
                access: params.access,
                fact_types: params.fact_types,
                budget: candidate_limit,
            },
        )
        .await?;

        let expanded_queries = expand_query_with_aliases(service, query).await;
        let direct_fact_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        for expanded_query in &expanded_queries {
            if expanded_query == query {
                continue;
            }
            let extra_records = select_fact_records_for_query(
                service,
                FactQueryParams {
                    cutoff_iso: params.cutoff_iso,
                    query_opt: Some(expanded_query),
                    limit: candidate_limit,
                    fact_types: params.fact_types,
                },
            )
            .await?;
            for fact in
                filter_facts_by_constraints(extra_records.records, params.access, params.fact_types)
            {
                if !direct_fact_ids.contains(&fact.fact_id) {
                    expanded_facts.push(fact);
                }
            }
        }
        let base_direct_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        let source_query_evidence = super::admission::build_query_evidence(
            Some(query),
            Vec::new(),
            &[],
            params.query_flags,
        );
        let mut source_episode_ids = Vec::new();
        let mut seen_source_episode_ids = HashSet::new();
        for fact in direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
        {
            if super::admission::episode_candidate_is_admitted_at_reference_time(
                &source_query_evidence,
                &fact.content,
                Some(fact.t_valid),
            ) && seen_source_episode_ids.insert(fact.source_episode.clone())
            {
                source_episode_ids.push(fact.source_episode.clone());
            }
            if source_episode_ids.len() >= params.budget.max(1) as usize {
                break;
            }
        }
        for episode_id in source_episode_ids {
            let remaining = candidate_limit.saturating_sub(source_facts.len() as i32);
            if remaining <= 0 {
                break;
            }
            let sibling_records = service
                .graph_store
                .select_active_facts_by_episode(&episode_id, params.cutoff_iso, remaining)
                .await?;
            for sibling in
                filter_facts_by_constraints(sibling_records, params.access, params.fact_types)
            {
                if !base_direct_ids.contains(&sibling.fact_id)
                    && !source_facts
                        .iter()
                        .any(|fact| fact.fact_id == sibling.fact_id)
                {
                    source_facts.push(sibling);
                }
            }
        }

        let experience_query_terms =
            expand_experience_query_terms(params.query_terms, &direct_facts);
        let experience_topic_terms = experience_query_terms
            .iter()
            .filter(|term| !params.query_terms.contains(term))
            .cloned()
            .collect::<Vec<_>>();
        let mut experience_facts = collect_recent_experience_facts(
            service,
            RecentExperienceRequest {
                cutoff: params.cutoff,
                access: params.access,
                budget: candidate_limit,
                fact_types: params.fact_types,
            },
            &experience_query_terms,
            &experience_topic_terms,
            &base_direct_ids,
        )
        .await?;

        if !experience_topic_terms.is_empty() {
            let topical_floor = direct_facts
                .first()
                .map(|fact| fact.ft_score)
                .unwrap_or(0.0);
            for fact in &mut experience_facts {
                fact.ft_score = fact.ft_score.max(topical_floor + 1.0);
            }
        }

        // Triple-expanded facts: facts linked via matching triples.
        let triple_facts =
            collect_triple_facts(service, params.cutoff_iso, query, candidate_limit).await?;
        for fact in triple_facts {
            if !base_direct_ids.contains(&fact.fact_id) {
                expanded_facts.push(fact);
            }
        }

        direct_facts.extend(experience_facts);
        direct_facts.sort_by(|left, right| {
            right
                .ft_score
                .total_cmp(&left.ft_score)
                .then_with(|| right.t_valid.cmp(&left.t_valid))
                .then_with(|| left.fact_id.cmp(&right.fact_id))
        });

        let direct_fact_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
            .chain(source_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        let graph_query_evidence = super::admission::build_query_evidence(
            Some(query),
            Vec::new(),
            &[],
            params.query_flags,
        );
        let admitted_graph_seeds =
            super::graph::admitted_graph_seed_facts(&graph_query_evidence, &direct_facts);

        graph_facts = collect_graph_facts(
            service,
            CollectGraphFactsRequest {
                cutoff_iso: params.cutoff_iso,
                raw_query: query,
                access: params.access,
                fact_types: params.fact_types,
                direct_fact_ids: &direct_fact_ids,
                lexical_facts: &admitted_graph_seeds,
                max_hops: params.query_flags.max_graph_hops(),
                budget: candidate_limit,
            },
        )
        .await?;

        // A lexical query can return a fact from one matching word even
        // though it does not independently pass admission. Keep those IDs
        // eligible for a stronger, separately evidenced community-summary
        // path. Ranking fuses the duplicate by fact ID and carries its typed
        // community evidence onto the same candidate.
        let independently_grounded_ids = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
            .chain(source_facts.iter())
            .chain(graph_facts.iter().map(|candidate| &candidate.fact))
            .filter(|fact| {
                super::admission::episode_candidate_is_admitted_at_reference_time(
                    &graph_query_evidence,
                    &fact.content,
                    Some(fact.t_valid),
                )
            })
            .map(|fact| fact.fact_id.clone())
            .collect::<HashSet<_>>();

        community_facts = collect_community_facts(
            service,
            CollectCommunityFactsRequest {
                cutoff_iso: params.cutoff_iso,
                query,
                access: params.access,
                fact_types: params.fact_types,
                direct_fact_ids: &independently_grounded_ids,
                budget: candidate_limit,
            },
        )
        .await?;

        semantic_facts = collect_semantic_facts(
            service,
            CollectSemanticFactsRequest {
                cutoff: params.cutoff,
                query: params.raw_query_opt.unwrap_or(query),
                access: params.access,
                fact_types: params.fact_types,
                // Fusion needs the ANN score even for facts discovered by
                // lexical, graph or community channels. Deduplication belongs
                // to ranking, after the independent evidence is combined.
                excluded_fact_ids: &HashSet::new(),
                budget: candidate_limit,
            },
        )
        .await?;
    }

    Ok(RetrievedFactChannels {
        direct_facts,
        temporal_facts,
        expanded_facts,
        source_facts,
        graph_facts,
        community_facts,
        semantic_facts,
        direct_retrieval_tier,
    })
}

/// Phase 2: build the ranked list from the channels. The
/// `BuildRankedContextFactsRequest` constructor is
/// shape-preserving; the query branch and the no-query
/// branch return the same ranked list through different
/// channel combinations. When the query is absent, the
/// graph/community/semantic channels are empty and the
/// `build_ranked_context_facts` call only sees the direct
/// channel.
async fn rank_fact_channels(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
    channels: RetrievedFactChannels,
) -> Result<Vec<RankedContextFact>, MemoryError> {
    let RetrievedFactChannels {
        direct_facts,
        temporal_facts,
        expanded_facts,
        source_facts,
        graph_facts,
        community_facts,
        semantic_facts,
        direct_retrieval_tier,
    } = channels;

    if params.query_opt.is_some() {
        let mut lexical_facts = direct_facts
            .into_iter()
            .map(|fact| (fact, direct_retrieval_tier))
            .collect::<Vec<_>>();
        lexical_facts.extend(
            temporal_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::TemporalExpanded)),
        );
        lexical_facts.extend(
            expanded_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::AliasExpanded)),
        );
        lexical_facts.extend(
            source_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::SourceExpanded)),
        );

        Ok(build_ranked_context_facts(
            BuildRankedContextFactsRequest {
                lexical_facts,
                graph_facts,
                community_facts,
                semantic_facts,
                query_opt: params.raw_query_opt,
                semantic_available: service.embedding_provider.is_enabled(),
                cutoff: params.cutoff,
            },
            super::fact_decayed_confidence,
        ))
    } else {
        Ok(build_ranked_context_facts(
            BuildRankedContextFactsRequest {
                lexical_facts: direct_facts
                    .into_iter()
                    .map(|fact| (fact, RetrievalTier::Direct))
                    .collect(),
                graph_facts: Vec::new(),
                community_facts: Vec::new(),
                semantic_facts: Vec::new(),
                query_opt: params.raw_query_opt,
                semantic_available: service.embedding_provider.is_enabled(),
                cutoff: params.cutoff,
            },
            super::fact_decayed_confidence,
        ))
    }
}

/// Phase 3: timeline/relevance selection, episode-fallback
/// decision, and the rescue log. Returns the
/// pre-selection clone alongside the selected list so
/// the first-person appender can reference dropped facts.
async fn select_context_facts(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
    mut ranked_facts: Vec<RankedContextFact>,
    current_query: bool,
) -> Result<SelectedContext, MemoryError> {
    let timeline_mode = params.resolved_view_mode == Some("timeline")
        || (params.resolved_view_mode.is_none() && params.query_flags.wants_timeline);
    // Episodic fallback has no fact-level reconciliation identity. It cannot
    // prove that a statement remains current, so it must not bypass the
    // relation projection for a current-intent query.
    let (episode_fallback_items, episode_fallback_source_anchors) = if current_query {
        (Vec::new(), Vec::new())
    } else if let Some(query) = params.query_opt {
        let items = collect_episode_fallback_items(service, params, query).await?;
        let mut seen_source_anchors = HashSet::new();
        let source_anchors = items
            .iter()
            .filter(|item| {
                item.provenance
                    .get("source_context")
                    .is_some_and(serde_json::Value::is_object)
            })
            .map(|item| item.source_episode.clone())
            .filter(|episode_id| {
                !episode_id.is_empty() && seen_source_anchors.insert(episode_id.clone())
            })
            .collect::<Vec<_>>();
        let filtered = super::reranking::filter_items(
            service,
            params.raw_query_opt.unwrap_or(query),
            items,
            params.budget.max(1) as usize,
            timeline_mode,
        )
        .await?;
        (filtered, source_anchors)
    } else {
        (Vec::new(), Vec::new())
    };

    if ranked_facts.is_empty() {
        if params.query_opt.is_some() {
            return Ok(SelectedContext {
                selected: Vec::new(),
                ranked_candidates: Vec::new(),
                episode_fallback_items,
                episode_fallback_source_anchors,
                prefer_episode_content: false,
            });
        }
        return Ok(SelectedContext {
            selected: Vec::new(),
            ranked_candidates: Vec::new(),
            episode_fallback_items: Vec::new(),
            episode_fallback_source_anchors: Vec::new(),
            prefer_episode_content: false,
        });
    }

    apply_time_window(&mut ranked_facts, params.window_start, params.window_end);
    let ranked_candidates = ranked_facts.clone();
    let selected_ranked = if timeline_mode {
        sort_ranked_context_facts_for_timeline(&mut ranked_facts);
        ranked_facts
            .into_iter()
            .take(params.budget.max(1) as usize)
            .collect::<Vec<_>>()
    } else {
        let temporal_focus = params
            .query_opt
            .and_then(|query| infer_temporal_window(query, params.cutoff));
        select_ranked_context_facts(
            ranked_facts,
            params.budget.max(1) as usize,
            temporal_focus,
            params.query_terms.to_vec(),
        )
    };

    let strategy = EpisodeFallbackStrategy;
    let prefer_episode_content = strategy.decide(
        &selected_ranked,
        &episode_fallback_items,
        params.query_terms,
    ) == FallbackDecision::UseEpisodes;

    if params.query_opt.is_some() {
        use crate::logging::LogLevel;
        use serde_json::json;

        service.logger.log(
            log_event(
                "assemble_context.episode_rescue",
                json!({"namespace": params.namespace, "query": params.query_opt}),
                build_episode_rescue_log_result(
                    episode_fallback_items.len(),
                    selected_ranked.len(),
                    prefer_episode_content,
                ),
                Some(params.access),
                None,
                None,
            ),
            LogLevel::Debug,
        );
    }

    // When the fallback decision is to use episodes, the
    // selection phase still records what would have been
    // selected so the orchestrator can decide what to
    // return. The pre-selection clone is what the
    // first-person appender references, regardless of the
    // fallback decision.
    Ok(SelectedContext {
        selected: selected_ranked,
        ranked_candidates,
        episode_fallback_items,
        episode_fallback_source_anchors,
        prefer_episode_content,
    })
}

/// Phase 4: finalize. The orchestrator's second half is
/// preserved verbatim: serialize the selected ranked facts
/// into items, run the first-person appenders, and return
/// the result. The `prefer_episode_content` decision was
/// already logged in `select_context_facts`; the orchestrator
/// re-evaluates it here because the decision affects which
/// stream is returned.
async fn finalize_with_first_person_appenders(
    params: &DefaultContextParams<'_>,
    selection: SelectedContext,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    // The service handle is held for symmetry with the
    // other phase functions; future phases may need to
    // log through the same logger. The current phase does
    // not use it because the rescue log was emitted in
    // `select_context_facts`.
    let SelectedContext {
        selected: selected_ranked,
        ranked_candidates,
        episode_fallback_items,
        episode_fallback_source_anchors: _,
        prefer_episode_content,
    } = selection;

    if selected_ranked.is_empty() && !episode_fallback_items.is_empty() {
        // The ranked list was empty and the episode fallback
        // produced items; this is the "empty-fact episode
        // fallback" path. The selection phase already
        // captured the empty ranked list and the fallback
        // items; here we return the fallback list.
        return Ok(episode_fallback_items);
    }

    if selected_ranked.is_empty() {
        return Ok(Vec::new());
    }

    if prefer_episode_content {
        return Ok(episode_fallback_items);
    }

    let selected_terms = selected_fact_matched_terms(&selected_ranked, params.query_terms);
    let mut results = selected_ranked
        .into_iter()
        .map(|ranked| ranked_fact_to_item(ranked, params.cutoff, super::fact_decayed_confidence))
        .collect::<Vec<_>>();

    maybe_append_first_person_episode_item(
        &mut results,
        &episode_fallback_items,
        &selected_terms,
        params.raw_query_opt,
        params.query_terms,
        params.budget.max(1) as usize,
    );

    maybe_append_first_person_ranked_fact_item(
        &mut results,
        &ranked_candidates,
        params.raw_query_opt,
        params.query_terms,
        params.budget.max(1) as usize,
        params.cutoff,
    );

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;

    fn create_test_fact(
        fact_id: &str,
        content: &str,
        t_valid: chrono::DateTime<chrono::Utc>,
    ) -> Fact {
        Fact {
            fact_id: fact_id.to_string(),
            fact_type: "note".to_string(),
            content: content.to_string(),
            quote: content.to_string(),
            source_episode: "episode:test".to_string(),
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
        content: &str,
        tier: RetrievalTier,
        t_valid: chrono::DateTime<chrono::Utc>,
    ) -> RankedContextFact {
        RankedContextFact {
            fact: create_test_fact(fact_id, content, t_valid),
            evidence: crate::memory::retrieval::types::CandidateEvidence::default(),
            rationale: "test rationale".to_string(),
            retrieval_tier: tier,
            fusion_score: 1.0,
            source_priority: 0,
            decayed_confidence: 1.0,
            query_alignment_factor: 1.0,
            grounding_score: 1.0,
            semantic_available: false,
            matched_query_terms: Vec::new(),
            graph_trace: None,
        }
    }

    fn episode_item(content: &str) -> AssembledContextItem {
        AssembledContextItem {
            fact_id: "episode_fallback:episode:july".to_string(),
            content: content.to_string(),
            quote: content.to_string(),
            source_episode: "episode:july".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn query_requests_multiple_evidence_for_plural_targets_and_cross_person_questions() {
        for query in [
            "What programming languages has James worked with?",
            "What pets does Jolene have?",
            "What types of movies does Nate enjoy watching the most?",
            "Where has Evan been on roadtrips with his family?",
            "What items did Calvin buy in March 2023?",
            "How do Jon and Gina both like to destress?",
            "Which body measurements and diagnostics does the HTTP plan require?",
        ] {
            assert!(
                query_requests_multiple_evidence(query),
                "the query requests multiple pieces of evidence: {query}"
            );
        }
    }

    #[test]
    fn query_requests_multiple_evidence_rejects_singular_questions_and_proper_names() {
        for query in [
            "What did the charity race raise awareness for?",
            "What is John's main focus in local politics?",
            "Which year did Audrey adopt the first three of her dogs?",
            "What type of car did Evan get after his old Prius broke down?",
            "What type of movies does Nate enjoy watching the most?",
            "Which team did John sign with on 21 May, 2023?",
        ] {
            assert!(
                !query_requests_multiple_evidence(query),
                "the query requests one answer: {query}"
            );
        }
    }

    #[test]
    fn singular_query_keeps_only_the_highest_ranked_item_per_source_group() {
        let mut primary = episode_item("best match");
        primary.source_episode = "episode:primary".to_string();
        let mut distractor = episode_item("weaker related match");
        distractor.source_episode = "episode:distractor".to_string();
        let mut separate_session = episode_item("different conversation session");
        separate_session.source_episode = "episode:separate".to_string();
        let groups = HashMap::from([
            ("episode:primary".to_string(), "session:a".to_string()),
            ("episode:distractor".to_string(), "session:a".to_string()),
            ("episode:separate".to_string(), "session:b".to_string()),
        ]);

        let selected = retain_best_source_group_items(
            vec![primary, distractor, separate_session],
            &groups,
            &HashSet::new(),
            &HashMap::new(),
        );

        assert_eq!(
            selected
                .iter()
                .map(|item| item.source_episode.as_str())
                .collect::<Vec<_>>(),
            ["episode:primary", "episode:separate"]
        );
    }

    #[test]
    fn direct_source_question_bundle_survives_singular_group_deduplication() {
        let mut answer = episode_item("answer");
        answer.source_episode = "episode:answer".to_string();
        let mut antecedent = episode_item("query related antecedent");
        antecedent.source_episode = "episode:antecedent".to_string();
        let mut distractor = episode_item("unrelated source turn");
        distractor.source_episode = "episode:distractor".to_string();
        let groups = HashMap::from([
            ("episode:answer".to_string(), "session:a".to_string()),
            ("episode:antecedent".to_string(), "session:a".to_string()),
            ("episode:distractor".to_string(), "session:a".to_string()),
        ]);
        let direct_bundle = HashSet::from([
            ("session:a".to_string(), "episode:answer".to_string()),
            ("session:a".to_string(), "episode:antecedent".to_string()),
        ]);

        let selected = retain_best_source_group_items(
            vec![answer, antecedent, distractor],
            &groups,
            &direct_bundle,
            &HashMap::new(),
        );

        assert_eq!(
            selected
                .iter()
                .map(|item| item.source_episode.as_str())
                .collect::<Vec<_>>(),
            ["episode:answer", "episode:antecedent"]
        );
    }

    #[test]
    fn direct_lexical_evidence_precedes_a_higher_scoring_graph_neighbor_in_the_same_group() {
        let mut graph_neighbor = episode_item("semantically related graph neighbor");
        graph_neighbor.source_episode = "episode:graph".to_string();
        graph_neighbor.retrieval_tier = Some("graph".to_string());
        graph_neighbor.provenance["retrieval_admission"] = serde_json::json!({"basis": "semantic"});
        graph_neighbor.provenance["reranking"] = serde_json::json!({"relevance_logit": 1.19});

        let mut direct = episode_item("direct lexical answer");
        direct.source_episode = "episode:direct".to_string();
        direct.retrieval_tier = Some("direct".to_string());
        direct.provenance["retrieval_admission"] = serde_json::json!({
            "basis": "lexical",
            "matched_content_terms": ["charity", "race", "awareness"]
        });
        direct.provenance["reranking"] = serde_json::json!({"relevance_logit": 1.02});

        let groups = HashMap::from([
            ("episode:graph".to_string(), "session:a".to_string()),
            ("episode:direct".to_string(), "session:a".to_string()),
        ]);

        let selected = retain_best_source_group_items(
            vec![graph_neighbor, direct],
            &groups,
            &HashSet::new(),
            &HashMap::new(),
        );

        assert_eq!(
            selected
                .iter()
                .map(|item| item.source_episode.as_str())
                .collect::<Vec<_>>(),
            ["episode:direct"]
        );
    }

    #[test]
    fn selected_context_carries_episode_fallback_decision() {
        let selection = SelectedContext {
            selected: Vec::new(),
            ranked_candidates: Vec::new(),
            episode_fallback_items: Vec::new(),
            episode_fallback_source_anchors: Vec::new(),
            prefer_episode_content: true,
        };
        assert!(selection.prefer_episode_content);
    }

    fn query_terms() -> Vec<String> {
        crate::shared::search::search_query_terms("platform planning notes july 2025")
    }

    #[test]
    fn source_answer_must_belong_to_a_named_subject_in_the_query() {
        let subjects = vec!["evan".to_string()];

        assert!(episode_speaker_matches_subject(
            "Evan: We went to the Rockies.",
            &subjects
        ));
        assert!(!episode_speaker_matches_subject(
            "Sam: We went to the Rockies.",
            &subjects
        ));
    }

    #[test]
    fn decide_uses_episodes_when_episode_overlap_is_stronger() {
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);
        // The ranked fact has no lexical overlap with the query terms.
        let selected = vec![create_ranked_test_fact(
            "fact:noise",
            "Acme Corp quarterly renewal workflow.",
            RetrievalTier::Direct,
            fact_time,
        )];
        let episodes = vec![episode_item(
            "Platform planning notes July 2025: release scope, integrations, and response workflow updates.",
        )];

        let decision = EpisodeFallbackStrategy.decide(&selected, &episodes, &query_terms());
        assert_eq!(decision, FallbackDecision::UseEpisodes);
    }

    #[test]
    fn decide_uses_ranked_when_fact_overlap_is_equal_or_better() {
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);
        // The ranked fact directly matches the query terms.
        let selected = vec![create_ranked_test_fact(
            "fact:strong",
            "Platform planning notes July 2025 for release scope and integrations.",
            RetrievalTier::Direct,
            fact_time,
        )];
        let episodes = vec![episode_item(
            "Platform notes July 2025 with rollout reminders.",
        )];

        let decision = EpisodeFallbackStrategy.decide(&selected, &episodes, &query_terms());
        assert_eq!(decision, FallbackDecision::UseRanked);
    }

    #[test]
    fn decide_uses_ranked_when_no_episode_items_exist() {
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);
        let selected = vec![create_ranked_test_fact(
            "fact:weak",
            "Acme Corp quarterly renewal workflow.",
            RetrievalTier::Direct,
            fact_time,
        )];

        let decision = EpisodeFallbackStrategy.decide(&selected, &[], &query_terms());
        assert_eq!(decision, FallbackDecision::UseRanked);
    }

    #[test]
    fn decide_uses_ranked_when_graph_tier_is_present() {
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);
        // Graph-expanded facts block the episode rescue regardless of overlap.
        let selected = vec![create_ranked_test_fact(
            "fact:graph",
            "Acme Corp quarterly renewal workflow.",
            RetrievalTier::GraphExpanded,
            fact_time,
        )];
        let episodes = vec![episode_item(
            "Platform planning notes July 2025: release scope, integrations, and response workflow updates.",
        )];

        let decision = EpisodeFallbackStrategy.decide(&selected, &episodes, &query_terms());
        assert_eq!(decision, FallbackDecision::UseRanked);
    }
}
