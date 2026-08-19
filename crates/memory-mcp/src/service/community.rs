//! Community domain rules shared by incremental, lifecycle, graph, and retrieval paths.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::service::deterministic_community_id;
use crate::service::parse_iso;
use crate::service::value_helpers::{string_from_value, unwrap_array_value};

/// A canonical connected component of entity records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommunityMembership {
    pub(crate) community_id: String,
    pub(crate) member_entities: Vec<String>,
}

impl CommunityMembership {
    /// Builds a canonical membership from entity IDs.
    pub(crate) fn from_entities<I>(entities: I) -> Option<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let member_entities = normalize_member_entities(entities);
        if member_entities.len() < 2 || member_entities.iter().any(|id| !is_entity_id(id)) {
            return None;
        }

        Some(Self {
            community_id: deterministic_community_id(&member_entities),
            member_entities,
        })
    }
}

/// The persisted community fields shared by storage-facing consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommunityRecord {
    pub(crate) community_id: String,
    pub(crate) summary: String,
    pub(crate) member_entities: Vec<String>,
    pub(crate) updated_at: Option<DateTime<Utc>>,
}

/// Returns whether a record ID belongs to the entity table.
#[must_use]
pub(crate) fn is_entity_id(record_id: &str) -> bool {
    record_id.starts_with("entity:")
}

