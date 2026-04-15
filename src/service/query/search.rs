use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

/// Normalize text by lowercasing and collapsing whitespace.
pub fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Preprocess a search query by stripping episode references, boolean operators,
/// quoted phrases, and collapsing whitespace.
pub fn preprocess_search_query(raw: &str) -> String {
    search_query_terms(raw).join(" ")
}

/// Extract normalized search terms from a natural-language query.
pub fn search_query_terms(raw: &str) -> Vec<String> {
    static EPISODE_REF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)episode:[a-z0-9_-]+").expect("episode_ref regex is valid")
    });
    static QUOTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""([^"]*)""#).expect("quoted regex is valid"));

    let s = EPISODE_REF.replace_all(raw, " ");
    let s = QUOTED.replace_all(&s, " $1 ");

    let mut terms = Vec::new();
    for token in s.split_whitespace() {
        terms.extend(
            token
                .split(|character: char| !character.is_alphanumeric())
                .filter_map(normalize_search_term),
        );
        terms.extend(search_reference_terms(token));
    }

    terms
}

/// Extract collapsed reference-like terms (for example `R-0712` -> `r0712`).
pub fn search_reference_terms(raw: &str) -> Vec<String> {
    raw.split_whitespace()
        .filter_map(normalize_reference_like_term)
        .collect()
}

/// Deduplicate query terms while preserving their original order.
pub fn unique_query_terms(query_terms: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut unique = Vec::with_capacity(query_terms.len());

    for term in query_terms {
        if seen.insert(term.clone()) {
            unique.push(term.clone());
        }
    }

    unique
}

/// Hard anchors are query terms whose shape already indicates high specificity.
///
/// This intentionally avoids any domain-specific allowlist: numeric ids and
/// mixed alphanumeric tokens (for example `300k`) behave like anchors
/// regardless of the product/domain vocabulary.
pub fn query_hard_anchor_terms(query_terms: &[String]) -> HashSet<String> {
    unique_query_terms(query_terms)
        .into_iter()
        .filter(|term| query_term_is_hard_anchor(term))
        .collect()
}

/// Returns the maximum document frequency a term may have and still qualify as
/// a soft anchor within a candidate pool.
pub fn soft_anchor_doc_freq_threshold(total_docs: usize) -> usize {
    total_docs.div_ceil(4).clamp(1, 3)
}

/// Soft anchors are dynamically inferred from the candidate pool.
///
/// Long rare terms (`openshift`) and one-off short rare terms (`jfr`, `nic`) can
/// both become anchors without relying on enumerated project names.
pub fn query_term_should_be_soft_anchor(term: &str, doc_freq: usize, total_docs: usize) -> bool {
    if doc_freq == 0 || doc_freq > soft_anchor_doc_freq_threshold(total_docs) {
        return false;
    }

    term.len() >= 4 || (term.len() >= 3 && doc_freq == 1)
}

/// Higher values mean the term is rarer within the current candidate pool.
pub fn query_term_rarity_weight(doc_freq: usize, total_docs: usize) -> f64 {
    (((total_docs.max(1) + 1) as f64) / ((doc_freq + 1) as f64)).ln_1p()
}

fn query_term_is_hard_anchor(term: &str) -> bool {
    let has_digit = term.chars().any(|character| character.is_ascii_digit());
    let has_alpha = term
        .chars()
        .any(|character| character.is_ascii_alphabetic());
    has_digit && (has_alpha || term.chars().all(|character| character.is_ascii_digit()))
}

fn normalize_reference_like_term(raw: &str) -> Option<String> {
    let token = raw.trim_matches(|character: char| !character.is_alphanumeric());
    if token.len() < 3 || looks_like_iso_date(token) {
        return None;
    }

    let has_separator = token.chars().any(|character| !character.is_alphanumeric());
    if !has_separator {
        return None;
    }

    let collapsed = token
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if collapsed.len() < 3 {
        return None;
    }

    let has_digit = collapsed
        .chars()
        .any(|character| character.is_ascii_digit());
    let has_alpha = collapsed
        .chars()
        .any(|character| character.is_ascii_alphabetic());

    if !has_digit {
        return None;
    }

    if has_alpha || collapsed.len() >= 6 {
        Some(collapsed)
    } else {
        None
    }
}

fn looks_like_iso_date(token: &str) -> bool {
    let segments = token
        .split(['-', '/'])
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    match segments.as_slice() {
        [year, month] | [year, month, ..]
            if year.len() == 4
                && year.chars().all(|character| character.is_ascii_digit())
                && month.len() == 2
                && month.chars().all(|character| character.is_ascii_digit()) =>
        {
            true
        }
        _ => false,
    }
}

fn normalize_search_term(raw: &str) -> Option<String> {
    let token = raw.trim().to_ascii_lowercase();
    if token.len() < 2 {
        return None;
    }

    if matches!(token.as_str(), "or" | "and" | "not") || is_search_stopword(&token) {
        return None;
    }

    Some(singularize_search_term(&token))
}

fn singularize_search_term(token: &str) -> String {
    if token.len() > 4 && token.ends_with("ies") {
        let stem = token.trim_end_matches("ies");
        return format!("{stem}y");
    }

    if token.len() > 4
        && token.ends_with('s')
        && !token.ends_with("ss")
        && !token.ends_with("us")
        && !token.ends_with("is")
        && !token.ends_with("as")
    {
        return token[..token.len() - 1].to_string();
    }

    token.to_string()
}

fn is_search_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "are"
            | "as"
            | "at"
            | "be"
            | "did"
            | "do"
            | "does"
            | "for"
            | "from"
            | "get"
            | "give"
            | "given"
            | "go"
            | "going"
            | "gone"
            | "had"
            | "has"
            | "have"
            | "how"
            | "in"
            | "into"
            | "is"
            | "it"
            | "its"
            | "likely"
            | "made"
            | "make"
            | "of"
            | "on"
            | "said"
            | "say"
            | "that"
            | "the"
            | "their"
            | "them"
            | "there"
            | "these"
            | "this"
            | "tell"
            | "to"
            | "told"
            | "took"
            | "take"
            | "was"
            | "were"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "with"
            | "would"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_text_lowercases_and_collapses_whitespace() {
        assert_eq!(normalize_text("  Hello   WORLD  "), "hello world");
        assert_eq!(normalize_text("Test"), "test");
        assert_eq!(normalize_text(""), "");
    }

    #[test]
    fn preprocess_search_query_strips_episode_references() {
        let result = preprocess_search_query("query episode:abc123 more");
        assert_eq!(result, "query more");
    }

    #[test]
    fn preprocess_search_query_strips_boolean_operators() {
        let result = preprocess_search_query("hello OR world AND test NOT foo");
        assert_eq!(result, "hello world test foo");
    }

    #[test]
    fn preprocess_search_query_handles_quoted_phrases() {
        let result = preprocess_search_query(r#"search "quoted phrase" terms"#);
        assert_eq!(result, "search quoted phrase term");
    }

    #[test]
    fn preprocess_search_query_drops_question_stopwords_and_short_words() {
        let result = preprocess_search_query("What did a user say in the group?");
        assert_eq!(result, "user group");
    }

    #[test]
    fn preprocess_search_query_case_insensitive_episode_ref() {
        let result = preprocess_search_query("test EPISODE:ABC123 query");
        assert_eq!(result, "test query");
    }

    #[test]
    fn search_query_terms_normalize_case_and_punctuation() {
        let result = search_query_terms("When did Caroline go to the LGBTQ support group?");

        assert_eq!(result, vec!["caroline", "lgbtq", "support", "group"]);
    }

    #[test]
    fn search_query_terms_normalize_simple_plural_forms() {
        let result = search_query_terms("blockers decisions updates stories");

        assert_eq!(result, vec!["blocker", "decision", "update", "story"]);
    }

    #[test]
    fn search_query_terms_preserve_non_plural_words_ending_with_s() {
        let result = search_query_terms("atlas analysis status access");

        assert_eq!(result, vec!["atlas", "analysis", "status", "access"]);
    }

    #[test]
    fn search_query_terms_preserve_reference_like_tokens() {
        let result = search_query_terms("Requirement R-0712 depends on work-item-9794206");

        assert!(result.contains(&"0712".to_string()));
        assert!(result.contains(&"r0712".to_string()));
        assert!(result.contains(&"9794206".to_string()));
        assert!(result.contains(&"workitem9794206".to_string()));
    }

    #[test]
    fn query_hard_anchor_terms_detect_numeric_and_mixed_alnum_tokens() {
        let anchors = query_hard_anchor_terms(&[
            "work".to_string(),
            "9794206".to_string(),
            "300k".to_string(),
            "openshift".to_string(),
        ]);

        assert!(anchors.contains("9794206"));
        assert!(anchors.contains("300k"));
        assert!(!anchors.contains("work"));
        assert!(!anchors.contains("openshift"));
    }

    #[test]
    fn query_term_should_be_soft_anchor_accepts_long_rare_terms_and_short_singletons() {
        assert!(query_term_should_be_soft_anchor("openshift", 1, 3));
        assert!(query_term_should_be_soft_anchor("jfr", 1, 3));
        assert!(!query_term_should_be_soft_anchor("rollout", 2, 3));
        assert!(!query_term_should_be_soft_anchor("id", 1, 3));
    }
}
