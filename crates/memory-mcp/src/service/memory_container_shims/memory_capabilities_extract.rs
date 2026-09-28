//! Container-shaped entry points moved out of the memory context.
//!
//! The `From<&MemoryService>` impls and the `fn(&MemoryService, ..)`
//! wrappers for this module. They are adapters, so they live on the
//! side allowed to know the container; the memory context keeps the
//! port-based implementation they delegate to.

pub use crate::memory::capabilities::extract::ExtractCapability;

use crate::error::MemoryError;
use crate::memory::capabilities::deps::ExtractDeps;
use crate::models::{AccessPayload, ExtractResult};

impl ExtractCapability {
    /// Extracts entities and facts from an episode.
    ///
    /// # Arguments
    ///
    /// * `deps` - The extraction pipeline's dependencies.
    /// * `episode_id` - The episode to extract from.
    /// * `access` - Optional access context for authorization.
    /// * `zero_shot_labels` - Optional custom entity labels for GLiNER extraction.
    ///   When provided, these labels override the default NER configuration.
    pub async fn extract_from_service(
        service: &crate::service::MemoryService,
        episode_id: &str,
        access: Option<AccessPayload>,
        zero_shot_labels: Option<&[String]>,
    ) -> Result<ExtractResult, MemoryError> {
        Self::extract_with(
            &ExtractDeps::from(service),
            episode_id,
            access,
            zero_shot_labels,
        )
        .await
    }
}
