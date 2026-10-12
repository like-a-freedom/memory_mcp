//! Memory-owned episode reads for context assembly.
//!
//! Memory owns the episode lifecycle, so the episode reads the
//! retrieval pipeline performs belong here rather than in the
//! knowledge store. The method bodies are unchanged from the single
//! store this split came from.
//!
//! `select_episodes_via_entity` is the deliberate exception: it joins
//! `episode`, `fact` and `edge`, so it spans two owners. It is kept
//! here with its reasoning recorded rather than split on a table
//! boundary that would be arbitrary. See
//! `docs/architecture/decisions/0001-typed-record-accessors.md`.

use crate::storage::table_scope::{MemoryTables, ReleaseOwnedTable};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

const SOURCE_CONTEXT_ANCHOR_LIMIT: usize = 50;
pub(crate) const SOURCE_CONTEXT_RADIUS: i64 = 4;
const SOURCE_CONTEXT_EPISODE_LIMIT: i64 =
    (SOURCE_CONTEXT_ANCHOR_LIMIT * (SOURCE_CONTEXT_RADIUS as usize * 2 + 1)) as i64;

#[derive(Debug, Clone)]
struct SourcePositionWindow {
    group: String,
    first: i64,
    last: i64,
}

/// Read-side store for the `episode` table during context assembly.
#[derive(Clone)]
pub struct EpisodeContextStore {
    db: BoundDbClient,
}

