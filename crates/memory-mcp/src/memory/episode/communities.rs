//! Community detection, BFS traversal, and summary generation.

use std::collections::{BTreeSet, HashSet, VecDeque};

use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::knowledge::community::{
    CommunityMembership, CommunityRecord, is_entity_id, parse_community_record,
};
use crate::memory::capabilities::deps::ExtractDeps;
use crate::shared::temporal::normalize_dt;
use crate::shared::temporal::now;
use crate::storage::GraphDirection;
use crate::storage::value_helpers::unwrap_string;

fn community_edge_endpoints(record: &Value) -> Option<(String, String)> {
    let map = record.as_object()?;
    let in_id = map.get("in").and_then(unwrap_string)?;
    let out_id = map.get("out").and_then(unwrap_string)?;
    Some((in_id, out_id))
}

/// Update community memberships after entity changes.
///
/// `new_node_ids` are the episode/fact records this extract just wrote. They
/// seed the component walk so the traversal reaches this extract's fresh
/// neighborhood without re-walking historical entity subgraphs that are
/// already recorded in stored community membership.
pub(crate) async fn update_communities(
    service: &ExtractDeps,
    entity_ids: &[String],
    new_node_ids: &[String],
) -> Result<(), MemoryError> {
    if entity_ids.len() < 2 {
        return Ok(());
    }

    let member_entities =
        collect_connected_entity_component(service, entity_ids, new_node_ids).await?;
    let Some(membership) = CommunityMembership::from_entities(member_entities) else {
        return Ok(());
    };

    let summary = crate::memory::community_summary::build_community_summary(
        &service.db_client,
        &service.active_namespace,
        &membership.member_entities,
    )
    .await?;
    let overlapping = find_overlapping_communities(service, &membership.member_entities).await?;
    let payload = json!({
        "community_id": membership.community_id,
        "member_entities": membership.member_entities,
        "summary": summary,
        "updated_at": normalize_dt(now()),
    });

    service
        .knowledge_graph_store()
        .upsert_community(&membership.community_id, payload)
        .await?;

    for stale in overlapping
        .into_iter()
        .filter(|community| community.community_id != membership.community_id)
    {
        service
            .knowledge_graph_store()
            .delete_community(&stale.community_id)
            .await?;
    }

    Ok(())
}

/// BFS traversal over active edges to find all connected entities.
///
/// The walk is incremental: entities already recorded in a stored community
/// membership are seeded into the visited set directly and never expanded.
/// That keeps per-extract statement count proportional to the fresh
/// neighborhood (touched entities plus `new_node_ids`) instead of the whole
/// accumulated component. The pruning is safe because
///
/// * stored communities are member-disjoint (`update_communities` merges
///   overlapping ones and deletes the stale rows), and
/// * the active edge graph is monotone under supersession
///   (`invalidate_conflicting_edges` closes same-triple versions only, so
///   stored community members stay pairwise connected), and
/// * every new edge this extract wrote is incident to a touched entity or a
///   node in `new_node_ids`, so the fresh neighborhood is fully reachable from
///   the initial queue.
///
/// Fresh entities (never stored in a community) keep the historical full
/// walk; their private neighborhoods are the only subgraphs that can exist
/// outside recorded community membership.
pub(crate) async fn collect_connected_entity_component(
    service: &ExtractDeps,
    entity_ids: &[String],
    new_node_ids: &[String],
) -> Result<Vec<String>, MemoryError> {
    let cutoff = normalize_dt(now());
    let known_connected = known_connected_entities(service, entity_ids).await?;

    let mut visited: BTreeSet<String> = known_connected.iter().cloned().collect();
    let mut queue = VecDeque::new();
    let mut queued: HashSet<String> = HashSet::new();

    // This extract's fresh nodes are always walked: they carry the new edges.
    for node_id in new_node_ids {
        if queued.insert(node_id.clone()) {
            queue.push_back(node_id.clone());
        }
    }

    // Touched entities outside recorded community membership keep the
    // historical walk; members of stored communities are already seeded.
    for entity_id in entity_ids.iter().filter(|id| is_entity_id(id)) {
        if visited.insert(entity_id.clone())
            && !known_connected.contains(entity_id)
            && queued.insert(entity_id.clone())
        {
            queue.push_back(entity_id.clone());
        }
    }

    while let Some(current) = queue.pop_front() {
        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            let edges = crate::knowledge::graph_store::KnowledgeGraphStore::new(
                service.db_client.clone(),
                service.active_namespace.clone(),
            )
            .select_edge_neighbors(&current, &cutoff, direction)
            .await?;

            for (in_id, out_id) in edges.iter().filter_map(community_edge_endpoints) {
                let neighbor = match direction {
                    GraphDirection::Incoming => in_id,
                    GraphDirection::Outgoing => out_id,
                };

                if is_entity_id(&neighbor) {
                    if visited.insert(neighbor.clone())
                        && !known_connected.contains(&neighbor)
                        && queued.insert(neighbor.clone())
                    {
                        queue.push_back(neighbor);
                    }
                    continue;
                }

                if is_traversable_context_node(&neighbor) && queued.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    Ok(visited.into_iter().collect())
}

/// Entities already recorded in a stored community that overlaps the touched
/// entities. These subgraphs are known-connected and must not be re-walked.
async fn known_connected_entities(
    service: &ExtractDeps,
    entity_ids: &[String],
) -> Result<HashSet<String>, MemoryError> {
    let communities = crate::knowledge::graph_store::KnowledgeGraphStore::new(
        service.db_client.clone(),
        service.active_namespace.clone(),
    )
    .select_communities_by_member_entities(entity_ids)
    .await?;

    Ok(communities
        .iter()
        .filter_map(parse_community_record)
        .flat_map(|community| community.member_entities)
        .collect())
}

fn is_traversable_context_node(record_id: &str) -> bool {
    record_id.starts_with("episode:") || record_id.starts_with("fact:")
}

pub(crate) async fn find_overlapping_communities(
    service: &ExtractDeps,
    member_entities: &[String],
) -> Result<Vec<CommunityRecord>, MemoryError> {
    let member_set: HashSet<_> = member_entities.iter().cloned().collect();

    let communities = crate::knowledge::graph_store::KnowledgeGraphStore::new(
        service.db_client.clone(),
        service.active_namespace.clone(),
    )
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
    fn community_edge_endpoints_accept_minimal_neighbor_projection() {
        let record = json!({
            "in": {"RecordId": {"table": "entity", "key": "alice"}},
            "out": {"RecordId": {"table": "episode", "key": "meeting"}},
            "relation": "mentioned_in"
        });

        assert_eq!(
            community_edge_endpoints(&record),
            Some(("entity:alice".to_string(), "episode:meeting".to_string()))
        );
    }
}
