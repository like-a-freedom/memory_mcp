//! Knowledge policies and queries — public interface.
//!
//! Knowledge owns entities, aliases, facts, claims, triples,
//! communities, extraction/reconciliation and knowledge
//! queries.
//!
//! Reads are expressed as owner-named [`KnowledgeReadScope`]
//! values rather than caller-supplied table names, so an
//! application-facing API cannot reach an arbitrary table.
//! The full canonical fact and entity query surface stays
//! behind the owner infra layer; a cross-owner optimized read
//! is only exposed through an explicit provider contract.

use serde_json::Value;

use crate::error::MemoryError;

/// A read that knowledge is willing to serve, named by owner
/// rather than by table string.
///
/// This is deliberately a closed enum: there is no variant
/// that carries a caller-provided table name, so the previous
/// `select_table(&str)` escape hatch has no representation
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnowledgeReadScope {
    /// Bi-temporally visible canonical facts.
    Facts,
    /// Source episodes, which memory owns but knowledge reads
    /// for provenance assembly.
    Episodes,
}

impl KnowledgeReadScope {
    /// Owning bounded context for this scope.
    pub const fn owner(self) -> &'static str {
        match self {
            Self::Facts => "knowledge",
            Self::Episodes => "memory",
        }
    }
}

/// A completed owner-scoped read.
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeRead {
    pub scope: KnowledgeReadScope,
    pub owner: &'static str,
    pub rows: usize,
}

/// Port supplying the rows for a scope.
///
/// The port is owner-injected: the implementation decides how
/// to read, and the caller cannot name a table.
#[async_trait::async_trait]
pub trait KnowledgeReadPort: Send + Sync {
    async fn read_scope(&self, scope: KnowledgeReadScope) -> Result<Vec<Value>, MemoryError>;
}

/// Serve a scope through an injected port, reporting the owner
/// that answered it.
pub async fn read_through_knowledge_port(
    port: &(impl KnowledgeReadPort + ?Sized),
    scope: KnowledgeReadScope,
) -> Result<KnowledgeRead, MemoryError> {
    let rows = port.read_scope(scope).await?;
    Ok(KnowledgeRead {
        scope,
        owner: scope.owner(),
        rows: rows.len(),
    })
}

/// Owner knowledge of canonical fact reads.
pub async fn owned_fact_scan(
    port: &(impl KnowledgeReadPort + ?Sized),
) -> Result<Vec<Value>, MemoryError> {
    port.read_scope(KnowledgeReadScope::Facts).await
}

/// Owner knowledge of the episode read used for provenance.
pub async fn owned_episode_scan(
    port: &(impl KnowledgeReadPort + ?Sized),
) -> Result<Vec<Value>, MemoryError> {
    port.read_scope(KnowledgeReadScope::Episodes).await
}
