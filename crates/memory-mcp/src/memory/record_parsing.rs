use serde_json::Value;

use crate::models::{Episode, SourceContext};
use crate::shared::temporal::parse_iso;
use crate::storage::value_helpers::{json_i64, json_string, unwrap_array_value};

/// Parse an episode from a database record.
#[must_use]
pub fn episode_from_record(record: &serde_json::Map<String, Value>) -> Option<Episode> {
    let source_context = match (
        record.get("source_group").and_then(json_string),
        record
            .get("source_position")
            .and_then(json_i64)
            .and_then(|position| u64::try_from(position).ok()),
    ) {
        (Some(group), Some(position)) => Some(SourceContext {
            group: group.to_string(),
            position,
        }),
        (None, None) => None,
        _ => return None,
    };
    Some(Episode {
        episode_id: json_string(record.get("episode_id")?)?.to_string(),
        source_type: json_string(record.get("source_type")?)?.to_string(),
        source_id: json_string(record.get("source_id")?)?.to_string(),
        content: json_string(record.get("content")?)?.to_string(),
        t_ref: parse_iso(json_string(record.get("t_ref")?)?)?,
        t_ingested: parse_iso(json_string(record.get("t_ingested")?)?)?,
        // Legacy partition fields are optional in the scope-free schema. They
        // remain readable for old records but are never used for routing.
        scope: record
            .get("scope")
            .and_then(json_string)
            .unwrap_or_default()
            .to_string(),
        visibility_scope: record
            .get("visibility_scope")
            .and_then(json_string)
            .unwrap_or_default()
            .to_string(),
        policy_tags: record
            .get("policy_tags")
            .and_then(unwrap_array_value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(json_string)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        source_lineage: record
            .get("source_lineage")
            .and_then(json_string)
            .map(ToString::to_string),
        source_context,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn episode_from_record_parses_full_record() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));
        record.insert("source_type".to_string(), json!("email"));
        record.insert("source_id".to_string(), json!("msg-123"));
        record.insert("content".to_string(), json!("Test content"));
        record.insert("t_ref".to_string(), json!("2024-01-15T10:30:00Z"));
        record.insert("t_ingested".to_string(), json!("2024-01-15T10:31:00Z"));
        record.insert("scope".to_string(), json!("org"));
        record.insert("visibility_scope".to_string(), json!("org"));
        record.insert("policy_tags".to_string(), json!(["tag1", "tag2"]));

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.episode_id, "episode:test123");
        assert_eq!(episode.source_type, "email");
        assert_eq!(episode.source_id, "msg-123");
        assert_eq!(episode.content, "Test content");
        assert_eq!(episode.scope, "org");
        assert_eq!(episode.visibility_scope, "org");
        assert_eq!(episode.policy_tags, vec!["tag1", "tag2"]);
    }

    #[test]
    fn episode_from_record_returns_none_for_missing_required_field() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));

        assert!(episode_from_record(&record).is_none());
    }

    #[test]
    fn episode_from_record_returns_none_for_malformed_reference_timestamp() {
        let record = json!({
            "episode_id": "episode:malformed-time",
            "source_type": "document",
            "source_id": "malformed-time",
            "content": "an episode with an invalid stored timestamp",
            "t_ref": "not-a-datetime",
            "t_ingested": "2026-10-14T10:00:00Z",
        });

        assert!(episode_from_record(record.as_object().expect("object record")).is_none());
    }

    #[test]
    fn episode_from_record_handles_wrapped_string_values() {
        let mut record = serde_json::Map::new();
        record.insert(
            "episode_id".to_string(),
            json!({"String": "episode:test123"}),
        );
        record.insert("source_type".to_string(), json!({"String": "email"}));
        record.insert("source_id".to_string(), json!({"String": "msg-123"}));
        record.insert("content".to_string(), json!({"String": "Test"}));
        record.insert(
            "t_ref".to_string(),
            json!({"String": "2024-01-15T10:30:00Z"}),
        );
        record.insert(
            "t_ingested".to_string(),
            json!({"String": "2024-01-15T10:31:00Z"}),
        );
        record.insert("scope".to_string(), json!({"String": "org"}));
        record.insert(
            "policy_tags".to_string(),
            json!({"Array": [{"String": "tag1"}]}),
        );

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.episode_id, "episode:test123");
        assert_eq!(episode.policy_tags, vec!["tag1"]);
    }

    #[test]
    fn episode_from_record_uses_defaults_for_optional_fields() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));
        record.insert("source_type".to_string(), json!("email"));
        record.insert("source_id".to_string(), json!("msg-123"));
        record.insert("content".to_string(), json!("Test"));
        record.insert("t_ref".to_string(), json!("2024-01-15T10:30:00Z"));
        record.insert("t_ingested".to_string(), json!("2024-01-15T10:31:00Z"));
        record.insert("scope".to_string(), json!("org"));

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.visibility_scope, "");
        assert!(episode.policy_tags.is_empty());
    }
}
