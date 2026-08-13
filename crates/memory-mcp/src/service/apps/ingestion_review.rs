use chrono::Utc;

use crate::models::{IngestRequest, Provenance};
use crate::service::{
    CommitIngestionReviewOutcome, CommitIngestionReviewRequest, IngestionReviewBundle,
    IngestionReviewItem, IngestionReviewSource, IngestionReviewSummary, MemoryError,
    PrepareIngestionReviewRequest,
};

impl crate::service::MemoryService {
    pub async fn prepare_ingestion_review(
        &self,
        request: PrepareIngestionReviewRequest,
    ) -> Result<IngestionReviewBundle, MemoryError> {
        let episode_id = match (
            request
                .source_text
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
            request
                .draft_episode_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
        ) {
            (Some(source_text), None) => {
                let now = Utc::now();
                crate::service::capabilities::ingest::IngestCapability::ingest(
                    &self.build_context(),
                    IngestRequest {
                        source_type: "app_ingestion_review".to_string(),
                        source_id: format!(
                            "ingestion-review:{}",
                            crate::service::hash_prefix(source_text)
                        ),
                        content: source_text.to_string(),
                        t_ref: now,
                        t_ingested: Some(now),
                        policy_tags: vec![],
                    },
                    None,
                )
                .await?
            }
            (None, Some(draft_episode_id)) => draft_episode_id.to_string(),
            (Some(_), Some(_)) => {
                return Err(MemoryError::Validation(
                    "provide either source_text or draft_episode_id, not both".to_string(),
                ));
            }
            (None, None) => {
                return Err(MemoryError::Validation(
                    "source_text or draft_episode_id is required".to_string(),
                ));
            }
        };

        let (episode_record, _) = self.find_episode_record(&episode_id).await?;
        let episode = episode_record
            .as_ref()
            .and_then(crate::service::episode_from_record)
            .ok_or_else(|| MemoryError::NotFound(format!("episode not found: {episode_id}")))?;

        let item = IngestionReviewItem {
            item_id: format!("draft:{}", episode.episode_id),
            status: "pending".to_string(),
            kind: "draft_fact".to_string(),
            fact_type: "note".to_string(),
            content: episode.content.clone(),
            quote: episode.content.clone(),
            source_episode: episode.episode_id.clone(),
            entity_links: Vec::new(),
            confidence: 0.8,
            t_valid: episode.t_ref,
            reason: None,
        };
        let items = vec![item];

        Ok(IngestionReviewBundle {
            source: IngestionReviewSource {
                source_text: request.source_text,
                draft_episode_id: Some(episode.episode_id),
            },
            summary: IngestionReviewSummary::from_items(&items),
            items,
        })
    }

    pub async fn commit_ingestion_review(
        &self,
        request: CommitIngestionReviewRequest,
    ) -> Result<CommitIngestionReviewOutcome, MemoryError> {
        let mut fact_ids = Vec::new();
        for item in request
            .items
            .iter()
            .filter(|item| matches!(item.status.as_str(), "approved" | "edited"))
        {
            let fact_id = self
                .add_fact(
                    &item.fact_type,
                    &item.content,
                    &item.quote,
                    &item.source_episode,
                    item.t_valid,
                    item.confidence,
                    item.entity_links.clone(),
                    vec![],
                    Provenance::agent_observation(&item.source_episode),
                )
                .await?;
            fact_ids.push(fact_id);
        }

        Ok(CommitIngestionReviewOutcome {
            committed_count: fact_ids.len(),
            fact_ids,
        })
    }
}

#[cfg(feature = "mcp-apps")]
pub fn apply_ingestion_review_status(
    items: &mut [IngestionReviewItem],
    item_ids: &[String],
    status: &str,
    reason: Option<&str>,
) -> IngestionReviewSummary {
    for item in items.iter_mut() {
        if item_ids.iter().any(|candidate| candidate == &item.item_id) {
            item.status = status.to_string();
            if status == "approved" {
                item.reason = None;
            } else if let Some(reason) = reason {
                item.reason = Some(reason.to_string());
            }
        }
    }
    IngestionReviewSummary::from_items(items)
}

