//! Community detection, BFS traversal, and summary generation.

use std::collections::{BTreeSet, HashSet, VecDeque};

use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::service::community::{
    CommunityMembership, CommunityRecord, is_entity_id, parse_community_record,
};
use crate::service::normalize_dt;
use crate::service::now;
use crate::service::parse_iso;
use crate::service::service_context::ServiceContext;
use crate::service::value_helpers::string_from_value;
use crate::storage::GraphDirection;

use super::edges::StoredEdgeVersion;

fn unwrap_string(value: &Value) -> Option<String> {
    string_from_value(value)
}

fn stored_edge_version_for_community(record: &Value) -> Option<StoredEdgeVersion> {
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

/// Update community memberships after entity changes.
pub(crate) async fn update_communities(
    service: &ServiceContext,
    entity_ids: &[String],
) -> Result<(), MemoryError> {
    if entity_ids.len() < 2 {
        return Ok(());
    }

    let member_entities = collect_connected_entity_component(service, entity_ids).await?;
    let Some(membership) = CommunityMembership::from_entities(member_entities) else {
        return Ok(());
    };

    let summary = build_community_summary(service, &membership.member_entities).await?;
    let overlapping = find_overlapping_communities(service, &membership.member_entities).await?;
    let payload = json!({
        "community_id": membership.community_id,
        "member_entities": membership.member_entities,
        "summary": summary,
        "updated_at": normalize_dt(now()),
    });

    service
        .app_store()
        .upsert_community(&membership.community_id, payload)
        .await?;

    for stale in overlapping
        .into_iter()
        .filter(|community| community.community_id != membership.community_id)
    {
        service
            .app_store()
            .delete_community(&stale.community_id)
            .await?;
    }

    Ok(())
}

/// BFS traversal over active edges to find all connected entities.
pub(crate) async fn collect_connected_entity_component(
    service: &ServiceContext,
    entity_ids: &[String],
) -> Result<Vec<String>, MemoryError> {
    let cutoff = normalize_dt(now());
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    let mut traversed_nodes = HashSet::new();

    for entity_id in entity_ids.iter().filter(|id| is_entity_id(id)) {
        if visited.insert(entity_id.clone()) {
            queue.push_back(entity_id.clone());
        }
    }

    while let Some(current) = queue.pop_front() {
        if !traversed_nodes.insert(current.clone()) {
            continue;
        }

        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            let edges = service
                .episode_store()
                .select_edge_neighbors(&current, &cutoff, direction)
                .await?;

            for edge in edges.iter().filter_map(stored_edge_version_for_community) {
                let neighbor = match direction {
                    GraphDirection::Incoming => edge.in_id,
                    GraphDirection::Outgoing => edge.out_id,
                };

                if is_entity_id(&neighbor) {
                    if visited.insert(neighbor.clone()) {
                        queue.push_back(neighbor);
                    }
                    continue;
                }

                if is_traversable_context_node(&neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    Ok(visited.into_iter().collect())
}

fn is_traversable_context_node(record_id: &str) -> bool {
    record_id.starts_with("episode:") || record_id.starts_with("fact:")
}

/// Build a human-readable summary of community members.
pub(crate) async fn build_community_summary(
    service: &ServiceContext,
    member_entities: &[String],
) -> Result<String, MemoryError> {
    let records = service
        .episode_store()
        .select_entities_by_ids(member_entities)
        .await?;
    let mut names = records
        .iter()
        .filter_map(|record| record.as_object())
        .filter_map(|record| {
            record
                .get("canonical_name")
                .and_then(unwrap_string)
                .or_else(|| record.get("entity_id").and_then(unwrap_string))
                .or_else(|| record.get("id").and_then(unwrap_string))
        })
        .collect::<Vec<_>>();

    names.sort();
    names.dedup();

    let labels = if names.is_empty() {
        let mut fallback = member_entities.to_vec();
        fallback.sort();
        fallback.dedup();
        fallback
    } else {
        names
    };

    Ok(condense_community_labels(&labels))
}

fn condense_community_labels(labels: &[String]) -> String {
    let preview = labels
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let remaining = labels.len().saturating_sub(3);
    if remaining > 0 {
        format!("{preview} (+{remaining} more)")
    } else {
        preview
    }
}

pub(crate) async fn find_overlapping_communities(
    service: &ServiceContext,
    member_entities: &[String],
) -> Result<Vec<CommunityRecord>, MemoryError> {
    let member_set: HashSet<_> = member_entities.iter().cloned().collect();

    let communities = service
        .episode_store()
        .select_communities_by_member_entities(member_entities)
        .await?;

    Ok(communities
        .iter()
        .filter_map(parse_community_record)
        .filter(|community| {
            community
                .member_entities
                .iter()
                .any(|member| member_set.contains(member))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_edge_version_for_community_handles_record_id_endpoints() {
        let record = json!({
            "edge_id": "edge:test",
            "in": {"RecordId": {"table": "entity", "key": "alice"}},
            "relation": "met",
            "out": {"RecordId": {"table": "entity", "key": "bob"}},
            "t_valid": "2026-04-11T16:00:00Z",
            "t_ingested": "2026-04-11T16:00:01Z"
        });

        let stored =
            stored_edge_version_for_community(&record).expect("stored community edge version");

        assert_eq!(stored.in_id, "entity:alice");
        assert_eq!(stored.out_id, "entity:bob");
    }
}
