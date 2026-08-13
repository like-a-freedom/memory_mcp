//! Typed command-dispatch table for the `app_command` MCP tool (ADR-0023).
//!
//! Each app command declares a static [`AppCommandDescriptor`] carrying its
//! action aliases, its owning app, and a typed executor. Adding or renaming a
//! command is a one-row change here; the MCP adapter (`mcp/handlers.rs`) no
//! longer contains a per-command match arm.

use std::future::Future;
use std::pin::Pin;

use rmcp::ErrorData;
use serde_json::Value;
use serde_json::json;

use super::workflow::AppCommand;
use super::{LifecycleCommand, LifecycleCommandOutcome};
use crate::service::apps::ingestion_review::{
    apply_ingestion_review_edit, apply_ingestion_review_status,
};
use crate::service::apps::lifecycle::execute_lifecycle_command;
use crate::service::error::MemoryError;
use crate::service::{CommitIngestionReviewRequest, IngestionReviewItem, MemoryService};
use crate::tools::parsers::parse_datetime;

/// The handler-facing dependencies a command executor needs: the parsed
/// command, the owning service, and the session-scoped view of the world.
///
/// Session mutations flow back through the returned [`AppCommandOutcome`]:
/// any executor that changes the session payload places the replacement on
/// `new_payload`; the handler persists it via the session manager. Sessions
/// that must terminate set `close_session`, which removes the session after
/// the response has been shaped.
pub struct AppContext<'a> {
    pub service: &'a MemoryService,
    pub session_id: &'a str,
    pub app: &'a str,
    pub payload: Value,
}

/// The shaped, protocol-neutral result of executing a command. The MCP
/// adapter converts this into an `AppCommandResult`.
pub struct AppCommandOutcome {
    pub action: &'static str,
    /// Replacement session payload, if the command mutated session state.
    pub new_payload: Option<Value>,
    /// Whether this command closes the session (commit/cancel/close_session).
    pub close_session: bool,
    pub details: Value,
}

impl AppCommandOutcome {
    fn persist(
        action: &'static str,
        new_payload: Value,
        refresh_required: bool,
        details: Value,
    ) -> Self {
        // refresh_required is encoded in `details` for the response; it is
        // also embedded in the persisted session payload via `meta`.
        let _ = refresh_required;
        Self {
            action,
            new_payload: Some(new_payload),
            close_session: false,
            details,
        }
    }

    fn closed(action: &'static str, refresh_required: bool, details: Value) -> Self {
        let _ = refresh_required;
        Self {
            action,
            new_payload: None,
            close_session: true,
            details,
        }
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

type ExecuteFn = for<'a> fn(
    &'a AppContext<'a>,
    &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>>;

/// A static command row. The match on `names` is what the MCP adapter uses
/// for dispatch; `app` documents the owning application (kept for
/// observability and tests — `AppCommand::parse` is the authority that a
/// command is only constructed in the right app context).
pub struct AppCommandDescriptor {
    pub names: &'static [&'static str],
    #[allow(dead_code)] // asserted by dispatch tests, not read on the hot path
    pub app: &'static str,
    pub execute: ExecuteFn,
}

pub const COMMAND_TABLE: &[AppCommandDescriptor] = &[
    AppCommandDescriptor {
        names: &["approve_items", "approve_ingestion_items"],
        app: "ingestion_review",
        execute: execute_approve_items,
    },
    AppCommandDescriptor {
        names: &["reject_items", "reject_ingestion_items"],
        app: "ingestion_review",
        execute: execute_reject_items,
    },
    AppCommandDescriptor {
        names: &["edit_item"],
        app: "ingestion_review",
        execute: execute_edit_item,
    },
    AppCommandDescriptor {
        names: &["commit_review", "commit_ingestion_review"],
        app: "ingestion_review",
        execute: execute_commit_review,
    },
    AppCommandDescriptor {
        names: &["cancel_review", "cancel_ingestion_review"],
        app: "ingestion_review",
        execute: execute_cancel_review,
    },
    AppCommandDescriptor {
        names: &["archive_candidates"],
        app: "lifecycle",
        execute: execute_archive_candidates,
    },
    AppCommandDescriptor {
        names: &["restore_archived"],
        app: "lifecycle",
        execute: execute_restore_archived,
    },
    AppCommandDescriptor {
        names: &["recompute_decay"],
        app: "lifecycle",
        execute: execute_recompute_decay,
    },
    AppCommandDescriptor {
        names: &["rebuild_communities"],
        app: "lifecycle",
        execute: execute_rebuild_communities,
    },
    AppCommandDescriptor {
        names: &["export_diff"],
        app: "diff",
        execute: execute_export_diff,
    },
    AppCommandDescriptor {
        names: &["expand_neighbors"],
        app: "graph",
        execute: execute_expand_neighbors,
    },
    AppCommandDescriptor {
        names: &["open_edge_details"],
        app: "graph",
        execute: execute_open_edge_details,
    },
    AppCommandDescriptor {
        names: &["use_path_as_context"],
        app: "graph",
        execute: execute_use_path_as_context,
    },
    AppCommandDescriptor {
        names: &["close_session"],
        app: "any",
        execute: execute_close_session,
    },
];

/// Look up the descriptor for a validated command. Because `parse` has
/// already rejected unknown actions, a miss here means the table and the
/// parser drifted — a programming error, reported as internal rather than
/// silently falling through.
pub fn find_descriptor(command: &AppCommand) -> Result<&'static AppCommandDescriptor, ErrorData> {
    let name = command.action_name();
    if let Some(descriptor) = COMMAND_TABLE.iter().find(|d| d.names.contains(&name)) {
        return Ok(descriptor);
    }
    Err(internal(format!(
        "no command descriptor registered for validated action `{name}`"
    )))
}

