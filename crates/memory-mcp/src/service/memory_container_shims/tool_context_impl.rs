//! Service-side implementation of the transport-facing tool port.
//!
//! The tool handlers reach the service through this port and never
//! through the shared container. Each capability assembles the
//! dependency struct it declares, so what a tool can reach is
//! exactly what that tool needs.

use crate::error::MemoryError;
use crate::models::{
    AccessPayload, AssembleContextRequest, AssembledContextItem, EntityCandidate, ExplainRequest,
    IngestRequest, InvalidateRequest,
};
use crate::tools::context::{ToolContext, ToolEvent};

impl ToolContext for crate::service::MemoryService {
    fn record(&self, event: ToolEvent) {
        match event.duration {
            Some(duration) => self.log_tool_event_with_duration(
                event.op,
                event.args,
                event.result,
                event.level,
                duration,
                event.request_id.as_deref(),
            ),
            None => self.log_tool_event(
                event.op,
                event.args,
                event.result,
                event.level,
                event.request_id.as_deref(),
            ),
        }
    }

    async fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_ingest::IngestCapability::ingest_from_service(self, request, access).await
    }

    async fn extract(
        &self,
        episode_id: &str,
        access: Option<AccessPayload>,
        zero_shot_labels: Option<&[String]>,
    ) -> Result<crate::models::ExtractResult, MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_extract::ExtractCapability::extract_from_service(self, episode_id, access, zero_shot_labels).await
    }

    async fn resolve(
        &self,
        candidate: EntityCandidate,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_resolve::ResolveCapability::resolve_from_service(self, candidate, access).await
    }

    async fn explain(
        &self,
        request: ExplainRequest,
        access: Option<AccessPayload>,
    ) -> Result<Vec<crate::models::ExplainItem>, MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_explain::ExplainCapability::explain_from_service(self, request, access).await
    }

    async fn invalidate(
        &self,
        request: InvalidateRequest,
        access: Option<AccessPayload>,
    ) -> Result<(), MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability::invalidate_from_service(self, request, access).await
    }

    async fn assemble_context(
        &self,
        request: AssembleContextRequest,
    ) -> Result<Vec<AssembledContextItem>, MemoryError> {
        crate::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(self, request).await
    }

    async fn find_episode(
        &self,
        episode_id: &str,
    ) -> Result<Option<crate::models::Episode>, MemoryError> {
        // A wrong-kind id yields no episode rather than an error, so
        // the best-effort completion log does not fail an extract
        // that already succeeded.
        let record = match self.find_episode_record(episode_id).await {
            Ok(record) => record,
            Err(MemoryError::Validation(_)) => None,
            Err(error) => return Err(error),
        };
        Ok(record
            .as_ref()
            .and_then(crate::memory::episode::episode_from_record))
    }
}
