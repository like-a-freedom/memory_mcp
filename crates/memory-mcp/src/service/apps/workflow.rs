use serde_json::Value;

use super::types::{LifecycleCommand, LifecycleOperation};
use crate::service::MemoryError;

/// Protocol-neutral input used to classify an app command.
#[derive(Debug, Clone, Default)]
pub struct AppCommandInput {
    pub action: String,
    pub item_ids: Vec<String>,
    pub target_ids: Vec<String>,
    pub target_id: Option<String>,
    pub item_id: Option<String>,
    pub patch_json: Option<String>,
    pub reason: Option<String>,
    pub dry_run: bool,
    pub confirmed: bool,
    pub format: Option<String>,
    pub direction: Option<String>,
    pub depth: Option<i32>,
}

/// Validated app command. Parsing and cross-app invariants live here; the MCP
/// adapter is responsible only for session persistence and response shaping.
#[derive(Debug, Clone)]
pub enum AppCommand {
    ApproveItems {
        item_ids: Vec<String>,
    },
    RejectItems {
        item_ids: Vec<String>,
        reason: String,
    },
    EditItem {
        item_id: String,
        patch: Value,
    },
    CommitReview,
    CancelReview,
    Lifecycle(LifecycleCommand),
    ExportDiff {
        format: String,
    },
    ExpandNeighbors {
        target_id: String,
        direction: String,
        depth: i32,
    },
    OpenEdgeDetails {
        edge_id: String,
    },
    UsePathAsContext {
        path_id: String,
    },
    CloseSession,
}

