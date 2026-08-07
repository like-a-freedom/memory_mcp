//! Shared NER artifact lifecycle domain.
//!
//! Task 4 provides the pure manifest, state, and progress contracts with no
//! network or filesystem side effects beyond the sinks themselves. Task 5 adds
//! acquisition, leases, activation, and recovery orchestration.

pub(crate) mod manifest;
pub(crate) mod progress;
pub(crate) mod state;

pub(crate) use manifest::{
    ArtifactRequirement, NerArtifactSpec, PreparedCheckpoint, RevisionStatus, ValidationStatus,
    artifact_identity,
};
pub(crate) use progress::{
    CliProgressSink, JsonLineProgressSink, ModelProgressEvent, ModelProgressPhase,
    ModelProgressSink, ThrottledProgressSink,
};
pub(crate) use state::{PersistedArtifactState, RevisionState, persist_state, read_state};
