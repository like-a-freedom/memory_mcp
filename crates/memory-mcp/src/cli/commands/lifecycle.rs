//! User-facing lifecycle maintenance CLI commands.
//!
//! These handlers are deliberately thin: lifecycle policy and storage writes
//! remain owned by `MemoryService`, while the CLI supplies typed arguments and
//! structured output.

use serde_json::json;

use crate::cli::args::{LifecycleArgs, LifecycleOperation};
use crate::cli::commands::write_response;
use crate::service::{LifecycleOperation as ServiceLifecycleOperation, MemoryError, MemoryService};

pub async fn run(service: &MemoryService, args: LifecycleArgs) -> Result<(), MemoryError> {
    let response = build_response(service, args).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))
}

async fn build_response(
    service: &MemoryService,
    args: LifecycleArgs,
) -> Result<serde_json::Value, MemoryError> {
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
            ServiceLifecycleOperation::ArchiveCandidates
                .validate_confirmation(dry_run, confirmed)?;
            json!({
                "operation": "archive_candidates",
                "result": service.archive_candidates(&target_ids, dry_run).await?,
            })
        }
        LifecycleOperation::RestoreArchived {
            target_ids,
            confirmed,
        } => {
            ServiceLifecycleOperation::RestoreArchived.validate_confirmation(false, confirmed)?;
            json!({
                "operation": "restore_archived",
                "result": service.restore_archived(&target_ids).await?,
            })
        }
        LifecycleOperation::RecomputeDecay { dry_run, confirmed } => {
            ServiceLifecycleOperation::RecomputeDecay.validate_confirmation(dry_run, confirmed)?;
            json!({
                "operation": "recompute_decay",
                "result": service.recompute_decay(dry_run).await?,
            })
        }
        LifecycleOperation::RebuildCommunities { dry_run, confirmed } => {
            ServiceLifecycleOperation::RebuildCommunities
                .validate_confirmation(dry_run, confirmed)?;
            json!({
                "operation": "rebuild_communities",
                "result": service.rebuild_communities(dry_run).await?,
            })
        }
    };

    Ok(response)
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
    async fn every_mutating_operation_requires_confirmation() {
        let service = service();
        let cases = [
            (
                LifecycleOperation::ArchiveCandidates {
                    target_ids: vec!["episode:test".to_string()],
                    dry_run: false,
                    confirmed: false,
                },
                "archive_candidates requires `confirmed=true` unless `dry_run=true`",
            ),
            (
                LifecycleOperation::RestoreArchived {
                    target_ids: vec!["episode:test".to_string()],
                    confirmed: false,
                },
                "restore_archived requires `confirmed=true`",
            ),
            (
                LifecycleOperation::RecomputeDecay {
                    dry_run: false,
                    confirmed: false,
                },
                "recompute_decay requires `confirmed=true` unless `dry_run=true`",
            ),
            (
                LifecycleOperation::RebuildCommunities {
                    dry_run: false,
                    confirmed: false,
                },
                "rebuild_communities requires `confirmed=true` unless `dry_run=true`",
            ),
        ];

        for (operation, expected_message) in cases {
            let result = run(&service, LifecycleArgs { operation }).await;
            assert!(matches!(
                result,
                Err(MemoryError::Validation(message)) if message == expected_message
            ));
        }
    }

    #[tokio::test]
    async fn successful_operations_use_operation_result_envelopes() {
        let service = service();
        let cases = [
            (LifecycleOperation::Dashboard, "dashboard"),
            (
                LifecycleOperation::ArchiveCandidates {
                    target_ids: vec!["episode:test".to_string()],
                    dry_run: true,
                    confirmed: false,
                },
                "archive_candidates",
            ),
            (
                LifecycleOperation::RestoreArchived {
                    target_ids: vec!["episode:test".to_string()],
                    confirmed: true,
                },
                "restore_archived",
            ),
            (
                LifecycleOperation::RecomputeDecay {
                    dry_run: true,
                    confirmed: false,
                },
                "recompute_decay",
            ),
            (
                LifecycleOperation::RebuildCommunities {
                    dry_run: true,
                    confirmed: false,
                },
                "rebuild_communities",
            ),
        ];

        for (operation, expected_operation) in cases {
            let response = build_response(&service, LifecycleArgs { operation })
                .await
                .expect("lifecycle operation should succeed");
            assert_eq!(response["operation"], expected_operation);
            assert!(response["result"].is_object());
            assert_eq!(response.as_object().map(|object| object.len()), Some(2));
        }
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
