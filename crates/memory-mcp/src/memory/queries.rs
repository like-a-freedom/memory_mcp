//! SurrealDB query builders for the memory domain.
//!
//! The episode-content fallback that context assembly runs when no fact
//! matched, and the three bounded lookups that resolve episodes linked to an
//! entity. They lived in `storage::queries` because that is where the other
//! SQL was, which is the same reason they were in the wrong place.

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

/// First-hit union after each term's own order/cap, then same-view hydration.
pub(crate) fn build_select_episode_candidate_batch_query(
    cutoff: &str,
    terms: &[String],
    limit: i32,
    seen: &[String],
) -> (String, Value) {
    let mut vars = serde_json::Map::from_iter([
        ("cutoff".into(), json!(cutoff)),
        ("limit".into(), json!(limit)),
        ("seen".into(), json!(seen)),
    ]);
    // Normalize each visible source once, then retain only thin membership
    // rows. Independent term limits still apply before the first-hit union;
    // no full source or normalized text is copied into a term's sort input.
    let queries = terms
        .iter()
        .map(|term| {
            if term.trim().is_empty() {
                String::new()
            } else {
                term.to_lowercase()
            }
        })
        .collect::<Vec<_>>();
    vars.insert("queries".into(), json!(queries));
    // At most eight borrowed predicates; a database closure would clone its
    // captured long normalized string for each partially matching term.
    let predicates = (0..terms.len())
        .map(|rank| format!("$normalized CONTAINS $queries[{rank}]"))
        .collect::<Vec<_>>()
        .join(", ");
    let pool = format!(
        "(SELECT id AS record, episode_id, t_ref, {{ LET $normalized = string::lowercase(content); RETURN IF $normalized CONTAINSALL $queries THEN array::repeat(true, array::len($queries)) ELSE IF $normalized CONTAINSANY $queries THEN [{predicates}] ELSE array::repeat(false, array::len($queries)) END; }} AS matched_terms FROM episode WHERE t_ref <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)))"
    );
    vars.insert(
        "term_ranks".into(),
        json!((0..terms.len()).collect::<Vec<_>>()),
    );
    let groups = "array::map($term_ranks, |$term_rank| (SELECT record, episode_id, t_ref, $term_rank AS term_rank FROM $pool WHERE matched_terms[$term_rank] = true ORDER BY t_ref DESC, episode_id ASC LIMIT $limit))";
    let union = format!(
        "SELECT VALUE hit[1].* FROM (SELECT episode_id, array::min([term_rank, record, episode_id, t_ref]) AS hit FROM array::flatten({groups}) GROUP BY episode_id) WHERE episode_id NOT IN $seen ORDER BY hit[0] ASC, hit[3] DESC, hit[2] ASC"
    );
    (
        format!("RETURN {{ LET $pool = {pool}; RETURN ({union}); }}"),
        Value::Object(vars),
    )
}

/// Selects immutable episode content visible at both a world-valid instant
/// and one exact server-owned knowledge revision. Legacy rows without a
/// committed revision are intentionally omitted from historical reads.
pub(crate) fn build_select_episodes_at_revision_query(
    valid_at: &str,
    known_revision: i64,
    query_contains: Option<&str>,
    limit: i32,
) -> (String, Value) {
    let mut clauses = vec![
        "known_revision IS NOT NONE".to_string(),
        "known_revision <= $known_revision".to_string(),
        "t_ref <= type::datetime($valid_at)".to_string(),
    ];
    let mut vars = serde_json::Map::from_iter([
        ("valid_at".to_string(), json!(valid_at)),
        ("known_revision".to_string(), json!(known_revision)),
        ("limit".to_string(), json!(limit.max(1))),
    ]);
    if let Some(query) = query_contains.filter(|query| !query.trim().is_empty()) {
        vars.insert("query".to_string(), json!(query.to_lowercase()));
        clauses.push("string::contains(string::lowercase(content), $query)".into());
    }
    (
        format!(
            "SELECT episode_id, source_type, source_id, content, t_ref, t_ingested, scope, visibility_scope, policy_tags, source_lineage, source_group, source_position, known_revision FROM episode WHERE {} ORDER BY t_ref DESC, episode_id ASC LIMIT $limit",
            clauses.join(" AND ")
        ),
        Value::Object(vars),
    )
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
/// not survive index selection on SurrealDB 3.3. See the 2026-10-09
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
    build_select_source_episodes_via_facts_query_with_access(fact_ids, None)
}

