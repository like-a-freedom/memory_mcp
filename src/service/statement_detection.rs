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
        r"\b(prefer|prefers|dislike|dislikes|enjoy|enjoys|love|loves|hate|hates|value|values)\b",
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
