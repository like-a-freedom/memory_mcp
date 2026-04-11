//! Temporal fact retrieval and query expansion.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, NaiveDate, Utc, Weekday};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemporalQueryExpansion {
    pub(crate) temporal_groups: Vec<Vec<String>>,
    pub(crate) residual_query: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TemporalWindow {
    pub(crate) start: DateTime<Utc>,
    pub(crate) end: DateTime<Utc>,
}

pub(crate) fn expand_temporal_synonyms(
    query: &str,
    cutoff: DateTime<Utc>,
) -> Option<TemporalQueryExpansion> {
    let tokens = query
        .split_whitespace()
        .map(normalize_temporal_token)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let mut temporal_groups = Vec::new();
    let mut consumed = vec![false; tokens.len()];
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].as_str();

        if let Some(month) = month_number(token) {
            if let Some(year_token) = tokens
                .get(index + 1)
                .filter(|next| is_four_digit_year(next))
                && let Ok(year) = year_token.parse::<i32>()
            {
                push_temporal_group(
                    &mut temporal_groups,
                    vec![format!("{token} {year}"), format!("{year}-{month:02}")],
                );
                consumed[index] = true;
                consumed[index + 1] = true;
                index += 2;
                continue;
            }

            push_temporal_group(&mut temporal_groups, vec![token.to_string()]);
            consumed[index] = true;
            index += 1;
            continue;
        }

        if is_weekday_name(token) {
            push_temporal_group(&mut temporal_groups, weekday_group(cutoff, token));
            consumed[index] = true;
            index += 1;
            continue;
        }

        if let Some(quarter) = parse_quarter_token(token) {
            push_temporal_group(&mut temporal_groups, quarter_group(cutoff, quarter));
            consumed[index] = true;
            index += 1;
            continue;
        }

        if token == "quarter"
            && let Some(next) = tokens.get(index + 1)
            && let Some(quarter) = parse_quarter_token(next)
        {
            push_temporal_group(&mut temporal_groups, quarter_group(cutoff, quarter));
            consumed[index] = true;
            consumed[index + 1] = true;
            index += 2;
            continue;
        }

        if token == "last" && tokens.get(index + 1).is_some_and(|next| next == "quarter") {
            push_temporal_group(&mut temporal_groups, previous_quarter_group(cutoff));
            consumed[index] = true;
            consumed[index + 1] = true;
            index += 2;
            continue;
        }

        if token == "this" && tokens.get(index + 1).is_some_and(|next| next == "week") {
            push_temporal_group(&mut temporal_groups, current_week_group(cutoff));
            consumed[index] = true;
            consumed[index + 1] = true;
            index += 2;
            continue;
        }

        if let Some(relative_shift_days) = relative_day_shift(token) {
            let target_date = cutoff.date_naive() + chrono::Duration::days(relative_shift_days);
            push_temporal_group(&mut temporal_groups, day_group_queries(target_date));
            consumed[index] = true;
            index += 1;
            continue;
        }

        if let Some(date) = parse_iso_date(token) {
            push_temporal_group(&mut temporal_groups, day_group_queries(date));
            consumed[index] = true;
            index += 1;
            continue;
        }

        index += 1;
    }

    if temporal_groups.is_empty() {
        return None;
    }

    let residual_terms = tokens
        .into_iter()
        .enumerate()
        .filter_map(|(idx, token)| (!consumed[idx]).then_some(token))
        .collect::<Vec<_>>();

    Some(TemporalQueryExpansion {
        temporal_groups,
        residual_query: (!residual_terms.is_empty()).then(|| residual_terms.join(" ")),
    })
}

fn normalize_temporal_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .to_ascii_lowercase()
}

fn month_number(token: &str) -> Option<u32> {
    match token {
        "january" => Some(1),
        "february" => Some(2),
        "march" => Some(3),
        "april" => Some(4),
        "may" => Some(5),
        "june" => Some(6),
        "july" => Some(7),
        "august" => Some(8),
        "september" => Some(9),
        "october" => Some(10),
        "november" => Some(11),
        "december" => Some(12),
        _ => None,
    }
}

fn is_four_digit_year(token: &str) -> bool {
    token.len() == 4 && token.chars().all(|ch| ch.is_ascii_digit())
}

fn is_weekday_name(token: &str) -> bool {
    matches!(
        token,
        "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday"
    )
}

