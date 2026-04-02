//! APP modules extracted from core.rs for SRP compliance.
//!
//! This module contains all MCP APP implementations:
//! - APP-01: Inspector (entity, fact, episode views)
//! - APP-02: Temporal Diff
//! - APP-03: Ingestion Review
//! - APP-04: Lifecycle Console
//! - APP-05: Graph Path Explorer

use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::IngestRequest;
use crate::service::core::{
    AddFactRequest, MemoryService, draft_entity_candidate, draft_payload_datetime,
    draft_payload_f64, draft_payload_str, draft_payload_string_array, resolve_draft_reference,
    string_from_value,
};
use crate::service::error::MemoryError;
use crate::storage::json_f64;
use std::collections::{BTreeSet, HashMap};

impl MemoryService {
    pub async fn open_inspector_entity(
        &self,
        entity_id: &str,
        scope: &str,
        page_size: usize,
        _cursor: Option<&str>,
    ) -> Result<serde_json::Value, MemoryError> {
        let namespace = self.resolve_namespace_for_scope(scope)?;
        let record = self.db_client.select_one(entity_id, &namespace).await?;
        let Some(record) = record else {
            return Err(MemoryError::NotFound(format!(
                "entity not found: {entity_id}"
            )));
        };

        let entity_name = record
            .get("canonical_name")
            .and_then(string_from_value)
            .unwrap_or_default();

        // Fetch related facts
        let facts_sql = "SELECT fact_id, content, confidence, t_valid, t_invalid \
                         FROM fact WHERE entity_links CONTAINS $entity_id \
                         ORDER BY confidence DESC LIMIT $limit";
        let facts_result = self
            .db_client
            .query(
                facts_sql,
                Some(json!({"entity_id": entity_id, "limit": page_size as i64})),
                &namespace,
            )
            .await?;

        let facts: Vec<serde_json::Value> = facts_result
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let m = v.as_object()?;
                        let fact_id = m.get("fact_id").and_then(string_from_value)?;
                        let content = m
                            .get("content")
                            .and_then(string_from_value)
                            .unwrap_or_default();
                        let confidence = m.get("confidence").and_then(json_f64).unwrap_or(0.0);
                        let state = Self::fact_state(&json!(m));
                        Some(json!({
                            "fact_id": fact_id,
                            "content": content,
                            "confidence": confidence,
                            "state": state,
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Fetch edges
        let edges_sql = "SELECT edge_id, relation, out, strength, confidence, t_valid, t_invalid \
                         FROM edge WHERE in = <record> $entity_id LIMIT 20";
        let edges_result = self
            .db_client
            .query(edges_sql, Some(json!({"entity_id": entity_id})), &namespace)
            .await?;

        let edges: Vec<serde_json::Value> = edges_result
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let m = v.as_object()?;
                        let relation = m.get("relation").and_then(string_from_value)?;
                        let target = m.get("out").and_then(string_from_value).unwrap_or_default();
                        let confidence = m.get("confidence").and_then(json_f64).unwrap_or(0.0);
                        Some(json!({
                            "relation": relation,
                            "target_id": target,
                            "confidence": confidence,
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let has_more = facts_result
            .as_array()
            .map(|a| a.len() >= page_size)
            .unwrap_or(false);

        Ok(json!({
            "session_id": null,
            "target_type": "entity",
            "target_id": entity_id,
            "entity": {
                "entity_id": entity_id,
                "canonical_name": entity_name,
                "entity_type": record.get("entity_type").and_then(string_from_value).unwrap_or_default(),
                "aliases": record.get("aliases").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default(),
            },
            "facts": facts,
            "edges": edges,
            "pagination": {
                "has_more": has_more,
                "total_visible": facts.len(),
            },
        }))
    }

    /// Opens inspector view for a fact (APP-01, FR-INS-03).
    pub async fn open_inspector_fact(
        &self,
        fact_id: &str,
        scope: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let namespace = self.resolve_namespace_for_scope(scope)?;
        let record = self.db_client.select_one(fact_id, &namespace).await?;
        let Some(record) = record else {
            return Err(MemoryError::NotFound(format!("fact not found: {fact_id}")));
        };

        let confidence = record.get("confidence").and_then(json_f64).unwrap_or(0.0);
        let t_valid_str = record
            .get("t_valid")
            .and_then(string_from_value)
            .unwrap_or_default();

        let state = Self::fact_state(&record);

        Ok(json!({
            "session_id": null,
            "target_type": "fact",
            "target_id": fact_id,
            "fact": {
                "fact_id": fact_id,
                "fact_type": record.get("fact_type").and_then(string_from_value).unwrap_or_default(),
                "content": record.get("content").and_then(string_from_value).unwrap_or_default(),
                "quote": record.get("quote").and_then(string_from_value).unwrap_or_default(),
                "source_episode": record.get("source_episode").and_then(string_from_value).unwrap_or_default(),
                "confidence": confidence,
                "provenance": record.get("provenance").cloned().unwrap_or(json!({})),
                "t_valid": t_valid_str,
                "t_ingested": record.get("t_ingested").and_then(string_from_value).unwrap_or_default(),
                "t_invalid": record.get("t_invalid").and_then(string_from_value),
                "state": state,
            },
        }))
    }

    /// Opens inspector view for an episode (APP-01, FR-INS-04).
    pub async fn open_inspector_episode(
        &self,
        episode_id: &str,
        scope: &str,
        page_size: usize,
        _cursor: Option<&str>,
    ) -> Result<serde_json::Value, MemoryError> {
        let namespace = self.resolve_namespace_for_scope(scope)?;
        let record = self.db_client.select_one(episode_id, &namespace).await?;
        let Some(record) = record else {
            return Err(MemoryError::NotFound(format!(
                "episode not found: {episode_id}"
            )));
        };

        let archived_at = record.get("archived_at").and_then(string_from_value);
        let status = if archived_at.is_some() {
            "archived"
        } else {
            "active"
        };

        // Fetch related facts
        let facts_sql = "SELECT fact_id, content, confidence \
                         FROM fact WHERE source_episode = $episode_id \
                         ORDER BY confidence DESC LIMIT $limit";
        let facts_result = self
            .db_client
            .query(
                facts_sql,
                Some(json!({"episode_id": episode_id, "limit": page_size as i64})),
                &namespace,
            )
            .await?;

        let facts: Vec<serde_json::Value> = facts_result
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let m = v.as_object()?;
                        Some(json!({
                            "fact_id": m.get("fact_id").and_then(string_from_value)?,
                            "content": m.get("content").and_then(string_from_value).unwrap_or_default(),
                            "confidence": m.get("confidence").and_then(json_f64).unwrap_or(0.0),
                            "state": "active",
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(json!({
            "session_id": null,
            "target_type": "episode",
            "target_id": episode_id,
            "episode": {
                "episode_id": episode_id,
                "source_type": record.get("source_type").and_then(string_from_value).unwrap_or_default(),
                "source_id": record.get("source_id").and_then(string_from_value).unwrap_or_default(),
                "t_ref": record.get("t_ref").and_then(string_from_value).unwrap_or_default(),
                "t_ingested": record.get("t_ingested").and_then(string_from_value).unwrap_or_default(),
                "status": status,
                "archived_at": archived_at,
            },
            "facts": facts,
        }))
    }

    /// Archives an episode (APP-01, FR-INS-06).
    pub async fn archive_episode(&self, episode_id: &str) -> Result<(), MemoryError> {
        let (record, namespace) = self.find_episode_record(episode_id).await?;
        let namespace =
            namespace.ok_or_else(|| MemoryError::NotFound("episode not found".into()))?;
        let mut updated =
            record.ok_or_else(|| MemoryError::NotFound("episode not found".into()))?;

        let archived_at = super::normalize_dt(self.now());
        updated.insert("archived_at".to_string(), json!(archived_at));
        updated.insert("status".to_string(), json!("archived"));

        self.db_client
            .update(episode_id, Value::Object(updated), &namespace)
            .await?;

        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), json!("archive_episode"));
        event.insert("args".to_string(), json!({"episode_id": episode_id}));
        event.insert("result".to_string(), json!({"status": "archived"}));
        self.logger.log(event, LogLevel::Info);

        Ok(())
    }

    /// Closes an app session (FR-COM-08).
    pub async fn close_app_session(&self, session_id: &str) -> Result<(), MemoryError> {
        self.app_session_manager.close_session(session_id).await
    }
    // --- MCP Apps: APP-02 Temporal Diff ---

    #[allow(clippy::too_many_arguments)]
    /// Opens a temporal diff session (APP-02, FR-DIFF-01).
    pub async fn open_temporal_diff(
        &self,
        scope: &str,
        target_type: &str,
        target_id: Option<&str>,
        as_of_left: &str,
        as_of_right: &str,
        time_axis: &str,
        filters: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, MemoryError> {
        let namespace = self.resolve_namespace_for_scope(scope)?;

        let result = if target_type == "entity" {
            let Some(entity_id) = target_id else {
                return Err(MemoryError::InvalidParameter(
                    "entity target requires target_id".to_string(),
                ));
            };
            crate::service::apps::diff::TemporalDiff::compare_entity(
                entity_id,
                scope,
                as_of_left,
                as_of_right,
                time_axis,
                &*self.db_client,
                &namespace,
            )
            .await
        } else {
            crate::service::apps::diff::TemporalDiff::compare_scope(
                scope,
                as_of_left,
                as_of_right,
                time_axis,
                filters,
                &*self.db_client,
                &namespace,
            )
            .await
        }?;

        let diff_json =
            serde_json::to_value(&result).map_err(|e| MemoryError::App(e.to_string()))?;

        let target = serde_json::json!({
            "scope": scope,
            "target_type": target_type,
            "target_id": target_id,
            "as_of_left": as_of_left,
            "as_of_right": as_of_right,
            "time_axis": time_axis,
            "result": diff_json,
        });

        let session = self
            .app_session_manager
            .create_session("temporal_diff", scope, serde_json::json!({}), target, None)
            .await?;

        Ok(serde_json::json!({
            "session_id": session.session_id,
            "app_id": session.app_id,
            "scope": session.scope,
            "state": session.state,
            "result": diff_json,
        }))
    }

    /// Exports temporal diff in requested format (APP-02, FR-DIFF-07).
    pub async fn export_temporal_diff(
        &self,
        session_id: &str,
        format: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "temporal_diff")?;
        let result = session
            .target
            .get("result")
            .ok_or_else(|| MemoryError::App("diff result not found in session".to_string()))?;

        let output = if format == "markdown" {
            let added = result
                .get("added")
                .and_then(|v| v.as_array())
                .map(|arr| arr.len())
                .unwrap_or(0);
            let removed = result
                .get("removed")
                .and_then(|v| v.as_array())
                .map(|arr| arr.len())
                .unwrap_or(0);
            let changed = result
                .get("changed")
                .and_then(|v| v.as_array())
                .map(|arr| arr.len())
                .unwrap_or(0);

            serde_json::json!({
                "format": "markdown",
                "content": format!(
                    "# Temporal Diff\n\n- Added: {}\n- Removed: {}\n- Changed: {}\n\n---\n\n## Details\n\n```json\n{}\n```",
                    added, removed, changed,
                    serde_json::to_string_pretty(result).unwrap_or_default()
                )
            })
        } else {
            serde_json::json!({
                "format": "json",
                "content": result
            })
        };

        self.app_session_manager.touch_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "message": format!("Exported as {}", format),
            "export": output,
        }))
    }

    /// Opens inspector from diff session (APP-02, FR-DIFF-05).
    pub async fn open_memory_inspector_from_diff(
        &self,
        session_id: &str,
        target_id: &str,
        target_type: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        let scope = &session.scope;

        let view = match target_type {
            "entity" => {
                self.open_inspector_entity(target_id, scope, 20, None)
                    .await?
            }
            "fact" => self.open_inspector_fact(target_id, scope).await?,
            "episode" => {
                self.open_inspector_episode(target_id, scope, 20, None)
                    .await?
            }
            _ => {
                return Err(MemoryError::InvalidParameter(format!(
                    "invalid target_type: {}",
                    target_type
                )));
            }
        };

        self.app_session_manager.touch_session(session_id).await?;

        Ok(view)
    }
    // --- MCP Apps: APP-04 Lifecycle Console ---

    /// Opens a lifecycle console session (APP-04, FR-LIFE-01).
    #[allow(clippy::too_many_arguments)]
    pub async fn open_lifecycle_console(
        &self,
        scope: &str,
        filters: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, MemoryError> {
        let view =
            crate::service::apps::lifecycle::LifecycleConsole::load_dashboard(self, scope, filters)
                .await?;

        let target = serde_json::json!({
            "scope": scope,
            "filters": filters,
            "view": view,
        });

        let session = self
            .app_session_manager
            .create_session(
                "lifecycle_console",
                scope,
                serde_json::json!({}),
                target,
                None,
            )
            .await?;

        Ok(serde_json::json!({
            "session_id": session.session_id,
            "app_id": session.app_id,
            "scope": session.scope,
            "state": session.state,
            "view": session.target.get("view").cloned().unwrap_or_else(|| json!({})),
            "message": "Lifecycle console opened",
        }))
    }

    pub async fn get_lifecycle_dashboard(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "lifecycle_console")?;

        let filters = session.target.get("filters");
        let dashboard = crate::service::apps::lifecycle::LifecycleConsole::load_dashboard(
            self,
            &session.scope,
            filters,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(serde_json::json!({
            "filters": filters.cloned().unwrap_or_else(|| json!({})),
            "dashboard": dashboard,
        }))
    }

    /// Archives candidates (APP-04, FR-LIFE-03).
    pub async fn archive_candidates(
        &self,
        session_id: &str,
        candidate_ids: &[String],
        dry_run: bool,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "lifecycle_console")?;

        let result = crate::service::apps::lifecycle::LifecycleConsole::archive_candidates(
            self,
            &session.scope,
            candidate_ids,
            dry_run,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(result)
    }

    /// Restores archived episodes (APP-04, FR-LIFE-03).
    pub async fn restore_archived(
        &self,
        session_id: &str,
        episode_ids: &[String],
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "lifecycle_console")?;

        let result = crate::service::apps::lifecycle::LifecycleConsole::restore_archived(
            self,
            &session.scope,
            episode_ids,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(result)
    }

    /// Recomputes decay (APP-04, FR-LIFE-03).
    pub async fn recompute_decay(
        &self,
        session_id: &str,
        target_ids: Option<&[String]>,
        dry_run: bool,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "lifecycle_console")?;

        let result = crate::service::apps::lifecycle::LifecycleConsole::recompute_decay(
            self,
            &session.scope,
            target_ids,
            dry_run,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(result)
    }

    /// Rebuilds communities (APP-04, FR-LIFE-08).
    pub async fn rebuild_communities(
        &self,
        session_id: &str,
        dry_run: bool,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "lifecycle_console")?;

        let result = crate::service::apps::lifecycle::LifecycleConsole::rebuild_communities(
            self,
            &session.scope,
            dry_run,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(result)
    }

    /// Gets lifecycle task status (APP-04, FR-LIFE-07).
    pub async fn get_lifecycle_task_status(
        &self,
        task_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(serde_json::json!({
            "task_id": task_id,
            "status": "completed",
            "execution_mode": "inline",
            "message": "Lifecycle actions execute inline; no background task is tracked",
        }))
    }

    // --- MCP Apps: APP-05 Graph Path Explorer ---

    /// Opens a graph path explorer session (APP-05, FR-GRAPH-01).
    pub async fn open_graph_path(
        &self,
        scope: &str,
        from_entity_id: &str,
        to_entity_id: &str,
        as_of: Option<&str>,
        max_depth: i32,
    ) -> Result<serde_json::Value, MemoryError> {
        let _namespace = self.resolve_namespace_for_scope(scope)?;
        let as_of_dt = as_of
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        let result = crate::service::apps::graph::GraphPathExplorer::find_path(
            from_entity_id,
            to_entity_id,
            scope,
            as_of_dt,
            Some(max_depth as usize),
            &self.db_client,
        )
        .await?;

        let target = serde_json::json!({
            "scope": scope,
            "from_entity_id": from_entity_id,
            "to_entity_id": to_entity_id,
            "as_of": as_of_dt.to_rfc3339(),
            "result": result,
        });

        let session = self
            .app_session_manager
            .create_session("graph_path", scope, serde_json::json!({}), target, None)
            .await?;

        Ok(serde_json::json!({
            "session_id": session.session_id,
            "app_id": session.app_id,
            "scope": session.scope,
            "state": session.state,
            "path": result.path,
            "path_found": result.path_found,
            "reason_if_empty": result.reason_if_empty,
        }))
    }

    /// Expands graph neighbors (APP-05, FR-GRAPH-04).
    pub async fn expand_graph_neighbors(
        &self,
        session_id: &str,
        entity_id: &str,
        direction: &str,
        depth: i32,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "graph_path")?;

        let namespace = &session.scope;

        let dir = match direction {
            "in" => crate::storage::GraphDirection::Incoming,
            "out" => crate::storage::GraphDirection::Outgoing,
            _ => crate::storage::GraphDirection::Incoming,
        };

        let as_of = self.now();

        let result = crate::service::apps::graph::GraphPathExplorer::expand_neighbors(
            entity_id,
            namespace,
            dir,
            depth as usize,
            as_of,
            &self.db_client,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "entity_id": entity_id,
            "neighbors": result,
        }))
    }

    /// Opens edge details (APP-05, FR-GRAPH-02).
    pub async fn open_edge_details(
        &self,
        session_id: &str,
        edge_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "graph_path")?;

        let namespace = self.resolve_namespace_for_scope(&session.scope)?;

        let record = self.db_client.select_one(edge_id, &namespace).await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "edge_id": edge_id,
            "details": record,
        }))
    }

    /// Uses path as context (APP-05, FR-GRAPH-06).
    pub async fn use_path_as_context(
        &self,
        session_id: &str,
        _path_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "graph_path")?;

        let path_json =
            serde_json::to_string(&session.target).map_err(|e| MemoryError::App(e.to_string()))?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "message": "Path ready for use as context",
            "path_serialized": path_json,
        }))
    }
    // --- MCP Apps: APP-03 Ingestion Review ---

    /// Opens an ingestion review session (APP-03, FR-ING-01).
    pub async fn open_ingestion_review(
        &self,
        scope: &str,
        source_text: Option<&str>,
        draft_episode_id: Option<&str>,
        ttl_seconds: Option<i64>,
    ) -> Result<serde_json::Value, MemoryError> {
        let namespace = self.resolve_namespace_for_scope(scope)?;
        let ttl = ttl_seconds.unwrap_or(86400);

        let draft_id = crate::service::apps::ingestion::IngestionReview::create_draft(
            scope,
            serde_json::json!({}),
            source_text.unwrap_or(""),
            draft_episode_id.unwrap_or(""),
            ttl,
            &*self.db_client,
            &namespace,
        )
        .await?;

        let target = serde_json::json!({
            "draft_id": draft_id,
            "scope": scope,
        });

        let session = self
            .app_session_manager
            .create_session(
                "ingestion_review",
                scope,
                serde_json::json!({}),
                target,
                Some(ttl),
            )
            .await?;

        Ok(serde_json::json!({
            "session_id": session.session_id,
            "app_id": session.app_id,
            "scope": session.scope,
            "state": session.state,
            "draft_id": draft_id,
        }))
    }

    /// Gets draft summary (APP-03, FR-ING-02).
    pub async fn get_draft_summary(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "ingestion_review")?;
        let draft_id = Self::require_target_str(&session, "draft_id")?;

        let namespace = self.resolve_namespace_for_scope(&session.scope)?;

        let summary = crate::service::apps::ingestion::IngestionReview::get_draft_summary(
            draft_id,
            &*self.db_client,
            &namespace,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        serde_json::to_value(summary).map_err(|e| MemoryError::App(e.to_string()))
    }

    /// Approves ingestion items (APP-03, FR-ING-03).
    pub async fn approve_ingestion_items(
        &self,
        session_id: &str,
        item_ids: &[String],
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "ingestion_review")?;
        let draft_id = Self::require_target_str(&session, "draft_id")?;

        let namespace = self.resolve_namespace_for_scope(&session.scope)?;

        crate::service::apps::ingestion::IngestionReview::approve_items(
            draft_id,
            item_ids,
            &*self.db_client,
            &namespace,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "message": format!("Approved {} items", item_ids.len()),
            "refresh_required": true,
        }))
    }

    /// Rejects ingestion items (APP-03, FR-ING-03).
    pub async fn reject_ingestion_items(
        &self,
        session_id: &str,
        item_ids: &[String],
        reason: Option<&str>,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "ingestion_review")?;
        let draft_id = Self::require_target_str(&session, "draft_id")?;

        let namespace = self.resolve_namespace_for_scope(&session.scope)?;

        crate::service::apps::ingestion::IngestionReview::reject_items(
            draft_id,
            item_ids,
            reason.unwrap_or(""),
            &*self.db_client,
            &namespace,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "message": format!("Rejected {} items", item_ids.len()),
            "refresh_required": true,
        }))
    }

    pub(crate) async fn edit_ingestion_item(
        &self,
        session_id: &str,
        item_id: &str,
        patch: &serde_json::Value,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "ingestion_review")?;
        let draft_id = Self::require_target_str(&session, "draft_id")?;

        let namespace = self.resolve_namespace_for_scope(&session.scope)?;

        crate::service::apps::ingestion::IngestionReview::edit_item(
            draft_id,
            item_id,
            patch,
            &*self.db_client,
            &namespace,
        )
        .await?;

        self.app_session_manager.touch_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "message": format!("Edited item {}", item_id),
            "refresh_required": true,
        }))
    }

    /// Cancels an ingestion review (APP-03, FR-ING-04).
    pub async fn cancel_ingestion_review(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "ingestion_review")?;
        let draft_id = Self::require_target_str(&session, "draft_id")?;

        let namespace = self.resolve_namespace_for_scope(&session.scope)?;

        crate::service::apps::ingestion::IngestionReview::cancel_draft(
            draft_id,
            &*self.db_client,
            &namespace,
        )
        .await?;

        self.app_session_manager.close_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "message": "Ingestion review cancelled",
        }))
    }

    /// Commits an ingestion review (APP-03, FR-ING-04).
    pub async fn commit_ingestion_review(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let session = self.app_session_manager.get_session(session_id).await?;
        Self::require_app(&session, "ingestion_review")?;
        let draft_id = Self::require_target_str(&session, "draft_id")?;
        let namespace = self.resolve_namespace_for_scope(&session.scope)?;

        let items = crate::service::apps::ingestion::IngestionReview::get_draft_items(
            draft_id,
            &*self.db_client,
            &namespace,
        )
        .await?;

        let approved: Vec<_> = items
            .iter()
            .filter(|item| matches!(item.status.as_str(), "approved" | "edited"))
            .collect();

        if approved.is_empty() {
            return Ok(serde_json::json!({
                "ok": false,
                "message": "No approved items to commit",
                "commit_summary": {
                    "entities": 0,
                    "facts": 0,
                    "edges": 0,
                },
            }));
        }

        let source_episode_id = self
            .commit_source_episode_for_draft(draft_id, &session.scope, &approved)
            .await?;

        let (entity_ids_by_item, entity_ids_by_name, created_entity_ids) =
            self.commit_entities(&approved).await?;

        let (created_fact_ids, edge_ids_from_facts) = self
            .commit_facts(
                &approved,
                draft_id,
                source_episode_id.as_deref(),
                &session.scope,
                &entity_ids_by_item,
                &entity_ids_by_name,
            )
            .await?;

        let edge_ids_explicit = self
            .commit_edges(
                &approved,
                draft_id,
                &session.scope,
                &entity_ids_by_item,
                &entity_ids_by_name,
            )
            .await?;

        let mut created_edge_ids = edge_ids_from_facts;
        created_edge_ids.extend(edge_ids_explicit);

        if !created_entity_ids.is_empty() || !created_edge_ids.is_empty() {
            let _ = super::episode::rebuild_all_communities(self, &session.scope).await?;
        }

        self.finalize_commit(
            draft_id,
            &namespace,
            session_id,
            &created_entity_ids,
            &created_fact_ids,
            &created_edge_ids,
            source_episode_id,
            approved.len(),
        )
        .await
    }

    /// Commits entity items and returns mappings and created IDs.
    async fn commit_entities(
        &self,
        approved: &[&crate::models::DraftItem],
    ) -> Result<
        (
            HashMap<String, String>,
            HashMap<String, String>,
            Vec<String>,
        ),
        MemoryError,
    > {
        let mut entity_ids_by_item = HashMap::new();
        let mut entity_ids_by_name = HashMap::new();
        let mut created_entity_ids = Vec::new();

        for item in approved.iter().filter(|item| item.item_type == "entity") {
            let candidate = draft_entity_candidate(item)?;
            let entity_id = self.resolve(candidate.clone(), None).await?;
            entity_ids_by_item.insert(item.item_id.clone(), entity_id.clone());
            entity_ids_by_name.insert(
                super::normalize_text(&candidate.canonical_name),
                entity_id.clone(),
            );
            created_entity_ids.push(entity_id);
        }

        Ok((entity_ids_by_item, entity_ids_by_name, created_entity_ids))
    }

    /// Commits fact items and returns fact IDs with edge IDs from fact relations.
    async fn commit_facts(
        &self,
        approved: &[&crate::models::DraftItem],
        draft_id: &str,
        source_episode_id: Option<&str>,
        scope: &str,
        entity_ids_by_item: &HashMap<String, String>,
        entity_ids_by_name: &HashMap<String, String>,
    ) -> Result<(Vec<String>, Vec<String>), MemoryError> {
        let mut created_fact_ids = Vec::new();
        let mut created_edge_ids = Vec::new();

        for item in approved.iter().filter(|item| item.item_type == "fact") {
            let (fact_id, fact_entity_links) = self
                .commit_draft_fact(
                    draft_id,
                    item,
                    source_episode_id,
                    scope,
                    entity_ids_by_item,
                    entity_ids_by_name,
                )
                .await?;
            created_fact_ids.push(fact_id.clone());

            for entity_id in fact_entity_links {
                let edge = crate::models::Edge {
                    in_id: entity_id,
                    relation: "involved_in".to_string(),
                    out_id: fact_id.clone(),
                    strength: 0.8,
                    confidence: item.confidence.max(0.0),
                    provenance: json!({
                        "draft_id": draft_id,
                        "draft_item_id": item.item_id,
                        "source_episode": source_episode_id,
                    }),
                    t_valid: self.now(),
                    t_ingested: self.now(),
                    t_invalid: None,
                    t_invalid_ingested: None,
                };

                super::episode::store_edge(self, &edge, scope).await?;
                created_edge_ids.push(super::ids::deterministic_edge_id(
                    &edge.in_id,
                    &edge.relation,
                    &edge.out_id,
                    edge.t_valid,
                ));
            }
        }

        Ok((created_fact_ids, created_edge_ids))
    }

    /// Commits explicit edge items.
    async fn commit_edges(
        &self,
        approved: &[&crate::models::DraftItem],
        draft_id: &str,
        scope: &str,
        entity_ids_by_item: &HashMap<String, String>,
        entity_ids_by_name: &HashMap<String, String>,
    ) -> Result<Vec<String>, MemoryError> {
        let mut created_edge_ids = Vec::new();

        for item in approved.iter().filter(|item| item.item_type == "edge") {
            let edge_id = self
                .commit_draft_edge(
                    draft_id,
                    item,
                    scope,
                    entity_ids_by_item,
                    entity_ids_by_name,
                )
                .await?;
            created_edge_ids.push(edge_id);
        }

        Ok(created_edge_ids)
    }

    /// Finalizes commit: updates draft status, closes session, returns result.
    #[allow(clippy::too_many_arguments)]
    async fn finalize_commit(
        &self,
        draft_id: &str,
        namespace: &str,
        session_id: &str,
        created_entity_ids: &[String],
        created_fact_ids: &[String],
        created_edge_ids: &[String],
        source_episode_id: Option<String>,
        approved_count: usize,
    ) -> Result<serde_json::Value, MemoryError> {
        let commit_summary = serde_json::json!({
            "entities": created_entity_ids.len(),
            "facts": created_fact_ids.len(),
            "edges": created_edge_ids.len(),
        });

        self.db_client
            .query(
                "UPDATE draft_ingestion SET status = 'committed' WHERE draft_id = $draft_id",
                Some(json!({"draft_id": draft_id})),
                namespace,
            )
            .await?;
        self.app_session_manager.close_session(session_id).await?;

        Ok(serde_json::json!({
            "ok": true,
            "message": format!("Committed {} reviewed items", approved_count),
            "refresh_required": false,
            "commit_summary": commit_summary,
            "source_episode_id": source_episode_id,
            "created": {
                "entity_ids": created_entity_ids,
                "fact_ids": created_fact_ids,
                "edge_ids": created_edge_ids,
            }
        }))
    }

    async fn commit_source_episode_for_draft(
        &self,
        draft_id: &str,
        scope: &str,
        approved_items: &[&crate::models::DraftItem],
    ) -> Result<Option<String>, MemoryError> {
        if let Some(existing_episode_id) = approved_items.iter().find_map(|item| {
            draft_payload_str(&item.payload, "draft_episode_id")
                .or_else(|| draft_payload_str(&item.payload, "source_episode"))
                .filter(|value| !value.trim().is_empty())
        }) {
            return Ok(Some(existing_episode_id));
        }

        let source_text = approved_items.iter().find_map(|item| {
            draft_payload_str(&item.payload, "source_text")
                .or_else(|| draft_payload_str(&item.payload, "content"))
                .or_else(|| item.source_snippet.clone())
                .filter(|value| !value.trim().is_empty())
        });

        let Some(source_text) = source_text else {
            return Ok(None);
        };

        let episode_id = self
            .ingest(
                IngestRequest {
                    source_type: "draft_ingestion".to_string(),
                    source_id: draft_id.to_string(),
                    content: source_text,
                    t_ref: self.now(),
                    scope: scope.to_string(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: Vec::new(),
                },
                None,
            )
            .await?;

        Ok(Some(episode_id))
    }

    async fn commit_draft_fact(
        &self,
        draft_id: &str,
        item: &crate::models::DraftItem,
        source_episode_id: Option<&str>,
        scope: &str,
        entity_ids_by_item: &HashMap<String, String>,
        entity_ids_by_name: &HashMap<String, String>,
    ) -> Result<(String, Vec<String>), MemoryError> {
        let content = draft_payload_str(&item.payload, "content")
            .or_else(|| draft_payload_str(&item.payload, "source_text"))
            .or_else(|| item.source_snippet.clone())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                MemoryError::InvalidParameter(format!(
                    "draft fact {} is missing content",
                    item.item_id
                ))
            })?;
        let quote = draft_payload_str(&item.payload, "quote").unwrap_or_else(|| content.clone());
        let fact_type =
            draft_payload_str(&item.payload, "fact_type").unwrap_or_else(|| "note".to_string());
        let t_valid =
            draft_payload_datetime(&item.payload, "t_valid").unwrap_or_else(|| self.now());
        let source_episode = source_episode_id.ok_or_else(|| {
            MemoryError::InvalidParameter(format!(
                "draft fact {} requires source text or draft_episode_id before commit",
                item.item_id
            ))
        })?;
        let policy_tags = draft_payload_string_array(&item.payload, "policy_tags");
        let entity_links = draft_payload_string_array(&item.payload, "entity_links")
            .into_iter()
            .filter_map(|reference| {
                resolve_draft_reference(&reference, entity_ids_by_item, entity_ids_by_name)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let fact_id = self
            .add_fact(AddFactRequest {
                fact_type: &fact_type,
                content: &content,
                quote: &quote,
                source_episode,
                t_valid,
                scope,
                confidence: draft_payload_f64(&item.payload, "confidence")
                    .unwrap_or(item.confidence),
                entity_links: entity_links.clone(),
                policy_tags,
                provenance: json!({
                    "draft_id": draft_id,
                    "draft_item_id": item.item_id,
                    "source_episode": source_episode,
                    "rationale": item.rationale,
                }),
            })
            .await?;

        Ok((fact_id, entity_links))
    }

    async fn commit_draft_edge(
        &self,
        draft_id: &str,
        item: &crate::models::DraftItem,
        scope: &str,
        entity_ids_by_item: &HashMap<String, String>,
        entity_ids_by_name: &HashMap<String, String>,
    ) -> Result<String, MemoryError> {
        let from_reference = draft_payload_str(&item.payload, "from_id")
            .or_else(|| draft_payload_str(&item.payload, "from"))
            .or_else(|| draft_payload_str(&item.payload, "in"))
            .ok_or_else(|| {
                MemoryError::InvalidParameter(format!(
                    "draft edge {} is missing from_id",
                    item.item_id
                ))
            })?;
        let to_reference = draft_payload_str(&item.payload, "to_id")
            .or_else(|| draft_payload_str(&item.payload, "to"))
            .or_else(|| draft_payload_str(&item.payload, "out"))
            .ok_or_else(|| {
                MemoryError::InvalidParameter(format!(
                    "draft edge {} is missing to_id",
                    item.item_id
                ))
            })?;
        let from_id =
            resolve_draft_reference(&from_reference, entity_ids_by_item, entity_ids_by_name)
                .ok_or_else(|| {
                    MemoryError::InvalidParameter(format!(
                        "could not resolve edge endpoint `{}` for {}",
                        from_reference, item.item_id
                    ))
                })?;
        let to_id = resolve_draft_reference(&to_reference, entity_ids_by_item, entity_ids_by_name)
            .ok_or_else(|| {
                MemoryError::InvalidParameter(format!(
                    "could not resolve edge endpoint `{}` for {}",
                    to_reference, item.item_id
                ))
            })?;
        let relation = draft_payload_str(&item.payload, "relation")
            .unwrap_or_else(|| "related_to".to_string());
        let t_valid =
            draft_payload_datetime(&item.payload, "t_valid").unwrap_or_else(|| self.now());
        let edge = crate::models::Edge {
            in_id: from_id,
            relation,
            out_id: to_id,
            strength: draft_payload_f64(&item.payload, "strength").unwrap_or(1.0),
            confidence: draft_payload_f64(&item.payload, "confidence").unwrap_or(item.confidence),
            provenance: json!({
                "draft_id": draft_id,
                "draft_item_id": item.item_id,
                "scope": scope,
            }),
            t_valid,
            t_ingested: self.now(),
            t_invalid: None,
            t_invalid_ingested: None,
        };

        let namespace = self.resolve_namespace_for_scope(scope)?;
        super::episode::store_edge(self, &edge, &namespace).await?;

        Ok(super::ids::deterministic_edge_id(
            &edge.in_id,
            &edge.relation,
            &edge.out_id,
            edge.t_valid,
        ))
    }
}
