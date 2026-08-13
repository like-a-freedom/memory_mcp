use chrono::Utc;
use serde_json::json;

#[cfg(feature = "mcp-apps")]
use super::types::{LifecycleCommand, LifecycleCommandOutcome};
use crate::service::{
    ArchiveCandidatesOutcome, LifecycleDashboard, LifecycleDefaults, LifecycleView, MemoryError,
    RebuildCommunitiesOutcome, RecomputeDecayOutcome, RestoreArchivedOutcome,
    run_community_rebuild_pass, run_decay_pass,
};

#[cfg(feature = "mcp-apps")]
pub(crate) async fn execute_lifecycle_command(
    service: &crate::service::MemoryService,
    command: LifecycleCommand,
) -> Result<LifecycleCommandOutcome, MemoryError> {
    match command {
        LifecycleCommand::ArchiveCandidates {
            target_ids,
            dry_run,
            confirmed,
        } => {
            if target_ids.is_empty() {
                return Err(MemoryError::Validation(
                    "archive_candidates requires at least one target id".to_string(),
                ));
            }
            if !dry_run && !confirmed {
                return Err(MemoryError::Validation(
                    "archive_candidates requires confirmed=true unless dry_run=true".to_string(),
                ));
            }
            Ok(LifecycleCommandOutcome::ArchiveCandidates(
                service.archive_candidates(&target_ids, dry_run).await?,
            ))
        }
        LifecycleCommand::RestoreArchived {
            target_ids,
            confirmed,
        } => {
            if target_ids.is_empty() {
                return Err(MemoryError::Validation(
                    "restore_archived requires at least one target id".to_string(),
                ));
            }
            if !confirmed {
                return Err(MemoryError::Validation(
                    "restore_archived requires confirmed=true".to_string(),
                ));
            }
            Ok(LifecycleCommandOutcome::RestoreArchived(
                service.restore_archived(&target_ids).await?,
            ))
        }
        LifecycleCommand::RecomputeDecay { dry_run, confirmed } => {
            if !dry_run && !confirmed {
                return Err(MemoryError::Validation(
                    "recompute_decay requires confirmed=true unless dry_run=true".to_string(),
                ));
            }
            Ok(LifecycleCommandOutcome::RecomputeDecay(
                service.recompute_decay(dry_run).await?,
            ))
        }
        LifecycleCommand::RebuildCommunities { dry_run, confirmed } => {
            if !dry_run && !confirmed {
                return Err(MemoryError::Validation(
                    "rebuild_communities requires confirmed=true unless dry_run=true".to_string(),
                ));
            }
            Ok(LifecycleCommandOutcome::RebuildCommunities(
                service.rebuild_communities(dry_run).await?,
            ))
        }
    }
}

impl crate::service::MemoryService {
    pub async fn build_lifecycle_view(&self) -> Result<LifecycleView, MemoryError> {
        let dashboard = self.lifecycle_dashboard().await?;
        let policy = self.lifecycle_policy();

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

    pub async fn lifecycle_dashboard(&self) -> Result<LifecycleDashboard, MemoryError> {
        let active_facts = self.app_store().select_active_facts(10_000).await?;
        let policy = self.lifecycle_policy();
        let cutoff = crate::service::normalize_dt(
            Utc::now() - chrono::Duration::days(policy.archival_age_days as i64),
        );
        let archival_candidates = self
            .app_store()
            .select_episodes_for_archival(&cutoff, 1_000)
            .await?;
        let communities = self.app_store().select_communities().await?;

        Ok(LifecycleDashboard {
            active_facts: active_facts.len(),
            archival_candidates: archival_candidates.len(),
            archival_candidate_ids: archival_candidates
                .iter()
                .filter_map(|record| {
                    record
                        .get("episode_id")
                        .and_then(crate::service::value_helpers::json_string)
                        .map(ToString::to_string)
                })
                .collect(),
            communities: communities.len(),
        })
    }

    pub async fn archive_candidates(
        &self,
        target_ids: &[String],
        dry_run: bool,
    ) -> Result<ArchiveCandidatesOutcome, MemoryError> {
        if !dry_run {
            for episode_id in target_ids {
                self.app_store()
                    .update_record(
                        episode_id,
                        json!({
                            "status": "archived",
                            "archived_at": crate::service::normalize_dt(Utc::now()),
                        }),
                    )
                    .await?;
            }
        }

        Ok(ArchiveCandidatesOutcome {
            dry_run,
            target_ids: target_ids.to_vec(),
            archived_count: if dry_run { 0 } else { target_ids.len() },
        })
    }

    pub async fn restore_archived(
        &self,
        target_ids: &[String],
    ) -> Result<RestoreArchivedOutcome, MemoryError> {
        for episode_id in target_ids {
            self.app_store()
                .update_record(
                    episode_id,
                    json!({
                        "status": "active",
                        "archived_at": serde_json::Value::Null,
                    }),
                )
                .await?;
        }

        Ok(RestoreArchivedOutcome {
            target_ids: target_ids.to_vec(),
            restored_count: target_ids.len(),
        })
    }

    pub async fn recompute_decay(
        &self,
        dry_run: bool,
    ) -> Result<RecomputeDecayOutcome, MemoryError> {
        let invalidated = if dry_run {
            0
        } else {
            let policy = self.lifecycle_policy();
            run_decay_pass(
                self,
                policy.decay_confidence_threshold,
                policy.decay_half_life_days,
            )
            .await?
        };

        Ok(RecomputeDecayOutcome {
            dry_run,
            invalidated,
        })
    }

    pub async fn rebuild_communities(
        &self,
        dry_run: bool,
    ) -> Result<RebuildCommunitiesOutcome, MemoryError> {
        let rebuilt = if dry_run {
            0
        } else {
            run_community_rebuild_pass(self).await?
        };

        Ok(RebuildCommunitiesOutcome { dry_run, rebuilt })
    }
}
