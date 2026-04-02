//! Validation helpers for service operations.

use super::error::MemoryError;
use crate::models::{EntityCandidate, IngestRequest};

/// Validates that a string field is non-empty after trimming whitespace.
///
/// # Errors
///
/// Returns [`MemoryError::Validation`] if the field is empty or whitespace-only.
fn require_non_empty(value: &str, field_name: &str) -> Result<(), MemoryError> {
    if value.trim().is_empty() {
        return Err(MemoryError::Validation(format!("{field_name} is required")));
    }
    Ok(())
}

/// Validate an ingest request.
pub fn validate_ingest_request(request: &IngestRequest) -> Result<(), MemoryError> {
    require_non_empty(&request.source_type, "source_type")?;
    require_non_empty(&request.source_id, "source_id")?;
    require_non_empty(&request.content, "content")?;
    require_non_empty(&request.scope, "scope")?;
    Ok(())
}

/// Validate an entity candidate.
pub fn validate_entity_candidate(candidate: &EntityCandidate) -> Result<(), MemoryError> {
    require_non_empty(&candidate.entity_type, "entity_type")?;
    require_non_empty(&candidate.canonical_name, "canonical_name")?;
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
    require_non_empty(fact_type, "fact_type")?;
    require_non_empty(content, "content")?;
    require_non_empty(quote, "quote")?;
    require_non_empty(source_episode, "source_episode")?;
    require_non_empty(scope, "scope")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_valid_ingest_request() -> IngestRequest {
        IngestRequest {
            source_type: "email".to_string(),
            source_id: "msg-123".to_string(),
            content: "Test content".to_string(),
            t_ref: Utc::now(),
            scope: "org".to_string(),
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        }
    }

    #[test]
    fn validate_ingest_request_accepts_valid_input() {
        let request = create_valid_ingest_request();
        assert!(validate_ingest_request(&request).is_ok());
    }

    #[test]
    fn validate_ingest_request_rejects_empty_source_type() {
        let mut request = create_valid_ingest_request();
        request.source_type = "".to_string();
        let result = validate_ingest_request(&request);
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("source_type")));
    }

    #[test]
    fn validate_ingest_request_rejects_whitespace_only_source_type() {
        let mut request = create_valid_ingest_request();
        request.source_type = "   ".to_string();
        let result = validate_ingest_request(&request);
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("source_type")));
    }

    #[test]
    fn validate_ingest_request_rejects_empty_source_id() {
        let mut request = create_valid_ingest_request();
        request.source_id = "".to_string();
        let result = validate_ingest_request(&request);
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("source_id")));
    }

    #[test]
    fn validate_ingest_request_rejects_empty_content() {
        let mut request = create_valid_ingest_request();
        request.content = "".to_string();
        let result = validate_ingest_request(&request);
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("content")));
    }

    #[test]
    fn validate_ingest_request_rejects_empty_scope() {
        let mut request = create_valid_ingest_request();
        request.scope = "".to_string();
        let result = validate_ingest_request(&request);
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("scope")));
    }

    fn create_valid_entity_candidate() -> EntityCandidate {
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "John Doe".to_string(),
            aliases: vec![],
        }
    }

    #[test]
    fn validate_entity_candidate_accepts_valid_input() {
        let candidate = create_valid_entity_candidate();
        assert!(validate_entity_candidate(&candidate).is_ok());
    }

    #[test]
    fn validate_entity_candidate_rejects_empty_entity_type() {
        let mut candidate = create_valid_entity_candidate();
        candidate.entity_type = "".to_string();
        let result = validate_entity_candidate(&candidate);
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("entity_type")));
    }

    #[test]
    fn validate_entity_candidate_rejects_empty_canonical_name() {
        let mut candidate = create_valid_entity_candidate();
        candidate.canonical_name = "".to_string();
        let result = validate_entity_candidate(&candidate);
        assert!(
            matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("canonical_name"))
        );
    }

    #[test]
    fn validate_entity_candidate_accepts_with_aliases() {
        let mut candidate = create_valid_entity_candidate();
        candidate.aliases = vec!["JD".to_string(), "Johnny".to_string()];
        assert!(validate_entity_candidate(&candidate).is_ok());
    }

    #[test]
    fn validate_fact_input_accepts_valid_input() {
        let result = validate_fact_input("note", "Test content", "Quote", "episode:123", "org");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_fact_input_rejects_empty_fact_type() {
        let result = validate_fact_input("", "content", "quote", "episode:123", "org");
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("fact_type")));
    }

    #[test]
    fn validate_fact_input_rejects_empty_content() {
        let result = validate_fact_input("note", "", "quote", "episode:123", "org");
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("content")));
    }

    #[test]
    fn validate_fact_input_rejects_empty_quote() {
        let result = validate_fact_input("note", "content", "", "episode:123", "org");
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("quote")));
    }

    #[test]
    fn validate_fact_input_rejects_empty_source_episode() {
        let result = validate_fact_input("note", "content", "quote", "", "org");
        assert!(
            matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("source_episode"))
        );
    }

    #[test]
    fn validate_fact_input_rejects_empty_scope() {
        let result = validate_fact_input("note", "content", "quote", "episode:123", "");
        assert!(matches!(result, Err(MemoryError::Validation(msg)) if msg.contains("scope")));
    }

    #[test]
    fn validate_fact_input_accepts_various_fact_types() {
        for fact_type in ["note", "metric", "promise", "decision"] {
            let result = validate_fact_input(fact_type, "content", "quote", "episode:123", "org");
            assert!(result.is_ok(), "Failed for fact_type: {}", fact_type);
        }
    }

    #[test]
    fn validate_fact_input_trims_whitespace() {
        let result = validate_fact_input("  note  ", "content", "quote", "episode:123", "org");
        assert!(result.is_ok());
    }
}
