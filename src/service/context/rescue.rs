use std::collections::HashSet;

use serde_json::{Value, json};

use crate::models::AssembledContextItem;
use crate::service::decayed_confidence;

use super::lexical;
use super::ranking;
use super::scoring::ranked_fact_to_item;

fn query_is_first_person_memory(query_opt: Option<&str>) -> bool {
    query_opt.is_some_and(ranking::query_is_first_person_memory)
}

const FIRST_PERSON_RESCUE_QUERY_TERMS: &[&str] = &[
    "what",
    "would",
    "should",
    "could",
    "want",
    "suggest",
    "recommend",
];

fn conversational_overlap_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 4)
        .collect()
}

pub(super) fn first_person_rescue_query_terms(
    raw_query_opt: Option<&str>,
    query_terms: &[String],
) -> Vec<String> {
    let Some(raw_query) = raw_query_opt else {
        return query_terms.to_vec();
    };
    if !query_is_first_person_memory(Some(raw_query)) {
        return query_terms.to_vec();
    }

    let mut terms = query_terms.to_vec();
    let mut seen_terms = terms.iter().cloned().collect::<HashSet<_>>();
    for token in conversational_overlap_tokens(raw_query) {
        if !FIRST_PERSON_RESCUE_QUERY_TERMS.contains(&token.as_str()) {
            continue;
        }
        if seen_terms.insert(token.clone()) {
            terms.push(token);
        }
    }

    terms
}

