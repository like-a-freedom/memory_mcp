//! Container-shaped entry points moved out of the memory context.
//!
//! The `From<&MemoryService>` impls and the `fn(&MemoryService, ..)`
//! wrappers for this module. They are adapters, so they live on the
//! side allowed to know the container; the memory context keeps the
//! port-based implementation they delegate to.

use crate::memory::capabilities::deps::{ExtractDeps, IngestDeps};
use crate::memory::ingestion_review::IngestionReviewDeps;

impl From<&crate::service::MemoryService> for IngestionReviewDeps {
    fn from(service: &crate::service::MemoryService) -> Self {
        Self {
            ingest: IngestDeps::from(service),
            extract: ExtractDeps::from(service),
        }
    }
}