/// Sorts and deduplicates member IDs without imposing a record-shape policy.
///
/// Persisted legacy rows may contain IDs from before the entity prefix was
/// enforced, so parsing normalizes them but does not reject them. New
/// memberships are validated by [`CommunityMembership::from_entities`].
#[must_use]
pub(crate) fn normalize_member_entities<I>(entities: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    entities
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Parses a community row from plain or SurrealDB-wrapped JSON.
///
/// Both `community_id` and the database-generated `id` fallback are accepted.
/// Summary and members remain optional so callers that only need an ID can
/// inspect legacy rows without inventing missing presentation data.
#[must_use]
pub(crate) fn parse_community_record(value: &Value) -> Option<CommunityRecord> {
    let map = value.as_object()?;
    let community_id = map
        .get("community_id")
        .and_then(string_from_value)
        .or_else(|| map.get("id").and_then(string_from_value))?;
    let summary = map
        .get("summary")
        .and_then(string_from_value)
        .unwrap_or_default();
    let member_entities = map
        .get("member_entities")
        .and_then(unwrap_array_value)
        .map(|values| normalize_member_entities(values.iter().filter_map(string_from_value)))
        .unwrap_or_default();
    let updated_at = map
        .get("updated_at")
        .and_then(string_from_value)
        .as_deref()
        .and_then(parse_iso);

    Some(CommunityRecord {
        community_id,
        summary,
        member_entities,
        updated_at,
    })
}

/// Derives all multi-entity communities from an already-active edge scan.
///
/// Context records (`episode:*` and `fact:*`) are intentionally retained as
/// union-find nodes: they allow entities linked through a shared context record
/// to converge into one community. Output ordering and member ordering are
/// deterministic and independent of edge scan order.
#[must_use]
pub(crate) fn converge_communities_from_active_edges(
    edge_records: &[Value],
) -> Vec<CommunityMembership> {
    let mut union_find = UnionFind::default();
    let mut entity_nodes = BTreeSet::<String>::new();

    for record in edge_records {
        let Some((left, right)) = edge_endpoints_from_record(record) else {
            continue;
        };

        union_find.union(&left, &right);
        if is_entity_id(&left) {
            entity_nodes.insert(left);
        }
        if is_entity_id(&right) {
            entity_nodes.insert(right);
        }
    }

    let mut grouped_entities = BTreeMap::<String, BTreeSet<String>>::new();
    for entity_id in entity_nodes {
        let root = union_find.find(&entity_id);
        grouped_entities.entry(root).or_default().insert(entity_id);
    }

    let mut communities = grouped_entities
        .into_values()
        .filter_map(CommunityMembership::from_entities)
        .collect::<Vec<_>>();
    communities.sort_by(|left, right| left.community_id.cmp(&right.community_id));
    communities
}

fn edge_endpoints_from_record(record: &Value) -> Option<(String, String)> {
    let map = record.as_object()?;
    let left = map.get("in").and_then(string_from_value)?;
    let right = map.get("out").and_then(string_from_value)?;
    Some((left, right))
}

#[derive(Debug, Default)]
struct UnionFind {
    parent: HashMap<String, String>,
    rank: HashMap<String, usize>,
}

impl UnionFind {
    fn find(&mut self, node: &str) -> String {
        let parent = self
            .parent
            .entry(node.to_string())
            .or_insert_with(|| node.to_string())
            .clone();
        self.rank.entry(node.to_string()).or_insert(0);

        if parent == node {
            return parent;
        }

        let root = self.find(&parent);
        self.parent.insert(node.to_string(), root.clone());
        root
    }

    fn union(&mut self, left: &str, right: &str) {
        let left_root = self.find(left);
        let right_root = self.find(right);

        if left_root == right_root {
            return;
        }

        let left_rank = *self.rank.get(&left_root).unwrap_or(&0);
        let right_rank = *self.rank.get(&right_root).unwrap_or(&0);

        if left_rank < right_rank {
            self.parent.insert(left_root, right_root);
        } else if left_rank > right_rank {
            self.parent.insert(right_root, left_root);
        } else {
            self.parent.insert(right_root.clone(), left_root.clone());
            self.rank.insert(left_root, left_rank + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn community_membership_from_entities_sorts_and_deduplicates() {
        let membership = CommunityMembership::from_entities([
            "entity:bob".to_string(),
            "entity:alice".to_string(),
            "entity:bob".to_string(),
        ])
        .expect("two distinct members should form a community");

        assert_eq!(
            membership.member_entities,
            vec!["entity:alice", "entity:bob"]
        );
        assert_eq!(
            membership.community_id,
            crate::service::deterministic_community_id(&membership.member_entities)
        );
    }

    #[test]
    fn community_membership_from_entities_rejects_singletons() {
        assert!(CommunityMembership::from_entities(["entity:alice".to_string()]).is_none());
        assert!(CommunityMembership::from_entities(Vec::<String>::new()).is_none());
    }

    #[test]
    fn converge_communities_from_active_edges_is_edge_order_independent() {
        let edges = vec![
            json!({"in": "entity:alice", "out": "episode:shared"}),
            json!({"in": "entity:bob", "out": "episode:shared"}),
            json!({"in": "entity:bob", "out": "fact:joint"}),
            json!({"in": "entity:carol", "out": "fact:joint"}),
            json!({"in": "not-an-entity", "out": "episode:ignored"}),
            json!({"in": "entity:malformed"}),
        ];
        let reversed = edges.iter().rev().cloned().collect::<Vec<_>>();

        assert_eq!(
            converge_communities_from_active_edges(&edges),
            converge_communities_from_active_edges(&reversed)
        );
    }

    #[test]
    fn converge_communities_from_active_edges_connects_through_context_nodes() {
        let communities = converge_communities_from_active_edges(&[
            json!({"in": "entity:alice", "out": "episode:shared"}),
            json!({"in": "entity:bob", "out": "episode:shared"}),
            json!({"in": "entity:bob", "out": "fact:joint"}),
            json!({"in": "entity:carol", "out": "fact:joint"}),
        ]);

        assert_eq!(communities.len(), 1);
        assert_eq!(
            communities[0].member_entities,
            vec!["entity:alice", "entity:bob", "entity:carol"]
        );
    }

    #[test]
    fn converge_communities_from_active_edges_ignores_malformed_edges() {
        let communities = converge_communities_from_active_edges(&[
            json!("not an edge"),
            json!({"in": "entity:alice"}),
            json!({"out": "entity:bob"}),
            json!({"in": 42, "out": "entity:bob"}),
            json!({"in": "entity:solo", "out": "episode:orphan"}),
        ]);

        assert!(communities.is_empty());
    }

    #[test]
    fn parse_community_record_supports_id_fallback_and_wrapped_arrays() {
        let record = parse_community_record(&json!({
            "id": {"String": "community:atlas"},
            "summary": {"String": "Atlas workstream"},
            "member_entities": {"Array": [
                {"String": "entity:bob"},
                {"String": "entity:alice"},
                {"String": "entity:bob"}
            ]},
            "updated_at": {"String": "2026-08-19T10:00:00Z"}
        }))
        .expect("legacy community record should parse");

        assert_eq!(record.community_id, "community:atlas");
        assert_eq!(record.summary, "Atlas workstream");
        assert_eq!(record.member_entities, vec!["entity:alice", "entity:bob"]);
        assert_eq!(
            record.updated_at,
            Some("2026-08-19T10:00:00Z".parse().expect("valid timestamp"))
        );
    }

    #[test]
    fn parse_community_record_preserves_id_only_records() {
        let record = parse_community_record(&json!({"community_id": "community:legacy"}))
            .expect("id-only community record should parse");

        assert_eq!(record.community_id, "community:legacy");
        assert!(record.summary.is_empty());
        assert!(record.member_entities.is_empty());
        assert!(record.updated_at.is_none());
    }

    #[test]
    fn is_entity_id_accepts_only_entity_record_ids() {
        assert!(is_entity_id("entity:alice"));
        assert!(!is_entity_id("entity"));
        assert!(!is_entity_id("episode:one"));
        assert!(!is_entity_id(""));
    }
}
