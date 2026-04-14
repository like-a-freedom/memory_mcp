use std::collections::HashSet;

use crate::models::{ExtractedEntity, FactType};
use crate::service::statement_detection::{
    is_document_action_item, is_experience_statement, is_metric_statement, is_promise_statement,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StructuredSummaryLabel {
    Decision,
    Fact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StructuredSummarySection {
    Labeled(StructuredSummaryLabel),
    Thematic(String),
}

impl StructuredSummaryLabel {
    pub(super) fn from_token(token: &str) -> Option<Self> {
        match crate::service::normalize_text(token).as_str() {
            "decision" | "decisions" => Some(Self::Decision),
            "fact" | "facts" => Some(Self::Fact),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StructuredSummaryFactCandidate {
    pub(super) fact_type: String,
    pub(super) content: String,
    pub(super) quote: String,
}

fn strip_list_marker(line: &str) -> (&str, bool) {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        return (rest.trim_start(), true);
    }

    let digit_count = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count > 0 {
        let rest = &trimmed[digit_count..];
        if let Some(rest) = rest.strip_prefix(". ") {
            return (rest.trim_start(), true);
        }
    }

    (trimmed, false)
}

fn strip_markdown_heading_marker(line: &str) -> (&str, bool) {
    let trimmed = line.trim_start();
    let heading_marker_len = trimmed.chars().take_while(|ch| *ch == '#').count();
    if heading_marker_len == 0 {
        return (trimmed, false);
    }

    let rest = trimmed[heading_marker_len..].trim_start();
    (rest, !rest.is_empty())
}

pub(super) fn strip_markdown_inline_formatting(value: &str) -> String {
    value
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_string()
}

fn structured_summary_heading_label(tokens: &[String]) -> Option<StructuredSummaryLabel> {
    if tokens.iter().any(|token| token == "decision") {
        return Some(StructuredSummaryLabel::Decision);
    }

    if tokens.iter().any(|token| token == "fact") {
        return Some(StructuredSummaryLabel::Fact);
    }

    let has_pending = tokens
        .iter()
        .any(|token| matches!(token.as_str(), "pending" | "open"));
    let has_item = tokens
        .iter()
        .any(|token| matches!(token.as_str(), "item" | "step" | "followup" | "todo"));
    let has_action = tokens
        .iter()
        .any(|token| matches!(token.as_str(), "action" | "todo" | "followup"));
    let has_next_steps =
        tokens.iter().any(|token| token == "next") && tokens.iter().any(|token| token == "step");

    if (has_pending && has_item) || has_action || has_next_steps {
        return Some(StructuredSummaryLabel::Fact);
    }

    None
}

fn structured_summary_section_heading(line: &str) -> Option<StructuredSummarySection> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (body, is_list_item) = strip_list_marker(trimmed);
    if is_list_item || split_structured_summary_label(body).is_some() {
        return None;
    }

    let (heading_body, has_markdown_heading) = strip_markdown_heading_marker(body);
    let has_trailing_colon = body.ends_with(':');
    let heading = heading_body
        .strip_suffix(':')
        .unwrap_or(heading_body)
        .trim();
    if heading.is_empty() {
        return None;
    }

    let heading = strip_markdown_inline_formatting(heading);
    let heading_terms = crate::service::query::search_query_terms(&heading);
    if heading_terms.is_empty() {
        return None;
    }

    if !has_markdown_heading && !has_trailing_colon && heading_terms.len() > 4 {
        return None;
    }

    if let Some(label) = structured_summary_heading_label(&heading_terms) {
        return Some(StructuredSummarySection::Labeled(label));
    }

    if has_markdown_heading || has_trailing_colon {
        return Some(StructuredSummarySection::Thematic(heading));
    }

    None
}

fn split_structured_summary_label(line: &str) -> Option<(StructuredSummaryLabel, &str)> {
    let (prefix, remainder) = line.split_once(':')?;
    let normalized_prefix = strip_markdown_inline_formatting(prefix);
    let label = StructuredSummaryLabel::from_token(&normalized_prefix)?;
    let remainder = remainder.trim();
    if remainder.is_empty() {
        return None;
    }
    Some((label, remainder))
}

fn contextualize_structured_summary_fact_content(
    section: &StructuredSummarySection,
    fact_content: &str,
) -> String {
    match section {
        StructuredSummarySection::Labeled(_) => fact_content.to_string(),
        StructuredSummarySection::Thematic(heading) => {
            let normalized_heading = crate::service::normalize_text(heading);
            let normalized_content = crate::service::normalize_text(fact_content);
            if !normalized_heading.is_empty() && normalized_content.starts_with(&normalized_heading)
            {
                fact_content.to_string()
            } else {
                format!("{heading}: {fact_content}")
            }
        }
    }
}

fn classify_structured_summary_fact_type(
    section: &StructuredSummarySection,
    content: &str,
) -> &'static str {
    match section {
        StructuredSummarySection::Labeled(StructuredSummaryLabel::Decision) => {
            FactType::Decision.as_str()
        }
        StructuredSummarySection::Labeled(StructuredSummaryLabel::Fact)
        | StructuredSummarySection::Thematic(_) => {
            let normalized = content.to_lowercase();
            if is_metric_statement(content) {
                FactType::Metric.as_str()
            } else if is_promise_statement(&normalized) || is_document_action_item(content) {
                FactType::Promise.as_str()
            } else if is_experience_statement(content) {
                FactType::Experience.as_str()
            } else {
                FactType::Note.as_str()
            }
        }
    }
}

pub(super) fn structured_summary_fact_candidates(
    content: &str,
) -> Vec<StructuredSummaryFactCandidate> {
    let mut section = None;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for raw_line in content.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(next_section) = structured_summary_section_heading(trimmed) {
            section = Some(next_section);
            continue;
        }

        let (body, is_list_item) = strip_list_marker(trimmed);
        let parsed = split_structured_summary_label(body)
            .map(|(label, fact_content)| {
                (
                    StructuredSummarySection::Labeled(label),
                    fact_content.trim(),
                )
            })
            .or_else(|| {
                section
                    .clone()
                    .filter(|_| is_list_item)
                    .map(|active_section| (active_section, body.trim()))
            });

        let Some((active_section, fact_content)) = parsed else {
            section = None;
            continue;
        };

        let fact_content = strip_markdown_inline_formatting(fact_content);

        if fact_content.is_empty() {
            continue;
        }

        let fact_type = classify_structured_summary_fact_type(&active_section, &fact_content);
        let content = contextualize_structured_summary_fact_content(&active_section, &fact_content);
        let dedupe_key = format!(
            "{}\u{001f}{}",
            fact_type,
            crate::service::normalize_text(&content)
        );
        if !seen.insert(dedupe_key) {
            continue;
        }

        candidates.push(StructuredSummaryFactCandidate {
            fact_type: fact_type.to_string(),
            content,
            quote: fact_content,
        });
    }

    candidates
}

pub(super) fn entity_links_for_fact_content(
    content: &str,
    entities: &[ExtractedEntity],
) -> Vec<String> {
    let normalized_content = crate::service::normalize_text(content);
    if normalized_content.is_empty() {
        return Vec::new();
    }

    let mut seen = HashSet::new();
    let mut entity_links = Vec::new();

    for entity in entities {
        let normalized_name = crate::service::normalize_text(&entity.canonical_name);
        if normalized_name.is_empty()
            || !normalized_content.contains(&normalized_name)
            || !seen.insert(entity.entity_id.clone())
        {
            continue;
        }
        entity_links.push(entity.entity_id.clone());
    }

    entity_links
}

pub(super) fn sanitized_content_for_entity_extraction(content: &str) -> String {
    let mut section = None;
    let mut sanitized_lines = Vec::new();

    for raw_line in content.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            sanitized_lines.push(String::new());
            continue;
        }

        if let Some(next_section) = structured_summary_section_heading(trimmed) {
            section = Some(next_section);
            continue;
        }

        let (body, is_list_item) = strip_list_marker(trimmed);
        if let Some((_, remainder)) = split_structured_summary_label(body) {
            sanitized_lines.push(strip_markdown_inline_formatting(remainder));
            continue;
        }

        if is_list_item && section.is_some() {
            sanitized_lines.push(strip_markdown_inline_formatting(body));
            continue;
        }

        section = None;
        sanitized_lines.push(strip_markdown_inline_formatting(trimmed));
    }

    sanitized_lines.join("\n")
}