fn internal(message: impl Into<String>) -> ErrorData {
    crate::mcp::session::internal_error(message)
}

fn invalid_params(message: impl Into<String>) -> ErrorData {
    crate::mcp::session::invalid_params(message)
}

fn mcp_error(err: MemoryError) -> ErrorData {
    crate::mcp::mcp_error(err)
}

fn upsert_json_field(payload: &mut Value, key: &str, value: Value) {
    if let Some(object) = payload.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

/// Shared tail for ingestion-review payload edits: decode items, run the
/// domain transform, re-encode, persist, and return the summary.
async fn edit_session_items<F>(ctx: &AppContext<'_>, mutate: F) -> Result<(Value, Value), ErrorData>
where
    F: FnOnce(&mut Vec<IngestionReviewItem>) -> Result<Value, ErrorData>,
{
    let mut payload = ctx.payload.clone();
    let Some(items) = payload.get_mut("items").and_then(Value::as_array_mut) else {
        return Err(internal("ingestion review session is missing items"));
    };
    let mut typed_items: Vec<IngestionReviewItem> =
        serde_json::from_value(Value::Array(items.clone()))
            .map_err(|error| internal(format!("invalid ingestion review items: {error}")))?;
    let summary = mutate(&mut typed_items)?;
    *items = serde_json::to_value(&typed_items)
        .map_err(|error| internal(format!("failed to encode ingestion review items: {error}")))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    upsert_json_field(&mut payload, "summary", summary.clone());
    Ok((payload, summary))
}

/// Shared tail for lifecycle commands: run the domain command, rebuild the
/// enriched session payload, and shape the outcome details.
async fn run_lifecycle(
    ctx: &AppContext<'_>,
    command: LifecycleCommand,
    action: &'static str,
) -> Result<(AppCommandOutcome, LifecycleCommandOutcome, Value), ErrorData> {
    let outcome = execute_lifecycle_command(ctx.service, command)
        .await
        .map_err(mcp_error)?;
    let lifecycle_value = serde_json::to_value(
        ctx.service
            .build_lifecycle_view()
            .await
            .map_err(mcp_error)?,
    )
    .map_err(|error| internal(format!("failed to encode lifecycle view: {error}")))?;
    let enriched = crate::mcp::session::enrich_session_payload(
        ctx.app,
        ctx.session_id,
        ctx.payload
            .get("meta")
            .and_then(|meta| meta.get("ttl_seconds"))
            .and_then(Value::as_i64),
        lifecycle_value,
    );
    let dashboard_value = enriched.get("dashboard").cloned().unwrap_or(Value::Null);
    Ok((
        AppCommandOutcome::persist(action, enriched, true, json!({})),
        outcome,
        dashboard_value,
    ))
}

fn lifecycle_details(
    action: &'static str,
    outcome: LifecycleCommandOutcome,
    payload_dashboard: Value,
) -> Value {
    match (action, outcome) {
        ("archive_candidates", LifecycleCommandOutcome::ArchiveCandidates(o)) => json!({
            "ok": true,
            "message": if o.dry_run {
                format!("Dry-run ready to archive {} candidate(s)", o.target_ids.len())
            } else {
                format!("Archived {} candidate(s)", o.target_ids.len())
            },
            "refresh_required": true,
            "dry_run": o.dry_run,
            "target_ids": o.target_ids,
            "archived_count": o.archived_count,
            "dashboard": payload_dashboard,
        }),
        ("restore_archived", LifecycleCommandOutcome::RestoreArchived(o)) => json!({
            "ok": true,
            "message": format!("Restored {} archived episode(s)", o.restored_count),
            "refresh_required": true,
            "target_ids": o.target_ids,
            "restored_count": o.restored_count,
            "dashboard": payload_dashboard,
        }),
        ("recompute_decay", LifecycleCommandOutcome::RecomputeDecay(o)) => json!({
            "ok": true,
            "message": if o.dry_run {
                "Dry-run decay recomputation refreshed lifecycle metrics".to_string()
            } else {
                format!("Recomputed decay and invalidated {} fact(s)", o.invalidated)
            },
            "refresh_required": true,
            "dry_run": o.dry_run,
            "invalidated": o.invalidated,
            "dashboard": payload_dashboard,
        }),
        ("rebuild_communities", LifecycleCommandOutcome::RebuildCommunities(o)) => json!({
            "ok": true,
            "message": if o.dry_run {
                "Dry-run community rebuild refreshed lifecycle metrics".to_string()
            } else {
                format!("Rebuilt {} community record(s)", o.rebuilt)
            },
            "refresh_required": true,
            "dry_run": o.dry_run,
            "rebuilt": o.rebuilt,
            "dashboard": payload_dashboard,
        }),
        _ => Value::Null,
    }
}

fn execute_approve_items<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::ApproveItems { item_ids } = cmd else {
            return Err(internal("validated app command did not match action"));
        };
        let (payload, summary) = edit_session_items(ctx, |items| {
            serde_json::to_value(apply_ingestion_review_status(
                items, item_ids, "approved", None,
            ))
            .map_err(|error| {
                internal(format!(
                    "failed to encode ingestion review summary: {error}"
                ))
            })
        })
        .await?;
        Ok(AppCommandOutcome::persist(
            "approve_items",
            payload,
            true,
            json!({
                "ok": true,
                "message": format!("Approved {} ingestion review item(s)", item_ids.len()),
                "refresh_required": true,
                "updated_item_ids": item_ids,
                "summary": summary,
            }),
        ))
    })
}

