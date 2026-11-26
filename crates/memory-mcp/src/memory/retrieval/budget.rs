use crate::models::{AssembleContextRequest, AssembledContextItem};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use super::admission::{build_query_evidence, episode_candidate_is_admitted_at_reference_time};
use super::filtering;
use super::params::DefaultContextParams;
use super::ranking::default_episode_fallback_rationale;
use super::scoring::selected_fact_query_term_coverage;
use super::types::{RankedContextFact, RetrievalTier};
use crate::error::MemoryError;
use crate::memory::retrieval::RetrievalContext;
use crate::shared::search_lexical::matched_query_terms_for_text;

// Leave space for the source metadata and delimiters in downstream context windows.
const MAX_EVIDENCE_EXCERPT_BYTES: usize = 2048;

pub(super) fn bound_context_evidence(
    items: &mut [AssembledContextItem],
    request: &AssembleContextRequest,
) {
    let query = request.query.as_str();
    let query_terms = crate::shared::search::search_query_terms(query);
    let control_labels = if query_terms
        .iter()
        .any(|term| matches!(term.as_str(), "hidden" | "invisible"))
    {
        Vec::new()
    } else {
        super::query_mode::control_query_labels(query)
    };
    // Annotation completeness is no evidence of recency. Explicit state/time
    // questions retain their existing narrative/distance ordering instead.
    let prefer_visible_container = request.as_of.is_none()
        && request.window_start.is_none()
        && request.window_end.is_none()
        && request.view_mode.is_none()
        && super::temporal::infer_temporal_window(query, crate::shared::temporal::now()).is_none()
        && !query
            .split('"')
            .step_by(2)
            .flat_map(|part| part.split(|ch: char| !ch.is_alphanumeric()))
            .any(|word| {
                matches!(
                    word.to_lowercase().as_str(),
                    "after"
                        | "before"
                        | "later"
                        | "earlier"
                        | "latest"
                        | "previous"
                        | "first"
                        | "last"
                        | "then"
                        | "when"
                        | "since"
                        | "until"
                        | "during"
                        | "while"
                        | "current"
                        | "new"
                        | "old"
                        | "updated"
                        | "changed"
                        | "now"
                )
            });
    for item in items {
        if item.content.len().max(item.quote.len()) <= MAX_EVIDENCE_EXCERPT_BYTES {
            continue;
        }
        if !item.provenance.is_object() {
            item.provenance = serde_json::json!({});
        }

        let same_text = item.content == item.quote;
        let episode_fallback = item.provenance["episode_fallback"] == true && same_text;
        if let Some(range) = evidence_excerpt_range(
            &item.content,
            &query_terms,
            &control_labels,
            prefer_visible_container,
        ) {
            if episode_fallback {
                item.provenance["episode_excerpt"] = serde_json::json!({
                    "start_byte": range.start,
                    "end_byte": range.end,
                    "source_bytes": item.content.len(),
                });
                item.quote = item.content[range.clone()].to_owned();
            } else {
                item.provenance["content_excerpt"] = excerpt_metadata(&range, item.content.len());
                if same_text {
                    item.provenance["quote_excerpt"] = item.provenance["content_excerpt"].clone();
                    item.quote = item.content[range.clone()].to_owned();
                }
            }
            item.content = item.content[range].to_owned();
        }
        if let Some(range) = evidence_excerpt_range(
            &item.quote,
            &query_terms,
            &control_labels,
            prefer_visible_container,
        ) {
            item.provenance["quote_excerpt"] = excerpt_metadata(&range, item.quote.len());
            item.quote = item.quote[range].to_owned();
        }
    }
}

fn excerpt_metadata(range: &Range<usize>, bytes: usize) -> serde_json::Value {
    serde_json::json!({
        "start_byte": range.start,
        "end_byte": range.end,
        "original_bytes": bytes,
    })
}

fn evidence_excerpt_range<'a>(
    text: &'a str,
    query_terms: &[String],
    control_labels: &[String],
    prefer_visible_container: bool,
) -> Option<Range<usize>> {
    if text.len() <= MAX_EVIDENCE_EXCERPT_BYTES {
        return None;
    }
    if query_terms.is_empty() {
        return Some(0..text.floor_char_boundary(MAX_EVIDENCE_EXCERPT_BYTES));
    }

    let last_start = text.len() - MAX_EVIDENCE_EXCERPT_BYTES;
    let margin = MAX_EVIDENCE_EXCERPT_BYTES / 4;
    // Repeated source windows have the same score for this immutable query.
    // Borrow only for this excerpt search; no cache outlives or owns the source.
    let mut scores = HashMap::new();
    let mut scorer = super::lexical::TextLexicalScorer::new(query_terms);
    let mut score_text =
        |window: &'a str| *scores.entry(window).or_insert_with(|| scorer.score(window));
    let mut best_score = None;
    let mut best_range = 0..0;
    // Overlap adjacent windows so short evidence can survive a window boundary.
    for offset in (0..last_start)
        .step_by(MAX_EVIDENCE_EXCERPT_BYTES / 2)
        .chain(std::iter::once(last_start))
    {
        let start = text.ceil_char_boundary(offset);
        let end = text.floor_char_boundary(offset + MAX_EVIDENCE_EXCERPT_BYTES);
        let overlap = score_text(&text[start..end]);
        if best_score.is_some_and(|(best_overlap, _)| overlap < best_overlap) {
            continue;
        }
        // Prefer room around a match for the words carrying its answer.
        let inner_start = text.ceil_char_boundary(offset + margin);
        let inner_end = text.floor_char_boundary(offset + 3 * margin);
        let interior_overlap = score_text(&text[inner_start..inner_end]);
        let score = (overlap, interior_overlap);
        if best_score.is_none_or(|best| score > best) {
            best_score = Some(score);
            best_range = start..end;
        }
    }
    Some(preserve_control_block(
        text,
        query_terms,
        control_labels,
        prefer_visible_container,
        best_range,
    ))
}

