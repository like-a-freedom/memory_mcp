//! Content statement type detection using regex patterns.

use std::sync::LazyLock;

use regex::Regex;

static PROMISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(i will|i'll|will\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule)|going to\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule))\b")
        .expect("promise regex is valid")
});

static METRIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(ARR|MRR|NRR|revenue|churn|ROI|LTV|CAC|NPS|EBITDA)\b|\$\d")
        .expect("metric regex is valid")
});

static EXPERIENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(prefer|prefers|dislike|dislikes|enjoy|enjoys|love|loves|hate|hates|value|values|avoid|avoids|aversion)\b|\b(do not enjoy|don't enjoy|do not like|don't like|not interested)\b",
    )
    .expect("experience regex is valid")
});

static ACTION_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(action items?|next steps|follow-?ups?|todo)\s*:")
        .expect("action-item header regex is valid")
});

static ACTION_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:[-*]|\d+\.)\s+[a-z]+(?:\s+[a-z]+){0,2}\s*(?::|-)\s*(?:send|review|share|update|prepare|schedule|confirm|draft|deliver|complete|close|fix|follow(?:\s+|-)?up)\b")
        .expect("action-item line regex is valid")
});

static EMAIL_HEADER_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^\s*(subject|from|to|cc|bcc|sent|date|reply-to)\s*:")
        .expect("email header regex is valid")
});

static EMAIL_ADDRESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b")
        .expect("email address regex is valid")
});

/// Check if content contains a promise statement.
#[must_use]
pub fn is_promise_statement(content: &str) -> bool {
    PROMISE_RE.is_match(content)
}

/// Detects metric-related content using word-boundary matching.
pub fn is_metric_statement(content: &str) -> bool {
    METRIC_RE.is_match(content)
}

/// Detects preference/profile statements that should be stored as experience facts.
#[must_use]
pub fn is_experience_statement(content: &str) -> bool {
    let normalized = content.to_lowercase();
    EXPERIENCE_RE.is_match(&normalized)
}

/// Detects document-style action items as promise-like commitments.
#[must_use]
pub fn is_document_action_item(content: &str) -> bool {
    let normalized = content.to_lowercase();
    ACTION_HEADER_RE.is_match(&normalized) && ACTION_LINE_RE.is_match(&normalized)
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
        .filter(|line| EMAIL_HEADER_LINE_RE.is_match(line))
        .count();
    let email_lines = lines
        .iter()
        .filter(|line| EMAIL_ADDRESS_RE.is_match(line))
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