/// Applies a protocol-neutral edit to one review item and recomputes its
/// summary. Unknown JSON fields are ignored by the typed domain model, while a
/// successful edit marks a previously unclassified item as `edited`.
#[cfg(feature = "mcp-apps")]
pub fn apply_ingestion_review_edit(
    items: &mut [IngestionReviewItem],
    item_id: &str,
    patch: &serde_json::Value,
) -> Result<IngestionReviewSummary, MemoryError> {
    let patch = patch.as_object().ok_or_else(|| {
        MemoryError::Validation("ingestion review edit must be a JSON object".to_string())
    })?;
    let item = items
        .iter_mut()
        .find(|item| item.item_id == item_id)
        .ok_or_else(|| {
            MemoryError::NotFound(format!("Unknown ingestion review item: {item_id}"))
        })?;
    let mut value = serde_json::to_value(&*item).map_err(|error| {
        MemoryError::Validation(format!("failed to encode review item: {error}"))
    })?;
    let object = value.as_object_mut().ok_or_else(|| {
        MemoryError::Validation("ingestion review item must encode as an object".to_string())
    })?;
    for (key, value) in patch {
        object.insert(key.clone(), value.clone());
    }
    if !patch.contains_key("status") {
        object.insert("status".to_string(), serde_json::json!("edited"));
    }
    *item = serde_json::from_value(value).map_err(|error| {
        MemoryError::Validation(format!("invalid ingestion review edit: {error}"))
    })?;
    Ok(IngestionReviewSummary::from_items(items))
}

#[cfg(all(test, feature = "mcp-apps"))]
mod tests {
    use super::*;

    #[test]
    fn apply_status_updates_selected_items_and_clears_approval_reason() {
        let now = Utc::now();
        let mut items = vec![
            IngestionReviewItem {
                item_id: "item:1".to_string(),
                status: "rejected".to_string(),
                kind: "draft_fact".to_string(),
                fact_type: "note".to_string(),
                content: "one".to_string(),
                quote: "one".to_string(),
                source_episode: "episode:1".to_string(),
                entity_links: Vec::new(),
                confidence: 0.9,
                t_valid: now,
                reason: Some("old reason".to_string()),
            },
            IngestionReviewItem {
                item_id: "item:2".to_string(),
                status: "pending".to_string(),
                kind: "draft_fact".to_string(),
                fact_type: "note".to_string(),
                content: "two".to_string(),
                quote: "two".to_string(),
                source_episode: "episode:1".to_string(),
                entity_links: Vec::new(),
                confidence: 0.8,
                t_valid: now,
                reason: None,
            },
        ];

        let summary =
            apply_ingestion_review_status(&mut items, &["item:1".to_string()], "approved", None);

        assert_eq!(items[0].status, "approved");
        assert_eq!(items[0].reason, None);
        assert_eq!(items[1].status, "pending");
        assert_eq!(summary.approved, 1);
        assert_eq!(summary.pending, 1);
    }

    #[test]
    fn apply_edit_updates_typed_item_and_marks_it_edited() {
        let now = Utc::now();
        let mut items = vec![IngestionReviewItem {
            item_id: "item:1".to_string(),
            status: "pending".to_string(),
            kind: "draft_fact".to_string(),
            fact_type: "note".to_string(),
            content: "old".to_string(),
            quote: "old".to_string(),
            source_episode: "episode:1".to_string(),
            entity_links: Vec::new(),
            confidence: 0.8,
            t_valid: now,
            reason: None,
        }];
        let summary = apply_ingestion_review_edit(
            &mut items,
            "item:1",
            &serde_json::json!({"content": "new"}),
        )
        .expect("edit should be valid");
        assert_eq!(items[0].content, "new");
        assert_eq!(items[0].status, "edited");
        assert_eq!(summary.edited, 1);
    }
}
