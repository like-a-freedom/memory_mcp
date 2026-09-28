//! Container-shaped entry points moved out of the memory context.
//!
//! The `From<&MemoryService>` impls and the `fn(&MemoryService, ..)`
//! wrappers for this module. They are adapters, so they live on the
//! side allowed to know the container; the memory context keeps the
//! port-based implementation they delegate to.

pub use crate::memory::capabilities::resolve::ResolveCapability;

use crate::error::MemoryError;
use crate::memory::capabilities::deps::ResolveDeps;
use crate::models::{AccessPayload, EntityCandidate};

impl ResolveCapability {
    /// Resolves an entity candidate, returning the canonical entity ID.
    ///
    /// Uses fuzzy matching via `EntityResolver` to deduplicate entities
    /// with similar names (e.g., "Иван Петров" vs "I. Petrov").
    pub async fn resolve_from_service(
        service: &crate::service::MemoryService,
        candidate: EntityCandidate,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        Self::resolve_with(&ResolveDeps::from(service), candidate, access).await
    }
}
