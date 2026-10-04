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
    // The lifecycle passes take the narrow port; the CLI converts once
    // rather than at each arm.
    let handles = crate::platform::lifecycle_runtime::handles_from(service);
    let response = match args.operation {
        LifecycleOperation::Dashboard => json!({
            "operation": "dashboard",
            "result": handles.build_lifecycle_view().await?,
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
                "result": handles.archive_candidates(&target_ids, dry_run).await?,
            })
        }
        LifecycleOperation::RestoreArchived {
            target_ids,
            confirmed,
        } => {
            ServiceLifecycleOperation::RestoreArchived.validate_confirmation(false, confirmed)?;
            json!({
                "operation": "restore_archived",
                "result": handles.restore_archived(&target_ids).await?,
            })
        }
        LifecycleOperation::RecomputeDecay { dry_run, confirmed } => {
            ServiceLifecycleOperation::RecomputeDecay.validate_confirmation(dry_run, confirmed)?;
            json!({
                "operation": "recompute_decay",
                "result": handles.recompute_decay(dry_run).await?,
            })
        }
        LifecycleOperation::RebuildCommunities { dry_run, confirmed } => {
            ServiceLifecycleOperation::RebuildCommunities
                .validate_confirmation(dry_run, confirmed)?;
            json!({
                "operation": "rebuild_communities",
                "result": handles.rebuild_communities(dry_run).await?,
            })
        }
    };

    Ok(response)
}
