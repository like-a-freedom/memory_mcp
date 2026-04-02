use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::service::error::MemoryError;
use crate::storage::{DbClient, json_string};

/// Returns JSON fallback per §2.5 for APP-02.
#[must_use]
pub fn diff_fallback(result: &DiffResult) -> serde_json::Value {
    serde_json::json!({
        "added": result.added.iter().map(|i| serde_json::json!({"id": i.id, "type": i.item_type, "summary": i.summary})).collect::<Vec<_>>(),
        "removed": result.removed.iter().map(|i| serde_json::json!({"id": i.id, "type": i.item_type, "summary": i.summary})).collect::<Vec<_>>(),
        "changed": result.changed.iter().map(|i| serde_json::json!({"id": i.id, "type": i.item_type, "summary": i.summary, "changes": i.changes})).collect::<Vec<_>>(),
    })
}

pub struct TemporalDiff;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    pub added: Vec<DiffItem>,
    pub removed: Vec<DiffItem>,
    pub changed: Vec<ChangedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffItem {
    pub id: String,
    pub item_type: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedItem {
    pub id: String,
    pub item_type: String,
    pub summary: String,
    pub changes: Vec<FieldChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldChange {
    pub field: String,
    pub left_value: serde_json::Value,
    pub right_value: serde_json::Value,
}

impl TemporalDiff {
    /// Compares facts in a scope between two time points.
    pub async fn compare_scope(
        scope: &str,
        as_of_left: &str,
        as_of_right: &str,
        time_axis: &str,
        filters: Option<&Value>,
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<DiffResult, MemoryError> {
        let left =
            fetch_facts_at(scope, None, as_of_left, time_axis, filters, db, namespace).await?;
        let right =
            fetch_facts_at(scope, None, as_of_right, time_axis, filters, db, namespace).await?;
        Ok(compute_diff(left, right))
    }

    /// Compares facts for one entity between two time points.
    pub async fn compare_entity(
        entity_id: &str,
        scope: &str,
        as_of_left: &str,
        as_of_right: &str,
        time_axis: &str,
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<DiffResult, MemoryError> {
        let left = fetch_facts_at(
            scope,
            Some(entity_id),
            as_of_left,
            time_axis,
            None,
            db,
            namespace,
        )
        .await?;
        let right = fetch_facts_at(
            scope,
            Some(entity_id),
            as_of_right,
            time_axis,
            None,
            db,
            namespace,
        )
        .await?;
        Ok(compute_diff(left, right))
    }
}

fn temporal_inequality(time_axis: &str) -> &'static str {
    match time_axis {
        "transaction" => {
            "t_ingested <= type::datetime($cutoff) AND (t_invalid_ingested IS NONE OR t_invalid_ingested > type::datetime($cutoff))"
        }
        _ => {
            "t_valid <= type::datetime($cutoff) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff))"
        }
    }
}

async fn fetch_facts_at(
    scope: &str,
    entity_id: Option<&str>,
    cutoff: &str,
    time_axis: &str,
    filters: Option<&Value>,
    db: &dyn DbClient,
    namespace: &str,
) -> Result<Vec<Value>, MemoryError> {
    let temporal = temporal_inequality(time_axis);
    let mut sql = format!("SELECT * FROM fact WHERE scope = $scope AND {temporal}");
    let mut vars = json!({"scope": scope, "cutoff": cutoff});

    if let Some(eid) = entity_id {
        sql.push_str(" AND entity_links CONTAINS $entity_id");
        vars.as_object_mut()
            .unwrap()
            .insert("entity_id".into(), json!(eid));
    }
    #[allow(clippy::collapsible_if)]
    if let Some(f) = filters {
        if let Some(ft) = f.get("fact_type").and_then(|v| v.as_str()) {
            sql.push_str(" AND fact_type = $fact_type");
            vars.as_object_mut()
                .unwrap()
                .insert("fact_type".into(), json!(ft));
        }
    }
    sql.push_str(" ORDER BY fact_id ASC");

    let result = db.query(&sql, Some(vars), namespace).await?;
    Ok(result.as_array().cloned().unwrap_or_default())
}

fn fact_key(fact: &Value) -> String {
    fact.get("fact_id")
        .and_then(json_string)
        .map(str::to_string)
        .unwrap_or_default()
}

fn fact_summary(fact: &Value) -> String {
    fact.get("content")
        .and_then(json_string)
        .unwrap_or("")
        .to_string()
}

fn compute_diff(left: Vec<Value>, right: Vec<Value>) -> DiffResult {
    let left_map: HashMap<String, &Value> = left.iter().map(|f| (fact_key(f), f)).collect();
    let right_map: HashMap<String, &Value> = right.iter().map(|f| (fact_key(f), f)).collect();

    let added = right
        .iter()
        .filter(|f| !left_map.contains_key(&fact_key(f)))
        .map(|f| DiffItem {
            id: fact_key(f),
            item_type: f
                .get("fact_type")
                .and_then(json_string)
                .unwrap_or("fact")
                .to_string(),
            summary: fact_summary(f),
        })
        .collect();

    let removed = left
        .iter()
        .filter(|f| !right_map.contains_key(&fact_key(f)))
        .map(|f| DiffItem {
            id: fact_key(f),
            item_type: f
                .get("fact_type")
                .and_then(json_string)
                .unwrap_or("fact")
                .to_string(),
            summary: fact_summary(f),
        })
        .collect();

    let changed = left
        .iter()
        .filter_map(|lf| {
            let key = fact_key(lf);
            let rf = *right_map.get(&key)?;
            let diffs = field_diffs(lf, rf);
            if diffs.is_empty() {
                return None;
            }
            Some(ChangedItem {
                id: key,
                item_type: lf
                    .get("fact_type")
                    .and_then(json_string)
                    .unwrap_or("fact")
                    .to_string(),
                summary: fact_summary(lf),
                changes: diffs,
            })
        })
        .collect();

    DiffResult {
        added,
        removed,
        changed,
    }
}

fn field_diffs(left: &Value, right: &Value) -> Vec<FieldChange> {
    let fields = ["content", "confidence", "t_invalid"];
    fields
        .iter()
        .filter_map(|&field| {
            let lv = left.get(field).cloned().unwrap_or(Value::Null);
            let rv = right.get(field).cloned().unwrap_or(Value::Null);
            if lv == rv {
                return None;
            }
            Some(FieldChange {
                field: field.to_string(),
                left_value: lv,
                right_value: rv,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_fallback_empty_result() {
        let result = DiffResult {
            added: vec![],
            removed: vec![],
            changed: vec![],
        };
        let val = diff_fallback(&result);
        assert!(val["added"].as_array().unwrap().is_empty());
        assert!(val["removed"].as_array().unwrap().is_empty());
        assert!(val["changed"].as_array().unwrap().is_empty());
    }

    #[test]
    fn diff_fallback_with_items() {
        let result = DiffResult {
            added: vec![DiffItem {
                id: "fact:1".to_string(),
                item_type: "fact".to_string(),
                summary: "new fact".to_string(),
            }],
            removed: vec![DiffItem {
                id: "fact:2".to_string(),
                item_type: "fact".to_string(),
                summary: "old fact".to_string(),
            }],
            changed: vec![ChangedItem {
                id: "fact:3".to_string(),
                item_type: "fact".to_string(),
                summary: "changed fact".to_string(),
                changes: vec![FieldChange {
                    field: "content".to_string(),
                    left_value: json!("old"),
                    right_value: json!("new"),
                }],
            }],
        };
        let val = diff_fallback(&result);
        assert_eq!(val["added"].as_array().unwrap().len(), 1);
        assert_eq!(val["removed"].as_array().unwrap().len(), 1);
        assert_eq!(val["changed"].as_array().unwrap().len(), 1);
        assert_eq!(val["changed"][0]["changes"][0]["field"], "content");
    }

    #[test]
    fn fact_key_extracts_fact_id() {
        let fact = json!({"fact_id": "fact:abc", "content": "test"});
        assert_eq!(fact_key(&fact), "fact:abc");
    }

    #[test]
    fn fact_key_returns_empty_for_missing() {
        let fact = json!({"content": "no id"});
        assert_eq!(fact_key(&fact), "");
    }

    #[test]
    fn fact_summary_extracts_content() {
        let fact = json!({"fact_id": "f:1", "content": "some content"});
        assert_eq!(fact_summary(&fact), "some content");
    }

    #[test]
    fn fact_summary_returns_empty_for_missing() {
        let fact = json!({"fact_id": "f:1"});
        assert_eq!(fact_summary(&fact), "");
    }

    #[test]
    fn compute_diff_detects_added_facts() {
        let left = vec![json!({"fact_id": "f:1", "content": "existing"})];
        let right = vec![
            json!({"fact_id": "f:1", "content": "existing"}),
            json!({"fact_id": "f:2", "content": "new"}),
        ];
        let diff = compute_diff(left, right);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].id, "f:2");
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn compute_diff_detects_removed_facts() {
        let left = vec![
            json!({"fact_id": "f:1", "content": "first"}),
            json!({"fact_id": "f:2", "content": "second"}),
        ];
        let right = vec![json!({"fact_id": "f:1", "content": "first"})];
        let diff = compute_diff(left, right);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0].id, "f:2");
        assert!(diff.added.is_empty());
    }

    #[test]
    fn compute_diff_detects_changed_confidence() {
        let left = vec![json!({"fact_id": "f:1", "content": "test", "confidence": 0.9})];
        let right = vec![json!({"fact_id": "f:1", "content": "test", "confidence": 0.5})];
        let diff = compute_diff(left, right);
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].changes[0].field, "confidence");
    }

    #[test]
    fn compute_diff_empty_inputs() {
        let diff = compute_diff(vec![], vec![]);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn compute_diff_no_changes() {
        let left = vec![json!({"fact_id": "f:1", "content": "same"})];
        let right = vec![json!({"fact_id": "f:1", "content": "same"})];
        let diff = compute_diff(left, right);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn field_diffs_detects_content_change() {
        let left = json!({"content": "old", "confidence": 0.9});
        let right = json!({"content": "new", "confidence": 0.9});
        let diffs = field_diffs(&left, &right);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].field, "content");
    }

    #[test]
    fn field_diffs_detects_multiple_changes() {
        let left = json!({"content": "old", "confidence": 0.9, "t_invalid": null});
        let right =
            json!({"content": "new", "confidence": 0.5, "t_invalid": "2025-01-01T00:00:00Z"});
        let diffs = field_diffs(&left, &right);
        assert_eq!(diffs.len(), 3);
    }

    #[test]
    fn field_diffs_no_changes() {
        let left = json!({"content": "same", "confidence": 0.9});
        let right = json!({"content": "same", "confidence": 0.9});
        let diffs = field_diffs(&left, &right);
        assert!(diffs.is_empty());
    }

    #[test]
    fn diff_result_serializes() {
        let result = DiffResult {
            added: vec![],
            removed: vec![],
            changed: vec![],
        };
        let val = serde_json::to_value(&result).unwrap();
        assert!(val.get("added").is_some());
    }

    #[test]
    fn changed_item_serializes() {
        let item = ChangedItem {
            id: "f:1".to_string(),
            item_type: "fact".to_string(),
            summary: "test".to_string(),
            changes: vec![FieldChange {
                field: "confidence".to_string(),
                left_value: json!(0.9),
                right_value: json!(0.5),
            }],
        };
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["id"], "f:1");
        assert_eq!(val["changes"][0]["left_value"], 0.9);
    }
}
