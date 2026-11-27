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

fn split_query_clauses(query: &str) -> Vec<(&str, Option<char>)> {
    let mut clauses = Vec::new();
    let mut start = 0;
    let characters = query.char_indices().collect::<Vec<_>>();

    for (index, (offset, character)) in characters.iter().enumerate() {
        let next_is_boundary = characters
            .get(index + 1)
            .is_none_or(|(_, next)| next.is_whitespace());
        let boundary = matches!(character, '?' | '!' | ';' | '\n' | '\r')
            || (*character == '.' && next_is_boundary && !is_abbreviation_period(query, *offset));
        if boundary {
            let clause = query[start..*offset].trim();
            if !clause.is_empty() {
                clauses.push((clause, Some(*character)));
            }
            start = offset + character.len_utf8();
        }
    }

    let clause = query[start..].trim();
    if !clause.is_empty() {
        clauses.push((clause, None));
    }
    clauses
}

fn is_abbreviation_period(query: &str, period_offset: usize) -> bool {
    let token_start = query[..period_offset]
        .rfind(char::is_whitespace)
        .map_or(0, |index| index + 1);
    let token = &query[token_start..period_offset];
    let segments = token.split('.').filter(|segment| !segment.is_empty());
    let segments = segments.collect::<Vec<_>>();
    if segments.len() > 1
        && segments.iter().all(|segment| {
            segment
                .chars()
                .filter(|character| character.is_alphabetic())
                .count()
                == 1
        })
    {
        return true;
    }

    matches!(
        token.to_ascii_lowercase().as_str(),
        "dr" | "mr" | "mrs" | "ms" | "prof" | "sr" | "jr" | "st" | "vs" | "etc" | "e.g" | "i.e"
    )
}

pub(super) fn query_clauses(query: &str) -> Vec<&str> {
    split_query_clauses(query)
        .into_iter()
        .map(|(clause, _)| clause)
        .collect()
}

/// Remove only recognized presentation directives. Ordinary context and unknown
/// qualifiers remain evidence requirements, even after a question mark.
pub(super) fn evidence_query_surface(query: &str) -> std::borrow::Cow<'_, str> {
    let clauses = split_query_clauses(query);
    let presentation = clauses
        .iter()
        .map(|(text, _)| is_presentation_directive(text))
        .collect::<Vec<_>>();
    if !presentation.contains(&true) || !presentation.contains(&false) {
        return std::borrow::Cow::Borrowed(query);
    }
    let mut result = String::with_capacity(query.len());
    for ((clause, terminator), presentation) in clauses.into_iter().zip(presentation) {
        if presentation {
            continue;
        }
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(clause);
        if let Some(terminator) = terminator {
            result.push(terminator);
        }
    }
    std::borrow::Cow::Owned(result)
}

