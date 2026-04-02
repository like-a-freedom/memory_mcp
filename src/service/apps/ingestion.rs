use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::models::{AppActionResult, DraftIngestion, DraftItem};
use crate::service::error::MemoryError;
use crate::storage::DbClient;

/// Draft summary with item counts by type and status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftSummary {
    pub draft_id: String,
    pub scope: String,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
    pub total_items: usize,
    pub by_type: DraftItemCounts,
    pub by_status: DraftStatusCounts,
}

/// Item counts grouped by type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftItemCounts {
    pub entity: usize,
    pub fact: usize,
    pub edge: usize,
}

/// Item counts grouped by review status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftStatusCounts {
    pub pending: usize,
    pub approved: usize,
    pub edited: usize,
    pub rejected: usize,
}

/// Returns JSON fallback per §2.5 for APP-03.
#[must_use]
pub fn ingestion_fallback(draft_id: &str, items: &[DraftItemFallback]) -> serde_json::Value {
    serde_json::json!({
        "draft_id": draft_id,
        "candidates": {
            "entities": items.iter().filter(|i| i.item_type == "entity").map(|i| &i.payload).collect::<Vec<_>>(),
            "facts": items.iter().filter(|i| i.item_type == "fact").map(|i| &i.payload).collect::<Vec<_>>(),
            "edges": items.iter().filter(|i| i.item_type == "edge").map(|i| &i.payload).collect::<Vec<_>>(),
        },
        "pending_actions": "use app_command(action=commit_review) to finalize after reviewing the resource payload",
    })
}

/// APP-03 Ingestion Review — human-in-the-loop draft workflow.
pub struct IngestionReview;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftItemFallback {
    pub item_type: String,
    pub payload: serde_json::Value,
}

