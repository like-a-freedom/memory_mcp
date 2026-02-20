//! Deterministic ID generation utilities.

use sha2::{Digest, Sha256};

/// Generate a 24-character hex hash prefix.
#[must_use]
pub fn hash_prefix(payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)[..24].to_string()
}

/// Generate a deterministic episode ID.
#[must_use]
pub fn deterministic_episode_id(
    source_type: &str,
    source_id: &str,
    t_ref: chrono::DateTime<chrono::Utc>,
    scope: &str,
) -> String {
    let payload = format!(
        "{}|{}|{}|{}",
        super::normalize_text(source_type),
        super::normalize_text(source_id),
        super::normalize_dt(t_ref),
        super::normalize_text(scope),
    );
    format!("episode:{}", hash_prefix(&payload))
}

/// Generate a deterministic entity ID.
#[must_use]
pub fn deterministic_entity_id(entity_type: &str, canonical_name: &str) -> String {
    let payload = format!(
        "{}|{}",
        super::normalize_text(entity_type),
        super::normalize_text(canonical_name)
    );
    format!("entity:{}", hash_prefix(&payload))
}

/// Generate a deterministic fact ID.
#[must_use]
pub fn deterministic_fact_id(
    fact_type: &str,
    content: &str,
    source_episode: &str,
    t_valid: chrono::DateTime<chrono::Utc>,
) -> String {
    let payload = format!(
        "{}|{}|{}|{}",
        super::normalize_text(fact_type),
        super::normalize_text(content),
        super::normalize_text(source_episode),
        super::normalize_dt(t_valid),
    );
    format!("fact:{}", hash_prefix(&payload))
}

/// Generate a deterministic community ID.
#[must_use]
pub fn deterministic_community_id(member_entities: &[String]) -> String {
    let mut members = member_entities.to_vec();
    members.sort();
    format!("community:{}", hash_prefix(&members.join("|")))
}

/// Generate a deterministic edge ID.
#[must_use]
pub fn deterministic_edge_id(
    from_id: &str,
    relation: &str,
    to_id: &str,
    t_valid: chrono::DateTime<chrono::Utc>,
) -> String {
    let payload = format!(
        "{}|{}|{}|{}",
        super::normalize_text(from_id),
        super::normalize_text(relation),
        super::normalize_text(to_id),
        super::normalize_dt(t_valid),
    );
    format!("edge:{}", hash_prefix(&payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn hash_prefix_produces_24_char_hex() {
        let hash = hash_prefix("test payload");
        assert_eq!(hash.len(), 24);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_prefix_is_deterministic() {
        let hash1 = hash_prefix("test");
        let hash2 = hash_prefix("test");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn hash_prefix_differs_for_different_inputs() {
        let hash1 = hash_prefix("test1");
        let hash2 = hash_prefix("test2");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn deterministic_episode_id_starts_with_prefix() {
        let t_ref = Utc::now();
        let id = deterministic_episode_id("email", "msg-123", t_ref, "org");
        assert!(id.starts_with("episode:"));
    }

    #[test]
    fn deterministic_episode_id_is_deterministic() {
        let t_ref = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let id1 = deterministic_episode_id("email", "msg-123", t_ref, "org");
        let id2 = deterministic_episode_id("email", "msg-123", t_ref, "org");
        assert_eq!(id1, id2);
    }

    #[test]
    fn deterministic_episode_id_normalizes_input() {
        let t_ref = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let id1 = deterministic_episode_id("Email", "MSG-123", t_ref, "ORG");
        let id2 = deterministic_episode_id("email", "msg-123", t_ref, "org");
        assert_eq!(id1, id2);
    }

    #[test]
    fn deterministic_entity_id_starts_with_prefix() {
        let id = deterministic_entity_id("person", "John Doe");
        assert!(id.starts_with("entity:"));
    }

    #[test]
    fn deterministic_entity_id_is_deterministic() {
        let id1 = deterministic_entity_id("person", "John Doe");
        let id2 = deterministic_entity_id("person", "John Doe");
        assert_eq!(id1, id2);
    }

    #[test]
    fn deterministic_fact_id_starts_with_prefix() {
        let t_valid = Utc::now();
        let id = deterministic_fact_id("metric", "ARR $5M", "episode:1", t_valid);
        assert!(id.starts_with("fact:"));
    }

    #[test]
    fn deterministic_community_id_sorts_members() {
        let members = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        let id1 = deterministic_community_id(&members);
        let members2 = vec!["b".to_string(), "c".to_string(), "a".to_string()];
        let id2 = deterministic_community_id(&members2);
        assert_eq!(id1, id2);
    }

    #[test]
    fn deterministic_edge_id_starts_with_prefix() {
        let t_valid = Utc::now();
        let id = deterministic_edge_id("entity:1", "knows", "entity:2", t_valid);
        assert!(id.starts_with("edge:"));
    }
}
