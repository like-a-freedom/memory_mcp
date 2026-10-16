use chrono::Utc;
use serde_json::json;

use crate::error::MemoryError;
use crate::memory::lifecycle_types::{
    ArchiveCandidatesOutcome, LifecycleDashboard, LifecycleDefaults, LifecycleView,
    RebuildCommunitiesOutcome, RecomputeDecayOutcome, RestoreArchivedOutcome, StockLevels,
};

impl crate::memory::lifecycle_workers::LifecycleHandles<'_> {
    pub async fn build_lifecycle_view(&self) -> Result<LifecycleView, MemoryError> {
        let service = self;
        let dashboard = self.lifecycle_dashboard().await?;
        let policy = service.policy;

        Ok(LifecycleView {
            dashboard,
            defaults: LifecycleDefaults {
                archival_age_days: policy.archival_age_days,
                decay_threshold: policy.decay_confidence_threshold,
                decay_half_life_days: policy.decay_half_life_days,
            },
            recent_actions: Vec::new(),
        })
    }

    /// Reads the inventory levels the lifecycle view reports.
    ///
    /// One reader for the levels, so the dashboard and the periodic
    /// stock stamp measure the same quantities the same way.
    pub(crate) async fn read_stock_levels(&self) -> Result<StockLevels, MemoryError> {
        let active_facts = self
            .knowledge_graph_store()
            .select_active_facts(10_000)
            .await?;
        let policy = self.policy;
        let cutoff = crate::shared::temporal::normalize_dt(
            Utc::now() - chrono::Duration::days(policy.archival_age_days as i64),
        );
        let archival_candidates = self
            .episode_store()
            .select_episodes_for_archival(&cutoff, 1_000)
            .await?;
        let communities = self.knowledge_graph_store().select_communities().await?;
        Ok(StockLevels {
            active_facts: active_facts.len(),
            archival_candidates: archival_candidates.len(),
            archival_candidate_ids: archival_candidates
                .iter()
                .filter_map(|record| {
                    record
                        .get("episode_id")
                        .and_then(crate::storage::value_helpers::json_string)
                        .map(ToString::to_string)
                })
                .collect(),
            communities: communities.len(),
        })
    }

    pub async fn lifecycle_dashboard(&self) -> Result<LifecycleDashboard, MemoryError> {
        let service = self;
        let mut operation_metrics =
            crate::observability::OperationMetrics::new("lifecycle_dashboard");
        let levels = service.read_stock_levels().await?;

        // A stock, not a flow: these are levels read from the store, so they are
        // set rather than added. Counting them made the metric the sum of
        // every inventory ever read — opening this dashboard grew the counter
        // by the size of the store, and its rate reported dashboard traffic
        // instead of growth in the data.
        operation_metrics.record_stock("active_facts", levels.active_facts);
        operation_metrics.record_stock("archival_candidates", levels.archival_candidates);
        operation_metrics.record_stock("communities", levels.communities);
        operation_metrics.success();
        Ok(LifecycleDashboard {
            active_facts: levels.active_facts,
            archival_candidates: levels.archival_candidates,
            archival_candidate_ids: levels.archival_candidate_ids,
            communities: levels.communities,
        })
    }

    /// Archives the named episodes.
    ///
    /// Takes the lifecycle port rather than the container: the pass
    /// reads the resolved policy and writes through the episode store,
    /// and nothing else.
    pub async fn archive_candidates(
        &self,
        target_ids: &[String],
        dry_run: bool,
    ) -> Result<ArchiveCandidatesOutcome, MemoryError> {
        let service = self;
        let mut operation_metrics =
            crate::observability::OperationMetrics::new("lifecycle_archive_candidates");
        if !dry_run {
            for episode_id in target_ids {
                service
                    .episode_store()
                    .update_episode(
                        episode_id,
                        json!({
                            "status": "archived",
                            "archived_at": crate::shared::temporal::normalize_dt(Utc::now()),
                        }),
                    )
                    .await?;
            }
        }

        let archived_count = if dry_run { 0 } else { target_ids.len() };
        operation_metrics.record_result("archived", archived_count);
        operation_metrics.success();
        Ok(ArchiveCandidatesOutcome {
            dry_run,
            target_ids: target_ids.to_vec(),
            archived_count,
        })
    }

    pub async fn restore_archived(
        &self,
        target_ids: &[String],
    ) -> Result<RestoreArchivedOutcome, MemoryError> {
        let service = self;
        let mut operation_metrics =
            crate::observability::OperationMetrics::new("lifecycle_restore_archived");
        for episode_id in target_ids {
            service
                .episode_store()
                .update_episode(
                    episode_id,
                    json!({
                        "status": "active",
                        "archived_at": serde_json::Value::Null,
                    }),
                )
                .await?;
        }

        operation_metrics.record_result("restored", target_ids.len());
        operation_metrics.success();
        Ok(RestoreArchivedOutcome {
            target_ids: target_ids.to_vec(),
            restored_count: target_ids.len(),
        })
    }

    pub async fn recompute_decay(
        &self,
        dry_run: bool,
    ) -> Result<RecomputeDecayOutcome, MemoryError> {
        let service = self;
        let mut operation_metrics =
            crate::observability::OperationMetrics::new("lifecycle_recompute_decay");
        let invalidated = if dry_run {
            0
        } else {
            let policy = service.policy;
            crate::memory::lifecycle_workers::decay::run_decay_pass(
                service,
                policy.decay_confidence_threshold,
                policy.decay_half_life_days,
            )
            .await?
        };

        operation_metrics.record_result("decay_invalidated", invalidated);
        operation_metrics.success();
        Ok(RecomputeDecayOutcome {
            dry_run,
            invalidated,
        })
    }

    pub async fn rebuild_communities(
        &self,
        dry_run: bool,
    ) -> Result<RebuildCommunitiesOutcome, MemoryError> {
        let service = self;
        let mut operation_metrics =
            crate::observability::OperationMetrics::new("lifecycle_rebuild_communities");
        let rebuilt = if dry_run {
            0
        } else {
            crate::memory::lifecycle_workers::run_community_rebuild_pass(service).await?
        };

        operation_metrics.record_result("communities", rebuilt);
        operation_metrics.success();
        Ok(RebuildCommunitiesOutcome { dry_run, rebuilt })
    }
}
