//! Procedural memory service: candidate management, ranking, and review.
//!
//! Candidates derive only from accepted lesson evidence linked to trusted
//! outcomes. They group deterministically, append evidence, derive a Beta
//! posterior from counts, and never auto-promote. The procedure gate must
//! pass before promotion is enabled.
//!
//! See the Agent Memory Lifecycle section of
//! [`hooks/README.md`](../../../../hooks/README.md) for the recall-then-capture
//! loop this procedure gate serves.

pub mod ranking;
pub mod review;

pub use ranking::{CandidateRankingEntry, rank_candidates};
pub use review::{ReviewAction, ReviewDecision, review_candidate};
