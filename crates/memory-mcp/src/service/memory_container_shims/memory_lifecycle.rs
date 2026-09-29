//! Container-shaped entry points moved out of the memory context.
//!
//! The `From<&MemoryService>` impls and the `fn(&MemoryService, ..)`
//! wrappers for this module. They are adapters, so they live on the
//! side allowed to know the container; the memory context keeps the
//! port-based implementation they delegate to.

// The module compiles without `mcp-apps` — `service::apps::dispatch` is what
// calls into it, and that is gated — so the imports belong to the gate too.
// Ungated, every build without the feature paid three unused-import warnings
// for types the gated function is the only user of.
#[cfg(feature = "mcp-apps")]
use crate::memory::lifecycle_types::LifecycleOperation;
#[cfg(feature = "mcp-apps")]
use crate::memory::lifecycle_types::{LifecycleCommand, LifecycleCommandOutcome};
#[cfg(feature = "mcp-apps")]
use crate::service::MemoryError;

#[cfg(feature = "mcp-apps")]
pub(crate) async fn execute_lifecycle_command(
    service: &crate::service::MemoryService,
    command: LifecycleCommand,
) -> Result<LifecycleCommandOutcome, MemoryError> {
    // The four lifecycle passes take the narrow port; converting once
    // here keeps the match arms readable.
    let handles = crate::platform::lifecycle_runtime::handles_from(service);
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
            LifecycleOperation::ArchiveCandidates.validate_confirmation(dry_run, confirmed)?;
            Ok(LifecycleCommandOutcome::ArchiveCandidates(
                handles.archive_candidates(&target_ids, dry_run).await?,
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
            LifecycleOperation::RestoreArchived.validate_confirmation(false, confirmed)?;
            Ok(LifecycleCommandOutcome::RestoreArchived(
                handles.restore_archived(&target_ids).await?,
            ))
        }
        LifecycleCommand::RecomputeDecay { dry_run, confirmed } => {
            LifecycleOperation::RecomputeDecay.validate_confirmation(dry_run, confirmed)?;
            Ok(LifecycleCommandOutcome::RecomputeDecay(
                handles.recompute_decay(dry_run).await?,
            ))
        }
        LifecycleCommand::RebuildCommunities { dry_run, confirmed } => {
            LifecycleOperation::RebuildCommunities.validate_confirmation(dry_run, confirmed)?;
            Ok(LifecycleCommandOutcome::RebuildCommunities(
                handles.rebuild_communities(dry_run).await?,
            ))
        }
    }
}