fn execute_reject_items<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::RejectItems { item_ids, reason } = cmd else {
            return Err(internal("validated app command did not match action"));
        };
        let reason = reason.clone();
        let (payload, summary) = edit_session_items(ctx, |items| {
            serde_json::to_value(apply_ingestion_review_status(
                items,
                item_ids,
                "rejected",
                Some(reason.as_str()),
            ))
            .map_err(|error| {
                internal(format!(
                    "failed to encode ingestion review summary: {error}"
                ))
            })
        })
        .await?;
        Ok(AppCommandOutcome::persist(
            "reject_items",
            payload,
            true,
            json!({
                "ok": true,
                "message": format!("Rejected {} ingestion review item(s)", item_ids.len()),
                "refresh_required": true,
                "updated_item_ids": item_ids,
                "summary": summary,
            }),
        ))
    })
}

fn execute_edit_item<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::EditItem { item_id, patch } = cmd else {
            return Err(internal("validated app command did not match action"));
        };
        let (payload, summary) = edit_session_items(ctx, |items| {
            let summary = apply_ingestion_review_edit(items, item_id, patch).map_err(mcp_error)?;
            serde_json::to_value(summary).map_err(|error| {
                internal(format!(
                    "failed to encode ingestion review summary: {error}"
                ))
            })
        })
        .await?;
        Ok(AppCommandOutcome::persist(
            "edit_item",
            payload,
            true,
            json!({
                "ok": true,
                "message": format!("Edited ingestion review item {item_id}"),
                "refresh_required": true,
                "item_id": item_id,
                "summary": summary,
            }),
        ))
    })
}

