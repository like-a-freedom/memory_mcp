//! Agent-memory lifecycle orchestration.
//!
//! This module hosts the internal lifecycle capabilities
//! (`LifecycleCapture`, `LifecycleWorkerRuntime`) and their supporting policy
//! and recall helpers. These are **not** registered in `tools/list`, are not
//! CLI subcommands, and have no public JSON schema. They call the same
//! service/tool modules used by `assemble_context` and inline `extract`.
//!
//! See ADR 0016 and `docs/agent_integration/CONTRACT.md`.

pub mod projection;
pub mod worker;

pub use crate::memory::agent_memory::capture::{
    AgentMemoryStoreBackend, LifecycleCapture, LifecycleCaptureResult,
};
pub use crate::memory::agent_memory::policy::CapturePolicy;
pub use crate::memory::agent_memory::recall::{
    LifecycleRecall, LifecycleRecallResult, MAX_SESSIONS, MEMORY_IS_DATA_PREAMBLE, RecallDecision,
    RecallKey, RecallPipeline, SessionTraceRegistry, evaluate_recall,
};
pub use projection::run_projection_pass;
pub use worker::LifecycleWorkerRuntime;