impl IngestionReview {
    /// Creates a new draft ingestion with a single item derived from the source text.
    ///
    /// Persists a `draft_ingestion` record and one `draft_item` (type "entity",
    /// status "pending").  Returns the draft ID in `draft:{uuid}` format.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Storage`] on database failures.
    pub async fn create_draft(
        scope: &str,
        access: serde_json::Value,
        source_text: &str,
        draft_episode_id: &str,
        ttl_seconds: i64,
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<String, MemoryError> {
        let draft_id = generate_draft_id();
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(ttl_seconds);

        let draft = DraftIngestion {
            draft_id: draft_id.clone(),
            scope: scope.to_string(),
            status: "open".to_string(),
            created_at: now,
            expires_at,
            access_ctx: access,
        };

        db.create(
            &format!("draft_ingestion:{draft_id}"),
            serde_json::to_value(&draft).map_err(|e| MemoryError::Storage(e.to_string()))?,
            namespace,
        )
        .await?;

        let item = DraftItem {
            draft_id: draft_id.clone(),
            item_id: format!("{draft_id}:item:0"),
            item_type: "fact".to_string(),
            status: "pending".to_string(),
            payload: json!({
                "fact_type": "note",
                "content": source_text,
                "quote": source_text,
                "source_text": source_text,
                "draft_episode_id": draft_episode_id,
                "policy_tags": [],
            }),
            original_payload: None,
            confidence: 1.0,
            rationale: Some("initial draft creation from source text".to_string()),
            source_snippet: Some(source_text.to_string()),
        };

        db.create(
            &format!("draft_item:{}", item.item_id),
            serde_json::to_value(&item).map_err(|e| MemoryError::Storage(e.to_string()))?,
            namespace,
        )
        .await?;

        Ok(draft_id)
    }

    /// Returns draft metadata together with item counts by type and status.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::DraftExpired`] when the draft has passed its TTL,
    /// [`MemoryError::NotFound`] when the draft does not exist,
    /// or [`MemoryError::Storage`] on database failures.
    pub async fn get_draft_summary(
        draft_id: &str,
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<DraftSummary, MemoryError> {
        let draft = load_and_check_draft(draft_id, db, namespace).await?;

        let items = db
            .query(
                "SELECT * FROM draft_item WHERE draft_id = $draft_id",
                Some(json!({"draft_id": draft_id})),
                namespace,
            )
            .await?;

        let items_vec = extract_items_array(&items);
        let mut by_type = DraftItemCounts {
            entity: 0,
            fact: 0,
            edge: 0,
        };
        let mut by_status = DraftStatusCounts {
            pending: 0,
            approved: 0,
            edited: 0,
            rejected: 0,
        };

        for item_val in &items_vec {
            let item_type = item_val
                .get("item_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match item_type {
                "entity" => by_type.entity += 1,
                "fact" => by_type.fact += 1,
                "edge" => by_type.edge += 1,
                _ => {}
            }

            let status = item_val
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match status {
                "pending" => by_status.pending += 1,
                "approved" => by_status.approved += 1,
                "edited" => by_status.edited += 1,
                "rejected" => by_status.rejected += 1,
                _ => {}
            }
        }

        Ok(DraftSummary {
            draft_id: draft.draft_id,
            scope: draft.scope,
            status: draft.status,
            created_at: draft.created_at.to_rfc3339(),
            expires_at: draft.expires_at.to_rfc3339(),
            total_items: items_vec.len(),
            by_type,
            by_status,
        })
    }

    /// Marks the specified items as approved.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::DraftExpired`] when the draft has passed its TTL,
    /// [`MemoryError::NotFound`] when the draft does not exist,
    /// or [`MemoryError::Storage`] on database failures.
    pub async fn approve_items(
        draft_id: &str,
        item_ids: &[String],
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<AppActionResult, MemoryError> {
        let _draft = load_and_check_draft(draft_id, db, namespace).await?;

        for item_id in item_ids {
            db.query(
                "UPDATE draft_item SET status = 'approved' WHERE item_id = $item_id AND draft_id = $draft_id",
                Some(json!({
                    "item_id": item_id,
                    "draft_id": draft_id,
                })),
                namespace,
            )
            .await?;
        }

        Ok(AppActionResult {
            ok: true,
            message: format!("{} items approved", item_ids.len()),
            refresh_required: true,
            updated_targets: vec![format!("draft_ingestion:{draft_id}")],
            task_id: None,
        })
    }

    /// Marks the specified items as rejected with a reason.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::DraftExpired`] when the draft has passed its TTL,
    /// [`MemoryError::NotFound`] when the draft does not exist,
    /// or [`MemoryError::Storage`] on database failures.
    pub async fn reject_items(
        draft_id: &str,
        item_ids: &[String],
        reason: &str,
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<AppActionResult, MemoryError> {
        let _draft = load_and_check_draft(draft_id, db, namespace).await?;

        for item_id in item_ids {
            db.query(
                "UPDATE draft_item SET status = 'rejected', rationale = $reason WHERE item_id = $item_id AND draft_id = $draft_id",
                Some(json!({
                    "item_id": item_id,
                    "draft_id": draft_id,
                    "reason": reason,
                })),
                namespace,
            )
            .await?;
        }

        Ok(AppActionResult {
            ok: true,
            message: format!("{} items rejected", item_ids.len()),
            refresh_required: true,
            updated_targets: vec![format!("draft_ingestion:{draft_id}")],
            task_id: None,
        })
    }

    pub(crate) async fn edit_item(
        draft_id: &str,
        item_id: &str,
        patch: &Value,
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<AppActionResult, MemoryError> {
        let _draft = load_and_check_draft(draft_id, db, namespace).await?;

        let record_id = format!("draft_item:{item_id}");
        let value = db
            .select_one(&record_id, namespace)
            .await?
            .ok_or_else(|| MemoryError::NotFound(format!("draft item not found: {item_id}")))?;

        let mut item: DraftItem = serde_json::from_value(value)
            .map_err(|error| MemoryError::Storage(error.to_string()))?;

        if item.draft_id != draft_id {
            return Err(MemoryError::NotFound(format!(
                "draft item {item_id} does not belong to {draft_id}"
            )));
        }

        let patch = patch
            .as_object()
            .ok_or_else(|| MemoryError::InvalidParameter("patch must be an object".to_string()))?;
        if patch.is_empty() {
            return Err(MemoryError::InvalidParameter(
                "patch must include at least one editable field".to_string(),
            ));
        }

        let mut payload = item.payload.as_object().cloned().unwrap_or_default();
        let original_payload = item.payload.clone();

        if let Some(content) = patch.get("content").and_then(Value::as_str) {
            payload.insert("content".to_string(), json!(content));
            if !payload.contains_key("quote") {
                payload.insert("quote".to_string(), json!(content));
            }
        }

        if let Some(canonical_name) = patch.get("canonical_name").and_then(Value::as_str) {
            payload.insert("canonical_name".to_string(), json!(canonical_name));
            if item.item_type == "entity" && !payload.contains_key("entity_type") {
                payload.insert("entity_type".to_string(), json!("concept"));
            }
        }

        if let Some(aliases) = patch.get("aliases").and_then(Value::as_array) {
            payload.insert("aliases".to_string(), Value::Array(aliases.clone()));
        }

        if let Some(relation) = patch.get("relation").and_then(Value::as_str) {
            payload.insert("relation".to_string(), json!(relation));
        }

        if let Some(confidence) = patch.get("confidence").and_then(Value::as_f64) {
            item.confidence = confidence;
            payload.insert("confidence".to_string(), json!(confidence));
        }

        if let Some(policy_tags) = patch.get("policy_tags").and_then(Value::as_array) {
            payload.insert("policy_tags".to_string(), Value::Array(policy_tags.clone()));
        }

        if item.item_type == "fact" && !payload.contains_key("fact_type") {
            payload.insert("fact_type".to_string(), json!("note"));
        }

        if item.original_payload.is_none() {
            item.original_payload = Some(original_payload);
        }
        item.payload = Value::Object(payload);
        item.status = "edited".to_string();

        db.update(
            &record_id,
            serde_json::to_value(&item).map_err(|error| MemoryError::Storage(error.to_string()))?,
            namespace,
        )
        .await?;

        Ok(AppActionResult {
            ok: true,
            message: format!("Edited item {item_id}"),
            refresh_required: true,
            updated_targets: vec![format!("draft_ingestion:{draft_id}")],
            task_id: None,
        })
    }

    /// Deletes the draft and all its associated items.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::DraftExpired`] when the draft has passed its TTL,
    /// [`MemoryError::NotFound`] when the draft does not exist,
    /// or [`MemoryError::Storage`] on database failures.
    pub async fn cancel_draft(
        draft_id: &str,
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<AppActionResult, MemoryError> {
        let _draft = load_and_check_draft(draft_id, db, namespace).await?;

        db.query(
            "DELETE draft_item WHERE draft_id = $draft_id",
            Some(json!({"draft_id": draft_id})),
            namespace,
        )
        .await?;

        db.query(
            "DELETE draft_ingestion WHERE draft_id = $draft_id",
            Some(json!({"draft_id": draft_id})),
            namespace,
        )
        .await?;

        Ok(AppActionResult {
            ok: true,
            message: "Draft cancelled".to_string(),
            refresh_required: false,
            updated_targets: vec![format!("draft_ingestion:{draft_id}")],
            task_id: None,
        })
    }

    pub async fn get_draft_items(
        draft_id: &str,
        db: &dyn DbClient,
        namespace: &str,
    ) -> Result<Vec<DraftItem>, MemoryError> {
        let _draft = load_and_check_draft(draft_id, db, namespace).await?;

        let items: Value = db
            .query(
                "SELECT * FROM draft_item WHERE draft_id = $draft_id",
                Some(json!({"draft_id": draft_id})),
                namespace,
            )
            .await?;

        let items: Vec<DraftItem> = items
            .as_array()
            .ok_or_else(|| MemoryError::Storage("Expected array".to_string()))?
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();

        Ok(items)
    }
}

/// Loads a draft by ID and verifies it has not expired.
async fn load_and_check_draft(
    draft_id: &str,
    db: &dyn DbClient,
    namespace: &str,
) -> Result<DraftIngestion, MemoryError> {
    let record_id = format!("draft_ingestion:{draft_id}");
    let value = db.select_one(&record_id, namespace).await?;

    let draft: DraftIngestion = value
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| MemoryError::NotFound(format!("Draft not found: {draft_id}")))?;

    if draft.expires_at < Utc::now() {
        return Err(MemoryError::DraftExpired(format!(
            "Draft {draft_id} has expired"
        )));
    }

    Ok(draft)
}

/// Generates a unique draft ID in `draft:{hash}` format.
fn generate_draft_id() -> String {
    use sha2::{Digest, Sha256};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(now.as_nanos().to_le_bytes());
    format!("draft:{:x}", hasher.finalize())
}

/// Extracts the result array from a SurrealDB query response.
fn extract_items_array(value: &serde_json::Value) -> Vec<serde_json::Value> {
    if let Some(arr) = value.as_array() {
        if arr.is_empty() {
            return Vec::new();
        }
        if let Some(inner) = arr.first().and_then(|v| v.as_array()) {
            return inner.clone();
        }
        return arr.clone();
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingestion_fallback_groups_items_by_type() {
        let items = vec![
            DraftItemFallback {
                item_type: "entity".to_string(),
                payload: json!({"name": "Alice"}),
            },
            DraftItemFallback {
                item_type: "fact".to_string(),
                payload: json!({"content": "works at Acme"}),
            },
            DraftItemFallback {
                item_type: "edge".to_string(),
                payload: json!({"from": "a", "to": "b"}),
            },
        ];
        let val = ingestion_fallback("draft:1", &items);
        assert_eq!(val["draft_id"], "draft:1");
        assert_eq!(val["candidates"]["entities"].as_array().unwrap().len(), 1);
        assert_eq!(val["candidates"]["facts"].as_array().unwrap().len(), 1);
        assert_eq!(val["candidates"]["edges"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn ingestion_fallback_empty_items() {
        let items: Vec<DraftItemFallback> = vec![];
        let val = ingestion_fallback("draft:empty", &items);
        assert!(val["candidates"]["entities"].as_array().unwrap().is_empty());
        assert!(val["candidates"]["facts"].as_array().unwrap().is_empty());
        assert!(val["candidates"]["edges"].as_array().unwrap().is_empty());
    }

    #[test]
    fn draft_summary_serializes() {
        let summary = DraftSummary {
            draft_id: "d:1".to_string(),
            scope: "org".to_string(),
            status: "pending".to_string(),
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-02T00:00:00Z".to_string(),
            total_items: 3,
            by_type: DraftItemCounts {
                entity: 1,
                fact: 1,
                edge: 1,
            },
            by_status: DraftStatusCounts {
                pending: 2,
                approved: 1,
                edited: 0,
                rejected: 0,
            },
        };
        let val = serde_json::to_value(&summary).unwrap();
        assert_eq!(val["total_items"], 3);
        assert_eq!(val["by_type"]["entity"], 1);
    }

    #[test]
    fn generate_draft_id_returns_unique_hashes() {
        let id1 = generate_draft_id();
        let id2 = generate_draft_id();
        assert!(id1.starts_with("draft:"));
        assert!(id2.starts_with("draft:"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn extract_items_array_from_empty_array() {
        assert!(extract_items_array(&json!([])).is_empty());
    }

    #[test]
    fn extract_items_array_from_nested_array() {
        let val = json!([[{"id": 1}, {"id": 2}]]);
        let items = extract_items_array(&val);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn extract_items_array_from_non_array() {
        assert!(extract_items_array(&json!("not an array")).is_empty());
        assert!(extract_items_array(&json!({"key": "val"})).is_empty());
        assert!(extract_items_array(&json!(null)).is_empty());
    }

    #[test]
    fn extract_items_array_from_flat_array() {
        let val = json!([{"id": 1}, {"id": 2}]);
        let items = extract_items_array(&val);
        assert_eq!(items.len(), 2);
    }
}
