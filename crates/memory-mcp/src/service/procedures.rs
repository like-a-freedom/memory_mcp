//! Procedural memory service: candidate management, ranking, and review.
//!
//! Candidates derive only from accepted lesson evidence linked to trusted
//! outcomes. They group deterministically, append evidence, derive a Beta
//! posterior from counts, and never auto-promote. The procedure gate must
//! pass before promotion is enabled.
//!
//! See `docs/superpowers/plans/2026-07-23-agent-memory-lifecycle-integration.md`
//! Tasks 10-11.

pub mod ranking;
pub mod review;

pub use ranking::{CandidateRankingEntry, rank_candidates};
pub use review::{ReviewAction, ReviewDecision, review_candidate};