fn execute_commit_review<'a>(
    ctx: &'a AppContext<'a>,
    _cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let items_value = ctx
            .payload
            .get("items")
            .cloned()
            .ok_or_else(|| internal("ingestion review session is missing items"))?;
        let items: Vec<IngestionReviewItem> =
            serde_json::from_value(items_value).map_err(|error| {
                internal(format!("failed to decode ingestion review items: {error}"))
            })?;
        let outcome = ctx
            .service
            .commit_ingestion_review(CommitIngestionReviewRequest { items })
            .await
            .map_err(mcp_error)?;
        Ok(AppCommandOutcome::closed(
            "commit_review",
            false,
            json!({
                "ok": true,
                "message": format!(
                    "Committed {} approved review item(s) and closed the session",
                    outcome.committed_count
                ),
                "refresh_required": false,
                "committed_count": outcome.committed_count,
                "fact_ids": outcome.fact_ids,
            }),
        ))
    })
}

fn execute_cancel_review<'a>(
    _ctx: &'a AppContext<'a>,
    _cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        Ok(AppCommandOutcome::closed(
            "cancel_review",
            false,
            json!({
                "ok": true,
                "message": "Cancelled review and closed the session",
                "refresh_required": false,
            }),
        ))
    })
}

fn execute_archive_candidates<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::Lifecycle(LifecycleCommand::ArchiveCandidates {
            target_ids,
            dry_run,
            confirmed,
        }) = cmd
        else {
            return Err(internal("validated app command did not match action"));
        };
        let command = LifecycleCommand::ArchiveCandidates {
            target_ids: target_ids.clone(),
            dry_run: *dry_run,
            confirmed: *confirmed,
        };
        let (shaped, outcome, dashboard) =
            run_lifecycle(ctx, command, "archive_candidates").await?;
        let details = lifecycle_details("archive_candidates", outcome, dashboard);
        Ok(AppCommandOutcome::persist(
            "archive_candidates",
            shaped.new_payload.unwrap_or_else(|| ctx.payload.clone()),
            true,
            details,
        ))
    })
}

fn execute_restore_archived<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::Lifecycle(LifecycleCommand::RestoreArchived {
            target_ids,
            confirmed,
        }) = cmd
        else {
            return Err(internal("validated app command did not match action"));
        };
        let command = LifecycleCommand::RestoreArchived {
            target_ids: target_ids.clone(),
            confirmed: *confirmed,
        };
        let (shaped, outcome, dashboard) = run_lifecycle(ctx, command, "restore_archived").await?;
        let details = lifecycle_details("restore_archived", outcome, dashboard);
        Ok(AppCommandOutcome::persist(
            "restore_archived",
            shaped.new_payload.unwrap_or_else(|| ctx.payload.clone()),
            true,
            details,
        ))
    })
}

fn execute_recompute_decay<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::Lifecycle(LifecycleCommand::RecomputeDecay { dry_run, confirmed }) = cmd
        else {
            return Err(internal("validated app command did not match action"));
        };
        let command = LifecycleCommand::RecomputeDecay {
            dry_run: *dry_run,
            confirmed: *confirmed,
        };
        let (shaped, outcome, dashboard) = run_lifecycle(ctx, command, "recompute_decay").await?;
        let details = lifecycle_details("recompute_decay", outcome, dashboard);
        Ok(AppCommandOutcome::persist(
            "recompute_decay",
            shaped.new_payload.unwrap_or_else(|| ctx.payload.clone()),
            true,
            details,
        ))
    })
}

fn execute_rebuild_communities<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::Lifecycle(LifecycleCommand::RebuildCommunities { dry_run, confirmed }) =
            cmd
        else {
            return Err(internal("validated app command did not match action"));
        };
        let command = LifecycleCommand::RebuildCommunities {
            dry_run: *dry_run,
            confirmed: *confirmed,
        };
        let (shaped, outcome, dashboard) =
            run_lifecycle(ctx, command, "rebuild_communities").await?;
        let details = lifecycle_details("rebuild_communities", outcome, dashboard);
        Ok(AppCommandOutcome::persist(
            "rebuild_communities",
            shaped.new_payload.unwrap_or_else(|| ctx.payload.clone()),
            true,
            details,
        ))
    })
}

