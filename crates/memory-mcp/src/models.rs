//! Data models and types for the Memory MCP system.
//!
//! This module defines the core data structures used throughout the application,
//! including request/response types, domain entities, and access control types.

mod access;
pub mod claim;
mod domain;
mod ids;
pub mod inbox_revision;
mod lifecycle_trace;
mod memory_event;
mod procedure;
mod provenance;
mod request;
pub(crate) mod rounding;

pub use access::*;
pub use domain::*;
pub use ids::*;
pub use lifecycle_trace::*;
pub use memory_event::*;
pub use procedure::*;
pub use provenance::*;
pub use request::*;

#[must_use]
pub fn default_budget() -> i32 {
    5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_type_from_str_and_clone() {
        #[allow(clippy::type_complexity)]
        let pairs: &[(String, fn(&str) -> Box<dyn std::any::Any>, bool)] = &[]; // placeholder
        for (input, _maker, _is_clone) in pairs {
            let _ = input;
        }

        let ep = EpisodeId::from("episode:abc123");
        assert_eq!(ep.0, "episode:abc123");
        assert_eq!(format!("{ep}"), "episode:abc123");
        assert_eq!(ep.clone().0, ep.0);

        let ent = EntityId::from("entity:bob");
        assert_eq!(ent.0, "entity:bob");
        assert_eq!(ent.clone().0, ent.0);

        let fact = FactId::from("fact:xyz");
        assert_eq!(fact.0, "fact:xyz");
        assert_eq!(fact.clone().0, fact.0);

        let comm = CommunityId::from("community:42");
        assert_eq!(comm.0, "community:42");

        let edge = EdgeId::from("edge:1");
        assert_eq!(edge.0, "edge:1");
    }

    #[test]
    fn default_budget_returns_5() {
        assert_eq!(default_budget(), 5);
    }

    #[test]
    fn episode_without_source_lineage_remains_compatible() {
        let value = serde_json::json!({
            "episode_id": "episode:legacy",
            "source_type": "note",
            "source_id": "source:legacy",
            "content": "legacy",
            "t_ref": "2026-08-24T00:00:00Z",
            "t_ingested": "2026-08-24T00:00:01Z",
            "scope": "",
            "visibility_scope": "",
            "policy_tags": []
        });
        let episode: Episode = serde_json::from_value(value).expect("legacy episode");
        assert_eq!(episode.source_lineage, None);
    }
}
