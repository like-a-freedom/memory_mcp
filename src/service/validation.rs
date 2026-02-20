//! Validation helpers for service operations.

use super::error::MemoryError;
use crate::models::{EntityCandidate, IngestRequest};

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