fn execute_export_diff<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::ExportDiff { format } = cmd else {
            return Err(internal("validated app command did not match action"));
        };
        let export = json!({
            "format": format,
            "generated_at": chrono::Utc::now().to_rfc3339(),
            "target": ctx.payload.get("target").cloned().unwrap_or(Value::Null),
            "range": ctx.payload.get("range").cloned().unwrap_or(Value::Null),
        });
        let mut payload = ctx.payload.clone();
        if let Some(object) = payload.as_object_mut() {
            object.insert("last_export".to_string(), export.clone());
            object
                .entry("exports".to_string())
                .or_insert_with(|| json!([]));
            if let Some(exports) = object.get_mut("exports").and_then(Value::as_array_mut) {
                exports.push(export.clone());
            }
        }
        Ok(AppCommandOutcome::persist(
            "export_diff",
            payload,
            true,
            json!({
                "ok": true,
                "message": format!("Prepared {format} diff export"),
                "refresh_required": true,
                "export": export,
            }),
        ))
    })
}

fn execute_expand_neighbors<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::ExpandNeighbors {
            target_id,
            direction,
            depth,
        } = cmd
        else {
            return Err(internal("validated app command did not match action"));
        };
        let cutoff = ctx
            .payload
            .get("target")
            .and_then(|target| target.get("as_of"))
            .and_then(Value::as_str)
            .and_then(parse_datetime)
            .unwrap_or_else(chrono::Utc::now);
        let expansion = crate::service::graph_neighbor_expansion(
            &ctx.service.app_store(),
            target_id,
            direction,
            *depth,
            cutoff,
        )
        .await
        .map_err(mcp_error)?;
        let mut payload = ctx.payload.clone();
        if let Some(expansions) = payload.get_mut("expansions").and_then(Value::as_array_mut) {
            expansions.push(expansion.clone());
        } else {
            upsert_json_field(&mut payload, "expansions", json!([expansion.clone()]));
        }
        Ok(AppCommandOutcome::persist(
            "expand_neighbors",
            payload,
            true,
            json!({
                "ok": true,
                "message": format!("Expanded {direction} neighbors for {target_id}"),
                "refresh_required": true,
                "expansion": expansion,
            }),
        ))
    })
}

fn execute_open_edge_details<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::OpenEdgeDetails { edge_id } = cmd else {
            return Err(internal("validated app command did not match action"));
        };
        let edge = ctx
            .service
            .app_store()
            .select_edge(edge_id)
            .await
            .map_err(mcp_error)?
            .ok_or_else(|| invalid_params(format!("Unknown graph edge: {edge_id}")))?;
        let mut payload = ctx.payload.clone();
        upsert_json_field(&mut payload, "selected_edge", edge.clone());
        Ok(AppCommandOutcome::persist(
            "open_edge_details",
            payload,
            true,
            json!({
                "ok": true,
                "message": format!("Loaded edge details for {edge_id}"),
                "refresh_required": true,
                "details": edge,
            }),
        ))
    })
}

fn execute_use_path_as_context<'a>(
    ctx: &'a AppContext<'a>,
    cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        let AppCommand::UsePathAsContext { path_id } = cmd else {
            return Err(internal("validated app command did not match action"));
        };
        let node_names = ctx
            .payload
            .get("graph")
            .and_then(|graph| graph.get("nodes"))
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|node| {
                        node.get("canonical_name")
                            .or_else(|| node.get("entity_id"))
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let preview = json!({
            "path_id": path_id,
            "summary": if node_names.is_empty() {
                format!("Graph context for {path_id}")
            } else {
                format!("Path context: {}", node_names.join(" -> "))
            },
            "node_names": node_names,
        });
        let mut payload = ctx.payload.clone();
        upsert_json_field(&mut payload, "context_preview", preview.clone());
        Ok(AppCommandOutcome::persist(
            "use_path_as_context",
            payload,
            true,
            json!({
                "ok": true,
                "message": "Prepared graph path context",
                "refresh_required": true,
                "context_preview": preview,
            }),
        ))
    })
}

