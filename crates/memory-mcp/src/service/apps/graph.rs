//! The knowledge-graph write paths and the app-session state machine.
//!
//! Reads live in `memory::retrieval::graph_reads`, reached through the
//! `GraphContext` contract implemented here for the container and for
//! context assembly. What remains is what writes: relating entities,
//! recording edges, and the traversal state the map app serialises.

use crate::error::MemoryError;
use crate::knowledge::graph_store::KnowledgeGraphStore;
use crate::logging::StdoutLogger;
use crate::memory::retrieval::graph_reads::GraphContext;
use crate::service::MemoryService;

impl GraphContext for MemoryService {
    fn knowledge_graph_store(&self) -> KnowledgeGraphStore {
        KnowledgeGraphStore::new(self.db_client.clone(), self.active_namespace.clone())
    }
    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}

impl MemoryService {
    /// Resolves an entity by its type and canonical name.
    ///
    /// Graph/entity convenience built on [`ResolveCapability`]; lives here with
    /// the other graph conveniences.
    pub async fn resolve_entity(
        &self,
        entity_type: &str,
        name: &str,
    ) -> Result<String, MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_resolve::ResolveCapability::resolve_from_service(
            self,
            crate::models::EntityCandidate {
                entity_type: entity_type.to_string(),
                canonical_name: name.to_string(),
                aliases: Vec::new(),
            },
            None,
        )
        .await
    }

    /// Creates a relationship edge between two entities.
    ///
    /// `attributes` says how we know the edge. This used to be hardcoded to
    /// `Inferred`/`1.0`/`0.8`/`Provenance::manual()`, which meant a caller
    /// could not record that an operator stated the relationship, or that
    /// confidence was anything but 0.8. See `EdgeAttributes`.
    pub async fn relate(
        &self,
        from_id: &str,
        relation: &str,
        to_id: &str,
        attributes: crate::models::EdgeAttributes,
    ) -> Result<(), MemoryError> {
        use crate::models::Edge;
        let edge = Edge {
            in_id: from_id.to_string(),
            relation: relation.to_string(),
            out_id: to_id.to_string(),
            origin: attributes.origin,
            strength: attributes.strength,
            confidence: attributes.confidence,
            provenance: attributes.provenance,
            t_valid: crate::shared::temporal::now(),
            t_ingested: crate::shared::temporal::now(),
            t_invalid: None,
            t_invalid_ingested: None,
        };
        crate::memory::episode::store_edge(
            &crate::memory::capabilities::deps::ExtractDeps::from(self),
            &edge,
        )
        .await
    }
}

/// The graph reads are also reached during context assembly, which holds a
/// narrow dependency set rather than the container. This is the second
/// implementor of the same read contract.
impl GraphContext for crate::memory::retrieval_deps::AssembleContextDeps {
    fn knowledge_graph_store(&self) -> KnowledgeGraphStore {
        self.graph_store.clone()
    }

    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}
