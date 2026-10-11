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
    let normalized_terms = crate::shared::search::search_query_terms(content);
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
    let alpha_terms = crate::shared::search::search_query_terms(trimmed)
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

    #[test]
    fn is_promise_statement_detects_promise_patterns() {
        assert!(is_promise_statement("i will finish this task"));
        assert!(is_promise_statement("i'll deliver the report tomorrow"));
        assert!(is_promise_statement("will complete the project"));
        assert!(is_promise_statement("going to implement the feature"));
        assert!(is_promise_statement("I will do this tomorrow"));
    }

    #[test]
    fn is_promise_statement_rejects_non_promise_patterns() {
        assert!(!is_promise_statement("this is just a note"));
        assert!(!is_promise_statement("meeting scheduled for tomorrow"));
        assert!(!is_promise_statement("review the document"));
        assert!(!is_promise_statement("the task is complete"));
    }

    #[test]
    fn is_promise_statement_detects_lowercase_variations() {
        assert!(is_promise_statement("i will finish this"));
        assert!(is_promise_statement("i'll deliver"));
        assert!(is_promise_statement("will complete the task"));
    }

    #[test]
    fn is_experience_statement_detects_preference_patterns() {
        assert!(is_experience_statement(
            "Alice Smith prefers weekly launch updates over ad-hoc pings."
        ));
        assert!(is_experience_statement("I enjoy quiet deep-work mornings."));
        assert!(is_experience_statement(
            "I tend to avoid high-rise buildings for accommodations."
        ));
        assert!(is_experience_statement(
            "I have a strong aversion to beachfront resorts."
        ));
        assert!(is_experience_statement(
            "I do not enjoy casinos or gaming environments."
        ));
    }

    #[test]
    fn is_experience_statement_rejects_non_preference_patterns() {
        assert!(!is_experience_statement("Atlas budget is $2M."));
        assert!(!is_experience_statement("I will send the deck tomorrow."));
    }

    #[test]
    fn is_document_action_item_detects_email_style_bullets() {
        assert!(is_document_action_item(
            "Subject: Atlas follow-up\n\nAction items:\n- Alice Smith: send revised deck by Friday\n- Bob Jones: review launch checklist by Monday"
        ));
    }

    #[test]
    fn is_document_action_item_rejects_plain_notes() {
        assert!(!is_document_action_item(
            "Meeting notes: Alice shared the deck."
        ));
        assert!(!is_document_action_item(
            "Action items: this section is empty for now"
        ));
    }

    #[test]
    fn is_summary_like_note_candidate_detects_dense_summary_content() {
        assert!(is_summary_like_note_candidate(
            "July 2025 planning summary: platform integrations ready, stakeholder approvals pending, response workflow scoped."
        ));
    }

    #[test]
    fn is_summary_like_note_candidate_rejects_short_content() {
        assert!(!is_summary_like_note_candidate("Short note only"));
    }
}