fn parse_quarter_token(token: &str) -> Option<u32> {
    match token {
        "q1" | "1" | "first" => Some(1),
        "q2" | "2" | "second" => Some(2),
        "q3" | "3" | "third" => Some(3),
        "q4" | "4" | "fourth" => Some(4),
        _ => None,
    }
}

fn previous_quarter_group(cutoff: DateTime<Utc>) -> Vec<String> {
    let current_quarter = ((cutoff.month() - 1) / 3) + 1;
    let (year, quarter) = if current_quarter == 1 {
        (cutoff.year() - 1, 4)
    } else {
        (cutoff.year(), current_quarter - 1)
    };
    quarter_group_for_year(year, quarter)
}

fn quarter_group(cutoff: DateTime<Utc>, quarter: u32) -> Vec<String> {
    quarter_group_for_year(cutoff.year(), quarter)
}

fn quarter_group_for_year(year: i32, quarter: u32) -> Vec<String> {
    let mut group = vec![format!("q{quarter}")];
    for month in ((quarter - 1) * 3 + 1)..=((quarter - 1) * 3 + 3) {
        let month_name = month_name(month);
        group.push(format!("{month_name} {year}"));
        group.push(format!("{year}-{month:02}"));
    }
    group
}

fn current_week_group(cutoff: DateTime<Utc>) -> Vec<String> {
    let sow = start_of_week(cutoff.date_naive());
    let mut group = Vec::new();
    for offset in 0..7 {
        group.extend(day_group_queries(sow + chrono::Duration::days(offset)));
    }
    group
}

fn weekday_group(cutoff: DateTime<Utc>, token: &str) -> Vec<String> {
    let Some(target_weekday) = weekday_from_name(token) else {
        return vec![token.to_string()];
    };
    let sow = start_of_week(cutoff.date_naive());
    let target_date = sow + chrono::Duration::days(target_weekday.num_days_from_monday() as i64);
    day_group_queries(target_date)
}

fn start_of_week(date: NaiveDate) -> NaiveDate {
    date - chrono::Duration::days(date.weekday().num_days_from_monday() as i64)
}