// Keep a short, contiguous accessibility control group intact. Explicit UI
// enumerations rank complete query-bearing groups; other reads keep overlap with
// the lexical window. Both plain trees and JSON-escaped trees occur in episodes.
// The source remains a byte-exact, bounded excerpt with original offsets.
fn preserve_control_block(
    text: &str,
    terms: &[String],
    control_labels: &[String],
    prefer_visible_container: bool,
    original: Range<usize>,
) -> Range<usize> {
    let is_control_term = |term: &str| {
        matches!(
            term,
            "dropdown" | "toolbar" | "option" | "options" | "button" | "buttons" | "tab" | "tabs"
        )
    };
    let enumerates_controls = terms.iter().any(|term| is_control_term(term));
    let enumerates_tabs = terms
        .iter()
        .any(|term| matches!(term.as_str(), "tab" | "tabs"))
        && !terms
            .iter()
            .any(|term| matches!(term.as_str(), "button" | "buttons" | "toolbar" | "dropdown"));
    let topical_terms: Vec<_> = terms
        .iter()
        // Every navigation list contains the structural role `list`. That
        // alone cannot establish a topical match for a requested tab group.
        .filter(|term| !is_control_term(term) && !(enumerates_tabs && term.as_str() == "list"))
        .cloned()
        .collect();
    struct Line<'text> {
        start: usize,
        end: usize,
        depth: usize,
        control: bool,
        button: bool,
        list: bool,
        link: bool,
        visible_label: Option<&'text str>,
        visible_container: bool,
    }
    let mut starts = vec![0];
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            starts.push(i + 1);
        } else if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'n') {
            starts.push(i + 2);
            i += 1;
        }
        i += 1;
    }
    let mut containers: Vec<(usize, bool)> = Vec::new();
    let lines: Vec<_> = starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).copied().unwrap_or(text.len());
            let mut body = &text[start..end];
            let mut depth = 0;
            loop {
                if let Some(rest) = body.strip_prefix("\\t") {
                    body = rest;
                    depth += 1;
                } else if let Some(rest) = body.strip_prefix('\t') {
                    body = rest;
                    depth += 1;
                } else if let Some(rest) = body.strip_prefix(' ') {
                    body = rest;
                    depth += 1;
                } else {
                    break;
                }
            }
            let role = body
                .strip_prefix('[')
                .and_then(|rest| rest.split_once("] "))
                .filter(|(id, _)| id.chars().all(|c| c.is_ascii_digit()))
                .map(|(_, role)| role)
                .unwrap_or("");
            let button = role.starts_with("button ");
            let control = button || role.starts_with("list ") || role.starts_with("listitem ");
            while containers
                .last()
                .is_some_and(|(parent_depth, _)| *parent_depth >= depth)
            {
                containers.pop();
            }
            let visible_container = containers.last().is_some_and(|(_, visible)| *visible);
            if role.split_whitespace().next().is_some_and(|name| {
                matches!(
                    name,
                    "main"
                        | "toolbar"
                        | "group"
                        | "region"
                        | "dialog"
                        | "tabpanel"
                        | "section"
                        | "Section"
                        | "navigation"
                )
            }) {
                containers.push((depth, visible_role_label(role).is_some()));
            }
            Line {
                start,
                end,
                depth,
                control,
                button,
                list: role.starts_with("list "),
                link: role.starts_with("link "),
                visible_label: button.then(|| visible_role_label(role)).flatten(),
                visible_container,
            }
        })
        .collect();
    let mut best = None;
    let mut overlapping_tabs: Option<Range<usize>> = None;
    let mut index = 0;
    while index < lines.len() {
        let first = &lines[index];
        let link_list = enumerates_tabs && first.list;
        if !(link_list || first.button && !enumerates_tabs) {
            index += 1;
            continue;
        }
        let mut controls = usize::from(first.button);
        let mut end_index = index + 1;
        while let Some(line) = lines.get(end_index) {
            if line.depth < first.depth || (link_list && line.depth == first.depth) {
                break;
            }
            if link_list && line.end - first.start > MAX_EVIDENCE_EXCERPT_BYTES {
                break;
            }
            if link_list && line.link {
                controls += 1;
            }
            if line.depth == first.depth {
                if !line.control {
                    break;
                }
                controls += 1;
            }
            end_index += 1;
        }
        let start_index = index;
        // A long enclosing list may contain a short eligible tab list. Keep
        // scanning its descendants; each list scan itself is byte bounded.
        index = if link_list {
            index + 1
        } else {
            end_index.max(index + 1)
        };
        if controls < 2 {
            continue;
        }
        // Include the closing sibling, which makes the group's end observable.
        let end = lines.get(end_index).map_or(text.len(), |line| line.end);
        let range = first.start..end;
        if range.len() > MAX_EVIDENCE_EXCERPT_BYTES
            || (!enumerates_controls
                && (range.start >= original.end || range.end <= original.start))
        {
            continue;
        }
        let block_score = super::lexical::lexical_query_score_for_text(&text[range.clone()], terms);
        if block_score == 0
            || (enumerates_controls
                && super::lexical::lexical_query_score_for_text(
                    &text[range.clone()],
                    &topical_terms,
                ) == 0)
        {
            continue;
        }
        if link_list && range.start < original.end && range.end > original.start {
            overlapping_tabs = Some(overlapping_tabs.map_or_else(
                || range.clone(),
                |seen| seen.start.min(range.start)..seen.end.max(range.end),
            ));
        }
        // For an enumeration the complete group is the evidence unit, not a
        // narrative about opening it. Rank eligible groups by their own query
        // support near the winning narrative; distance precedes lexical tie-breaks
        // so a different historical UI state cannot win on generic sort words.
        // No answer labels, source IDs or fixed distance-to-gold enter selection.
        let start = text.ceil_char_boundary(
            range
                .start
                .saturating_sub((MAX_EVIDENCE_EXCERPT_BYTES - range.len()) / 2),
        );
        let end = text.floor_char_boundary((start + MAX_EVIDENCE_EXCERPT_BYTES).min(text.len()));
        let context_score = if enumerates_controls {
            super::lexical::lexical_query_score_for_text(&text[start..end], terms)
        } else {
            0
        };
        let distance =
            range.start.saturating_sub(original.end) + original.start.saturating_sub(range.end);
        // A nearby hidden historical copy cannot establish the named controls
        // currently exposed by a UI. Count only explicit visibility of the
        // requested labels, not unrelated visible filters. Unknown visibility
        // remains eligible and preserves the previous distance ordering on ties.
        let visible_label_count = control_labels
            .iter()
            .filter(|label| {
                lines[start_index..end_index].iter().any(|line| {
                    line.visible_label.is_some_and(|name| {
                        super::query_mode::control_label_coverage(
                            name,
                            std::slice::from_ref(*label),
                        ) > 0.0
                    })
                })
            })
            .count();
        let container_confirmed_labels = if prefer_visible_container {
            control_labels
                .iter()
                .filter(|label| {
                    lines[start_index..end_index].iter().any(|line| {
                        line.visible_container
                            && line.visible_label.is_some_and(|name| {
                                super::query_mode::control_label_coverage(
                                    name,
                                    std::slice::from_ref(*label),
                                ) > 0.0
                            })
                    })
                })
                .count()
        } else {
            0
        };
        let score = (
            visible_label_count,
            container_confirmed_labels,
            std::cmp::Reverse(distance),
            block_score,
            context_score,
        );
        if best
            .as_ref()
            .is_some_and(|(best_score, _)| score <= *best_score)
        {
            continue;
        }
        best = Some((score, range));
    }
    let Some((_, mut block)) = best else {
        return original;
    };
    // A lexical window can intersect two neighboring navigation groups. Keep
    // both complete when their contiguous union fits; centering only the first
    // would cut the second even though it was already part of this window.
    if let Some(overlap) = overlapping_tabs
        && overlap.len() <= MAX_EVIDENCE_EXCERPT_BYTES
    {
        block = overlap;
    }
    let start = text.ceil_char_boundary(
        block
            .start
            .saturating_sub((MAX_EVIDENCE_EXCERPT_BYTES - block.len()) / 2),
    );
    let end = text.floor_char_boundary((start + MAX_EVIDENCE_EXCERPT_BYTES).min(text.len()));
    let adjusted = start..end;
    // An explicit UI enumeration may need the entire group even when some
    // lexical context lies outside it; other reads retain the lexical winner.
    if !enumerates_controls
        && super::lexical::lexical_query_score_for_text(&text[adjusted.clone()], terms)
            < super::lexical::lexical_query_score_for_text(&text[original.clone()], terms)
    {
        original
    } else {
        adjusted
    }
}

