use serde_json::Value;

use crate::models::Episode;
use crate::shared::temporal::parse_iso;
use crate::storage::value_helpers::{json_string, unwrap_array_value};

/// Parse an episode from a database record.
#[must_use]
pub fn episode_from_record(record: &serde_json::Map<String, Value>) -> Option<Episode> {
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
    })
}
