//! Performance evaluation surface (ADR-0082).
//!
//! Profile-driven capability runs over embedded SurrealDB, reduced into a
//! committed `memory-mcp-perf/v1` artifact with a regression verdict.
//! Design: `docs/superpowers/specs/2026-10-10-perf-evaluation-harness-design.md`.

pub mod artifact;
pub mod db_observer;
pub mod domain;
pub mod profile;
pub mod reducer;
pub mod runner;
pub mod sampler;
pub mod scenarios;
pub mod verdict;

pub use profile::{
    DriverKind, GateDirection, GateMetric, GateSpec, PerfProfile, ScenarioId, ScenarioSpec, load,
};

/// Errors of the perf surface: profile loading, artifact I/O, and run wiring.
#[derive(Debug, thiserror::Error)]
pub enum PerfError {
    #[error("json decode failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unsupported schema: {0}")]
    Schema(String),

    #[error("unsupported driver: {0}")]
    UnsupportedDriver(String),

    #[error("validation failed: {0}")]
    Validation(String),
}
