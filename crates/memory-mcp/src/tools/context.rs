//! The seam the MCP tool layer depends on.
//!
//! A tool handler needs two things from the service: somewhere to
//! write its start/done/error events, and one capability call.
//! Taking the whole `ServiceContext` for that is how the god-struct
//! reached the transport edge, so the transport depends on this
//! trait instead and names only what it uses.
//!
//! The trait is implemented for `ServiceContext` in the service
//! layer, which keeps the direction of the dependency pointing
//! inward: tools know their port, not the container.

use std::time::Duration;

use serde_json::Value;

use crate::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{
    AccessPayload, AssembleContextRequest, AssembledContextItem, EntityCandidate, ExplainRequest,
    IngestRequest, InvalidateRequest,
};

/// A recorded tool lifecycle event.
///
/// Returned rather than logged here so the trait stays free of the
/// service's logger; the caller decides what to do with it.
pub struct ToolEvent {
    pub op: &'static str,
    pub args: Value,
    pub result: Value,
    pub level: LogLevel,
    pub request_id: Option<String>,
    pub duration: Option<Duration>,
}

/// Everything an MCP tool handler is allowed to reach.
///
/// Implemented by the service layer; tools hold this, not
/// `ServiceContext`.
pub trait ToolContext {
    /// Records a tool lifecycle event.
    fn record(&self, event: ToolEvent);

    /// `ingest`
    fn ingest(
        &self,
        request: IngestRequest,
        access: Option<AccessPayload>,
    ) -> impl Future<Output = Result<String, MemoryError>> + Send;

    /// `extract`
    fn extract(
        &self,
        episode_id: &str,
        access: Option<AccessPayload>,
        zero_shot_labels: Option<&[String]>,
    ) -> impl Future<Output = Result<crate::models::ExtractResult, MemoryError>> + Send;

    /// `resolve`
    fn resolve(
        &self,
        candidate: EntityCandidate,
        access: Option<AccessPayload>,
    ) -> impl Future<Output = Result<String, MemoryError>> + Send;

    /// `explain`
    fn explain(
        &self,
        request: ExplainRequest,
        access: Option<AccessPayload>,
    ) -> impl Future<Output = Result<Vec<crate::models::ExplainItem>, MemoryError>> + Send;

    /// `invalidate`
    fn invalidate(
        &self,
        request: InvalidateRequest,
        access: Option<AccessPayload>,
    ) -> impl Future<Output = Result<(), MemoryError>> + Send;

    /// `assemble_context`
    fn assemble_context(
        &self,
        request: AssembleContextRequest,
    ) -> impl Future<Output = Result<Vec<AssembledContextItem>, MemoryError>> + Send;

    /// Look up a source episode for extract's completion log.
    ///
    /// Returns `None` when the episode is missing or the id names
    /// another record kind: the log is best-effort, so a lookup
    /// failure must not fail the extract that already succeeded.
    fn find_episode(
        &self,
        episode_id: &str,
    ) -> impl Future<Output = Result<Option<crate::models::Episode>, MemoryError>> + Send;
}