fn weekday_from_name(token: &str) -> Option<Weekday> {
    match token {
        "monday" => Some(Weekday::Mon),
        "tuesday" => Some(Weekday::Tue),
        "wednesday" => Some(Weekday::Wed),
        "thursday" => Some(Weekday::Thu),
        "friday" => Some(Weekday::Fri),
        "saturday" => Some(Weekday::Sat),
        "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}

fn relative_day_shift(token: &str) -> Option<i64> {
    match token {
        "yesterday" => Some(-1),
        "today" => Some(0),
        "tomorrow" => Some(1),
        _ => None,
    }
}

fn parse_iso_date(token: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(token, "%Y-%m-%d").ok()
}

fn day_group_queries(date: NaiveDate) -> Vec<String> {
    vec![
        date.format("%Y-%m-%d").to_string(),
        format!("{} {}", month_name(date.month()), date.year()),
        format!("{}-{:02}", date.year(), date.month()),
        weekday_name(date.weekday()).to_string(),
    ]
}

fn month_name(month: u32) -> &'static str {
    match month {
        1 => "january",
        2 => "february",
        3 => "march",
        4 => "april",
        5 => "may",
        6 => "june",
        7 => "july",
        8 => "august",
        9 => "september",
        10 => "october",
        11 => "november",
        12 => "december",
        _ => "",
    }
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

fn push_temporal_group(groups: &mut Vec<Vec<String>>, queries: Vec<String>) {
    let mut seen = HashSet::new();
    let group = queries
        .into_iter()
        .map(|query| query.trim().to_ascii_lowercase())
        .filter(|query| !query.is_empty())
        .filter(|query| seen.insert(query.clone()))
        .collect::<Vec<_>>();
    if !group.is_empty() {
        groups.push(group);
    }
}

pub(crate) fn infer_temporal_window(query: &str, cutoff: DateTime<Utc>) -> Option<TemporalWindow> {
    let tokens = query
        .split_whitespace()
        .map(normalize_temporal_token)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let explicit_years = tokens
        .iter()
        .filter(|token| is_four_digit_year(token))
        .filter_map(|token| token.parse::<i32>().ok())
        .collect::<HashSet<_>>();
    let shared_year = (explicit_years.len() == 1)
        .then(|| *explicit_years.iter().next().expect("shared year exists"));

    let mut ranges = Vec::<(NaiveDate, NaiveDate)>::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index].as_str();

        if let Some(date) = parse_iso_date(token) {
            ranges.push((date, date));
            index += 1;
            continue;
        }

        if let Some(month) = month_number(token) {
            let next_year = tokens
                .get(index + 1)
                .filter(|next| is_four_digit_year(next))
                .and_then(|next| next.parse::<i32>().ok());
            let year = next_year.or(shared_year).unwrap_or(cutoff.year());
            ranges.push(month_date_range(year, month));
            index += usize::from(next_year.is_some()) + 1;
            continue;
        }

        if let Some(quarter) = parse_quarter_token(token) {
            let next_year = tokens
                .get(index + 1)
                .filter(|next| is_four_digit_year(next))
                .and_then(|next| next.parse::<i32>().ok());
            let year = next_year.or(shared_year).unwrap_or(cutoff.year());
            ranges.push(quarter_date_range(year, quarter));
            index += usize::from(next_year.is_some()) + 1;
            continue;
        }

        if token == "quarter"
            && let Some(next) = tokens.get(index + 1)
            && let Some(quarter) = parse_quarter_token(next)
        {
            let next_year = tokens
                .get(index + 2)
                .filter(|year| is_four_digit_year(year))
                .and_then(|year| year.parse::<i32>().ok());
            let year = next_year.or(shared_year).unwrap_or(cutoff.year());
            ranges.push(quarter_date_range(year, quarter));
            index += if next_year.is_some() { 3 } else { 2 };
            continue;
        }

        if token == "last" && tokens.get(index + 1).is_some_and(|next| next == "quarter") {
            ranges.push(previous_quarter_date_range(cutoff));
            index += 2;
            continue;
        }

        if token == "this" && tokens.get(index + 1).is_some_and(|next| next == "week") {
            let start = start_of_week(cutoff.date_naive());
            let end = start + chrono::Duration::days(6);
            ranges.push((start, end));
            index += 2;
            continue;
        }

        if is_weekday_name(token) {
            let start = start_of_week(cutoff.date_naive());
            if let Some(target_weekday) = weekday_from_name(token) {
                let date =
                    start + chrono::Duration::days(target_weekday.num_days_from_monday() as i64);
                ranges.push((date, date));
                index += 1;
                continue;
            }
        }

        if let Some(relative_shift_days) = relative_day_shift(token) {
            let target_date = cutoff.date_naive() + chrono::Duration::days(relative_shift_days);
            ranges.push((target_date, target_date));
            index += 1;
            continue;
        }

        index += 1;
    }

    if ranges.is_empty() {
        return None;
    }

    let start_date = ranges.iter().map(|(start, _)| *start).min()?;
    let end_date = ranges.iter().map(|(_, end)| *end).max()?;

    Some(TemporalWindow {
        start: start_of_day(start_date),
        end: end_of_day(end_date),
    })
}

fn month_date_range(year: i32, month: u32) -> (NaiveDate, NaiveDate) {
    let start = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month start");
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_start = NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("valid next month");
    (start, next_start - chrono::Duration::days(1))
}

fn quarter_date_range(year: i32, quarter: u32) -> (NaiveDate, NaiveDate) {
    let start_month = ((quarter - 1) * 3) + 1;
    let end_month = start_month + 2;
    let (start, _) = month_date_range(year, start_month);
    let (_, end) = month_date_range(year, end_month);
    (start, end)
}

fn previous_quarter_date_range(cutoff: DateTime<Utc>) -> (NaiveDate, NaiveDate) {
    let current_quarter = ((cutoff.month() - 1) / 3) + 1;
    let (year, quarter) = if current_quarter == 1 {
        (cutoff.year() - 1, 4)
    } else {
        (cutoff.year(), current_quarter - 1)
    };
    quarter_date_range(year, quarter)
}

fn start_of_day(date: NaiveDate) -> DateTime<Utc> {
    DateTime::from_naive_utc_and_offset(date.and_hms_opt(0, 0, 0).expect("valid start of day"), Utc)
}

fn end_of_day(date: NaiveDate) -> DateTime<Utc> {
    DateTime::from_naive_utc_and_offset(
        date.and_hms_opt(23, 59, 59).expect("valid end of day"),
        Utc,
    )
}

pub(crate) struct CollectTemporalFactsRequest<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff_iso: &'a str,
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) query: &'a str,
    pub(crate) access: &'a crate::models::AccessContext,
    pub(crate) project: Option<&'a str>,
    pub(crate) fact_types: &'a [String],
    pub(crate) budget: i32,
}

