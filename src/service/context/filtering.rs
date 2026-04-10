//! Filtering functions for fact and episode records.

use std::collections::HashSet;

use serde_json::Value;

use crate::models::{AccessContext, Episode, Fact};
use crate::service::episode::{episode_from_record, fact_from_record};
use crate::service::value_helpers::json_string;

pub(crate) fn fact_is_active_at(fact: &Fact, cutoff: chrono::DateTime<chrono::Utc>) -> bool {
    if fact.t_valid > cutoff || fact.t_ingested > cutoff {
        return false;
    }

    match (fact.t_invalid, fact.t_invalid_ingested) {
        (None, _) => true,
        (Some(invalidated_at), _) if invalidated_at > cutoff => true,
        (_, Some(invalidated_ingested_at)) if invalidated_ingested_at > cutoff => true,
        _ => false,
    }
}

pub(crate) fn raw_object(record: &Value) -> Option<&serde_json::Map<String, Value>> {
    if let Some(map) = record.as_object() {
        Some(map)
    } else {
        record.get("Object").and_then(Value::as_object)
    }
}

pub(crate) fn raw_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        Some(array)
    } else {
        value.get("Array").and_then(Value::as_array)
    }
}

pub(crate) fn filter_facts_by_constraints(
    records: Vec<Value>,
    access: &AccessContext,
    project: Option<&str>,
    fact_types: &[String],
) -> Vec<Fact> {
    let mut facts = Vec::new();

    for record in records {
        let items: Vec<&Value> = if let Some(arr) = record.get("Array").and_then(|v| v.as_array()) {
            arr.iter().collect()
        } else {
            vec![&record]
        };

        for item in items {
            let fact_item = if let Some(obj) = item.get("Object") {
                obj
            } else {
                item
            };

            if !fact_record_allowed(fact_item, access, project, fact_types) {
                continue;
            }

            if let Some(fact) = fact_from_record(fact_item) {
                facts.push(fact);
            }
        }
    }

    facts
}

#[cfg(test)]
pub(crate) fn filter_facts_by_policy(records: Vec<Value>, access: &AccessContext) -> Vec<Fact> {
    filter_facts_by_constraints(records, access, None, &[])
}

pub(crate) fn fact_record_allowed(
    record: &Value,
    access: &AccessContext,
    project: Option<&str>,
    fact_types: &[String],
) -> bool {
    fact_record_matches_project(record, project)
        && fact_record_matches_type(record, fact_types)
        && fact_record_allowed_by_policy(record, access)
}

fn fact_record_allowed_by_policy(record: &Value, access: &AccessContext) -> bool {
    let Some(tags) = raw_object(record)
        .and_then(|map| map.get("policy_tags"))
        .and_then(raw_array)
        .map(|values| values.iter().filter_map(json_string).collect::<Vec<_>>())
    else {
        return true;
    };

    if tags.is_empty() {
        return true;
    }

    let Some(allowed_tags) = &access.allowed_tags else {
        return true;
    };

    let allowed = allowed_tags
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    tags.iter().any(|tag| allowed.contains(tag))
}

pub(crate) fn filter_episodes_by_constraints(
    records: Vec<Value>,
    access: &AccessContext,
    project: Option<&str>,
) -> Vec<Episode> {
    records
        .into_iter()
        .filter(|record| episode_record_allowed(record, access, project))
        .filter_map(|record| match record {
            Value::Object(map) => episode_from_record(&map),
            _ => record
                .get("Object")
                .and_then(Value::as_object)
                .and_then(episode_from_record),
        })
        .collect()
}

pub(crate) fn episode_record_allowed(
    record: &Value,
    access: &AccessContext,
    project: Option<&str>,
) -> bool {
    episode_record_matches_project(record, project)
        && episode_record_allowed_by_policy(record, access)
}

fn episode_record_allowed_by_policy(record: &Value, access: &AccessContext) -> bool {
    let Some(tags) = raw_object(record)
        .and_then(|map| map.get("policy_tags"))
        .and_then(raw_array)
        .map(|values| values.iter().filter_map(json_string).collect::<Vec<_>>())
    else {
        return true;
    };

    if tags.is_empty() {
        return true;
    }

    let Some(allowed_tags) = &access.allowed_tags else {
        return true;
    };

    let allowed = allowed_tags
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    tags.iter().any(|tag| allowed.contains(tag))
}

pub(crate) fn fact_record_matches_project(record: &Value, project: Option<&str>) -> bool {
    let Some(project) = project.filter(|project| !project.trim().is_empty()) else {
        return true;
    };

    raw_object(record)
        .and_then(|map| map.get("project"))
        .and_then(json_string)
        .is_some_and(|value| value == project)
}

pub(crate) fn episode_record_matches_project(record: &Value, project: Option<&str>) -> bool {
    let Some(project) = project.filter(|project| !project.trim().is_empty()) else {
        return true;
    };

    raw_object(record)
        .and_then(|map| map.get("project"))
        .and_then(json_string)
        .is_some_and(|value| value == project)
}

pub(crate) fn fact_record_matches_type(record: &Value, fact_types: &[String]) -> bool {
    if fact_types.is_empty() {
        return true;
    }

    raw_object(record)
        .and_then(|map| map.get("fact_type"))
        .and_then(json_string)
        .is_some_and(|value| fact_types.iter().any(|fact_type| fact_type == value))
}
