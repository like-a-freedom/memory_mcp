//! User-facing lifecycle maintenance CLI commands.
//!
//! These handlers are deliberately thin: lifecycle policy and storage writes
//! remain owned by `MemoryService`, while the CLI supplies typed arguments and
//! structured output.

use serde_json::json;

use crate::cli::args::{LifecycleArgs, LifecycleOperation};
use crate::cli::commands::write_response;
use crate::service::{MemoryError, MemoryService};

pub async fn run(service: &MemoryService, args: LifecycleArgs) -> Result<(), MemoryError> {
    let response = match args.operation {
        LifecycleOperation::Dashboard => json!({
            "operation": "dashboard",
            "result": service.build_lifecycle_view().await?,
        }),
        LifecycleOperation::ArchiveCandidates {
            target_ids,
            dry_run,
            confirmed,
        } => {
            if !dry_run && !confirmed {
                return Err(MemoryError::Validation(
                    "archive-candidates requires --confirmed unless --dry-run is set".into(),
                ));
            }
            json!({
                "operation": "archive_candidates",
                "result": service.archive_candidates(&target_ids, dry_run).await?,
            })
        }
        LifecycleOperation::RestoreArchived {
            target_ids,
            confirmed,
        } => {
            if !confirmed {
                return Err(MemoryError::Validation(
                    "restore-archived requires --confirmed".into(),
                ));
            }
            json!({
                "operation": "restore_archived",
                "result": service.restore_archived(&target_ids).await?,
            })
        }
        LifecycleOperation::RecomputeDecay { dry_run, confirmed } => {
            if !dry_run && !confirmed {
                return Err(MemoryError::Validation(
                    "recompute-decay requires --confirmed unless --dry-run is set".into(),
                ));
            }
            json!({
                "operation": "recompute_decay",
                "result": service.recompute_decay(dry_run).await?,
            })
        }
        LifecycleOperation::RebuildCommunities { dry_run, confirmed } => {
            if !dry_run && !confirmed {
                return Err(MemoryError::Validation(
                    "rebuild-communities requires --confirmed unless --dry-run is set".into(),
                ));
            }
            json!({
                "operation": "rebuild_communities",
                "result": service.rebuild_communities(dry_run).await?,
            })
        }
    };

    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> MemoryService {
        MemoryService::new(
            std::sync::Arc::new(crate::service::mock_db::MockDbClient::new()),
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("test service")
    }

    #[tokio::test]
    async fn mutating_operations_require_confirmation() {
        let service = service();

        let result = run(
            &service,
            LifecycleArgs {
                operation: LifecycleOperation::RecomputeDecay {
                    dry_run: false,
                    confirmed: false,
                },
            },
        )
        .await;

        assert!(
            matches!(result, Err(MemoryError::Validation(message)) if message.contains("--confirmed"))
        );
    }

    #[tokio::test]
    async fn dry_run_allows_decay_without_confirmation() {
        let service = service();

        let result = run(
            &service,
            LifecycleArgs {
                operation: LifecycleOperation::RecomputeDecay {
                    dry_run: true,
                    confirmed: false,
                },
            },
        )
        .await;

        assert!(result.is_ok());
    }
}
