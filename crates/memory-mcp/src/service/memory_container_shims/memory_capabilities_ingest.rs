//! Container-shaped entry points moved out of the memory context.
//!
//! The `From<&MemoryService>` impls and the `fn(&MemoryService, ..)`
//! wrappers for this module. They are adapters, so they live on the
//! side allowed to know the container; the memory context keeps the
//! port-based implementation they delegate to.

pub use crate::memory::capabilities::ingest::IngestCapability;

use crate::error::MemoryError;
use crate::memory::capabilities::deps::IngestDeps;
use crate::models::{AccessPayload, IngestRequest};

impl IngestCapability {
    /// Ingests a new episode through the memory-owned use case.
    pub async fn ingest_from_service(
        service: &crate::service::MemoryService,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        Self::ingest_with(&IngestDeps::from(service), request, access).await
    }
}
