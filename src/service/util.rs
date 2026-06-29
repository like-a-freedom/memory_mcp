//! Service utility functions for ID generation, rate limiting, validation, and query detection.
//!
//! - `ids`: Deterministic ID generation functions used by multiple domain services.
//! - `rate_limit`: In-memory rate limiter using token bucket algorithm.
//! - `statement_detection`: Content statement type detection (promises, metrics, experiences, etc.).
//! - `validation`: Shared validation for ingest requests, entity candidates, and fact input.

pub mod ids;
pub(crate) mod rate_limit;
mod statement_detection;
mod validation;

pub use ids::{
    deterministic_community_id, deterministic_edge_id, deterministic_entity_id,
    deterministic_episode_id, deterministic_fact_id, hash_prefix,
};
pub use statement_detection::{
    is_document_action_item, is_experience_statement, is_low_value_summary_candidate,
    is_metric_statement, is_promise_statement, is_summary_like_note_candidate,
};
pub use validation::{validate_entity_candidate, validate_fact_input, validate_ingest_request};

// Internal use only - not re-exported.
pub(crate) use rate_limit::RateLimiter;
