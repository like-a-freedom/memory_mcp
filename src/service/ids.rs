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
