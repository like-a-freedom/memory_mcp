#[cfg(feature = "mcp-apps")]
pub(crate) mod dispatch;
#[cfg(feature = "mcp-apps")]
pub(crate) mod session;
#[cfg(feature = "mcp-apps")]
pub(crate) mod session_lifecycle;
#[cfg(feature = "mcp-apps")]
mod workflow;

pub mod graph;
pub(crate) use crate::memory::lifecycle_types::LifecycleOperation;
pub use crate::memory::lifecycle_types::{
    ArchiveCandidatesOutcome, CommitIngestionReviewOutcome, CommitIngestionReviewRequest,
    DiffChange, DiffRequest, DiffSummary, DiffTarget, DiffView, DiffViewRange,
    IngestionReviewBundle, IngestionReviewItem, IngestionReviewSource, IngestionReviewSummary,
    LifecycleCommand, LifecycleCommandOutcome, LifecycleDashboard, LifecycleDefaults,
    LifecycleView, PrepareIngestionReviewRequest, RebuildCommunitiesOutcome, RecomputeDecayOutcome,
    RestoreArchivedOutcome,
};
pub use crate::platform::traversal_budget::GraphTraversalBudget;
#[cfg(test)]
pub use graph::edge_neighbor;
pub use graph::{graph_neighbor_expansion, graph_payload};
#[cfg(feature = "mcp-apps")]
pub(crate) use workflow::AppCommandInput;