pub(crate) fn build_select_source_episodes_via_facts_query_with_access(
    fact_ids: &[String],
    allowed_tags: Option<&[String]>,
) -> (String, Value) {
    let mut vars = serde_json::Map::from_iter([("fact_ids".to_string(), json!(fact_ids))]);
    let access_clause = if let Some(tags) = allowed_tags {
        vars.insert("allowed_tags".to_string(), json!(tags));
        " AND (policy_tags IS NONE OR policy_tags IS NULL OR policy_tags = [] OR policy_tags CONTAINSANY $allowed_tags)"
    } else {
        ""
    };

    (
        format!("SELECT source_episode FROM fact WHERE fact_id INSIDE $fact_ids{access_clause}"),
        Value::Object(vars),
    )
}

/// Stage 3: the episode rows themselves, newest first.
///
/// `episode.episode_id` is indexed by migration 053; this is the lookup that
/// index exists for.
pub fn build_select_episodes_by_ids_query(episode_ids: &[Value]) -> (String, Value) {
    build_select_episodes_by_ids_query_with_access(episode_ids, None)
}

pub(crate) fn build_select_episodes_by_ids_query_with_access(
    episode_ids: &[Value],
    allowed_tags: Option<&[String]>,
) -> (String, Value) {
    let mut vars = serde_json::Map::from_iter([("episode_ids".to_string(), json!(episode_ids))]);
    let access_clause = if let Some(tags) = allowed_tags {
        vars.insert("allowed_tags".to_string(), json!(tags));
        " AND (policy_tags IS NONE OR policy_tags IS NULL OR policy_tags = [] OR policy_tags CONTAINSANY $allowed_tags)"
    } else {
        ""
    };

    (
        format!(
            "SELECT * FROM episode WHERE episode_id INSIDE $episode_ids{access_clause} \
             ORDER BY t_ref DESC LIMIT 10"
        ),
        Value::Object(vars),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_bounded_episode_query_uses_valid_time_and_committed_revision() {
        let (sql, vars) =
            build_select_episodes_at_revision_query("2026-05-13T00:00:00Z", 7, None, 11);

        assert!(sql.contains("known_revision IS NOT NONE"));
        assert!(sql.contains("known_revision <= $known_revision"));
        assert!(sql.contains("t_ref <= type::datetime($valid_at)"));
        assert!(!sql.contains("t_ingested <="));
        assert_eq!(vars["known_revision"], 7);
        assert_eq!(vars["limit"], 11);
    }

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
    fn source_episode_lookup_applies_access_tags_to_facts() {
        let ids = vec!["fact:a".to_string()];
        let allowed_tags = vec!["team:blue".to_string()];
        let (sql, vars) =
            build_select_source_episodes_via_facts_query_with_access(&ids, Some(&allowed_tags));

        assert!(
            sql.contains("policy_tags CONTAINSANY $allowed_tags"),
            "{sql}"
        );
        assert_eq!(vars["allowed_tags"], json!(["team:blue"]));
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

    #[test]
    fn episode_lookup_applies_access_tags_before_ordering_and_limit() {
        let ids = vec![serde_json::json!("episode:1")];
        let allowed_tags = vec!["team:blue".to_string()];
        let (sql, vars) = build_select_episodes_by_ids_query_with_access(&ids, Some(&allowed_tags));

        assert!(
            sql.contains("policy_tags CONTAINSANY $allowed_tags"),
            "{sql}"
        );
        assert!(sql.find("policy_tags").unwrap() < sql.find("ORDER BY").unwrap());
        assert_eq!(vars["allowed_tags"], json!(["team:blue"]));
    }
}
