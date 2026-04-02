use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::service::episode::{episode_from_record, fact_from_record};
use crate::service::error::MemoryError;
use crate::storage::GraphDirection;

/// Returns JSON fallback per §2.5 for APP-01.
#[must_use]
pub fn inspector_fallback(target_type: &str, target_id: &str, data: &Value) -> Value {
    serde_json::json!({
        format!("{}_id", target_type): target_id,
        "content": data.get("content").or_else(|| data.get("canonical_name")).unwrap_or(&Value::Null),
        "state": data.get("state").unwrap_or(&Value::String("unknown".to_string())),
        "t_valid": data.get("t_valid").unwrap_or(&Value::Null),
        "confidence": data.get("confidence").unwrap_or(&Value::Null),
        "provenance": data.get("provenance").unwrap_or(&Value::Null),
    })
}

/// Read-oriented view model for APP-01.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectorView {
    pub session_id: String,
    pub target_type: String,
    pub target_id: String,
    pub entity: Option<EntityView>,
    pub fact: Option<FactView>,
    pub episode: Option<EpisodeView>,
    pub pagination: Option<PaginationState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationState {
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub total_visible: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityView {
    pub entity_id: String,
    pub entity_type: String,
    pub canonical_name: String,
    pub aliases: Vec<String>,
    pub facts: Vec<FactSummary>,
    pub edges: Vec<EdgeSummary>,
    pub communities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactView {
    pub fact_id: String,
    pub fact_type: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    pub confidence: f64,
    pub decayed_confidence: f64,
    pub provenance: Value,
    pub t_valid: chrono::DateTime<chrono::Utc>,
    pub t_ingested: chrono::DateTime<chrono::Utc>,
    pub t_invalid: Option<chrono::DateTime<chrono::Utc>>,
    pub t_invalid_ingested: Option<chrono::DateTime<chrono::Utc>>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeView {
    pub episode_id: String,
    pub source_type: String,
    pub source_id: String,
    pub t_ref: chrono::DateTime<chrono::Utc>,
    pub t_ingested: chrono::DateTime<chrono::Utc>,
    pub status: String,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub facts: Vec<FactSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactSummary {
    pub fact_id: String,
    pub content: String,
    pub confidence: f64,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSummary {
    pub edge_id: String,
    pub relation: String,
    pub target_id: String,
    pub confidence: f64,
    pub t_valid: chrono::DateTime<chrono::Utc>,
    pub t_invalid: Option<chrono::DateTime<chrono::Utc>>,
}

/// Derive a state badge string from a fact's decayed confidence and temporal validity.
fn fact_state(decayed: f64, t_invalid: Option<DateTime<Utc>>) -> String {
    if t_invalid.is_some() {
        "invalidated".to_string()
    } else if decayed >= 0.5 {
        "active".to_string()
    } else if decayed >= 0.1 {
        "stale".to_string()
    } else {
        "expired".to_string()
    }
}

/// Parse a numeric cursor string into an offset, defaulting to 0.
fn parse_cursor(cursor: Option<&str>) -> usize {
    cursor.and_then(|s| s.parse::<usize>().ok()).unwrap_or(0)
}

/// Helper: extract string from a JSON map field using `crate::storage::json_string`.
fn get_str<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(crate::storage::json_string)
}

/// Helper: extract f64 from a JSON map field using `crate::storage::json_f64`.
fn get_f64(map: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key).and_then(crate::storage::json_f64)
}

/// Build a [`FactSummary`] from a raw database record map.
fn fact_summary_from_map(m: &serde_json::Map<String, Value>, scope: &str) -> Option<FactSummary> {
    let fact_id = get_str(m, "fact_id")?.to_string();
    let content = get_str(m, "content").unwrap_or_default().to_string();
    let confidence = get_f64(m, "confidence").unwrap_or(0.0);
    let t_valid = get_str(m, "t_valid").and_then(crate::service::parse_iso)?;
    let fact_type = get_str(m, "fact_type").unwrap_or_default().to_string();
    let t_invalid = get_str(m, "t_invalid").and_then(crate::service::parse_iso);

    let dummy = crate::models::Fact {
        fact_id: fact_id.clone(),
        fact_type,
        content: content.clone(),
        quote: String::new(),
        source_episode: String::new(),
        t_ref: Some(t_valid),
        t_valid,
        t_ingested: t_valid,
        t_invalid,
        t_invalid_ingested: None,
        confidence,
        index_keys: vec![],
        access_count: 0,
        last_accessed: None,
        entity_links: vec![],
        scope: scope.to_string(),
        policy_tags: vec![],
        provenance: Value::Null,
        ft_score: 0.0,
    };
    let decayed = crate::service::decayed_confidence(&dummy, crate::service::now());
    let state = fact_state(decayed, t_invalid);

    Some(FactSummary {
        fact_id,
        content,
        confidence,
        state,
    })
}

/// Build an [`EdgeSummary`] from a raw database record map and the target node direction.
fn edge_summary_from_map(
    map: &serde_json::Map<String, Value>,
    target_id: String,
) -> Option<EdgeSummary> {
    let edge_id = get_str(map, "edge_id")
        .or_else(|| get_str(map, "id"))
        .unwrap_or_default()
        .to_string();
    let relation = get_str(map, "relation").unwrap_or_default().to_string();
    let confidence = get_f64(map, "confidence").unwrap_or(0.0);
    let t_valid = get_str(map, "t_valid")
        .and_then(crate::service::parse_iso)
        .unwrap_or_else(crate::service::now);
    let t_invalid = get_str(map, "t_invalid").and_then(crate::service::parse_iso);

    Some(EdgeSummary {
        edge_id,
        relation,
        target_id,
        confidence,
        t_valid,
        t_invalid,
    })
}

/// Opens an entity inspector view, fetching the entity record, related facts
/// (paginated), edges in both directions, and community memberships.
///
/// # Errors
///
/// Returns [`MemoryError::NotFound`] if the entity does not exist.
/// Returns [`MemoryError::Storage`] on database failures.
pub async fn open_entity(
    service: &crate::service::MemoryService,
    session_id: &str,
    entity_id: &str,
    page_size: i32,
    cursor: Option<&str>,
) -> Result<InspectorView, MemoryError> {
    let (record, namespace) = find_entity_record_any_namespace(service, entity_id).await?;
    let namespace =
        namespace.ok_or_else(|| MemoryError::NotFound(format!("entity not found: {entity_id}")))?;
    let record =
        record.ok_or_else(|| MemoryError::NotFound(format!("entity not found: {entity_id}")))?;

    let entity_type = get_str(&record, "entity_type")
        .unwrap_or_default()
        .to_string();
    let canonical_name = get_str(&record, "canonical_name")
        .unwrap_or(entity_id)
        .to_string();
    let aliases: Vec<String> = record
        .get("aliases")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(crate::storage::json_string)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let scope = get_str(&record, "scope").unwrap_or(&namespace).to_string();

    // Fetch related facts (paginated)
    let offset = parse_cursor(cursor);
    let fetch_limit = page_size + 1;
    let cutoff = crate::service::normalize_dt(crate::service::now());

    let fact_records = service
        .db_client
        .select_facts_by_entity_links(
            &namespace,
            &scope,
            &cutoff,
            &[entity_id.to_string()],
            fetch_limit,
        )
        .await?;

    let has_more = fact_records.len() > page_size as usize;
    let facts: Vec<FactSummary> = fact_records
        .iter()
        .filter_map(|v| v.as_object())
        .skip(offset)
        .take(page_size as usize)
        .filter_map(|m| fact_summary_from_map(m, &scope))
        .collect();

    let total_visible = facts.len();

    // Fetch edges in both directions
    let mut edges = Vec::new();
    for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
        let edge_records = service
            .db_client
            .select_edge_neighbors(&namespace, entity_id, &cutoff, direction)
            .await?;
        for record in &edge_records {
            let Some(map) = record.as_object() else {
                continue;
            };
            let in_id = get_str(map, "in").unwrap_or_default().to_string();
            let out_id = get_str(map, "out").unwrap_or_default().to_string();

            let target_id = match direction {
                GraphDirection::Incoming => in_id,
                GraphDirection::Outgoing => out_id,
            };
            if target_id == entity_id {
                continue;
            }

            if let Some(summary) = edge_summary_from_map(map, target_id) {
                edges.push(summary);
            }
        }
    }

    // Fetch communities
    let communities_result = service
        .db_client
        .select_communities_by_member_entities(&namespace, &[entity_id.to_string()])
        .await?;
    let community_ids: Vec<String> = communities_result
        .iter()
        .filter_map(|v| v.as_object())
        .filter_map(|m| {
            get_str(m, "community_id")
                .or_else(|| get_str(m, "id"))
                .map(String::from)
        })
        .collect();

    let entity_view = EntityView {
        entity_id: entity_id.to_string(),
        entity_type,
        canonical_name,
        aliases,
        facts: facts.clone(),
        edges,
        communities: community_ids,
    };

    Ok(InspectorView {
        session_id: session_id.to_string(),
        target_type: "entity".to_string(),
        target_id: entity_id.to_string(),
        entity: Some(entity_view),
        fact: None,
        episode: None,
        pagination: Some(PaginationState {
            has_more,
            next_cursor: if has_more {
                Some((offset + page_size as usize).to_string())
            } else {
                None
            },
            total_visible,
        }),
    })
}

/// Opens a fact inspector view, fetching the fact record and computing
/// the decayed confidence and temporal state badge.
///
/// # Errors
///
/// Returns [`MemoryError::NotFound`] if the fact does not exist.
/// Returns [`MemoryError::Storage`] on database failures.
pub async fn open_fact(
    service: &crate::service::MemoryService,
    session_id: &str,
    fact_id: &str,
) -> Result<InspectorView, MemoryError> {
    let (record, _namespace) = service.find_fact_record(fact_id).await?;
    let record =
        record.ok_or_else(|| MemoryError::NotFound(format!("fact not found: {fact_id}")))?;

    let value = Value::Object(record);
    let fact = fact_from_record(&value)
        .ok_or_else(|| MemoryError::NotFound(format!("fact not found: {fact_id}")))?;

    let decayed = crate::service::decayed_confidence(&fact, crate::service::now());
    let state = fact_state(decayed, fact.t_invalid);

    let fact_view = FactView {
        fact_id: fact.fact_id,
        fact_type: fact.fact_type,
        content: fact.content,
        quote: fact.quote,
        source_episode: fact.source_episode,
        confidence: fact.confidence,
        decayed_confidence: decayed,
        provenance: fact.provenance,
        t_valid: fact.t_valid,
        t_ingested: fact.t_ingested,
        t_invalid: fact.t_invalid,
        t_invalid_ingested: fact.t_invalid_ingested,
        state,
    };

    Ok(InspectorView {
        session_id: session_id.to_string(),
        target_type: "fact".to_string(),
        target_id: fact_id.to_string(),
        entity: None,
        fact: Some(fact_view),
        episode: None,
        pagination: None,
    })
}

/// Opens an episode inspector view, fetching the episode record and its
/// related facts (paginated).
///
/// # Errors
///
/// Returns [`MemoryError::NotFound`] if the episode does not exist.
/// Returns [`MemoryError::Storage`] on database failures.
pub async fn open_episode(
    service: &crate::service::MemoryService,
    session_id: &str,
    episode_id: &str,
    page_size: i32,
    cursor: Option<&str>,
) -> Result<InspectorView, MemoryError> {
    let (record, namespace) = service.find_episode_record(episode_id).await?;
    let namespace = namespace
        .ok_or_else(|| MemoryError::NotFound(format!("episode not found: {episode_id}")))?;
    let record =
        record.ok_or_else(|| MemoryError::NotFound(format!("episode not found: {episode_id}")))?;

    let episode = episode_from_record(&record)
        .ok_or_else(|| MemoryError::NotFound(format!("episode not found: {episode_id}")))?;

    // Extract status and archived_at from the raw record (not part of Episode model)
    let status = get_str(&record, "status").unwrap_or("active").to_string();
    let archived_at = get_str(&record, "archived_at").and_then(crate::service::parse_iso);

    // Fetch related facts (paginated)
    let offset = parse_cursor(cursor);
    let fetch_limit = page_size + 1;
    let cutoff = crate::service::normalize_dt(crate::service::now());

    let fact_records = service
        .db_client
        .select_active_facts_by_episode(&namespace, episode_id, &cutoff, fetch_limit)
        .await?;

    let has_more = fact_records.len() > page_size as usize;
    let facts: Vec<FactSummary> = fact_records
        .iter()
        .filter_map(|v| v.as_object())
        .skip(offset)
        .take(page_size as usize)
        .filter_map(|m| fact_summary_from_map(m, &episode.scope))
        .collect();

    let total_visible = facts.len();

    let episode_view = EpisodeView {
        episode_id: episode.episode_id,
        source_type: episode.source_type,
        source_id: episode.source_id,
        t_ref: episode.t_ref,
        t_ingested: episode.t_ingested,
        status,
        archived_at,
        facts: facts.clone(),
    };

    Ok(InspectorView {
        session_id: session_id.to_string(),
        target_type: "episode".to_string(),
        target_id: episode_id.to_string(),
        entity: None,
        fact: None,
        episode: Some(episode_view),
        pagination: Some(PaginationState {
            has_more,
            next_cursor: if has_more {
                Some((offset + page_size as usize).to_string())
            } else {
                None
            },
            total_visible,
        }),
    })
}

/// Search all namespaces for an entity record by ID.
async fn find_entity_record_any_namespace(
    service: &crate::service::MemoryService,
    entity_id: &str,
) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
    for namespace in &service.namespaces {
        let record = service.db_client.select_one(entity_id, namespace).await?;
        if let Some(Value::Object(map)) = record {
            return Ok((Some(map), Some(namespace.clone())));
        }
    }
    Ok((None, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fact_state_returns_invalidated_when_t_invalid_present() {
        let dt = Utc::now();
        assert_eq!(fact_state(0.9, Some(dt)), "invalidated");
    }

    #[test]
    fn fact_state_returns_active_when_decayed_above_threshold() {
        assert_eq!(fact_state(0.8, None), "active");
        assert_eq!(fact_state(0.5, None), "active");
    }

    #[test]
    fn fact_state_returns_stale_when_decayed_between_thresholds() {
        assert_eq!(fact_state(0.3, None), "stale");
        assert_eq!(fact_state(0.1, None), "stale");
    }

    #[test]
    fn fact_state_returns_expired_when_decayed_below_threshold() {
        assert_eq!(fact_state(0.05, None), "expired");
        assert_eq!(fact_state(0.0, None), "expired");
    }

    #[test]
    fn parse_cursor_defaults_to_zero() {
        assert_eq!(parse_cursor(None), 0);
        assert_eq!(parse_cursor(Some("invalid")), 0);
        assert_eq!(parse_cursor(Some("10")), 10);
    }

    #[test]
    fn inspector_fallback_includes_all_fields() {
        let data = json!({
            "content": "test content",
            "state": "active",
            "confidence": 0.9,
        });
        let val = inspector_fallback("entity", "e:1", &data);
        assert_eq!(val["entity_id"], "e:1");
        assert_eq!(val["state"], "active");
    }

    #[test]
    fn inspector_fallback_handles_missing_fields() {
        let data = json!({});
        let val = inspector_fallback("fact", "f:1", &data);
        assert_eq!(val["fact_id"], "f:1");
        assert!(val["state"].is_string());
    }

    #[test]
    fn inspector_fallback_uses_canonical_name_when_content_missing() {
        let data = json!({"canonical_name": "Alice"});
        let val = inspector_fallback("entity", "e:1", &data);
        assert_eq!(val["content"], "Alice");
    }

    #[test]
    fn get_str_extracts_from_map() {
        let map =
            serde_json::Map::from_iter([("name".to_string(), Value::String("test".to_string()))]);
        assert_eq!(get_str(&map, "name"), Some("test"));
        assert_eq!(get_str(&map, "missing"), None);
    }

    #[test]
    fn get_f64_extracts_from_map() {
        let map = serde_json::Map::from_iter([(
            "score".to_string(),
            Value::Number(serde_json::Number::from_f64(0.75).unwrap()),
        )]);
        assert_eq!(get_f64(&map, "score"), Some(0.75));
        assert_eq!(get_f64(&map, "missing"), None);
    }

    #[test]
    fn edge_summary_from_map_builds_complete_summary() {
        let mut map = serde_json::Map::new();
        map.insert("edge_id".to_string(), Value::String("edge:1".to_string()));
        map.insert(
            "relation".to_string(),
            Value::String("related_to".to_string()),
        );
        map.insert(
            "confidence".to_string(),
            Value::Number(serde_json::Number::from_f64(0.8).unwrap()),
        );

        let summary = edge_summary_from_map(&map, "entity:t".to_string());
        assert!(summary.is_some());
        let s = summary.unwrap();
        assert_eq!(s.edge_id, "edge:1");
        assert_eq!(s.relation, "related_to");
        assert_eq!(s.confidence, 0.8);
        assert_eq!(s.target_id, "entity:t");
    }

    #[test]
    fn edge_summary_from_map_handles_missing_fields() {
        let map = serde_json::Map::new();
        let summary = edge_summary_from_map(&map, "entity:t".to_string());
        assert!(summary.is_some());
        let s = summary.unwrap();
        assert!(s.edge_id.is_empty());
        assert_eq!(s.relation, "");
        assert_eq!(s.confidence, 0.0);
    }

    #[test]
    fn edge_summary_from_map_uses_id_fallback() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), Value::String("edge:fallback".to_string()));
        map.insert("relation".to_string(), Value::String("owns".to_string()));

        let summary = edge_summary_from_map(&map, "entity:t".to_string());
        assert!(summary.is_some());
        assert_eq!(summary.unwrap().edge_id, "edge:fallback");
    }

    #[test]
    fn inspector_view_serializes() {
        let view = InspectorView {
            session_id: "s1".to_string(),
            target_type: "entity".to_string(),
            target_id: "e:1".to_string(),
            entity: None,
            fact: None,
            episode: None,
            pagination: Some(PaginationState {
                has_more: false,
                next_cursor: None,
                total_visible: 0,
            }),
        };
        let val = serde_json::to_value(&view).unwrap();
        assert_eq!(val["target_type"], "entity");
    }

    #[test]
    fn pagination_state_serializes() {
        let state = PaginationState {
            has_more: true,
            next_cursor: Some("50".to_string()),
            total_visible: 25,
        };
        let val = serde_json::to_value(&state).unwrap();
        assert!(val["has_more"].as_bool().unwrap());
        assert_eq!(val["next_cursor"], "50");
    }

    #[test]
    fn fact_state_boundary_at_half() {
        assert_eq!(fact_state(0.5, None), "active");
        assert_eq!(fact_state(0.49, None), "stale");
    }

    #[test]
    fn fact_state_boundary_at_tenth() {
        assert_eq!(fact_state(0.1, None), "stale");
        assert_eq!(fact_state(0.09, None), "expired");
    }

    #[test]
    fn fact_state_invalidated_overrides_all() {
        let dt = Utc::now();
        assert_eq!(fact_state(1.0, Some(dt)), "invalidated");
        assert_eq!(fact_state(0.0, Some(dt)), "invalidated");
    }
}
