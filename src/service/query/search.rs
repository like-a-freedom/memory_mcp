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

    s.split_whitespace()
        .flat_map(|token| token.split(|character: char| !character.is_alphanumeric()))
        .filter_map(normalize_search_term)
        .collect()
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
}
