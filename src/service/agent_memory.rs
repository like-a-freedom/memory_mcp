//! Agent-memory lifecycle orchestration.
//!
//! This module hosts the internal lifecycle capabilities
//! (`LifecycleRecall`, `LifecycleCapture`) and their supporting policy and
//! projection helpers. These are **not** registered in `tools/list`, are not
//! CLI subcommands, and have no public JSON schema. They call the same
//! service/tool modules used by `assemble_context` and inline `extract`.
//!
//! See ADR 0016 and `docs/agent_integration/CONTRACT.md`.

pub mod capture;
pub mod policy;
#[allow(dead_code)]
pub mod worker;

// The full LifecycleRecall struct lands in Task 6.
#[allow(unused_imports)]
pub use capture::{AgentMemoryStoreBackend, LifecycleCapture, LifecycleCaptureResult};
#[allow(unused_imports)]
pub use policy::CapturePolicy;
#[allow(unused_imports)]
pub use worker::{run_projection_pass, spawn_projection_worker};

/// Marker struct for the internal selective-recall capability.
///
/// Not registered in `tools/list` or as a CLI subcommand. The implementation
/// lands in Task 6; this declaration reserves the type so downstream modules
/// can reference it.
#[allow(dead_code)]
pub(crate) struct LifecycleRecall;
