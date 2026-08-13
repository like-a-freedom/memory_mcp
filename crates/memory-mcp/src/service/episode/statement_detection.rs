//! Content statement type detection using regex patterns.

use std::sync::LazyLock;

use regex::Regex;

static PROMISE_RE: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"\b(i will|i'll|will\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule)|going to\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule))\b",
    )
});

static METRIC_RE: LazyLock<Result<Regex, regex::Error>> =
    LazyLock::new(|| Regex::new(r"\b(ARR|MRR|NRR|revenue|churn|ROI|LTV|CAC|NPS|EBITDA)\b|\$\d"));

static EXPERIENCE_RE: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"\b(prefer|prefers|dislike|dislikes|enjoy|enjoys|love|loves|hate|hates|value|values|avoid|avoids|aversion)\b|\b(do not enjoy|don't enjoy|do not like|don't like|not interested)\b",
    )
});

static ACTION_HEADER_RE: LazyLock<Result<Regex, regex::Error>> =
    LazyLock::new(|| Regex::new(r"(?im)^\s*(action items?|next steps|follow-?ups?|todo)\s*:"));

static ACTION_LINE_RE: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
    Regex::new(
        r"(?im)^\s*(?:[-*]|\d+\.)\s+(?:[a-z]+\s+[a-z]+\s*(?::|-)\s*)?(?:send|review|share|update|prepare|schedule|confirm|draft|deliver|complete|close|fix|follow(?:\s+|-)?up)\b",
    )
});

static EMAIL_HEADER_LINE_RE: LazyLock<Result<Regex, regex::Error>> =
    LazyLock::new(|| Regex::new(r"(?im)^\s*(subject|from|to|cc|bcc|sent|date|reply-to)\s*:"));

static EMAIL_ADDRESS_RE: LazyLock<Result<Regex, regex::Error>> =
    LazyLock::new(|| Regex::new(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b"));

fn regex_matches(regex: &Result<Regex, regex::Error>, content: &str) -> bool {
    regex.as_ref().is_ok_and(|regex| regex.is_match(content))
}

/// Check if content contains a promise statement.
#[must_use]
pub fn is_promise_statement(content: &str) -> bool {
    regex_matches(&PROMISE_RE, content)
}

/// Detects metric-related content using word-boundary matching.
pub fn is_metric_statement(content: &str) -> bool {
    regex_matches(&METRIC_RE, content)
        || ["ARR", "MRR", "NRR", "ROI", "LTV", "CAC", "NPS", "EBITDA"]
            .iter()
            .any(|marker| content.contains(marker))
}

/// Detects preference/profile statements that should be stored as experience facts.
#[must_use]
pub fn is_experience_statement(content: &str) -> bool {
    let normalized = content.to_lowercase();
    regex_matches(&EXPERIENCE_RE, &normalized)
}

/// Detects document-style action items as promise-like commitments.
#[must_use]
pub fn is_document_action_item(content: &str) -> bool {
    let normalized = content.to_lowercase();
    regex_matches(&ACTION_HEADER_RE, &normalized) && regex_matches(&ACTION_LINE_RE, &normalized)
}

/// Detects concise summary-like content that should remain searchable even when
/// it does not match any richer structured-fact heuristic.
#[must_use]
pub fn is_summary_like_note_candidate(content: &str) -> bool {
    let normalized_terms = crate::service::query::search_query_terms(content);
    normalized_terms.len() >= 6 && !is_low_value_summary_candidate(content)
}

/// Detects low-signal header/roster-style content that should not become a
/// standalone summary fact.
#[must_use]
pub fn is_low_value_summary_candidate(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return true;
    }

    let normalized = trimmed.to_lowercase();
    if is_metric_statement(content)
        || is_promise_statement(&normalized)
        || is_document_action_item(content)
        || is_experience_statement(content)
    {
        return false;
    }

    let lines = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return true;
    }

    let header_lines = lines
        .iter()
        .filter(|line| regex_matches(&EMAIL_HEADER_LINE_RE, line))
        .count();
    let email_lines = lines
        .iter()
        .filter(|line| regex_matches(&EMAIL_ADDRESS_RE, line))
        .count();
    let has_sentence_punctuation = trimmed.ends_with('.')
        || trimmed.contains(". ")
        || trimmed.ends_with('!')
        || trimmed.contains("! ")
        || trimmed.ends_with('?')
        || trimmed.contains("? ");
    let alpha_terms = crate::service::query::search_query_terms(trimmed)
        .into_iter()
        .filter(|term| {
            term.chars()
                .any(|character| character.is_ascii_alphabetic())
        })
        .count();

    let header_dominated = header_lines > 0 && header_lines * 2 >= lines.len();
    let roster_dominated = email_lines > 0 && !has_sentence_punctuation && alpha_terms < 6;

    header_dominated || roster_dominated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_detection_handles_acronym_adjacent_to_cjk_text() {
        assert!(is_metric_statement("张三报告ARR为500万美元。"));
    }
}
