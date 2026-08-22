//! Edge storage, versioning, and conflict resolution.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::models::Edge;
use crate::service::ids;
use crate::service::normalize_dt;
use crate::service::parse_iso;
use crate::service::service_context::ServiceContext;
use crate::service::value_helpers::string_from_value;

/// Payload map for edge database records.
pub(crate) fn build_edge_payload(edge: &Edge, edge_id: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("edge_id".to_string(), Value::String(edge_id.to_string()));
    m.insert("in".to_string(), Value::String(edge.in_id.clone()));
    m.insert("relation".to_string(), Value::String(edge.relation.clone()));
    m.insert("out".to_string(), Value::String(edge.out_id.clone()));
    m.insert("origin".to_string(), json!(edge.origin));
    m.insert("strength".to_string(), json!(edge.strength));
    m.insert("confidence".to_string(), json!(edge.confidence));
    m.insert("provenance".to_string(), edge.provenance.to_json_value());
    m.insert(
        "t_valid".to_string(),
        Value::String(normalize_dt(edge.t_valid)),
    );
    m.insert(
        "t_ingested".to_string(),
        Value::String(normalize_dt(edge.t_ingested)),
    );
    if let Some(t_invalid) = edge.t_invalid {
        m.insert(
            "t_invalid".to_string(),
            Value::String(normalize_dt(t_invalid)),
        );
    }
    if let Some(t_invalid_ingested) = edge.t_invalid_ingested {
        m.insert(
            "t_invalid_ingested".to_string(),
            Value::String(normalize_dt(t_invalid_ingested)),
        );
    }
    m
}

/// Persist a new edge after confirming it does not already exist.
pub(crate) async fn store_edge(service: &ServiceContext, edge: &Edge) -> Result<(), MemoryError> {
    let edge_id =
        ids::deterministic_edge_id(&edge.in_id, &edge.relation, &edge.out_id, edge.t_valid);

    let existing = service.episode_store().select_one(&edge_id).await?;
    if existing.is_some() {
        return Ok(());
    }

    invalidate_conflicting_edges(service, edge).await?;

    let payload = build_edge_payload(edge, &edge_id);

    service
        .episode_store()
        .relate_edge(&edge_id, &edge.in_id, &edge.out_id, Value::Object(payload))
        .await?;

    Ok(())
}

/// Represents a persisted edge version for conflict detection.
#[derive(Debug)]
pub(crate) struct StoredEdgeVersion {
    pub(crate) edge_id: String,
    pub(crate) in_id: String,
    pub(crate) relation: String,
    pub(crate) out_id: String,
    pub(crate) t_valid: DateTime<Utc>,
    pub(crate) t_ingested: DateTime<Utc>,
    pub(crate) t_invalid: Option<DateTime<Utc>>,
    pub(crate) t_invalid_ingested: Option<DateTime<Utc>>,
}

async fn invalidate_conflicting_edges(
    service: &ServiceContext,
    new_edge: &Edge,
) -> Result<(), MemoryError> {
    let existing_edges = service
        .episode_store()
        .select_edges_for_triple(&new_edge.in_id, &new_edge.relation, &new_edge.out_id)
        .await?;

    for existing in existing_edges
        .iter()
        .filter_map(stored_edge_version_from_record)
        .filter(|existing| edge_versions_conflict(existing, new_edge))
    {
        // ADR-0039: supersession closes the old edge with the new edge's
        // t_valid/t_ingested so the audit trail records the superseding
        // version's times — caller-supplied timestamps through the close owner.
        service
            .close_store()
            .close_record(
                &existing.edge_id,
                &crate::storage::CloseTimestamps::at_pair(new_edge.t_valid, new_edge.t_ingested),
                None,
            )
            .await?;
    }

    Ok(())
}

/// Detect version conflicts: BOTH t_valid AND t_ingested must be <= for invalidation.
pub(crate) fn edge_versions_conflict(existing: &StoredEdgeVersion, new_edge: &Edge) -> bool {
    existing.in_id == new_edge.in_id
        && existing.relation == new_edge.relation
        && existing.out_id == new_edge.out_id
        && existing.t_valid <= new_edge.t_valid
        && existing.t_ingested <= new_edge.t_ingested
        && existing.t_invalid.is_none_or(|t| t > new_edge.t_valid)
        && existing
            .t_invalid_ingested
            .is_none_or(|t| t > new_edge.t_ingested)
}

fn unwrap_string(value: &Value) -> Option<String> {
    string_from_value(value)
}

fn stored_edge_version_from_record(record: &Value) -> Option<StoredEdgeVersion> {
    let map = record.as_object()?;

    let edge_id = map
        .get("edge_id")
        .and_then(unwrap_string)
        .or_else(|| map.get("id").and_then(unwrap_string))?;

    Some(StoredEdgeVersion {
        edge_id,
        in_id: map.get("in").and_then(unwrap_string)?,
        relation: map.get("relation").and_then(unwrap_string)?,
        out_id: map.get("out").and_then(unwrap_string)?,
        t_valid: map
            .get("t_valid")
            .and_then(unwrap_string)
            .as_deref()
            .and_then(parse_iso)?,
        t_ingested: map
            .get("t_ingested")
            .and_then(unwrap_string)
            .as_deref()
            .and_then(parse_iso)?,
        t_invalid: map
            .get("t_invalid")
            .and_then(unwrap_string)
            .as_deref()
            .and_then(parse_iso),
        t_invalid_ingested: map
            .get("t_invalid_ingested")
            .and_then(unwrap_string)
            .as_deref()
            .and_then(parse_iso),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_edge_version_from_record_handles_record_id_endpoints() {
        let record = json!({
            "edge_id": "edge:test",
            "in": {"RecordId": {"table": "entity", "key": "alice"}},
            "relation": "knows",
            "out": {"RecordId": {"table": "entity", "key": "bob"}},
            "t_valid": "2026-04-11T16:00:00Z",
            "t_ingested": "2026-04-11T16:00:01Z"
        });

        let stored = stored_edge_version_from_record(&record).expect("stored edge version");

        assert_eq!(stored.in_id, "entity:alice");
        assert_eq!(stored.out_id, "entity:bob");
    }
}
