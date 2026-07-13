use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::service::{
    DiffChange, DiffRequest, DiffSummary, DiffTarget, DiffView, DiffViewRange, MemoryError,
};

impl crate::service::MemoryService {
    pub async fn build_diff(&self, request: DiffRequest) -> Result<DiffView, MemoryError> {
        let namespace = self.namespace_for_scope(&request.scope)?;
        let facts = self.app_store().select_facts(&namespace).await?;

        let left = facts_at(
            &facts,
            request.as_of_left,
            request.target_type.as_str(),
            request.target_id.as_deref(),
            request.time_axis.as_str(),
        );
        let right = facts_at(
            &facts,
            request.as_of_right,
            request.target_type.as_str(),
            request.target_id.as_deref(),
            request.time_axis.as_str(),
        );

        let mut changes = Vec::new();
        let mut unchanged_count = 0;

        for (fact_id, fact) in &right {
            if left.contains_key(fact_id) {
                unchanged_count += 1;
            } else {
                changes.push(DiffChange {
                    fact_id: fact.fact_id.clone(),
                    change_type: "added".to_string(),
                    content: fact.content.clone(),
                    quote: fact.quote.clone(),
                    source_episode: fact.source_episode.clone(),
                    t_valid: fact.t_valid,
                    t_ingested: fact.t_ingested,
                });
            }
        }

        for (fact_id, fact) in &left {
            if !right.contains_key(fact_id) {
                changes.push(DiffChange {
                    fact_id: fact.fact_id.clone(),
                    change_type: "removed".to_string(),
                    content: fact.content.clone(),
                    quote: fact.quote.clone(),
                    source_episode: fact.source_episode.clone(),
                    t_valid: fact.t_valid,
                    t_ingested: fact.t_ingested,
                });
            }
        }

        changes.sort_by(|left, right| {
            left.t_valid
                .cmp(&right.t_valid)
                .then_with(|| left.fact_id.cmp(&right.fact_id))
        });

        let added_count = changes
            .iter()
            .filter(|change| change.change_type == "added")
            .count();
        let removed_count = changes
            .iter()
            .filter(|change| change.change_type == "removed")
            .count();

        Ok(DiffView {
            target: DiffTarget {
                target_type: request.target_type,
                target_id: request.target_id,
            },
            range: DiffViewRange {
                as_of_left: request.as_of_left,
                as_of_right: request.as_of_right,
                time_axis: request.time_axis,
            },
            summary: DiffSummary {
                left_count: left.len(),
                right_count: right.len(),
                added_count,
                removed_count,
                unchanged_count,
                change_count: changes.len(),
            },
            changes,
        })
    }
}

fn facts_at(
    records: &[serde_json::Value],
    cutoff: DateTime<Utc>,
    target_type: &str,
    target_id: Option<&str>,
    time_axis: &str,
) -> HashMap<String, crate::models::Fact> {
    let mut facts = HashMap::new();

    for record in records {
        let Some(fact) = crate::service::fact_from_record(record) else {
            continue;
        };
        if !matches_target(&fact, target_type, target_id) || !visible_at(&fact, cutoff, time_axis) {
            continue;
        }
        facts.insert(fact.fact_id.clone(), fact);
    }

    facts
}

fn matches_target(fact: &crate::models::Fact, target_type: &str, target_id: Option<&str>) -> bool {
    match target_type {
        "scope" => true,
        "entity" => target_id.is_some_and(|target_id| {
            fact.entity_links
                .iter()
                .any(|entity_id| entity_id == target_id)
        }),
        "episode" => target_id.is_some_and(|target_id| fact.source_episode == target_id),
        "fact" => target_id.is_some_and(|target_id| fact.fact_id == target_id),
        _ => false,
    }
}

fn visible_at(fact: &crate::models::Fact, cutoff: DateTime<Utc>, time_axis: &str) -> bool {
    match time_axis {
        "ingested" => {
            if fact.t_ingested > cutoff {
                return false;
            }
            match fact.t_invalid_ingested {
                None => true,
                Some(invalidated_at) => invalidated_at > cutoff,
            }
        }
        _ => {
            if fact.t_valid > cutoff {
                return false;
            }
            match fact.t_invalid {
                None => true,
                Some(invalidated_at) if invalidated_at > cutoff => true,
                _ => false,
            }
        }
    }
}