fn matched_first_person_rescue_terms_for_text(
    text: &str,
    query_terms: &[String],
) -> HashSet<String> {
    if query_terms.is_empty() {
        return HashSet::new();
    }

    let mut content_terms = crate::service::query::search_query_terms(text)
        .into_iter()
        .collect::<HashSet<_>>();
    for token in conversational_overlap_tokens(text) {
        if FIRST_PERSON_RESCUE_QUERY_TERMS.contains(&token.as_str()) {
            content_terms.insert(token);
        }
    }

    query_terms
        .iter()
        .filter(|term| content_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

fn selected_item_matched_terms(
    selected_items: &[AssembledContextItem],
    query_terms: &[String],
) -> HashSet<String> {
    let mut matched_terms = HashSet::new();

    for item in selected_items {
        matched_terms.extend(matched_first_person_rescue_terms_for_text(
            &item.content,
            query_terms,
        ));
    }

    matched_terms
}

fn first_person_episode_grounding_bonus(content: &str) -> isize {
    let trimmed = content.trim_start();

    if trimmed.starts_with("Current user persona:")
        || trimmed.starts_with("User profile:")
        || trimmed.starts_with("Current profile:")
        || trimmed.starts_with("Profile:")
    {
        15
    } else if trimmed.starts_with("User:") {
        2
    } else if trimmed.starts_with("Assistant:") {
        -2
    } else {
        0
    }
}

fn first_person_fact_grounding_bonus(content: &str) -> isize {
    let trimmed = content.trim_start();

    if trimmed.starts_with("User:") {
        8
    } else if trimmed.starts_with("Current user persona:")
        || trimmed.starts_with("User profile:")
        || trimmed.starts_with("Current profile:")
        || trimmed.starts_with("Profile:")
    {
        2
    } else if trimmed.starts_with("Assistant:") {
        -6
    } else {
        0
    }
}

fn matched_query_terms_for_text(text: &str, query_terms: &[String]) -> HashSet<String> {
    if query_terms.is_empty() {
        return HashSet::new();
    }

    let content_terms = crate::service::query::search_query_terms(text)
        .into_iter()
        .collect::<HashSet<_>>();

    query_terms
        .iter()
        .filter(|term| content_terms.contains(term.as_str()))
        .cloned()
        .collect()
}

pub(super) fn maybe_append_first_person_ranked_fact_item(
    results: &mut Vec<AssembledContextItem>,
    ranked_candidates: &[ranking::RankedContextFact],
    raw_query_opt: Option<&str>,
    query_terms: &[String],
    budget: usize,
    cutoff: chrono::DateTime<chrono::Utc>,
) {
    if !query_is_first_person_memory(raw_query_opt) || ranked_candidates.is_empty() {
        return;
    }

    let rescue_query_terms = first_person_rescue_query_terms(raw_query_opt, query_terms);
    if rescue_query_terms.len() <= query_terms.len() || rescue_query_terms.is_empty() {
        return;
    }

    let selected_terms = selected_item_matched_terms(results, &rescue_query_terms);
    let seen_fact_ids = results
        .iter()
        .map(|item| item.fact_id.as_str())
        .collect::<HashSet<_>>();
    let seen_source_episodes = results
        .iter()
        .map(|item| item.source_episode.as_str())
        .collect::<HashSet<_>>();

    let candidate = ranked_candidates
        .iter()
        .filter_map(|ranked| {
            if seen_fact_ids.contains(ranked.fact.fact_id.as_str())
                || seen_source_episodes.contains(ranked.fact.source_episode.as_str())
            {
                return None;
            }
            if !ranked.fact.content.trim_start().starts_with("User:") {
                return None;
            }

            let matched_terms = matched_first_person_rescue_terms_for_text(
                &ranked.fact.content,
                &rescue_query_terms,
            );
            let unique_term_count = matched_terms
                .iter()
                .filter(|term| !selected_terms.contains(term.as_str()))
                .count();
            if unique_term_count == 0 {
                return None;
            }

            let overlap_count = matched_terms.len();
            let lexical_score =
                lexical::lexical_query_score_for_text(&ranked.fact.content, &rescue_query_terms);
            let grounding_bonus = first_person_fact_grounding_bonus(&ranked.fact.content);
            let priority = (unique_term_count as isize * 8)
                + (overlap_count as isize * 4)
                + (lexical_score as isize * 2)
                + grounding_bonus;

            Some((priority, overlap_count, lexical_score, ranked))
        })
        .max_by(
            |(left_priority, left_overlap, left_lexical, left_ranked),
             (right_priority, right_overlap, right_lexical, right_ranked)| {
                left_priority
                    .cmp(right_priority)
                    .then_with(|| left_overlap.cmp(right_overlap))
                    .then_with(|| left_lexical.cmp(right_lexical))
                    .then_with(|| {
                        ranking::ranked_relevance_score(left_ranked)
                            .total_cmp(&ranking::ranked_relevance_score(right_ranked))
                    })
            },
        )
        .map(|(_, _, _, ranked)| ranked);

    let Some(candidate) = candidate.cloned() else {
        return;
    };

    if results.len() >= budget && budget > 0 {
        results.pop();
    }

    if results.len() < budget {
        results.push(ranked_fact_to_item(candidate, cutoff, decayed_confidence));
    }
}

pub(super) fn maybe_append_first_person_episode_item(
    results: &mut Vec<AssembledContextItem>,
    episode_items: &[AssembledContextItem],
    selected_terms: &HashSet<String>,
    query_opt: Option<&str>,
    query_terms: &[String],
    budget: usize,
) {
    if !query_is_first_person_memory(query_opt)
        || query_terms.is_empty()
        || episode_items.is_empty()
    {
        return;
    }

    let seen_fact_ids = results
        .iter()
        .map(|item| item.fact_id.as_str())
        .collect::<HashSet<_>>();
    let seen_source_episodes = results
        .iter()
        .map(|item| item.source_episode.as_str())
        .collect::<HashSet<_>>();

    let candidate = episode_items
        .iter()
        .filter_map(|item| {
            if seen_fact_ids.contains(item.fact_id.as_str())
                || seen_source_episodes.contains(item.source_episode.as_str())
            {
                return None;
            }

            let matched_terms = matched_query_terms_for_text(&item.content, query_terms);
            let unique_term_count = matched_terms
                .iter()
                .filter(|term| !selected_terms.contains(term.as_str()))
                .count();
            if unique_term_count == 0 {
                return None;
            }

            let lexical_score = lexical::lexical_query_score_for_text(&item.content, query_terms);
            let grounding_bonus = first_person_episode_grounding_bonus(&item.content);
            let priority =
                (unique_term_count as isize * 6) + (lexical_score as isize * 2) + grounding_bonus;

            Some((priority, lexical_score, item))
        })
        .max_by(
            |(left_priority, left_lexical, left_item),
             (right_priority, right_lexical, right_item)| {
                left_priority
                    .cmp(right_priority)
                    .then_with(|| left_lexical.cmp(right_lexical))
                    .then_with(|| right_item.content.len().cmp(&left_item.content.len()))
            },
        )
        .map(|(_, _, item)| item);

    let Some(candidate) = candidate.cloned() else {
        return;
    };

    if results.len() >= budget && budget > 0 {
        results.pop();
    }

    if results.len() < budget {
        results.push(candidate);
    }
}

pub(super) fn build_episode_rescue_log_result(
    episode_candidate_count: usize,
    selected_fact_count: usize,
    episode_rescue_used: bool,
) -> Value {
    json!({
        "episode_candidate_count": episode_candidate_count,
        "selected_fact_count": selected_fact_count,
        "episode_rescue_used": episode_rescue_used,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- conversational_overlap_tokens -------------------------------------

    #[test]
    fn conversational_overlap_extracts_long_enough_tokens() {
        let tokens = conversational_overlap_tokens("hi what would you suggest");
        assert!(tokens.iter().any(|t| t == "what"));
        assert!(tokens.iter().any(|t| t == "would"));
        assert!(tokens.iter().any(|t| t == "suggest"));
        assert!(tokens.iter().all(|t| t != "hi" && t != "you"));
    }

    #[test]
    fn conversational_overlap_handles_punctuation() {
        let tokens = conversational_overlap_tokens("what? should! we.");
        assert!(tokens.iter().any(|t| t == "what"));
        assert!(tokens.iter().any(|t| t == "should"));
    }

    #[test]
    fn conversational_overlap_empty_for_short_text() {
        assert!(conversational_overlap_tokens("hi").is_empty());
    }

    #[test]
    fn conversational_overlap_lowercases_tokens() {
        let tokens = conversational_overlap_tokens("WHAT Would");
        assert!(tokens.iter().any(|t| t == "what"));
        assert!(tokens.iter().any(|t| t == "would"));
    }

    // -- first_person_rescue_query_terms -----------------------------------

    #[test]
    fn rescue_terms_adds_missing_conversational_tokens() {
        let terms = first_person_rescue_query_terms(
            Some("what should I do about this"),
            &["do".to_string(), "this".to_string()],
        );
        assert!(terms.iter().any(|t| t == "what"));
        assert!(terms.iter().any(|t| t == "should"));
        assert!(terms.iter().all(|t| t != "about"));
    }

    #[test]
    fn rescue_terms_preserves_original_terms() {
        let terms = first_person_rescue_query_terms(
            Some("what should I do"),
            &["do".to_string(), "this".to_string()],
        );
        assert!(terms.iter().any(|t| t == "do"));
        assert!(terms.iter().any(|t| t == "this"));
    }

    #[test]
    fn rescue_terms_no_op_for_non_first_person_query() {
        let terms = first_person_rescue_query_terms(
            Some("how many widgets did we sell"),
            &["widgets".to_string(), "sell".to_string()],
        );
        assert_eq!(terms.len(), 2);
        assert_eq!(terms, vec!["widgets".to_string(), "sell".to_string()]);
    }

    #[test]
    fn rescue_terms_no_op_when_none_query() {
        let terms = first_person_rescue_query_terms(None, &["hello".to_string()]);
        assert_eq!(terms, vec!["hello".to_string()]);
    }

    #[test]
    fn rescue_terms_deduplicates_added_terms() {
        let terms = first_person_rescue_query_terms(
            Some("what would you do"),
            &["what".to_string(), "would".to_string()],
        );
        assert_eq!(terms.iter().filter(|t| t.as_str() == "what").count(), 1);
    }

    // -- first_person_episode_grounding_bonus ------------------------------

    #[test]
    fn episode_grounding_bonus_persona_profiles() {
        assert_eq!(
            first_person_episode_grounding_bonus("Current user persona: friendly"),
            15
        );
        assert_eq!(
            first_person_episode_grounding_bonus("User profile: admin"),
            15
        );
        assert_eq!(
            first_person_episode_grounding_bonus("Current profile: dev"),
            15
        );
        assert_eq!(first_person_episode_grounding_bonus("Profile: tester"), 15);
    }

    #[test]
    fn episode_grounding_bonus_user_prefix() {
        assert_eq!(first_person_episode_grounding_bonus("User: hello"), 2);
    }

    #[test]
    fn episode_grounding_bonus_assistant_penalty() {
        assert_eq!(first_person_episode_grounding_bonus("Assistant: hello"), -2);
    }

    #[test]
    fn episode_grounding_bonus_neutral() {
        assert_eq!(first_person_episode_grounding_bonus("Hello world"), 0);
        assert_eq!(first_person_episode_grounding_bonus(""), 0);
    }

    // -- first_person_fact_grounding_bonus ---------------------------------

    #[test]
    fn fact_grounding_bonus_user_prefix() {
        assert_eq!(first_person_fact_grounding_bonus("User: hello"), 8);
    }

    #[test]
    fn fact_grounding_bonus_profile_prefixes() {
        assert_eq!(
            first_person_fact_grounding_bonus("Current user persona: admin"),
            2
        );
        assert_eq!(first_person_fact_grounding_bonus("User profile: tester"), 2);
        assert_eq!(first_person_fact_grounding_bonus("Current profile: dev"), 2);
        assert_eq!(first_person_fact_grounding_bonus("Profile: mod"), 2);
    }

    #[test]
    fn fact_grounding_bonus_assistant_penalty() {
        assert_eq!(first_person_fact_grounding_bonus("Assistant: hello"), -6);
    }

    #[test]
    fn fact_grounding_bonus_neutral() {
        assert_eq!(first_person_fact_grounding_bonus("Hello world"), 0);
    }

    // -- matched_first_person_rescue_terms_for_text ------------------------

    #[test]
    fn matched_rescue_terms_finds_query_terms_and_conversational_tokens() {
        let terms = matched_first_person_rescue_terms_for_text(
            "what would you suggest for this problem",
            &[
                "suggest".to_string(),
                "problem".to_string(),
                "missing".to_string(),
            ],
        );
        assert!(terms.contains("suggest"));
        assert!(terms.contains("problem"));
        assert!(!terms.contains("missing"));
    }

    #[test]
    fn matched_rescue_terms_includes_conversational_tokens_from_content() {
        let terms = matched_first_person_rescue_terms_for_text(
            "I would recommend coffee",
            &["recommend".to_string()],
        );
        assert!(terms.contains("recommend"));
    }

    #[test]
    fn matched_rescue_terms_empty_when_no_match() {
        let terms =
            matched_first_person_rescue_terms_for_text("hello world", &["missing".to_string()]);
        assert!(terms.is_empty());
    }

    #[test]
    fn matched_rescue_terms_empty_query() {
        let terms = matched_first_person_rescue_terms_for_text("hello", &[]);
        assert!(terms.is_empty());
    }

    // -- build_episode_rescue_log_result -----------------------------------

    #[test]
    fn rescue_log_result_builds_json() {
        let result = build_episode_rescue_log_result(10, 3, true);
        assert_eq!(result["episode_candidate_count"], 10);
        assert_eq!(result["selected_fact_count"], 3);
        assert_eq!(result["episode_rescue_used"], true);
    }

    #[test]
    fn rescue_log_result_defaults() {
        let result = build_episode_rescue_log_result(0, 0, false);
        assert_eq!(result["episode_candidate_count"], 0);
        assert_eq!(result["selected_fact_count"], 0);
        assert_eq!(result["episode_rescue_used"], false);
    }

    // -- maybe_append_first_person_episode_item (scoring logic) ------------

    #[test]
    fn episode_grounding_bonus_leading_whitespace() {
        assert_eq!(
            first_person_episode_grounding_bonus("  User: hello"),
            2,
            "leading whitespace before 'User:' should still match",
        );
        assert_eq!(
            first_person_episode_grounding_bonus("  Current user persona: admin"),
            15,
            "leading whitespace before persona should still match",
        );
    }

    #[test]
    fn fact_grounding_bonus_leading_whitespace() {
        assert_eq!(
            first_person_fact_grounding_bonus("\nUser: hello"),
            8,
            "leading newline before 'User:' should still match",
        );
        assert_eq!(
            first_person_fact_grounding_bonus("\tAssistant: response"),
            -6,
            "leading tab before 'Assistant:' should still match",
        );
    }
}
