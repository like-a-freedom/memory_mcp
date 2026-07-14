mod diff;
pub(crate) mod graph;
mod ingestion_review;
mod lifecycle;
mod types;
#[cfg(feature = "mcp-apps")]
mod workflow;

pub use graph::GraphTraversalBudget;
pub use graph::{
    GraphPathSnapshot, edge_neighbor, entity_snapshot, graph_neighbor_expansion,
    graph_path_snapshot, graph_payload,
};
#[cfg(feature = "mcp-apps")]
pub(crate) use ingestion_review::{apply_ingestion_review_edit, apply_ingestion_review_status};
#[cfg(feature = "mcp-apps")]
pub(crate) use lifecycle::execute_lifecycle_command;
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
