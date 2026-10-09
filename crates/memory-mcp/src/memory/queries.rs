//! SurrealDB query builders for the memory domain.
//!
//! One builder: the episode-content fallback that context assembly runs when
//! no fact matched. It lived in `storage::queries` because that is where the
//! other SQL was, which is the same reason it was in the wrong place.

use serde_json::{Value, json};

pub fn build_select_episodes_by_content_query(
    cutoff: &str,
    query_contains: Option<&str>,
    limit: i32,
) -> (String, Value) {
    let where_clauses = [
        "t_ref <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff))".to_string(),
    ];

    let mut vars = serde_json::Map::from_iter([
        ("cutoff".to_string(), json!(cutoff)),
        ("limit".to_string(), json!(limit)),
    ]);

    let base_where = where_clauses.join(" AND ");

    let sql = if let Some(query) = query_contains.filter(|query| !query.trim().is_empty()) {
        vars.insert("query".to_string(), json!(query.to_lowercase()));
        format!(
            "SELECT * FROM episode WHERE {base_where} AND string::contains(string::lowercase(content), $query) ORDER BY t_ref DESC, episode_id ASC LIMIT $limit"
        )
    } else {
        format!(
            "SELECT * FROM episode WHERE {base_where} ORDER BY t_ref DESC, episode_id ASC LIMIT $limit"
        )
    };

    (sql, Value::Object(vars))
}

/// The temporal columns an episode carries.
///
/// `DbClient::create` used to guess which columns were datetimes, and for an
/// episode it guessed wrong: `t_ref` was written as the RFC-3339 string it
/// arrived as, and SurrealDB refused the record. The context names them.
/// The temporal columns an inbox-revision row carries.
pub const INBOX_REVISION_TEMPORAL_FIELDS: &[&str] = &[
    "t_ref",
    "discovered_at",
    "updated_at",
    "lease_expires_at",
    "processed_at",
];

pub const EPISODE_TEMPORAL_FIELDS: &[&str] = &["t_ref", "t_ingested", "archived_at"];

/// Stage 1 of the episode-via-entity read: the fact ids an entity is linked to.
///
/// Bound as two parts and constructed in-query because a `<record>` cast does
/// not survive index selection on SurrealDB 3.3.0. See the 2026-10-09
/// query-performance plan, Appendix A.
pub fn build_select_fact_ids_via_entity_query(
    entity_table: &str,
    entity_key: &str,
) -> (String, Value) {
    (
        "SELECT type::string(out) AS fact_id FROM edge \
         WHERE in = type::record($entity_table, $entity_key) AND relation = 'involved_in'"
            .to_string(),
        json!({"entity_table": entity_table, "entity_key": entity_key}),
    )
}

/// Stage 2: the episodes those facts came from.
///
/// `fact_id` already carries an index (migration 029), so a bound `INSIDE`
/// array is answered from it rather than by scanning the table.
pub fn build_select_source_episodes_via_facts_query(fact_ids: &[String]) -> (String, Value) {
    (
        "SELECT source_episode FROM fact WHERE fact_id INSIDE $fact_ids".to_string(),
        json!({ "fact_ids": fact_ids }),
    )
}

/// Stage 3: the episode rows themselves, newest first.
///
/// `episode.episode_id` is indexed by migration 053; this is the lookup that
/// index exists for.
pub fn build_select_episodes_by_ids_query(episode_ids: &[Value]) -> (String, Value) {
    (
        "SELECT * FROM episode WHERE episode_id INSIDE $episode_ids \
         ORDER BY t_ref DESC LIMIT 10"
            .to_string(),
        json!({ "episode_ids": episode_ids }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_ids_via_entity_binds_type_record_parts() {
        let (sql, vars) = build_select_fact_ids_via_entity_query("entity", "abc-123");
        assert!(
            sql.contains("in = type::record($entity_table, $entity_key)"),
            "{sql}"
        );
        assert!(sql.contains("relation = 'involved_in'"), "{sql}");
        assert!(!sql.contains("<record>"), "{sql}");
        assert_eq!(vars["entity_key"], "abc-123");
    }

    #[test]
    fn source_episodes_via_facts_binds_id_array() {
        let ids = vec!["fact:a".to_string(), "fact:b".to_string()];
        let (sql, vars) = build_select_source_episodes_via_facts_query(&ids);
        assert!(sql.contains("fact_id INSIDE $fact_ids"), "{sql}");
        assert_eq!(vars["fact_ids"][1], "fact:b");
    }

    #[test]
    fn episodes_by_ids_binds_raw_values_and_keeps_order_clause() {
        let ids = vec![
            serde_json::json!("episode:1"),
            serde_json::json!("episode:2"),
        ];
        let (sql, vars) = build_select_episodes_by_ids_query(&ids);
        assert!(
            sql.contains("WHERE episode_id INSIDE $episode_ids"),
            "{sql}"
        );
        assert!(sql.contains("ORDER BY t_ref DESC"), "{sql}");
        assert!(sql.contains("LIMIT 10"), "{sql}");
        assert_eq!(
            vars["episode_ids"],
            serde_json::json!(["episode:1", "episode:2"])
        );
    }
}
