//! Experience fact retrieval and appending.

use std::collections::HashSet;

use chrono::{DateTime, Utc};

use crate::models::{AccessContext, AssembledContextItem, FactType};
use crate::service::error::MemoryError;
use crate::service::query::{decayed_confidence, normalize_dt};

use super::filtering::{compare_facts_by_recency, fact_is_active_at, filter_facts_by_constraints};

pub(crate) struct RecentExperienceRequest<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) project: Option<&'a str>,
    pub(crate) access: &'a AccessContext,
    pub(crate) budget: i32,
    pub(crate) fact_types: &'a [String],
}

pub(crate) async fn append_recent_experience_items(
    results: &mut Vec<AssembledContextItem>,
    service: &crate::service::MemoryService,
    request: RecentExperienceRequest<'_>,
) -> Result<usize, MemoryError> {
    let budget = request.budget.max(1) as usize;
    if results.len() >= budget {
        return Ok(0);
    }

    if !request.fact_types.is_empty()
        && !request
            .fact_types
            .iter()
            .any(|fact_type| fact_type == FactType::Experience.as_str())
    {
        return Ok(0);
    }

    let records = service
        .db_client
        .select_active_facts(request.namespace, 500)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
    let experience_filter = vec![FactType::Experience.as_str().to_string()];
    let mut facts =
        filter_facts_by_constraints(records, request.access, request.project, &experience_filter)
            .into_iter()
            .filter(|fact| fact.scope == request.scope)
            .filter(|fact| fact_is_active_at(fact, request.cutoff))
            .collect::<Vec<_>>();

    facts.sort_by(|left, right| {
        right
            .t_ingested
            .cmp(&left.t_ingested)
            .then_with(|| compare_facts_by_recency(left, right))
    });

    let mut seen_fact_ids = results
        .iter()
        .map(|item| item.fact_id.clone())
        .collect::<HashSet<_>>();
    let mut appended = 0;

    for fact in facts {
        if results.len() >= budget || !seen_fact_ids.insert(fact.fact_id.clone()) {
            continue;
        }

        let confidence = decayed_confidence(&fact, request.cutoff);

        results.push(AssembledContextItem {
            fact_id: fact.fact_id,
            content: fact.content,
            quote: fact.quote,
            source_episode: fact.source_episode,
            confidence,
            provenance: fact.provenance,
            rationale: format!(
                "supplemental experience recent_t_ingested={}",
                normalize_dt(fact.t_ingested)
            ),
            retrieval_tier: None,
        });
        appended += 1;
    }

    Ok(appended)
}

pub(crate) fn supplemental_experience_count(results: &[AssembledContextItem]) -> usize {
    results
        .iter()
        .filter(|item| item.rationale.starts_with("supplemental experience "))
        .count()
}
