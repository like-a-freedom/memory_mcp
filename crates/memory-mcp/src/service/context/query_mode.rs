use chrono::{DateTime, Utc};

use super::ranking;
use super::temporal::infer_temporal_window;

const TIMELINE_HINT_TERMS: &[&str] = &[
    "timeline",
    "history",
    "chronology",
    "changed",
    "changes",
    "progress",
    "sequence",
    "when",
];
const STRONG_TIMELINE_HINT_TERMS: &[&str] = &["timeline", "history", "chronology", "sequence"];
const GRAPH_PATH_HINT_TERMS: &[&str] = &[
    "introduce",
    "intro",
    "connection",
    "connections",
    "connected",
    "path",
    "know",
    "knows",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct QueryFlags {
    pub(super) wants_timeline: bool,
    pub(super) wants_graph_path: bool,
    pub(super) wants_graph_context: bool,
    pub(super) is_first_person_memory: bool,
}

impl QueryFlags {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn max_graph_hops(&self) -> usize {
        if self.wants_graph_path { 2 } else { 1 }
    }

    pub(super) fn as_labels(&self) -> Vec<String> {
        let mut labels = Vec::new();

        if self.wants_timeline {
            labels.push("timeline".to_string());
        }
        if self.wants_graph_path {
            labels.push("graph_path".to_string());
        }
        if self.wants_graph_context {
            labels.push("graph_context".to_string());
        }
        if self.is_first_person_memory {
            labels.push("first_person_memory".to_string());
        }

        labels
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolvedViewMode {
    Standard,
    Timeline,
    Facets,
    WakeUp,
    Map,
}

impl ResolvedViewMode {
    pub(super) fn as_option_str(self) -> Option<&'static str> {
        match self {
            Self::Standard => None,
            Self::Timeline => Some("timeline"),
            Self::Facets => Some("facets"),
            Self::WakeUp => Some("wake_up"),
            Self::Map => Some("map"),
        }
    }
}

pub(super) fn query_phrase_candidates(query: &str) -> Vec<String> {
    let terms = query.split_whitespace().collect::<Vec<_>>();
    let mut phrases = Vec::new();

    for span_len in (1..=terms.len()).rev() {
        for start in 0..=terms.len().saturating_sub(span_len) {
            let phrase = terms[start..start + span_len].join(" ");
            if phrase.trim().len() >= 2 {
                phrases.push(phrase);
            }
        }
    }

    phrases.sort();
    phrases.dedup();
    phrases
}

pub(super) fn detect_query_flags(
    raw_query_opt: Option<&str>,
    query_terms: &[String],
    cutoff: DateTime<Utc>,
) -> QueryFlags {
    let normalized = raw_query_opt
        .map(crate::service::normalize_text)
        .unwrap_or_default();
    let has_temporal_focus =
        raw_query_opt.is_some_and(|query| infer_temporal_window(query, cutoff).is_some());
    let has_timeline_hint = query_terms
        .iter()
        .any(|term| TIMELINE_HINT_TERMS.contains(&term.as_str()));
    let has_strong_timeline_hint = query_terms
        .iter()
        .any(|term| STRONG_TIMELINE_HINT_TERMS.contains(&term.as_str()));
    let wants_timeline = has_strong_timeline_hint || (has_temporal_focus && has_timeline_hint);
    let wants_graph_path = query_terms
        .iter()
        .any(|term| GRAPH_PATH_HINT_TERMS.contains(&term.as_str()));

    QueryFlags {
        wants_timeline,
        wants_graph_path,
        wants_graph_context: !normalized.is_empty(),
        is_first_person_memory: raw_query_opt.is_some_and(ranking::query_is_first_person_memory),
    }
}

pub(super) fn resolve_view_mode(
    explicit_view_mode: Option<&str>,
    raw_query_opt: Option<&str>,
    query_terms: &[String],
    cutoff: DateTime<Utc>,
) -> (ResolvedViewMode, QueryFlags) {
    let flags = detect_query_flags(raw_query_opt, query_terms, cutoff);

    let resolved = match explicit_view_mode.map(str::trim) {
        Some("timeline") => ResolvedViewMode::Timeline,
        Some("facets") => ResolvedViewMode::Facets,
        Some("wake_up") => ResolvedViewMode::WakeUp,
        Some("map") => ResolvedViewMode::Map,
        _ if flags.wants_timeline => ResolvedViewMode::Timeline,
        _ => ResolvedViewMode::Standard,
    };

    (resolved, flags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn detect_query_flags_marks_timeline_and_path_queries() {
        let cutoff = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();

        let timeline_terms = crate::service::query::search_query_terms(
            "timeline of Atlas launch changes in March 2026",
        );
        let timeline_flags = detect_query_flags(
            Some("timeline of Atlas launch changes in March 2026"),
            &timeline_terms,
            cutoff,
        );
        assert!(timeline_flags.wants_timeline);
        assert!(!timeline_flags.wants_graph_path);
        assert!(timeline_flags.wants_graph_context);
        assert_eq!(
            timeline_flags.as_labels(),
            vec!["timeline".to_string(), "graph_context".to_string()]
        );

        let path_terms =
            crate::service::query::search_query_terms("who can introduce me to OpenAI");
        let path_flags =
            detect_query_flags(Some("who can introduce me to OpenAI"), &path_terms, cutoff);
        assert!(!path_flags.wants_timeline);
        assert!(path_flags.wants_graph_path);
        assert_eq!(path_flags.max_graph_hops(), 2);
    }

    #[test]
    fn resolve_view_mode_prefers_explicit_value_over_auto_detection() {
        let cutoff = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();
        let query_terms =
            crate::service::query::search_query_terms("timeline of Atlas launch changes");

        let (view_mode, flags) = resolve_view_mode(
            Some("map"),
            Some("timeline of Atlas launch changes"),
            &query_terms,
            cutoff,
        );

        assert_eq!(view_mode, ResolvedViewMode::Map);
        assert!(flags.wants_timeline);
    }
}