fn is_presentation_directive(clause: &str) -> bool {
    let mut normalized = crate::shared::search::normalize_text(clause);
    if ![
        "your final answer should ",
        "the final answer should ",
        "return the result as ",
        "mark your final answer ",
        "format the answer ",
        "format your answer ",
        "answer in ",
        "respond in ",
        "reply in ",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
    {
        return false;
    }
    // Examples describe the requested rendering. An unclosed/nested example
    // remains unknown and therefore cannot make a clause disappear.
    for marker in ["(e.g.", "(for example"] {
        while let Some(start) = normalized.find(marker) {
            let Some(end) = normalized[start..].find(')') else {
                return false;
            };
            if normalized[start + marker.len()..start + end].contains('(') {
                return false;
            }
            normalized.replace_range(start..=start + end, " ");
        }
    }
    let tokens = normalized
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let rendering = [
        "english", "russian", "french", "german", "spanish", "italian", "json", "xml", "yaml",
        "markdown", "csv", "boxed",
    ];
    tokens.iter().any(|token| rendering.contains(token))
        && tokens.iter().all(|token| {
            rendering.contains(token)
                || matches!(
                    *token,
                    "your"
                        | "the"
                        | "final"
                        | "answer"
                        | "should"
                        | "be"
                        | "return"
                        | "result"
                        | "as"
                        | "mark"
                        | "of"
                        | "phrases"
                        | "format"
                        | "in"
                        | "respond"
                        | "reply"
                        | "a"
                        | "an"
                        | "and"
                        | "or"
                        | "with"
                        | "without"
                        | "only"
                        | "no"
                        | "wrapped"
                        | "wrap"
                        | "comma"
                        | "separated"
                        | "list"
                        | "string"
                        | "object"
                        | "array"
                        | "plain"
                        | "text"
                        | "short"
                        | "brief"
                        | "concise"
                        | "explanation"
                        | "code"
                        | "block"
                        // Spelled-out examples are rendering instructions only
                        // inside the closed presentation grammar above. Any
                        // substantive qualifier still keeps the entire clause.
                        | "zero"
                        | "one"
                        | "two"
                        | "three"
                        | "four"
                        | "five"
                        | "six"
                        | "seven"
                        | "eight"
                        | "nine"
                        | "ten"
                )
        })
}

/// Keeps interrogative/request clauses for lexical candidate search while the
/// original full query remains available for admission and temporal rules.
pub(super) fn lexical_search_surface(query: &str) -> String {
    const QUERY_FRAME_STARTERS: &[&str] = &[
        "what",
        "which",
        "who",
        "whom",
        "whose",
        "where",
        "when",
        "why",
        "how",
        "tell",
        "show",
        "find",
        "list",
        "describe",
        "explain",
        "summarize",
        "recall",
        "remember",
    ];
    const AUXILIARY_QUESTION_STARTERS: &[&str] = &[
        "did", "does", "do", "is", "are", "was", "were", "has", "have", "had", "can", "could",
        "will", "would", "should",
    ];
    let question_clauses = split_query_clauses(query)
        .into_iter()
        .filter(|(clause, terminator)| {
            let first = clause
                .split(|character: char| !character.is_alphabetic())
                .find(|token| !token.is_empty());
            let framed_question = *terminator == Some('?')
                && clause.split([',', ':']).skip(1).any(|part| {
                    part.split_whitespace().next().is_some_and(|first| {
                        QUERY_FRAME_STARTERS
                            .iter()
                            .chain(AUXILIARY_QUESTION_STARTERS)
                            .any(|starter| first.eq_ignore_ascii_case(starter))
                    })
                });
            framed_question
                || first.is_some_and(|first| {
                    QUERY_FRAME_STARTERS
                        .iter()
                        .any(|starter| first.eq_ignore_ascii_case(starter))
                        || (*terminator == Some('?')
                            && AUXILIARY_QUESTION_STARTERS
                                .iter()
                                .any(|starter| first.eq_ignore_ascii_case(starter)))
                })
        })
        .map(|(clause, _)| clause)
        .collect::<Vec<_>>();

    if question_clauses.is_empty() {
        query.trim().to_owned()
    } else {
        question_clauses.join(" ")
    }
}

pub(super) fn detect_query_flags(
    raw_query_opt: Option<&str>,
    query_terms: &[String],
    cutoff: DateTime<Utc>,
) -> QueryFlags {
    let normalized = raw_query_opt
        .map(crate::shared::search::normalize_text)
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

// UI labels are identities, rather than interchangeable topical words. This
// preference is only for quoted labels in a control question; other questions
// retain their existing relevance ordering. It never grants admission or changes
// temporal/access rules, and independent reranker evidence keeps precedence.
pub(super) fn control_query_labels(query: &str) -> Vec<String> {
    let quote_parts = query.split('"').collect::<Vec<_>>();
    let outside_quotes = quote_parts
        .iter()
        .step_by(2)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    // A quoted excluded label is not positive membership evidence. Keep
    // ambiguous or negated questions on the ordinary relevance path.
    let outside_words = outside_quotes.to_lowercase();
    if outside_words
        .split(|ch: char| !ch.is_alphanumeric() && ch != '\'' && ch != '’')
        .any(|word| {
            matches!(
                word,
                "no" | "not"
                    | "never"
                    | "without"
                    | "except"
                    | "excluding"
                    | "exclude"
                    | "excludes"
                    | "excluded"
            ) || word.ends_with("n't")
                || word.ends_with("n’t")
        })
    {
        return Vec::new();
    }
    let terms = crate::shared::search::search_query_terms(&outside_quotes);
    if !terms
        .iter()
        .any(|term| matches!(term.as_str(), "toolbar" | "dropdown" | "button"))
    {
        return Vec::new();
    }
    let asks_membership = terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "include"
                | "including"
                | "contains"
                | "contain"
                | "named"
                | "labelled"
                | "labeled"
                | "called"
        )
    });
    // Bind the relation to the control noun before the first quoted anchor.
    let asks_between_controls = if let [before, _, connector, _, _] = quote_parts.as_slice()
        && connector.trim().eq_ignore_ascii_case("and")
    {
        let mut words = before.split_whitespace().rev();
        let between = words
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("between"));
        let role = match words.next() {
            Some(word)
                if word.eq_ignore_ascii_case("is") || word.eq_ignore_ascii_case("appears") =>
            {
                words.next()
            }
            role => role,
        };
        between
            && role.is_some_and(|role| {
                ["action", "button", "control", "option", "tab"]
                    .iter()
                    .any(|candidate| role.eq_ignore_ascii_case(candidate))
            })
    } else {
        false
    };
    if !asks_membership && !asks_between_controls {
        return Vec::new();
    }
    let mut labels = Vec::new();
    let mut rest = query;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('"') else {
            return Vec::new();
        };
        let label = crate::shared::search::normalize_text(&rest[..end]);
        if !label.is_empty() && !labels.contains(&label) {
            labels.push(label);
        }
        rest = &rest[end + 1..];
    }
    if asks_membership || labels.len() == 2 {
        labels
    } else {
        Vec::new()
    }
}