impl EpisodeContextStore {
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub(crate) fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
    }

    /// Full scan of the `episode` table.
    ///
    /// Owner-named, so a caller cannot name a table.
    pub async fn scan_episodes(&self) -> Result<Vec<Value>, MemoryError> {
        self.db.select_table(MemoryTables::table("episode")).await
    }

    /// Episode contents matching a query, bi-temporally scoped.
    pub async fn select_episodes_by_content(
        &self,
        cutoff: &str,
        query: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::memory::queries::build_select_episodes_by_content_query(cutoff, query, limit);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Read one source episode and a bounded, ordered neighborhood from its
    /// producer-declared stream. Both reads stay within the same namespace.
    pub async fn select_episode_by_id(&self, episode_id: &str) -> Result<Vec<Value>, MemoryError> {
        self.db
            .query_rows(
                "SELECT * FROM episode WHERE episode_id = $episode_id LIMIT 1",
                Some(json!({"episode_id": episode_id})),
            )
            .await
    }

    pub async fn select_source_neighborhood(
        &self,
        group: &str,
        position: u64,
        cutoff: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let position = i64::try_from(position)
            .map_err(|_| MemoryError::Validation("source position exceeds storage range".into()))?;
        self.db
            .query_rows(
                "SELECT * FROM episode WHERE source_group = $group \
                 AND source_position >= $first AND source_position <= $last \
                 AND t_ref <= type::datetime($cutoff) \
                 AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) \
                 ORDER BY source_position ASC LIMIT 9",
                Some(json!({
                    "group": group,
                    "first": position.saturating_sub(SOURCE_CONTEXT_RADIUS),
                    "last": position.saturating_add(SOURCE_CONTEXT_RADIUS),
                    "cutoff": cutoff,
                })),
            )
            .await
    }

    /// Read the bounded source neighborhoods for a candidate page in two
    /// indexed queries. Candidate episodes establish the producer-declared
    /// groups and positions; the second query reads at most four entries on
    /// either side, subject to the transaction cutoff and optional valid-time
    /// window.
    pub async fn select_source_neighborhoods_for_episodes(
        &self,
        episode_ids: &[String],
        cutoff: &str,
        window_start: Option<&str>,
        window_end: Option<&str>,
    ) -> Result<Vec<Value>, MemoryError> {
        let mut candidate_ids = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        for episode_id in episode_ids {
            if !episode_id.is_empty() && seen_ids.insert(episode_id.as_str()) {
                candidate_ids.push(episode_id.as_str());
                if candidate_ids.len() == SOURCE_CONTEXT_ANCHOR_LIMIT {
                    break;
                }
            }
        }
        if candidate_ids.is_empty() {
            return Ok(Vec::new());
        }

        let anchors = self
            .db
            .query_rows(
                "SELECT * FROM episode WHERE episode_id INSIDE $episode_ids \
                 AND t_ref <= type::datetime($cutoff) \
                 AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) \
                 ORDER BY episode_id ASC LIMIT $limit",
                Some(json!({
                    "episode_ids": candidate_ids,
                    "cutoff": cutoff,
                    "limit": SOURCE_CONTEXT_ANCHOR_LIMIT as i64,
                })),
            )
            .await?;

        let windows = anchors
            .iter()
            .filter_map(|record| {
                record
                    .get("Object")
                    .and_then(Value::as_object)
                    .or_else(|| record.as_object())
                    .and_then(crate::memory::episode::episode_from_record)
                    .and_then(|episode| episode.source_context)
            })
            .filter_map(|context| {
                let position = i64::try_from(context.position).ok()?;
                (position > 0).then(|| SourcePositionWindow {
                    group: context.group,
                    first: position.saturating_sub(SOURCE_CONTEXT_RADIUS).max(1),
                    last: position.saturating_add(SOURCE_CONTEXT_RADIUS),
                })
            })
            .collect::<Vec<_>>();
        let windows = merge_source_position_windows(windows);
        if windows.is_empty() {
            return Ok(Vec::new());
        }

        let mut vars = serde_json::Map::from_iter([
            ("cutoff".to_string(), json!(cutoff)),
            (
                "limit".to_string(),
                json!(source_context_window_limit(&windows)),
            ),
        ]);
        let mut range_predicates = Vec::with_capacity(windows.len());
        for (index, window) in windows.iter().enumerate() {
            let group_key = format!("group_{index}");
            let first_key = format!("first_{index}");
            let last_key = format!("last_{index}");
            vars.insert(group_key.clone(), json!(window.group));
            vars.insert(first_key.clone(), json!(window.first));
            vars.insert(last_key.clone(), json!(window.last));
            range_predicates.push(format!(
                "(source_group = ${group_key} AND source_position >= ${first_key} AND source_position <= ${last_key})"
            ));
        }
        let mut time_predicates = vec![
            "t_ref <= type::datetime($cutoff)".to_string(),
            "(t_ingested IS NONE OR t_ingested <= type::datetime($cutoff))".to_string(),
        ];
        if let Some(start) = window_start {
            vars.insert("window_start".to_string(), json!(start));
            time_predicates.push("t_ref >= type::datetime($window_start)".to_string());
        }
        if let Some(end) = window_end {
            vars.insert("window_end".to_string(), json!(end));
            time_predicates.push("t_ref <= type::datetime($window_end)".to_string());
        }
        let sql = format!(
            "SELECT * FROM episode WHERE ({}) AND {} ORDER BY source_group ASC, source_position ASC LIMIT $limit",
            range_predicates.join(" OR "),
            time_predicates.join(" AND "),
        );
        self.db.query_rows(&sql, Some(Value::Object(vars))).await
    }

    /// Episodes linked to an entity through the fact→edge graph
    /// (`entity ←edge← fact →episode`), newest first (graph-shaped
    /// read-model queries belong to this store).
    pub async fn select_episodes_via_entity(
        &self,
        entity_id: &str,
        allowed_tags: Option<&[String]>,
    ) -> Result<Vec<Value>, MemoryError> {
        let (fact_access_clause, episode_access_clause) = if allowed_tags.is_some() {
            (
                " AND (policy_tags IS NONE OR policy_tags IS NULL OR policy_tags = [] OR policy_tags CONTAINSANY $allowed_tags)",
                " AND (policy_tags IS NONE OR policy_tags IS NULL OR policy_tags = [] OR policy_tags CONTAINSANY $allowed_tags)",
            )
        } else {
            ("", "")
        };
        let sql = format!(
            "SELECT * FROM episode WHERE episode_id IN (\
             SELECT VALUE source_episode FROM fact WHERE fact_id IN (\
             SELECT VALUE type::string(out) FROM edge \
             WHERE in = <record> $entity_id AND relation = 'involved_in'){fact_access_clause}){episode_access_clause} \
             ORDER BY t_ref DESC LIMIT 10"
        );
        let vars = match allowed_tags {
            Some(tags) => json!({ "entity_id": entity_id, "allowed_tags": tags }),
            None => json!({ "entity_id": entity_id }),
        };
        self.db.query_rows(&sql, Some(vars)).await
    }
}

fn merge_source_position_windows(
    mut windows: Vec<SourcePositionWindow>,
) -> Vec<SourcePositionWindow> {
    windows.sort_by(|left, right| {
        left.group
            .cmp(&right.group)
            .then_with(|| left.first.cmp(&right.first))
    });
    let mut merged: Vec<SourcePositionWindow> = Vec::with_capacity(windows.len());
    for window in windows {
        if let Some(previous) = merged.last_mut()
            && previous.group == window.group
            && window.first <= previous.last.saturating_add(1)
        {
            previous.last = previous.last.max(window.last);
        } else {
            merged.push(window);
        }
    }
    merged
}

fn source_context_window_limit(windows: &[SourcePositionWindow]) -> i64 {
    windows
        .iter()
        .map(|window| window.last.saturating_sub(window.first).saturating_add(1))
        .fold(0_i64, i64::saturating_add)
        .clamp(1, SOURCE_CONTEXT_EPISODE_LIMIT)
}
