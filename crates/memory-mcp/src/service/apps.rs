mod diff;
#[cfg(feature = "mcp-apps")]
pub(crate) mod dispatch;
pub(crate) mod graph;
mod ingestion_review;
mod lifecycle;
mod types;
#[cfg(feature = "mcp-apps")]
mod workflow;

pub use graph::GraphTraversalBudget;
#[cfg(test)]
pub use graph::edge_neighbor;
#[cfg(feature = "mcp-apps")]
pub use graph::graph_neighbor_expansion;
pub use graph::graph_payload;
pub use types::{
    ArchiveCandidatesOutcome, CommitIngestionReviewOutcome, CommitIngestionReviewRequest,
    DiffChange, DiffRequest, DiffSummary, DiffTarget, DiffView, DiffViewRange,
    IngestionReviewBundle, IngestionReviewItem, IngestionReviewSource, IngestionReviewSummary,
    LifecycleCommand, LifecycleCommandOutcome, LifecycleDashboard, LifecycleDefaults,
    LifecycleView, PrepareIngestionReviewRequest, RebuildCommunitiesOutcome, RecomputeDecayOutcome,
    RestoreArchivedOutcome,
};
#[cfg(feature = "mcp-apps")]
pub(crate) use workflow::{AppCommand, AppCommandInput};
