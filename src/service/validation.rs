//! Validation helpers for service operations.

use crate::models::{EntityCandidate, IngestRequest};
use super::error::MemoryError;

/// Validate an ingest request.
pub fn validate_ingest_request(request: &IngestRequest) -> Result<(), MemoryError> {
    if request.source_type.trim().is_empty() {
        return Err(MemoryError::Validation("source_type is required".into()));
    }
    if request.source_id.trim().is_empty() {
        return Err(MemoryError::Validation("source_id is required".into()));
    }
    if request.content.trim().is_empty() {
        return Err(MemoryError::Validation("content is required".into()));
    }
    if request.scope.trim().is_empty() {
        return Err(MemoryError::Validation("scope is required".into()));
    }
    Ok(())
}

/// Validate an entity candidate.
pub fn validate_entity_candidate(candidate: &EntityCandidate) -> Result<(), MemoryError> {
    if candidate.entity_type.trim().is_empty() {
        return Err(MemoryError::Validation("entity_type is required".into()));
    }
    if candidate.canonical_name.trim().is_empty() {
        return Err(MemoryError::Validation("canonical_name is required".into()));
    }
    Ok(())
}

/// Validate fact input parameters.
pub fn validate_fact_input(
    fact_type: &str,
    content: &str,
    quote: &str,
    source_episode: &str,
    scope: &str,
) -> Result<(), MemoryError> {
    if fact_type.trim().is_empty() {
        return Err(MemoryError::Validation("fact_type is required".into()));
    }
    if content.trim().is_empty() {
        return Err(MemoryError::Validation("content is required".into()));
    }
    if quote.trim().is_empty() {
        return Err(MemoryError::Validation("quote is required".into()));
    }
    if source_episode.trim().is_empty() {
        return Err(MemoryError::Validation("source_episode is required".into()));
    }
    if scope.trim().is_empty() {
        return Err(MemoryError::Validation("scope is required".into()));
    }
    Ok(())
}