impl AppCommand {
    pub fn action_name(&self) -> &'static str {
        match self {
            Self::ApproveItems { .. } => "approve_items",
            Self::RejectItems { .. } => "reject_items",
            Self::EditItem { .. } => "edit_item",
            Self::CommitReview => "commit_review",
            Self::CancelReview => "cancel_review",
            Self::Lifecycle(LifecycleCommand::ArchiveCandidates { .. }) => "archive_candidates",
            Self::Lifecycle(LifecycleCommand::RestoreArchived { .. }) => "restore_archived",
            Self::Lifecycle(LifecycleCommand::RecomputeDecay { .. }) => "recompute_decay",
            Self::Lifecycle(LifecycleCommand::RebuildCommunities { .. }) => "rebuild_communities",
            Self::ExportDiff { .. } => "export_diff",
            Self::ExpandNeighbors { .. } => "expand_neighbors",
            Self::OpenEdgeDetails { .. } => "open_edge_details",
            Self::UsePathAsContext { .. } => "use_path_as_context",
            Self::CloseSession => "close_session",
        }
    }

    pub fn parse(app: &str, input: AppCommandInput) -> Result<Self, MemoryError> {
        let action = input.action.as_str();
        let require_app = |expected: &str| {
            if app == expected {
                Ok(())
            } else {
                Err(MemoryError::Validation(format!(
                    "{action} is only supported for {expected} sessions"
                )))
            }
        };
        let require_items = || {
            if input.item_ids.is_empty() {
                Err(MemoryError::Validation(format!(
                    "`item_ids` is required for {action}"
                )))
            } else {
                Ok(())
            }
        };

        match action {
            "approve_items" | "approve_ingestion_items" => {
                require_app("ingestion_review")?;
                require_items()?;
                Ok(Self::ApproveItems {
                    item_ids: input.item_ids,
                })
            }
            "reject_items" | "reject_ingestion_items" => {
                require_app("ingestion_review")?;
                require_items()?;
                Ok(Self::RejectItems {
                    item_ids: input.item_ids,
                    reason: input
                        .reason
                        .unwrap_or_else(|| "Rejected from app review".to_string()),
                })
            }
            "edit_item" => {
                require_app("ingestion_review")?;
                let item_id = input.item_id.ok_or_else(|| {
                    MemoryError::Validation("`item_id` is required for edit_item".to_string())
                })?;
                let patch_json = input.patch_json.ok_or_else(|| {
                    MemoryError::Validation("`patch_json` is required for edit_item".to_string())
                })?;
                let patch = serde_json::from_str::<Value>(&patch_json).map_err(|error| {
                    MemoryError::Validation(format!("`patch_json` must be valid JSON: {error}"))
                })?;
                if !patch.is_object() {
                    return Err(MemoryError::Validation(
                        "`patch_json` must encode a JSON object".to_string(),
                    ));
                }
                Ok(Self::EditItem { item_id, patch })
            }
            "commit_review" | "commit_ingestion_review" => {
                require_app("ingestion_review")?;
                Ok(Self::CommitReview)
            }
            "cancel_review" | "cancel_ingestion_review" => {
                require_app("ingestion_review")?;
                Ok(Self::CancelReview)
            }
            "archive_candidates" => {
                require_app("lifecycle")?;
                if input.target_ids.is_empty() {
                    return Err(MemoryError::Validation(
                        "`target_ids` is required for archive_candidates".to_string(),
                    ));
                }
                LifecycleOperation::ArchiveCandidates
                    .validate_confirmation(input.dry_run, input.confirmed)?;
                Ok(Self::Lifecycle(LifecycleCommand::ArchiveCandidates {
                    target_ids: input.target_ids,
                    dry_run: input.dry_run,
                    confirmed: input.confirmed,
                }))
            }
            "restore_archived" => {
                require_app("lifecycle")?;
                if input.target_ids.is_empty() {
                    return Err(MemoryError::Validation(
                        "`target_ids` is required for restore_archived".to_string(),
                    ));
                }
                LifecycleOperation::RestoreArchived
                    .validate_confirmation(false, input.confirmed)?;
                Ok(Self::Lifecycle(LifecycleCommand::RestoreArchived {
                    target_ids: input.target_ids,
                    confirmed: input.confirmed,
                }))
            }
            "recompute_decay" => {
                require_app("lifecycle")?;
                LifecycleOperation::RecomputeDecay
                    .validate_confirmation(input.dry_run, input.confirmed)?;
                Ok(Self::Lifecycle(LifecycleCommand::RecomputeDecay {
                    dry_run: input.dry_run,
                    confirmed: input.confirmed,
                }))
            }
            "rebuild_communities" => {
                require_app("lifecycle")?;
                LifecycleOperation::RebuildCommunities
                    .validate_confirmation(input.dry_run, input.confirmed)?;
                Ok(Self::Lifecycle(LifecycleCommand::RebuildCommunities {
                    dry_run: input.dry_run,
                    confirmed: input.confirmed,
                }))
            }
            "export_diff" => {
                require_app("diff")?;
                let format = input.format.ok_or_else(|| {
                    MemoryError::Validation("`format` is required for export_diff".to_string())
                })?;
                Ok(Self::ExportDiff { format })
            }
            "expand_neighbors" => {
                require_app("graph")?;
                let target_id = input.target_id.ok_or_else(|| {
                    MemoryError::Validation(
                        "`target_id` is required for expand_neighbors".to_string(),
                    )
                })?;
                let direction = input.direction.ok_or_else(|| {
                    MemoryError::Validation(
                        "`direction` is required for expand_neighbors".to_string(),
                    )
                })?;
                Ok(Self::ExpandNeighbors {
                    target_id,
                    direction,
                    depth: input.depth.unwrap_or(1).max(1),
                })
            }
            "open_edge_details" => {
                require_app("graph")?;
                Ok(Self::OpenEdgeDetails {
                    edge_id: input.target_id.ok_or_else(|| {
                        MemoryError::Validation(
                            "`target_id` is required for open_edge_details".to_string(),
                        )
                    })?,
                })
            }
            "use_path_as_context" => {
                require_app("graph")?;
                Ok(Self::UsePathAsContext {
                    path_id: input.target_id.unwrap_or_else(|| "current".to_string()),
                })
            }
            "close_session" => Ok(Self::CloseSession),
            _ => Err(MemoryError::Validation(format!(
                "Unsupported app action: {action}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_cross_app_and_missing_confirmation() {
        let error = AppCommand::parse(
            "graph",
            AppCommandInput {
                action: "archive_candidates".to_string(),
                target_ids: vec!["episode:1".to_string()],
                ..Default::default()
            },
        )
        .expect_err("cross-app action must fail");
        assert!(error.to_string().contains("lifecycle"));

        let error = AppCommand::parse(
            "lifecycle",
            AppCommandInput {
                action: "recompute_decay".to_string(),
                ..Default::default()
            },
        )
        .expect_err("destructive command must require confirmation");
        assert!(error.to_string().contains("confirmed=true"));
    }
}