pub(super) fn control_label_coverage(content: &str, labels: &[String]) -> f64 {
    let content = crate::shared::search::normalize_text(content);
    let hits = labels
        .iter()
        .filter(|label| {
            content.match_indices(label.as_str()).any(|(start, found)| {
                let end = start + found.len();
                !content[..start]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric)
                    && !content[end..]
                        .chars()
                        .next()
                        .is_some_and(char::is_alphanumeric)
            })
        })
        .count();
    hits as f64 / labels.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn lexical_search_surface_excludes_non_question_followup_sentence() {
        assert_eq!(
            lexical_search_surface(
                "Which clearance measurements are recorded for Slate Basin Spillway 58? No source identifies an explicit correction."
            ),
            "Which clearance measurements are recorded for Slate Basin Spillway 58"
        );
    }

    #[test]
    fn lexical_search_surface_preserves_multiple_question_clauses() {
        assert_eq!(
            lexical_search_surface("What did Redwood Lock record? Which source reported it?"),
            "What did Redwood Lock record Which source reported it"
        );
    }

    #[test]
    fn lexical_search_surface_keeps_unframed_search_phrases_intact() {
        assert_eq!(
            lexical_search_surface("Slate Basin Spillway 58"),
            "Slate Basin Spillway 58"
        );
    }

    #[test]
    fn lexical_search_surface_keeps_auxiliary_questions_and_drops_negative_directives() {
        assert_eq!(
            lexical_search_surface("Can you find Redwood Lock? Do not use archived records."),
            "Can you find Redwood Lock"
        );
    }

    #[test]
    fn lexical_search_surface_keeps_initialisms_inside_question_clauses() {
        assert_eq!(
            lexical_search_surface("What did J.R.R. Tolkien write? No citation was supplied."),
            "What did J.R.R. Tolkien write"
        );
    }

    #[test]
    fn detect_query_flags_marks_timeline_and_path_queries() {
        let cutoff = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();

        let timeline_terms = crate::shared::search::search_query_terms(
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
            crate::shared::search::search_query_terms("who can introduce me to OpenAI");
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
            crate::shared::search::search_query_terms("timeline of Atlas launch changes");

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
