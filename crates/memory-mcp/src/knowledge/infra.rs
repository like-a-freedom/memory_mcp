//! Knowledge-owned persistence adapter.
//!
//! The [`crate::knowledge::api::KnowledgeReadPort`] contract is
//! expressed in owner-named scopes; this is the only place that
//! turns a scope into a query. A caller cannot name a table, so an
//! arbitrary-table read has no representation here.

use crate::storage::table_scope::{KnowledgeTables, MemoryTables, ReleaseOwnedTable};
use std::sync::Arc;

use serde_json::Value;

use crate::error::MemoryError;
use crate::knowledge::api::{KnowledgeReadPort, KnowledgeReadScope};
use crate::storage::DbClient;

/// Knowledge's own read port, bound to the process's Active
/// Namespace.
#[derive(Clone)]
pub struct KnowledgeReadAdapter {
    db: Arc<dyn DbClient>,
    namespace: String,
}

impl KnowledgeReadAdapter {
    #[must_use]
    pub fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db,
            namespace: namespace.into(),
        }
    }
}

#[async_trait::async_trait]
impl KnowledgeReadPort for KnowledgeReadAdapter {
    async fn read_scope(&self, scope: KnowledgeReadScope) -> Result<Vec<Value>, MemoryError> {
        // The scope decides the table, and each is claimed by the context that
        // owns it. This used to be a `&str` resolved by a match, which is the
        // shape that let the allowlist drift out of step with the schema:
        // nothing about spelling the name connected it to an owner.
        let table = match scope {
            // Bi-temporally visible canonical facts.
            KnowledgeReadScope::Facts => KnowledgeTables::table("fact"),
            // Source episodes, read for provenance assembly. Owned by memory,
            // and reached from here as a reader rather than as a writer —
            // which is why the two arms name different contexts.
            KnowledgeReadScope::Episodes => MemoryTables::table("episode"),
        };
        self.db.select_table(table, &self.namespace).await
    }
}