// Accessibility roles quote names and may quote attribute values. A visibility
// word inside either value is data, not the standalone visibility attribute.
// Malformed/unclosed quoted values provide no positive visibility evidence.
fn visible_role_label(role: &str) -> Option<&str> {
    let (_, named) = role.split_once(" '")?;
    let mut escaped = false;
    let end = named.char_indices().find_map(|(index, ch)| {
        if escaped {
            escaped = false;
            None
        } else if ch == '\\' {
            escaped = true;
            None
        } else {
            (ch == '\'').then_some(index)
        }
    })?;
    let label = &named[..end];
    let attributes = named[end + 1..]
        .strip_prefix(',')?
        .trim_end()
        .trim_end_matches("\\n");
    let mut quote = None;
    let mut escaped = false;
    let mut start = 0;
    let mut visible = false;
    for (index, ch) in attributes.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(delimiter) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == ',' {
            visible |= attributes[start..index].trim() == "visible";
            start = index + 1;
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    visible |= attributes[start..].trim() == "visible";
    visible.then_some(label)
}

pub(super) fn should_prefer_episode_content(
    selected_facts: &[RankedContextFact],
    episode_items: &[AssembledContextItem],
    query_terms: &[String],
) -> bool {
    if episode_items.is_empty() || query_terms.is_empty() {
        return false;
    }

    // A rescue cannot discard the named controls supported by the fact pack
    // merely because unrelated words are spread across a long episode.
    let fact_labels = selected_facts
        .iter()
        .map(|f| f.evidence.control_label_coverage)
        .fold(0.0, f64::max);
    let episode_labels = episode_items
        .iter()
        .filter_map(|i| {
            i.provenance
                .get("control_label_coverage")
                .and_then(serde_json::Value::as_f64)
        })
        .fold(0.0, f64::max);
    if fact_labels > episode_labels {
        return false;
    }

    if episode_items.iter().any(|episode| {
        selected_facts
            .iter()
            .any(|fact| fact.fact.source_episode == episode.source_episode)
    }) {
        return false;
    }

    if selected_facts.iter().any(|fact| {
        matches!(
            fact.retrieval_tier,
            RetrievalTier::SourceExpanded | RetrievalTier::GraphExpanded
        )
    }) {
        return false;
    }

    let selected_fact_terms = selected_facts
        .iter()
        .flat_map(|fact| matched_query_terms_for_text(&fact.fact.content, query_terms))
        .collect::<HashSet<_>>();
    let episode_terms = episode_items
        .iter()
        .flat_map(|item| matched_query_terms_for_text(&item.content, query_terms))
        .collect::<HashSet<_>>();
    if !selected_fact_terms.is_subset(&episode_terms) {
        return false;
    }

    let best_fact_reranker = max_consistent_reranker_score(
        selected_facts
            .iter()
            .map(|fact| fact.evidence.reranking.as_ref().and_then(reranker_score)),
    );
    let best_episode_reranker = max_consistent_reranker_score(
        episode_items
            .iter()
            .map(|item| item.provenance.get("reranking").and_then(reranker_score)),
    );
    if let (Some((fact_signature, fact_score)), Some((episode_signature, episode_score))) =
        (best_fact_reranker, best_episode_reranker)
        && fact_signature == episode_signature
    {
        let episode_floor = episode_items
            .iter()
            .filter(|item| {
                item.provenance
                    .get("reranking")
                    .and_then(reranker_score)
                    .is_some_and(|(signature, score)| {
                        signature == episode_signature && score == episode_score
                    })
            })
            .filter_map(|item| {
                item.provenance
                    .get("reranking")
                    .and_then(|evidence| evidence.get("admission_floor"))
                    .and_then(serde_json::Value::as_f64)
            })
            .max_by(f64::total_cmp);
        if episode_floor.is_some_and(|floor| episode_score < floor) {
            return false;
        }
        if episode_score > fact_score {
            return true;
        }
    }

    let best_fact_overlap = selected_facts
        .iter()
        .map(|fact| super::lexical::lexical_query_score_for_fact(&fact.fact, query_terms))
        .max()
        .unwrap_or(0);

    let Some(best_episode_item) = episode_items.iter().max_by_key(|item| {
        super::lexical::lexical_query_score_for_text(&item.content, query_terms)
    }) else {
        return false;
    };

    let best_episode_overlap =
        super::lexical::lexical_query_score_for_text(&best_episode_item.content, query_terms);

    if best_episode_overlap <= best_fact_overlap {
        return false;
    }

    let best_episode_term_coverage =
        matched_query_terms_for_text(&best_episode_item.content, query_terms).len();
    let selected_fact_term_coverage =
        selected_fact_query_term_coverage(selected_facts, query_terms);

    best_episode_term_coverage > selected_fact_term_coverage
}

fn reranker_score(evidence: &serde_json::Value) -> Option<(&str, f64)> {
    let signature = evidence.get("model_signature")?.as_str()?;
    let score = evidence.get("relevance_logit")?.as_f64()?;
    (!signature.is_empty() && score.is_finite()).then_some((signature, score))
}

fn max_consistent_reranker_score<'a>(
    evidence: impl IntoIterator<Item = Option<(&'a str, f64)>>,
) -> Option<(&'a str, f64)> {
    let mut signature = None;
    let mut max_score: Option<f64> = None;
    for (candidate_signature, score) in evidence.into_iter().collect::<Option<Vec<_>>>()? {
        if signature.is_some_and(|signature| signature != candidate_signature) {
            return None;
        }
        signature = Some(candidate_signature);
        max_score = Some(max_score.map_or(score, |current| current.max(score)));
    }
    Some((signature?, max_score?))
}

pub(super) async fn collect_episode_fallback_items(
    service: &RetrievalContext,
    params: &DefaultContextParams<'_>,
    query: &str,
    #[cfg(feature = "eval-support")] trace: super::diagnostics::EvidenceRecorder<'_>,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let response_budget = params.budget.max(1) as usize;
    let candidate_budget = super::reranking::candidate_pool_limit(
        response_budget,
        service.embedding_provider.reranking_signature().is_some(),
    );
    let candidate_limit = candidate_budget.min(i32::MAX as usize) as i32;
    let episode_records = super::lexical::select_episode_records_for_query(
        service,
        params.cutoff_iso,
        Some(query),
        candidate_limit,
    )
    .await?;

    #[cfg(feature = "eval-support")]
    super::diagnostics::record_candidates(trace, "episodes.fetched", &episode_records);
    let query_terms = crate::shared::search::search_query_terms(query);
    let raw_query = params.raw_query_opt.unwrap_or(query);
    // A producer-declared conversation turn can locate an answer in its
    // neighborhood without independently asserting it. Use normalized terms
    // for that candidate pool, retaining every substantive follow-up clause.
    // Standalone episodes must satisfy the complete raw answer admission policy.
    let semantic_query = crate::shared::search::preprocess_search_query(
        &super::query_mode::evidence_query_surface(raw_query),
    );
    let context_evidence =
        build_query_evidence(Some(&semantic_query), Vec::new(), &[], params.query_flags);
    let query_evidence = build_query_evidence(Some(raw_query), Vec::new(), &[], params.query_flags);
    let reranker_available = service.embedding_provider.reranking_signature().is_some();
    let mut episodes = filtering::filter_episodes_by_constraints(episode_records, params.access);
    episodes.retain(|episode| {
        let query_evidence = if episode.source_context.is_some() {
            &context_evidence
        } else {
            &query_evidence
        };
        // Episode fallback bypasses fact reconciliation, so preserve the
        // declared-subject boundary for structured records. Free-form
        // conversation turns keep the discourse-neighborhood policy below.
        super::admission::candidate_matches_declared_query_subject(raw_query, &episode.content)
            && if reranker_available {
                super::admission::episode_candidate_is_rerankable_at_reference_time(
                    query_evidence,
                    &episode.content,
                    Some(episode.t_ref),
                )
            } else {
                episode_candidate_is_admitted_at_reference_time(
                    query_evidence,
                    &episode.content,
                    Some(episode.t_ref),
                )
            }
    });

    let episodes = super::lexical::rank_episodes_by_query(episodes, &query_terms, 0);

    use super::views::{EpisodeFallbackParams, build_episode_fallback_items};

    let mut items = build_episode_fallback_items(EpisodeFallbackParams {
        episodes,
        query_opt: Some(query),
        semantic_available: service.embedding_provider.is_enabled(),
        cutoff: params.cutoff,
        window_start: params.window_start,
        window_end: params.window_end,
        timeline_mode: params.resolved_view_mode == Some("timeline"),
        budget: candidate_limit,
        fallback_rationale_fn: default_episode_fallback_rationale,
    });
    let labels = super::query_mode::control_query_labels(
        &super::query_mode::lexical_search_surface(raw_query),
    );
    if !reranker_available && !labels.is_empty() {
        for item in &mut items {
            item.provenance["control_label_coverage"] = serde_json::json!(
                super::query_mode::control_label_coverage(&item.content, &labels)
            );
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::retrieval::types::{RankedContextFact, RetrievalTier};
    use crate::models::Fact;

    fn make_fact(content: &str) -> Fact {
        Fact {
            fact_id: "f:1".into(),
            fact_type: "note".into(),
            content: content.into(),
            quote: String::new(),
            source_episode: "ep:1".into(),
            t_valid: chrono::Utc::now(),
            t_ingested: chrono::Utc::now(),
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 0.9,
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".into(),
            policy_tags: vec![],
            provenance: crate::models::Provenance::manual(),
            ft_score: 0.0,
        }
    }

    fn make_ranked(fact: Fact, tier: RetrievalTier) -> RankedContextFact {
        RankedContextFact {
            fact,
            evidence: crate::memory::retrieval::types::CandidateEvidence::default(),
            rationale: "test".into(),
            retrieval_tier: tier,
            fusion_score: 0.5,
            source_priority: 0,
            decayed_confidence: 0.8,
            query_alignment_factor: 1.0,
            grounding_score: 0.5,
            semantic_available: false,
            matched_query_terms: vec![],
            graph_trace: None,
        }
    }

    fn make_episode_item(content: &str) -> AssembledContextItem {
        AssembledContextItem {
            fact_id: "ep:1".into(),
            content: content.into(),
            quote: String::new(),
            source_episode: "ep:1".into(),
            confidence: 0.9,
            relevance: None,
            grounding: None,
            semantic_available: None,
            provenance: serde_json::json!({}),
            rationale: String::new(),
            retrieval_tier: None,
            reconciliation: None,
            evidence_status: None,
            retrieval_evidence: None,
        }
    }

    // -- should_prefer_episode_content -------------------------------------

    #[test]
    fn prefers_false_when_no_episode_items() {
        assert!(!should_prefer_episode_content(
            &[],
            &[],
            &["coffee".to_string()]
        ));
    }

    #[test]
    fn prefers_false_when_graph_expanded_facts_present() {
        let fact = make_fact("coffee brewing guide");
        let ranked = make_ranked(fact, RetrievalTier::GraphExpanded);
        let episodes = vec![make_episode_item("coffee brewing techniques")];
        assert!(!should_prefer_episode_content(
            &[ranked],
            &episodes,
            &["coffee".to_string()]
        ));
    }

    #[test]
    fn prefers_false_when_source_expanded_facts_present() {
        let fact = make_fact("coffee brewing guide");
        let ranked = make_ranked(fact, RetrievalTier::SourceExpanded);
        let episodes = vec![make_episode_item("coffee brewing techniques")];
        assert!(!should_prefer_episode_content(
            &[ranked],
            &episodes,
            &["coffee".to_string()]
        ));
    }

    #[test]
    fn prefers_a_relevant_episode_that_covers_more_query_terms_than_its_best_fact() {
        let mut ranked = make_ranked(
            make_fact("HTTP body diagnostics record buffered bytes"),
            RetrievalTier::Direct,
        );
        ranked.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranksig:test",
            "relevance_logit": 3.0,
            "admission_floor": 0.0,
        }));
        let mut episode = make_episode_item(
            "HTTP body measurements and diagnostics include buffered bytes and retained resources",
        );
        episode.source_episode = "episode:other".into();
        episode.provenance["reranking"] = serde_json::json!({
            "model_signature": "reranksig:test",
            "relevance_logit": 0.5,
            "admission_floor": 0.0,
        });

        assert!(should_prefer_episode_content(
            &[ranked],
            &[episode],
            &[
                "http".to_string(),
                "body".to_string(),
                "measurements".to_string(),
                "diagnostics".to_string(),
                "retained".to_string(),
                "resources".to_string(),
            ]
        ));
    }

    #[test]
    fn prefers_ranked_facts_over_raw_fallback_from_the_same_source_episode() {
        let mut ranked = make_ranked(
            make_fact("HTTP body diagnostics record buffered bytes"),
            RetrievalTier::Direct,
        );
        ranked.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranksig:test",
            "relevance_logit": 0.2,
            "admission_floor": 0.0,
        }));
        let mut episode =
            make_episode_item("HTTP body measurements and diagnostics include buffered bytes");
        episode.provenance["reranking"] = serde_json::json!({
            "model_signature": "reranksig:test",
            "relevance_logit": 1.0,
            "admission_floor": 0.0,
        });

        assert!(!should_prefer_episode_content(
            &[ranked],
            &[episode],
            &[
                "http".to_string(),
                "body".to_string(),
                "diagnostics".to_string()
            ]
        ));
    }

    #[test]
    fn prefers_false_when_episode_overlap_not_better() {
        let fact = make_fact("coffee brewing guide for beginners");
        let ranked = make_ranked(fact, RetrievalTier::Direct);
        let episodes = vec![make_episode_item("unrelated topic")];
        assert!(!should_prefer_episode_content(
            &[ranked],
            &episodes,
            &["coffee".to_string()]
        ));
    }

    #[test]
    fn prefers_true_when_episode_coverage_exceeds_fact() {
        let fact = make_fact("coffee");
        let ranked = make_ranked(fact, RetrievalTier::Direct);
        let mut episode = make_episode_item("coffee brewing techniques from ethiopia");
        episode.source_episode = "episode:other".into();
        let episodes = vec![episode];
        assert!(should_prefer_episode_content(
            &[ranked],
            &episodes,
            &[
                "coffee".to_string(),
                "brewing".to_string(),
                "ethiopia".to_string()
            ]
        ),);
    }

    #[test]
    fn prefers_false_when_fact_has_better_coverage() {
        let fact = make_fact("coffee brewing techniques from ethiopia");
        let ranked = make_ranked(fact, RetrievalTier::Direct);
        let episodes = vec![make_episode_item("coffee")];
        assert!(!should_prefer_episode_content(
            &[ranked],
            &episodes,
            &[
                "coffee".to_string(),
                "brewing".to_string(),
                "ethiopia".to_string()
            ]
        ),);
    }

    #[test]
    fn does_not_prefer_episode_when_it_would_drop_selected_fact_query_terms() {
        let query_terms = vec![
            "jon".to_string(),
            "gina".to_string(),
            "destress".to_string(),
        ];
        let mut fact = make_ranked(
            make_fact("Gina encourages Jon to take breaks and dance it out to destress."),
            RetrievalTier::Direct,
        );
        fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 2.2,
        }));
        let episodes = vec![AssembledContextItem {
            content: "Jon says dancing is his passion and escape; he wants to teach others.".into(),
            provenance: serde_json::json!({
                "reranking": {
                    "model_signature": "reranker-v1",
                    "relevance_logit": 8.7,
                }
            }),
            ..make_episode_item(
                "Jon says dancing is his passion and escape; he wants to teach others.",
            )
        }];

        assert!(!should_prefer_episode_content(
            &[fact],
            &episodes,
            &query_terms
        ));
    }

    #[test]
    fn same_model_episode_score_can_win_when_it_preserves_selected_fact_query_terms() {
        let query_terms = vec![
            "jon".to_string(),
            "dancing".to_string(),
            "passion".to_string(),
        ];
        let mut fact = make_ranked(
            make_fact("Jon's passion includes dancing every weekend with friends."),
            RetrievalTier::Direct,
        );
        fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 2.2,
        }));
        let mut episode = AssembledContextItem {
            content: "Jon describes dancing as a passion.".to_string(),
            provenance: serde_json::json!({
                "reranking": {
                    "model_signature": "reranker-v1",
                    "relevance_logit": 8.7,
                }
            }),
            ..make_episode_item("Jon describes dancing as a passion.")
        };
        episode.source_episode = "episode:other".into();

        assert!(should_prefer_episode_content(
            &[fact],
            &[episode],
            &query_terms
        ));
    }

    #[test]
    fn partial_episode_cannot_replace_collective_fact_evidence_even_with_higher_reranker_score() {
        let query_terms = vec!["jon".to_string(), "gina".to_string(), "status".to_string()];
        let mut jon_fact = make_ranked(
            make_fact("Jon owns the deployment status."),
            RetrievalTier::Direct,
        );
        jon_fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 8.0,
        }));
        let mut gina_fact = make_ranked(
            make_fact("Gina owns the billing status."),
            RetrievalTier::Direct,
        );
        gina_fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 7.0,
        }));
        let episode = AssembledContextItem {
            content: "Jon owns the deployment status.".to_string(),
            provenance: serde_json::json!({
                "reranking": {
                    "model_signature": "reranker-v1",
                    "relevance_logit": 9.0,
                }
            }),
            ..make_episode_item("Jon owns the deployment status.")
        };

        assert!(!should_prefer_episode_content(
            &[jon_fact, gina_fact],
            &[episode],
            &query_terms,
        ));
    }

    #[test]
    fn incomparable_reranker_scores_do_not_override_lexical_fallback() {
        let query_terms = vec!["coffee".to_string(), "brewing".to_string()];
        let mut fact = make_ranked(
            make_fact("coffee brewing guide for beginners"),
            RetrievalTier::Direct,
        );
        fact.evidence.reranking = Some(serde_json::json!({
            "model_signature": "reranker-v1",
            "relevance_logit": 2.2,
        }));
        let episodes = vec![AssembledContextItem {
            provenance: serde_json::json!({
                "reranking": {
                    "model_signature": "reranker-v2",
                    "relevance_logit": 8.7,
                }
            }),
            ..make_episode_item("coffee")
        }];

        assert!(!should_prefer_episode_content(
            &[fact],
            &episodes,
            &query_terms
        ));
    }

    #[test]
    fn prefers_false_with_empty_query_terms() {
        let fact = make_fact("coffee");
        let ranked = make_ranked(fact, RetrievalTier::Direct);
        let episodes = vec![make_episode_item("coffee")];
        assert!(!should_prefer_episode_content(&[ranked], &episodes, &[]));
    }

    // -- matched_query_terms_for_text --------------------------------------

    #[test]
    fn matched_terms_finds_overlap() {
        let terms = matched_query_terms_for_text(
            "coffee brewing guide",
            &[
                "coffee".to_string(),
                "brewing".to_string(),
                "missing".to_string(),
            ],
        );
        assert_eq!(terms.len(), 2);
        assert!(terms.contains("coffee"));
        assert!(terms.contains("brewing"));
    }

    #[test]
    fn matched_terms_empty_when_no_overlap() {
        let terms = matched_query_terms_for_text("hello world", &["coffee".to_string()]);
        assert!(terms.is_empty());
    }

    #[test]
    fn matched_terms_empty_query() {
        let terms = matched_query_terms_for_text("hello world", &[]);
        assert!(terms.is_empty());
    }

    // -----------------------------------------------------------------------
    // Tests relocated from context.rs — full scenario coverage for
    // should_prefer_episode_content using realistic query/fact/episode inputs.
    // -----------------------------------------------------------------------

    fn create_test_fact(fact_id: &str, t_valid: chrono::DateTime<chrono::Utc>) -> Fact {
        Fact {
            fact_id: fact_id.to_string(),
            fact_type: "note".to_string(),
            content: "Test content".to_string(),
            quote: "Test quote".to_string(),
            source_episode: "episode:123".to_string(),
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
        source_episode: &str,
        t_valid: chrono::DateTime<chrono::Utc>,
        fusion_score: f64,
        ft_score: f64,
        access_count: i64,
        index_keys: &[&str],
    ) -> RankedContextFact {
        let mut fact = create_test_fact(fact_id, t_valid);
        fact.source_episode = source_episode.to_string();
        fact.ft_score = ft_score;
        fact.access_count = access_count;
        fact.index_keys = index_keys.iter().map(|key| (*key).to_string()).collect();

        RankedContextFact {
            fact,
            evidence: crate::memory::retrieval::types::CandidateEvidence::default(),
            rationale: "test rationale".to_string(),
            retrieval_tier: RetrievalTier::Direct,
            fusion_score,
            source_priority: 0,
            decayed_confidence: 1.0,
            query_alignment_factor: 1.0,
            grounding_score: 1.0,
            semantic_available: false,
            matched_query_terms: Vec::new(),
            graph_trace: None,
        }
    }

    #[test]
    fn should_prefer_episode_content_when_episode_overlap_is_stronger() {
        let query_terms =
            crate::shared::search::search_query_terms("platform planning notes july 2025");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![RankedContextFact {
            fact: Fact {
                content: "July 2025 platform licensing notes for renewal workflow.".to_string(),
                ..create_ranked_test_fact(
                    "fact:noise",
                    "episode:noise",
                    fact_time,
                    1.0,
                    4.0,
                    0,
                    &[],
                )
                .fact
            },
            ..create_ranked_test_fact("fact:noise", "episode:noise", fact_time, 1.0, 4.0, 0, &[])
        }];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:july".to_string(),
            content: "Platform planning notes July 2025: release scope, integrations, and response workflow updates.".to_string(),
            quote: "Platform planning notes July 2025: release scope, integrations, and response workflow updates.".to_string(),
            source_episode: "episode:july".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn should_not_prefer_episode_content_when_fact_overlap_is_equal_or_better() {
        let query_terms =
            crate::shared::search::search_query_terms("platform planning notes july 2025");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![RankedContextFact {
            fact: Fact {
                content: "Platform planning notes July 2025 for release scope and integrations."
                    .to_string(),
                ..create_ranked_test_fact(
                    "fact:strong",
                    "episode:strong",
                    fact_time,
                    1.0,
                    5.0,
                    0,
                    &[],
                )
                .fact
            },
            ..create_ranked_test_fact("fact:strong", "episode:strong", fact_time, 1.0, 5.0, 0, &[])
        }];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:july".to_string(),
            content: "Platform notes July 2025 with rollout reminders.".to_string(),
            quote: "Platform notes July 2025 with rollout reminders.".to_string(),
            source_episode: "episode:july".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn should_not_prefer_episode_content_over_graph_expanded_matches() {
        let query_terms = crate::shared::search::search_query_terms("bob jones");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2025-07-13T10:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![RankedContextFact {
            fact: Fact {
                content: "Prototype milestone is blocked.".to_string(),
                ..create_ranked_test_fact(
                    "fact:graph",
                    "episode:graph",
                    fact_time,
                    1.0,
                    0.0,
                    0,
                    &[],
                )
                .fact
            },
            retrieval_tier: RetrievalTier::GraphExpanded,
            ..create_ranked_test_fact("fact:graph", "episode:graph", fact_time, 1.0, 0.0, 0, &[])
        }];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:july".to_string(),
            content: "Alice Smith met Bob Jones to plan next steps.".to_string(),
            quote: "Alice Smith met Bob Jones to plan next steps.".to_string(),
            source_episode: "episode:july".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn should_not_prefer_episode_content_when_fact_captures_best_matching_summary_line() {
        let query_terms =
            crate::shared::search::search_query_terms("help kickoff naming localization alignment");
        let fact_time = chrono::DateTime::parse_from_rfc3339("2026-04-13T09:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![RankedContextFact {
            fact: Fact {
                content: "Help kickoff is open; naming and localization details need alignment across products.".to_string(),
                ..create_ranked_test_fact(
                    "fact:docs",
                    "episode:docs",
                    fact_time,
                    1.0,
                    6.0,
                    0,
                    &[],
                )
                .fact
            },
            ..create_ranked_test_fact("fact:docs", "episode:docs", fact_time, 1.0, 6.0, 0, &[])
        }];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:docs".to_string(),
            content: "Documentation and localization facts for product materials:\n\n- Fact: Help kickoff is open; naming and localization details need alignment.\n- Fact: Docs team is asking for final terminology in both languages.".to_string(),
            quote: "Documentation and localization facts for product materials:\n\n- Fact: Help kickoff is open; naming and localization details need alignment.\n- Fact: Docs team is asking for final terminology in both languages.".to_string(),
            source_episode: "episode:docs".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }

    #[test]
    fn should_not_prefer_episode_content_when_selected_facts_collectively_cover_query() {
        let query_terms = crate::shared::search::search_query_terms(
            "suite alpha beta gamma shared platform q3 2026 roadmap rollout controls versioning graphical rules",
        );
        let fact_time = chrono::DateTime::parse_from_rfc3339("2026-04-13T09:00:00Z")
            .expect("fact timestamp")
            .with_timezone(&chrono::Utc);

        let selected_facts = vec![
            RankedContextFact {
                fact: Fact {
                    content: "Suite Alpha, Suite Beta, and Suite Gamma launch on the shared platform in Q3 2026.".to_string(),
                    ..create_ranked_test_fact(
                        "fact:launch",
                        "episode:launch-summary",
                        fact_time,
                        1.0,
                        6.0,
                        0,
                        &[],
                    )
                    .fact
                },
                ..create_ranked_test_fact(
                    "fact:launch",
                    "episode:launch-summary",
                    fact_time,
                    1.0,
                    6.0,
                    0,
                    &[],
                )
            },
            RankedContextFact {
                fact: Fact {
                    content: "Roadmap adds staged rollout controls and export automation in Q4 2026.".to_string(),
                    ..create_ranked_test_fact(
                        "fact:roadmap",
                        "episode:launch-summary",
                        fact_time,
                        0.9,
                        5.0,
                        0,
                        &[],
                    )
                    .fact
                },
                ..create_ranked_test_fact(
                    "fact:roadmap",
                    "episode:launch-summary",
                    fact_time,
                    0.9,
                    5.0,
                    0,
                    &[],
                )
            },
            RankedContextFact {
                fact: Fact {
                    content: "Following wave adds workflow versioning and graphical rules.".to_string(),
                    ..create_ranked_test_fact(
                        "fact:followup",
                        "episode:launch-summary",
                        fact_time,
                        0.8,
                        4.0,
                        0,
                        &[],
                    )
                    .fact
                },
                ..create_ranked_test_fact(
                    "fact:followup",
                    "episode:launch-summary",
                    fact_time,
                    0.8,
                    4.0,
                    0,
                    &[],
                )
            },
        ];

        let episode_items = vec![AssembledContextItem {
            fact_id: "episode_fallback:episode:launch-summary".to_string(),
            content: "Quarterly launch brief:\n- Suite Alpha, Suite Beta, and Suite Gamma launch on the shared platform in Q3 2026.\n- Technical preview is September 30, 2026, with general availability in late October 2026.\n- Roadmap adds staged rollout controls and export automation in Q4 2026.\n- Following wave adds workflow versioning and graphical rules.".to_string(),
            quote: "Quarterly launch brief:\n- Suite Alpha, Suite Beta, and Suite Gamma launch on the shared platform in Q3 2026.\n- Technical preview is September 30, 2026, with general availability in late October 2026.\n- Roadmap adds staged rollout controls and export automation in Q4 2026.\n- Following wave adds workflow versioning and graphical rules.".to_string(),
            source_episode: "episode:launch-summary".to_string(),
            confidence: 1.0,
            provenance: serde_json::json!({"episode_fallback": true}),
            rationale: "fallback".to_string(),
            retrieval_tier: Some("fallback".to_string()),
            ..Default::default()
        }];

        assert!(!should_prefer_episode_content(
            &selected_facts,
            &episode_items,
            &query_terms,
        ));
    }
}
