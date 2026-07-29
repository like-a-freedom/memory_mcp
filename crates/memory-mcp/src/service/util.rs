//! Service utility functions for ID generation, rate limiting, and validation.
//!
//! - `ids`: Deterministic ID generation functions used by multiple domain services.
//! - `rate_limiter`: In-memory rate limiter using token bucket algorithm.
//! - `validation`: Shared validation for ingest requests, entity candidates, and fact input.

pub mod ids;
pub(crate) mod rate_limiter;
mod validation;

pub use ids::{
    deterministic_community_id, deterministic_edge_id, deterministic_entity_id,
    deterministic_episode_id, deterministic_fact_id, hash_prefix,
};
pub use validation::{validate_entity_candidate, validate_fact_input, validate_ingest_request};

// Internal use only - not re-exported.
pub(crate) use rate_limiter::RateLimiter;