pub(crate) async fn collect_temporal_facts(
    service: &crate::service::MemoryService,
    request: CollectTemporalFactsRequest<'_>,
) -> Result<Vec<crate::models::Fact>, crate::service::error::MemoryError> {
    use crate::service::query::search_query_terms;

    use super::filtering::{
        compare_facts_by_recency, fact_is_active_at, filter_facts_by_constraints,
    };
    use super::lexical::{lexical_query_overlap_for_fact, lexical_query_score_for_fact};

    let Some(expansion) = expand_temporal_synonyms(request.query, request.cutoff) else {
        return Ok(Vec::new());
    };
    let residual_query_terms = expansion
        .residual_query
        .as_deref()
        .map(search_query_terms)
        .unwrap_or_default();

    if let Some(temporal_window) = infer_temporal_window(request.query, request.cutoff) {
        let records = service
            .db_client
            .select_table("fact", request.namespace)
            .await
            .map_err(|err| {
                crate::service::error::MemoryError::Storage(format!("SurrealDB query error: {err}"))
            })?;

        let mut facts = filter_facts_by_constraints(
            records,
            request.access,
            request.project,
            request.fact_types,
        )
        .into_iter()
        .filter(|fact| fact.scope == request.scope)
        .filter(|fact| fact_is_active_at(fact, request.cutoff))
        .filter(|fact| fact.t_valid >= temporal_window.start && fact.t_valid <= temporal_window.end)
        .collect::<Vec<_>>();

        rank_temporal_candidate_facts(
            &mut facts,
            &residual_query_terms,
            compare_facts_by_recency,
            lexical_query_overlap_for_fact,
            lexical_query_score_for_fact,
        );
        facts.truncate(request.budget.max(1) as usize);
        return Ok(facts);
    }

    use crate::service::error::MemoryError;
    let search_limit = request.budget.max(1) * 4;
    let mut matched_facts_by_id = std::collections::HashMap::<String, crate::models::Fact>::new();
    let mut eligible_fact_ids: Option<std::collections::HashSet<String>> = None;

    for temporal_group in expansion.temporal_groups {
        let mut group_fact_ids = std::collections::HashSet::new();

        for temporal_query in temporal_group {
            let records = service
                .db_client
                .select_facts_filtered_advanced(
                    request.namespace,
                    request.scope,
                    request.cutoff_iso,
                    Some(&temporal_query),
                    search_limit,
                    request.project,
                    request.fact_types,
                )
                .await
                .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

            for fact in filter_facts_by_constraints(
                records,
                request.access,
                request.project,
                request.fact_types,
            ) {
                group_fact_ids.insert(fact.fact_id.clone());
                matched_facts_by_id
                    .entry(fact.fact_id.clone())
                    .or_insert(fact);
            }
        }

        if group_fact_ids.is_empty() {
            return Ok(Vec::new());
        }

        eligible_fact_ids = Some(match eligible_fact_ids {
            None => group_fact_ids,
            Some(mut existing) => {
                existing.retain(|fact_id| group_fact_ids.contains(fact_id));
                existing
            }
        });

        if eligible_fact_ids
            .as_ref()
            .is_some_and(std::collections::HashSet::is_empty)
        {
            return Ok(Vec::new());
        }
    }

    let mut facts = eligible_fact_ids
        .unwrap_or_default()
        .into_iter()
        .filter_map(|fact_id| matched_facts_by_id.remove(&fact_id))
        .collect::<Vec<_>>();
    rank_temporal_candidate_facts(
        &mut facts,
        &residual_query_terms,
        compare_facts_by_recency,
        lexical_query_overlap_for_fact,
        lexical_query_score_for_fact,
    );
    facts.truncate(request.budget.max(1) as usize);
    Ok(facts)
}

fn rank_temporal_candidate_facts(
    facts: &mut Vec<crate::models::Fact>,
    residual_query_terms: &[String],
    compare_recency: fn(&crate::models::Fact, &crate::models::Fact) -> std::cmp::Ordering,
    overlap_fn: fn(&crate::models::Fact, &[String]) -> usize,
    score_fn: fn(&crate::models::Fact, &[String]) -> usize,
) {
    if residual_query_terms.is_empty() {
        facts.sort_by(compare_recency);
        return;
    }

    facts.retain(|fact| overlap_fn(fact, residual_query_terms) > 0);
    facts.sort_by(|left, right| {
        score_fn(right, residual_query_terms)
            .cmp(&score_fn(left, residual_query_terms))
            .then_with(|| compare_recency(left, right))
    });
}
