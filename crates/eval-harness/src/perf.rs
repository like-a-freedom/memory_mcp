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

// filled in Task 2: `PerfError` (thiserror) lives in this root module.
