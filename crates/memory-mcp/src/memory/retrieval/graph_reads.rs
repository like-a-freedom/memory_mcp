//! Knowledge-graph reads used by the map view and by inline explanation.
//!
//! `find_hub_entities` and `list_communities` read entities and communities
//! through the knowledge graph store, reached through the [`GraphContext`]
//! trait rather than a container. They sit with the memory context because
//! the map view is the only caller, and the map view assembles memory's
//! context.
//!
//! `GraphContext` is a read contract — the knowledge graph store and a
//! logger — that names no adapter, so it is the port this context
//! implements. `GraphCommunity`, `edge_identity` and
//! `graph_community_from_value` are pure parsers over knowledge's own rows,
//! so they live here too; the write paths import them back.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::knowledge::CommunityRecord;
use crate::knowledge::community::parse_community_record;
use crate::knowledge::graph_store::KnowledgeGraphStore;
use crate::logging::{LogLevel, StdoutLogger};
use crate::platform::traversal_budget::GraphTraversalBudget;
use crate::shared::temporal::normalize_dt;
use crate::storage::GraphDirection;

/// A detected community of related entities, as the map view renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GraphCommunity {
    pub community_id: String,
    pub summary: String,
    pub member_entities: Vec<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub(crate) fn edge_identity(record: &Value) -> Option<String> {
    let map = record.as_object()?;

    map.get("edge_id")
        .and_then(crate::storage::value_helpers::unwrap_record_string)
        .or_else(|| {
            let in_id = map
                .get("in")
                .and_then(crate::storage::value_helpers::unwrap_record_string)?;
            let relation = map
                .get("relation")
                .and_then(crate::storage::value_helpers::unwrap_record_string)?;
            let out_id = map
                .get("out")
                .and_then(crate::storage::value_helpers::unwrap_record_string)?;
            Some(format!("{in_id}:{relation}:{out_id}"))
        })
}

pub(crate) fn graph_community_from_value(value: &Value) -> Option<GraphCommunity> {
    let CommunityRecord {
        community_id,
        summary,
        member_entities,
        updated_at,
    } = parse_community_record(value)?;

    if summary.is_empty() || member_entities.is_empty() {
        return None;
    }

    Some(GraphCommunity {
        community_id,
        summary,
        member_entities,
        updated_at,
    })
}

/// The read contract the graph walks go through: the knowledge graph store
/// and a logger, naming no adapter.
pub trait GraphContext: Send + Sync {
    fn knowledge_graph_store(&self) -> KnowledgeGraphStore;
    fn logger(&self) -> &StdoutLogger;
}

/// The budget the map view traverses with: the full scan, unbounded by the
/// reduced inline-explanation budget that shares the same struct.
pub(crate) const MAP_VIEW_BUDGET: GraphTraversalBudget = GraphTraversalBudget::FULL;

/// A candidate hub for the map view: an entity with enough connections to be
/// worth surfacing. The degree is what qualified it, so it travels with the
/// entity rather than being recomputed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HubEntity {
    pub entity_id: String,
    pub canonical_name: String,
    pub degree: usize,
}

/// The hub scan multiplies the caller's limit, because most entities are not
/// hubs and a limit-sized scan would miss the ones that are.
const HUB_CANDIDATE_SCAN_MULTIPLIER: usize = 12;

/// The highest-degree entities in the graph, for the map view's hub list.
pub(crate) async fn find_hub_entities(
    ctx: &impl GraphContext,
    cutoff: DateTime<Utc>,
    limit: i32,
    budget: GraphTraversalBudget,
) -> Result<Vec<HubEntity>, MemoryError> {
    let cutoff_iso = normalize_dt(cutoff);
    ctx.logger().log(
        crate::platform::log_event::log_event(
            "graph.hubs.start",
            json!({"cutoff": cutoff_iso, "limit": limit}),
            json!({}),
            None,
            None,
            None,
        ),
        LogLevel::Debug,
    );
    let entity_records = ctx.knowledge_graph_store().select_entities().await?;
    let mut hubs = Vec::new();
    let candidate_scan_limit =
        (limit.max(1) as usize * HUB_CANDIDATE_SCAN_MULTIPLIER).min(budget.max_hub_scan);

    for record in entity_records.into_iter().take(candidate_scan_limit) {
        let Some(map) = record.as_object() else {
            continue;
        };
        let Some(entity_id) = map
            .get("entity_id")
            .and_then(crate::storage::value_helpers::unwrap_record_string)
            .or_else(|| {
                map.get("id")
                    .and_then(crate::storage::value_helpers::unwrap_record_string)
            })
        else {
            continue;
        };

        let canonical_name = map
            .get("canonical_name")
            .and_then(crate::storage::value_helpers::unwrap_record_string)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| entity_id.clone());

        let mut unique_edges = HashSet::new();
        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            for edge in ctx
                .knowledge_graph_store()
                .select_edge_neighbors(&entity_id, &cutoff_iso, direction)
                .await?
            {
                if let Some(edge_key) = edge_identity(&edge) {
                    unique_edges.insert(edge_key);
                }
            }
        }

        if unique_edges.is_empty() {
            continue;
        }

        hubs.push(HubEntity {
            entity_id,
            canonical_name,
            degree: unique_edges.len(),
        });
    }

    hubs.sort_by(|left, right| {
        right
            .degree
            .cmp(&left.degree)
            .then_with(|| left.canonical_name.cmp(&right.canonical_name))
            .then_with(|| left.entity_id.cmp(&right.entity_id))
    });
    hubs.truncate(limit.max(1) as usize);
    ctx.logger().log(
        crate::platform::log_event::log_event(
            "graph.hubs.done",
            json!({"limit": limit}),
            json!({"count": hubs.len()}),
            None,
            None,
            None,
        ),
        LogLevel::Trace,
    );
    Ok(hubs)
}
pub(crate) async fn list_communities(
    ctx: &impl GraphContext,
    cutoff: DateTime<Utc>,
    limit: i32,
) -> Result<Vec<GraphCommunity>, MemoryError> {
    ctx.logger().log(
        crate::platform::log_event::log_event(
            "graph.communities.start",
            json!({"limit": limit}),
            json!({}),
            None,
            None,
            None,
        ),
        LogLevel::Debug,
    );
    let mut communities = ctx
        .knowledge_graph_store()
        .select_communities()
        .await?
        .into_iter()
        .filter_map(|record| graph_community_from_value(&record))
        .filter(|community| {
            community
                .updated_at
                .is_none_or(|updated_at| updated_at <= cutoff)
        })
        .collect::<Vec<_>>();

    communities.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.community_id.cmp(&right.community_id))
    });
    communities.truncate(limit.max(1) as usize);
    ctx.logger().log(
        crate::platform::log_event::log_event(
            "graph.communities.done",
            json!({"limit": limit}),
            json!({"count": communities.len()}),
            None,
            None,
            None,
        ),
        LogLevel::Trace,
    );
    Ok(communities)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A community with no members is not a community. Returning one would
    /// put an empty cluster in the map view, attributed to a real id.
    #[test]
    fn graph_community_from_value_returns_none_for_empty() {
        let value = json!({});
        assert!(graph_community_from_value(&value).is_none());
    }
}