fn execute_close_session<'a>(
    _ctx: &'a AppContext<'a>,
    _cmd: &'a AppCommand,
) -> BoxFuture<'a, Result<AppCommandOutcome, ErrorData>> {
    Box::pin(async move {
        Ok(AppCommandOutcome::closed(
            "close_session",
            false,
            json!({
                "ok": true,
                "message": "Session closed",
                "refresh_required": false,
            }),
        ))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_parsed_action_has_a_descriptor() {
        // The exhaustive set of actions AppCommand::parse accepts.
        for action in [
            "approve_items",
            "reject_items",
            "edit_item",
            "commit_review",
            "cancel_review",
            "archive_candidates",
            "restore_archived",
            "recompute_decay",
            "rebuild_communities",
            "export_diff",
            "expand_neighbors",
            "open_edge_details",
            "use_path_as_context",
            "close_session",
        ] {
            assert!(
                COMMAND_TABLE.iter().any(|d| d.names.contains(&action)),
                "missing command descriptor for `{action}`"
            );
        }
    }

    #[test]
    fn descriptor_app_fields_match_parser_policy() {
        for descriptor in COMMAND_TABLE {
            for &name in descriptor.names {
                let input = crate::service::apps::workflow::AppCommandInput {
                    action: name.to_string(),
                    item_ids: vec!["item:1".into()],
                    target_ids: vec!["episode:1".into()],
                    target_id: Some("entity:a".into()),
                    item_id: Some("item:1".into()),
                    patch_json: Some("{}".into()),
                    reason: None,
                    dry_run: false,
                    confirmed: true,
                    format: Some("json".into()),
                    direction: Some("outgoing".into()),
                    depth: Some(1),
                };
                let parsed = AppCommand::parse(descriptor.app, input);
                assert!(
                    parsed.is_ok(),
                    "descriptor `{name}@{}` failed to parse under its own app",
                    descriptor.app
                );
            }
        }
    }

    #[test]
    fn find_descriptor_covers_all_parsed_variants() {
        // Construct one real command per variant and resolve it.
        let cases: Vec<(&str, AppCommand)> = vec![
            (
                "approve_items",
                AppCommand::ApproveItems { item_ids: vec![] },
            ),
            (
                "reject_items",
                AppCommand::RejectItems {
                    item_ids: vec![],
                    reason: "r".into(),
                },
            ),
            (
                "edit_item",
                AppCommand::EditItem {
                    item_id: "i".into(),
                    patch: json!({}),
                },
            ),
            ("commit_review", AppCommand::CommitReview),
            ("cancel_review", AppCommand::CancelReview),
            (
                "archive_candidates",
                AppCommand::Lifecycle(LifecycleCommand::ArchiveCandidates {
                    target_ids: vec![],
                    dry_run: true,
                    confirmed: true,
                }),
            ),
            (
                "restore_archived",
                AppCommand::Lifecycle(LifecycleCommand::RestoreArchived {
                    target_ids: vec![],
                    confirmed: true,
                }),
            ),
            (
                "recompute_decay",
                AppCommand::Lifecycle(LifecycleCommand::RecomputeDecay {
                    dry_run: true,
                    confirmed: true,
                }),
            ),
            (
                "rebuild_communities",
                AppCommand::Lifecycle(LifecycleCommand::RebuildCommunities {
                    dry_run: true,
                    confirmed: true,
                }),
            ),
            (
                "export_diff",
                AppCommand::ExportDiff {
                    format: "json".into(),
                },
            ),
            (
                "expand_neighbors",
                AppCommand::ExpandNeighbors {
                    target_id: "e".into(),
                    direction: "outgoing".into(),
                    depth: 1,
                },
            ),
            (
                "open_edge_details",
                AppCommand::OpenEdgeDetails {
                    edge_id: "e".into(),
                },
            ),
            (
                "use_path_as_context",
                AppCommand::UsePathAsContext {
                    path_id: "p".into(),
                },
            ),
            ("close_session", AppCommand::CloseSession),
        ];
        for (expected, command) in cases {
            let descriptor = find_descriptor(&command).expect("descriptor");
            assert!(
                descriptor.names.contains(&expected),
                "descriptor for `{expected}` registered for {:?}",
                descriptor.names
            );
        }
    }
}
