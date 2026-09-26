//! Knowledge-owned persistence adapter.
//!
//! The [`crate::knowledge::api::KnowledgeReadPort`] contract is
//! expressed in owner-named scopes; this is the only place that
//! turns a scope into a query. A caller cannot name a table, so an
//! arbitrary-table read has no representation here.

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
        let table = match scope {
            // Bi-temporally visible canonical facts.
            KnowledgeReadScope::Facts => "fact",
            // Source episodes, read for provenance assembly.
            KnowledgeReadScope::Episodes => "episode",
        };
        self.db.select_table(table, &self.namespace).await
    }
}
