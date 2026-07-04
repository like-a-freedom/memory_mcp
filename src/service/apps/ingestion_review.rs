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
        let scope = request.scope.trim();
        if scope.is_empty() {
            return Err(MemoryError::Validation("scope is required".to_string()));
        }

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
                self.ingest(
                    IngestRequest {
                        source_type: "app_ingestion_review".to_string(),
                        source_id: format!(
                            "ingestion-review:{}",
                            crate::service::hash_prefix(source_text)
                        ),
                        content: source_text.to_string(),
                        t_ref: now,
                        scope: scope.to_string(),
                        project: None,
                        t_ingested: Some(now),
                        visibility_scope: None,
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

        if episode.scope != scope {
            return Err(MemoryError::Validation(format!(
                "draft episode scope mismatch: requested {scope}, episode uses {}",
                episode.scope
            )));
        }

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
        let scope = request.scope.trim();
        if scope.is_empty() {
            return Err(MemoryError::Validation("scope is required".to_string()));
        }

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
                    scope,
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
