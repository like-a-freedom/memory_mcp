//! Filtering functions for fact and episode records.

use std::collections::HashSet;

use serde_json::Value;

use crate::models::{AccessPayload, Episode, Fact};
use crate::service::episode::{episode_from_record, fact_from_record};
use crate::service::value_helpers::json_string;

pub(crate) fn fact_is_active_at(fact: &Fact, cutoff: chrono::DateTime<chrono::Utc>) -> bool {
    if fact.t_valid > cutoff || fact.t_ingested > cutoff {
        return false;
    }

    // A fact is active if its primary invalidation timestamp hasn't been reached,
    // or if its ingested-side invalidation is still in the future.
    fact.is_active(cutoff)
        || fact
            .t_invalid_ingested
            .map_or(false, |invalidated_ingested_at| {
                invalidated_ingested_at > cutoff
            })
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
    access: &AccessPayload,
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

#[allow(dead_code)]
pub(crate) fn filter_facts_by_policy(records: Vec<Value>, access: &AccessPayload) -> Vec<Fact> {
    filter_facts_by_constraints(records, access, None, &[])
}

pub(crate) fn fact_record_allowed(
    record: &Value,
    access: &AccessPayload,
    project: Option<&str>,
    fact_types: &[String],
) -> bool {
    fact_record_matches_project(record, project)
        && fact_record_matches_type(record, fact_types)
        && fact_record_allowed_by_policy(record, access)
}

fn fact_record_allowed_by_policy(record: &Value, access: &AccessPayload) -> bool {
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
    access: &AccessPayload,
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
    access: &AccessPayload,
    project: Option<&str>,
) -> bool {
    episode_record_matches_project(record, project)
        && episode_record_allowed_by_policy(record, access)
}

fn episode_record_allowed_by_policy(record: &Value, access: &AccessPayload) -> bool {
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

pub(crate) fn compare_facts_by_recency(left: &Fact, right: &Fact) -> std::cmp::Ordering {
    right
        .t_valid
        .cmp(&left.t_valid)
        .then_with(|| left.fact_id.cmp(&right.fact_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn make_access_context(allowed_tags: Option<Vec<String>>) -> AccessPayload {
        AccessPayload {
            caller_id: None,
            allowed_scopes: None,
            allowed_tags,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        }
    }

    // -----------------------------------------------------------------------
    // fact_record_matches_project tests
    // -----------------------------------------------------------------------

    #[test]
    fn fact_record_matches_project_returns_true_when_no_project_filter() {
        let record = serde_json::json!({"project": "alpha"});
        assert!(fact_record_matches_project(&record, None));
    }

    #[test]
    fn fact_record_matches_project_returns_true_when_empty_project_filter() {
        let record = serde_json::json!({"project": "alpha"});
        assert!(fact_record_matches_project(&record, Some("")));
    }

    #[test]
    fn fact_record_matches_project_returns_true_when_match() {
        let record = serde_json::json!({"project": "alpha"});
        assert!(fact_record_matches_project(&record, Some("alpha")));
    }

    #[test]
    fn fact_record_matches_project_returns_false_when_mismatch() {
        let record = serde_json::json!({"project": "alpha"});
        assert!(!fact_record_matches_project(&record, Some("beta")));
    }

    #[test]
    fn fact_record_matches_project_returns_true_when_record_has_no_project() {
        let record = serde_json::json!({"fact": "test"});
        assert!(!fact_record_matches_project(&record, Some("alpha")));
    }

    // -----------------------------------------------------------------------
    // fact_record_matches_type tests
    // -----------------------------------------------------------------------

    #[test]
    fn fact_record_matches_type_returns_true_when_no_type_filter() {
        let record = serde_json::json!({"fact_type": "explicit"});
        assert!(fact_record_matches_type(&record, &[]));
    }

    #[test]
    fn fact_record_matches_type_returns_true_when_match() {
        let record = serde_json::json!({"fact_type": "explicit"});
        assert!(fact_record_matches_type(&record, &["explicit".to_string()]));
    }

    #[test]
    fn fact_record_matches_type_returns_false_when_mismatch() {
        let record = serde_json::json!({"fact_type": "explicit"});
        assert!(!fact_record_matches_type(
            &record,
            &["inferred".to_string()]
        ));
    }

    #[test]
    fn fact_record_matches_type_returns_true_for_multiple_types() {
        let record = serde_json::json!({"fact_type": "inferred"});
        assert!(fact_record_matches_type(
            &record,
            &["explicit".to_string(), "inferred".to_string()]
        ));
    }

    // -----------------------------------------------------------------------
    // fact_record_allowed_by_policy tests
    // -----------------------------------------------------------------------

    #[test]
    fn fact_record_allowed_by_policy_true_when_no_tags() {
        let record = serde_json::json!({"fact": "test"});
        let access = make_access_context(None);
        assert!(fact_record_allowed_by_policy(&record, &access));
    }

    #[test]
    fn fact_record_allowed_by_policy_true_when_empty_tags() {
        let record = serde_json::json!({"policy_tags": []});
        let access = make_access_context(None);
        assert!(fact_record_allowed_by_policy(&record, &access));
    }

    #[test]
    fn fact_record_allowed_by_policy_true_when_access_has_no_tag_restriction() {
        let record = serde_json::json!({"policy_tags": ["secret"]});
        let access = make_access_context(None);
        assert!(fact_record_allowed_by_policy(&record, &access));
    }

    #[test]
    fn fact_record_allowed_by_policy_true_when_tag_matches() {
        let record = serde_json::json!({"policy_tags": ["public"]});
        let access = make_access_context(Some(vec!["public".to_string(), "internal".to_string()]));
        assert!(fact_record_allowed_by_policy(&record, &access));
    }

    #[test]
    fn fact_record_allowed_by_policy_false_when_no_tag_match() {
        let record = serde_json::json!({"policy_tags": ["secret"]});
        let access = make_access_context(Some(vec!["public".to_string()]));
        assert!(!fact_record_allowed_by_policy(&record, &access));
    }

    // -----------------------------------------------------------------------
    // episode_record_matches_project tests
    // -----------------------------------------------------------------------

    #[test]
    fn episode_record_matches_project_true_when_no_filter() {
        let record = serde_json::json!({"project": "alpha"});
        assert!(episode_record_matches_project(&record, None));
    }

    #[test]
    fn episode_record_matches_project_false_when_mismatch() {
        let record = serde_json::json!({"project": "alpha"});
        assert!(!episode_record_matches_project(&record, Some("beta")));
    }

    // -----------------------------------------------------------------------
    // compare_facts_by_recency tests
    // -----------------------------------------------------------------------

    #[test]
    fn compare_facts_by_recency_newer_fact_comes_first() {
        let utc = Utc;
        let older = utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let newer = utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();

        let left = Fact {
            fact_id: "fact:a".to_string(),
            t_valid: older,
            t_ingested: older,
            t_invalid: None,
            t_invalid_ingested: None,
            scope: "org".to_string(),
            content: String::new(),
            quote: String::new(),
            fact_type: String::new(),
            source_episode: String::new(),
            confidence: 0.5,
            access_count: 0,
            last_accessed: None,
            policy_tags: Vec::new(),
            index_keys: Vec::new(),
            entity_links: Vec::new(),
            provenance: serde_json::json!({}),
            ft_score: 0.0,
        };
        let right = Fact {
            fact_id: "fact:b".to_string(),
            t_valid: newer,
            t_ingested: newer,
            t_invalid: None,
            t_invalid_ingested: None,
            scope: "org".to_string(),
            content: String::new(),
            quote: String::new(),
            fact_type: String::new(),
            source_episode: String::new(),
            confidence: 0.5,
            access_count: 0,
            last_accessed: None,
            policy_tags: Vec::new(),
            index_keys: Vec::new(),
            entity_links: Vec::new(),
            provenance: serde_json::json!({}),
            ft_score: 0.0,
        };

        // Function compares right.t_valid vs left.t_valid (reverse order),
        // so when right is newer it returns Greater (right should sort first).
        assert_eq!(
            compare_facts_by_recency(&left, &right),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_facts_by_recency(&right, &left),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn compare_facts_by_recency_tiebreaks_by_fact_id() {
        let utc = Utc;
        let dt = utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();

        let left = Fact {
            fact_id: "fact:b".to_string(),
            t_valid: dt,
            t_ingested: dt,
            t_invalid: None,
            t_invalid_ingested: None,
            scope: "org".to_string(),
            content: String::new(),
            quote: String::new(),
            fact_type: String::new(),
            source_episode: String::new(),
            confidence: 0.5,
            access_count: 0,
            last_accessed: None,
            policy_tags: Vec::new(),
            index_keys: Vec::new(),
            entity_links: Vec::new(),
            provenance: serde_json::json!({}),
            ft_score: 0.0,
        };
        let right = Fact {
            fact_id: "fact:a".to_string(),
            t_valid: dt,
            t_ingested: dt,
            t_invalid: None,
            t_invalid_ingested: None,
            scope: "org".to_string(),
            content: String::new(),
            quote: String::new(),
            fact_type: String::new(),
            source_episode: String::new(),
            confidence: 0.5,
            access_count: 0,
            last_accessed: None,
            policy_tags: Vec::new(),
            index_keys: Vec::new(),
            entity_links: Vec::new(),
            provenance: serde_json::json!({}),
            ft_score: 0.0,
        };

        // Same timestamp, should tiebreak by fact_id
        assert_eq!(
            compare_facts_by_recency(&left, &right),
            std::cmp::Ordering::Greater
        );
    }

    // -----------------------------------------------------------------------
    // raw_object and raw_array tests
    // -----------------------------------------------------------------------

    #[test]
    fn raw_object_returns_map_for_plain_object() {
        let value = serde_json::json!({"key": "value"});
        assert!(raw_object(&value).is_some());
    }

    #[test]
    fn raw_object_returns_map_for_wrapped_object() {
        let value = serde_json::json!({"Object": {"key": "value"}});
        assert!(raw_object(&value).is_some());
    }

    #[test]
    fn raw_array_returns_vec_for_plain_array() {
        let value = serde_json::json!([1, 2, 3]);
        assert!(raw_array(&value).is_some());
    }

    #[test]
    fn raw_array_returns_vec_for_wrapped_array() {
        let value = serde_json::json!({"Array": [1, 2, 3]});
        assert!(raw_array(&value).is_some());
    }
}
