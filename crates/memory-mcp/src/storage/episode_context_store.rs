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

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

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
        self.db.select_table("episode").await
    }

    /// Episode contents matching a query, bi-temporally scoped.
    pub async fn select_episodes_by_content(
        &self,
        cutoff: &str,
        query: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Value>, MemoryError> {
        let (sql, vars) =
            crate::storage::queries::build_select_episodes_by_content_query(cutoff, query, limit);
        self.db.query_rows(&sql, Some(vars)).await
    }

    /// Episodes linked to an entity through the fact→edge graph
    /// (`entity ←edge← fact →episode`), newest first (graph-shaped
    /// read-model queries belong to this store).
    pub async fn select_episodes_via_entity(
        &self,
        entity_id: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        let sql = "SELECT * FROM episode WHERE episode_id IN (\
                   SELECT VALUE source_episode FROM fact WHERE fact_id IN (\
                   SELECT VALUE type::string(out) FROM edge \
                   WHERE in = <record> $entity_id AND relation = 'involved_in')) \
                   ORDER BY t_ref DESC LIMIT 10";
        self.db
            .query_rows(sql, Some(json!({ "entity_id": entity_id })))
            .await
    }
}
