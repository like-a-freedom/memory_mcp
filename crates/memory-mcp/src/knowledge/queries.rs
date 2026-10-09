//! SurrealDB query builders for the knowledge domain.
//!
//! These used to live in `storage::queries`, next to the table-generic
//! `select_one`/`create`/`update`. That put SQL for a fact, an edge and a
//! community inside the platform, and `the_storage_platform_names_no_domain_table`
//! in `tests/knowledge_read_scopes.rs` is what noticed.
//!
//! `BI_TEMPORAL_WHERE` is imported from [`crate::shared::temporal`] rather
//! than defined here: it is a domain invariant three modules filter on, and
//! a copy per owner would be a copy to keep in step.

use serde_json::{Value, json};

use crate::shared::record::split_record_id;
use crate::shared::temporal::{
    BI_TEMPORAL_WHERE, visibility_clause as build_fact_visibility_clause,
};
use crate::storage::GraphDirection;
use crate::storage::helpers::normalize_surreal_json;
use crate::storage::queries::build_set_assignments;

pub(crate) const NONSEMANTIC_FACT_PROJECTION: &str = "fact_id, fact_type, content, quote, source_episode, t_valid, t_ingested, \
     t_invalid, t_invalid_ingested, confidence, index_keys, access_count, last_accessed, \
     entity_links, scope, policy_tags, provenance";

/// The temporal columns a fact carries, and an edge's, which are the same.
pub const FACT_TEMPORAL_FIELDS: &[&str] = &[
    "t_valid",
    "t_ingested",
    "t_invalid",
    "t_invalid_ingested",
    "last_accessed",
    "embedding_updated_at",
];

/// The temporal columns the claim tables carry.
///
/// They live here because a `SET` clause written against them is knowledge's
/// SQL. `DbClient::create` and `build_upsert_query` take the list from the
/// caller rather than looking it up, because which columns are datetimes is a
/// property of the table and the table belongs to a context.
pub const CLAIM_TEMPORAL_FIELDS: &[&str] = &[
    "observed_at",
    "valid_from",
    "valid_to",
    "t_ingested",
    "t_invalid_ingested",
];
pub const CLAIM_JOB_TEMPORAL_FIELDS: &[&str] = &[
    "lease_expires_at",
    "created_at",
    "started_at",
    "updated_at",
    "completed_at",
    "processed",
];
pub const CLAIM_RELATION_TEMPORAL_FIELDS: &[&str] =
    &["evaluated_at", "t_ingested", "t_invalid_ingested"];

/// The temporal columns a triple carries. Not the fact set: `triple` has no
/// `t_valid` and is not queried by validity interval.
pub const TRIPLE_TEMPORAL_FIELDS: &[&str] = &["t_ingested", "t_invalid", "t_invalid_ingested"];

/// The entity table has no datetime columns: an id, a type, a canonical name
/// and its aliases. Empty rather than a copy of the fact set, because a name
/// that is not a column is inert while a missing one is not — an entity
/// payload carrying `updated_at` would be written as a string into a column
/// SurrealDB would have to coerce.
pub const ENTITY_TEMPORAL_FIELDS: &[&str] = &[];

/// The temporal columns a community carries.
pub const COMMUNITY_TEMPORAL_FIELDS: &[&str] = &["updated_at"];

pub fn build_select_facts_filtered_query(
    cutoff: &str,
    query_contains: Option<&str>,
    limit: i32,
    fact_types: &[String],
) -> (String, Value) {
    let mut where_clauses = vec![BI_TEMPORAL_WHERE.to_string()];

    let mut vars = serde_json::Map::from_iter([
        ("cutoff".to_string(), json!(cutoff)),
        ("limit".to_string(), json!(limit)),
    ]);

    if !fact_types.is_empty() {
        vars.insert("fact_types".to_string(), json!(fact_types));
        where_clauses.push("fact_type IN $fact_types".to_string());
    }

    let base_where = where_clauses.join(" AND ");

    let sql = if let Some(query) = query_contains.filter(|query| !query.trim().is_empty()) {
        let query_literal = surreal_string_literal(query);
        // Keep the query in the vars object for instrumentation and test
        // doubles; SurrealDB 3.0 uses the escaped literal in MATCHES above.
        vars.insert("query".to_string(), json!(query));
        format!(
            "SELECT {NONSEMANTIC_FACT_PROJECTION}, search::score(1) AS ft_score FROM fact WHERE {base_where} AND (content @1@ {query_literal} OR index_keys @1@ {query_literal}) ORDER BY ft_score DESC, t_valid DESC, fact_id ASC LIMIT $limit"
        )
    } else {
        format!(
            "SELECT {NONSEMANTIC_FACT_PROJECTION} FROM fact WHERE {base_where} ORDER BY t_valid DESC, fact_id ASC LIMIT $limit"
        )
    };

    (sql, Value::Object(vars))
}

/// Escapes a user-provided value as a SurrealQL double-quoted string literal.
///
/// SurrealDB 3.0 does not evaluate bound variables as MATCHES operands, so
/// full-text queries must use a literal while all ordinary filters remain bound.
pub(crate) fn surreal_string_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            character if character.is_control() => {
                use std::fmt::Write;
                let _ = write!(escaped, "\\u{:04x}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

pub fn build_select_facts_by_entity_links_query(
    cutoff: &str,
    entity_links: &[String],
    limit: i32,
) -> (String, Value) {
    (
        format!(
            "SELECT {NONSEMANTIC_FACT_PROJECTION} FROM fact WHERE {BI_TEMPORAL_WHERE} AND entity_links CONTAINSANY $entity_links ORDER BY t_valid DESC LIMIT $limit"
        ),
        json!({
            "cutoff": cutoff,
            "entity_links": entity_links,
            "limit": limit,
        }),
    )
}

/// Build a query to find nearest-neighbor facts via vector similarity (ANN).
pub fn build_select_facts_ann_query(
    cutoff: &str,
    query_vec: &[f64],
    limit: i32,
) -> (String, Value) {
    let ann_limit = limit.max(1);
    // HNSW ef_search defaults to 4 * K for better recall
    let ef_search = (ann_limit * 4).max(16);
    let sql = format!(
        "SELECT *, vector::similarity::cosine(embedding, $query_vec) AS sem_score \
         FROM fact \
         WHERE embedding IS NOT NONE \
           AND embedding IS NOT NULL \
           AND {BI_TEMPORAL_WHERE} \
           AND embedding <|{ann_limit}, {ef_search}|> $query_vec \
         ORDER BY sem_score DESC \
         LIMIT $limit"
    );
    (
        sql,
        json!({
            "cutoff": cutoff,
            "query_vec": query_vec,
            "limit": limit,
        }),
    )
}

pub fn build_select_active_facts_query(cutoff: &str, limit: i32) -> (String, Value) {
    let visibility = build_fact_visibility_clause("$cutoff");
    (
        format!("SELECT * FROM fact WHERE {visibility} ORDER BY t_valid ASC LIMIT $limit"),
        json!({"cutoff": cutoff, "limit": limit}),
    )
}

pub fn build_select_edges_filtered_page_query(
    cutoff: &str,
    limit: usize,
    start: usize,
) -> (String, Value) {
    (
        format!(
            "SELECT * FROM edge WHERE {BI_TEMPORAL_WHERE} ORDER BY in ASC, out ASC, t_valid DESC LIMIT $limit START $start"
        ),
        json!({ "cutoff": cutoff, "limit": limit, "start": start }),
    )
}

pub fn build_select_communities_by_member_entities_query(
    member_entities: &[String],
) -> (String, Value) {
    (
        "SELECT * FROM community WHERE member_entities CONTAINSANY $members ORDER BY community_id ASC".to_string(),
        json!({"members": member_entities}),
    )
}

pub fn build_select_edge_neighbors_query(
    node_id: &str,
    cutoff: &str,
    direction: GraphDirection,
) -> (String, Value) {
    let node_field = match direction {
        // For `RELATE from -> edge -> to`, incoming edges to `node_id` place the
        // node on the `out` side, while outgoing edges place it on `in`.
        GraphDirection::Incoming => "out",
        GraphDirection::Outgoing => "in",
    };

    let (predicate, vars) = bind_record_equality(node_field, node_id, cutoff);

    (
        format!(
            "SELECT in, out, relation FROM edge WHERE {predicate} AND {BI_TEMPORAL_WHERE} ORDER BY in ASC, out ASC, t_valid DESC"
        ),
        vars,
    )
}

/// Binds one side of a record-equality predicate and returns its SQL operand.
///
/// On SurrealDB 3.3.0 a `<record> $param` cast folds to a literal *after*
/// index selection and is answered with a TableScan; `type::record($table,
/// $key)` over two bound strings is an IndexScan at plan time. The cast also
/// parses its input as a record-id literal and truncates the key at the first
/// character outside `[A-Za-z0-9_]`, so it can silently name the wrong record.
/// A value that is not a record id has no table/key pair to bind and keeps the
/// cast.
///
/// `var_prefix` names the bound variables: `{prefix}_table`/`{prefix}_key` on
/// the split path, `{prefix}_id` on the fallback. Both callers that need a
/// two-sided predicate (the triple lookup and the RELATE endpoints) use `in`
/// and `out`; the single-sided neighbor predicates use `node`.
fn bind_record_operand(
    value: &str,
    var_prefix: &str,
    vars: &mut serde_json::Map<String, Value>,
) -> String {
    match split_record_id(value) {
        Some((table, key)) => {
            vars.insert(format!("{var_prefix}_table"), json!(table));
            vars.insert(format!("{var_prefix}_key"), json!(key));
            format!("type::record(${var_prefix}_table, ${var_prefix}_key)")
        }
        None => {
            vars.insert(format!("{var_prefix}_id"), json!(value));
            format!("<record> ${var_prefix}_id")
        }
    }
}

/// Binds `{field} = <record value>` so the predicate stays index-safe.
fn bind_record_equality(field: &str, value: &str, cutoff: &str) -> (String, Value) {
    let mut vars = serde_json::Map::from_iter([("cutoff".to_string(), json!(cutoff))]);
    let operand = bind_record_operand(value, "node", &mut vars);
    (format!("{field} = {operand}"), Value::Object(vars))
}

/// Finds an entity by one of its aliases.
///
/// `CONTAINSANY [$alias]` rather than `CONTAINS $alias`: on SurrealDB 3.3.0 a
/// `CONTAINS` membership test is never served by an index, while `CONTAINSANY`
/// over a one-element array is served by the array-element index
/// `entity_aliases` (`FIELDS aliases.*`). The two are the same membership test.
pub fn build_select_entity_by_alias_query(alias: &str) -> (String, Value) {
    (
        "SELECT entity_id FROM entity WHERE aliases CONTAINSANY [$alias] LIMIT 1".to_string(),
        json!({ "alias": alias }),
    )
}

/// Build the graph-app projection: unlike retrieval's neighbor walk, the app
/// needs a stable edge ID and summary metadata so it can open edge details.
pub fn build_select_graph_edge_neighbors_query(
    node_id: &str,
    cutoff: &str,
    direction: GraphDirection,
) -> (String, Value) {
    let node_field = match direction {
        GraphDirection::Incoming => "out",
        GraphDirection::Outgoing => "in",
    };

    let (predicate, vars) = bind_record_equality(node_field, node_id, cutoff);

    (
        format!(
            "SELECT edge_id, id, in, out, relation, origin, confidence, t_valid, t_ingested FROM edge WHERE {predicate} AND {BI_TEMPORAL_WHERE} ORDER BY in ASC, out ASC, t_valid DESC"
        ),
        vars,
    )
}

/// Binds both ends of an exact-triple lookup so each stays index-safe.
///
/// Every fact write probes for an identical edge through this, so a TableScan
/// here is paid twice per write. See `bind_record_operand` for why the
/// two-part binding matters.
pub fn build_select_edges_for_triple_query(
    in_id: &str,
    relation: &str,
    out_id: &str,
) -> (String, Value) {
    let mut vars = serde_json::Map::from_iter([("relation".to_string(), json!(relation))]);
    let in_operand = bind_record_operand(in_id, "in", &mut vars);
    let out_operand = bind_record_operand(out_id, "out", &mut vars);
    (
        format!(
            "SELECT * FROM edge WHERE in = {in_operand} AND relation = $relation AND out = {out_operand}"
        ),
        Value::Object(vars),
    )
}

pub fn build_relate_edge_query(
    edge_id: &str,
    from_id: &str,
    to_id: &str,
    content: Value,
) -> (String, Value) {
    let normalized = normalize_surreal_json(&content);
    let edge_record_literal = record_literal(edge_id);
    let (endpoint_bindings, endpoint_vars) = bind_record_endpoints(from_id, to_id);

    if let Value::Object(map) = normalized {
        let (assignments, mut vars) = build_set_assignments(FACT_TEMPORAL_FIELDS, map);
        let all_assignments = assignments;
        vars.insert("edge_id".to_string(), json!(edge_id));
        vars.extend(endpoint_vars);

        (
            format!(
                "{endpoint_bindings} RELATE $in -> {edge_record_literal} -> $out SET {} RETURN *",
                all_assignments.join(", ")
            ),
            Value::Object(vars),
        )
    } else {
        let mut vars = endpoint_vars;
        vars.insert("edge_id".to_string(), json!(edge_id));
        vars.insert("content".to_string(), normalized);

        (
            format!(
                "{endpoint_bindings} RELATE $in -> {edge_record_literal} -> $out SET content = $content RETURN *"
            ),
            Value::Object(vars),
        )
    }
}

/// Binds `$in`/`$out` for a RELATE, constructing both records in-query.
///
/// The `<record>` cast cannot be used here: it parses its input as a
/// record-id literal, so a key is truncated at the first character outside
/// `[A-Za-z0-9_]` and `entity:trip-a` is stored as the key `trip`. The edge
/// would then point at a record no entity is stored under, which is exactly
/// what `tests/record_id_integrity.rs` pins. See `bind_record_operand` for the
/// binding rule.
fn bind_record_endpoints(from_id: &str, to_id: &str) -> (String, serde_json::Map<String, Value>) {
    let mut vars = serde_json::Map::new();
    let from = bind_record_operand(from_id, "in", &mut vars);
    let to = bind_record_operand(to_id, "out", &mut vars);
    (format!("LET $in = {from}; LET $out = {to};"), vars)
}

fn record_literal(record_id: &str) -> String {
    record_id.split_once(':').map_or_else(
        || record_id.to_string(),
        |(table, key)| format!("{table}:⟨{key}⟩"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_select_facts_filtered_query_uses_safe_literal_for_fts_operand() {
        let (sql, vars) = build_select_facts_filtered_query(
            "2026-05-13T00:00:00Z",
            Some("Alice \"launch\""),
            10,
            &[],
        );

        assert!(sql.contains("search::score(1) AS ft_score"));
        assert!(sql.contains("content @1@ \"Alice \\\"launch\\\"\""));
        assert!(sql.contains("index_keys @1@ \"Alice \\\"launch\\\"\""));
        assert!(!sql.contains("@1@ $query"));
        assert!(!sql.contains("WHERE scope"));
        assert!(!sql.contains("AND scope"));
        assert!(!sql.contains("project"));
        assert_eq!(vars["limit"], 10);
    }

    #[test]
    fn surreal_string_literal_escapes_surrealql_control_characters() {
        assert_eq!(
            surreal_string_literal("quote\" slash\\ newline\n"),
            "\"quote\\\" slash\\\\ newline\\n\""
        );
    }

    #[test]
    fn build_select_edges_filtered_page_query_uses_limit_and_start_bindings() {
        let (sql, vars) = build_select_edges_filtered_page_query("2026-05-13T00:00:00Z", 250, 500);

        assert!(sql.contains("LIMIT $limit START $start"));
        assert!(sql.contains("ORDER BY in ASC, out ASC, t_valid DESC"));
        assert_eq!(vars["cutoff"], json!("2026-05-13T00:00:00Z"));
        assert_eq!(vars["limit"], json!(250));
        assert_eq!(vars["start"], json!(500));
    }

    #[test]
    fn entity_by_alias_uses_containsany_so_the_element_index_can_serve_it() {
        let (sql, vars) = build_select_entity_by_alias_query("al");
        assert!(sql.contains("aliases CONTAINSANY [$alias]"), "{sql}");
        assert!(!sql.contains("CONTAINS $alias"), "{sql}");
        assert_eq!(vars["alias"], "al");
    }

    #[test]
    fn edge_neighbors_binds_record_parts_not_cast() {
        let (sql, vars) = build_select_edge_neighbors_query(
            "entity:abc-123",
            "2026-01-01T00:00:00Z",
            GraphDirection::Outgoing,
        );
        assert!(
            sql.contains("= type::record($node_table, $node_key)"),
            "{sql}"
        );
        assert!(!sql.contains("<record>"), "cast must be gone: {sql}");
        assert_eq!(vars["node_table"], "entity");
        assert_eq!(vars["node_key"], "abc-123");
    }

    #[test]
    fn graph_edge_neighbors_binds_record_parts() {
        let (sql, vars) = build_select_graph_edge_neighbors_query(
            "fact:x9",
            "2026-01-01T00:00:00Z",
            GraphDirection::Incoming,
        );
        assert!(
            sql.contains("= type::record($node_table, $node_key)"),
            "{sql}"
        );
        assert!(!sql.contains("<record>"), "cast must be gone: {sql}");
        assert_eq!(vars["node_table"], "fact");
        assert_eq!(vars["node_key"], "x9");
    }

    #[test]
    fn neighbors_query_falls_back_to_cast_for_unsplit_id() {
        let (sql, vars) = build_select_edge_neighbors_query(
            "noid",
            "2026-01-01T00:00:00Z",
            GraphDirection::Incoming,
        );
        assert!(sql.contains("<record> $node_id"), "{sql}");
        assert_eq!(vars["node_id"], "noid");
    }

    #[test]
    fn relate_query_builds_endpoints_in_query_not_by_cast() {
        let (sql, vars) = build_relate_edge_query(
            "edge:abc",
            "entity:trip-a",
            "entity:trip-c",
            json!({"relation": "knows"}),
        );
        assert!(
            sql.contains("LET $in = type::record($in_table, $in_key);"),
            "{sql}"
        );
        assert!(
            sql.contains("LET $out = type::record($out_table, $out_key);"),
            "{sql}"
        );
        assert!(!sql.contains("<record>"), "{sql}");
        assert_eq!(vars["in_key"], "trip-a");
        assert_eq!(vars["out_key"], "trip-c");
    }

    #[test]
    fn relate_query_keeps_cast_for_an_unsplit_endpoint() {
        let (sql, vars) = build_relate_edge_query(
            "edge:abc",
            "noid",
            "entity:trip-c",
            json!({"relation": "knows"}),
        );
        assert!(sql.contains("LET $in = <record> $in_id;"), "{sql}");
        assert_eq!(vars["in_id"], "noid");
    }

    #[test]
    fn triple_lookup_binds_record_parts_for_both_ends() {
        let (sql, vars) =
            build_select_edges_for_triple_query("entity:trip-a", "knows", "entity:trip-c");
        assert!(
            sql.contains("in = type::record($in_table, $in_key)"),
            "{sql}"
        );
        assert!(
            sql.contains("out = type::record($out_table, $out_key)"),
            "{sql}"
        );
        assert!(!sql.contains("<record>"), "{sql}");
        assert_eq!(vars["relation"], "knows");
        assert_eq!(vars["out_key"], "trip-c");
    }

    #[test]
    fn triple_lookup_falls_back_to_cast_for_unsplit_ids() {
        let (sql, vars) = build_select_edges_for_triple_query("noid", "knows", "entity:trip-c");
        assert!(sql.contains("in = <record> $in_id"), "{sql}");
        assert_eq!(vars["in_id"], "noid");
    }

    #[test]
    fn build_select_active_facts_query_uses_full_bitemporal_visibility() {
        let (sql, vars) = build_select_active_facts_query("2026-05-13T00:00:00Z", 5);

        assert!(sql.contains("t_valid <= type::datetime($cutoff)"));
        assert!(sql.contains("t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)"));
        assert!(sql.contains("t_invalid_ingested > type::datetime($cutoff)"));
        assert_eq!(vars["cutoff"], "2026-05-13T00:00:00Z");
        assert_eq!(vars["limit"], 5);
    }
}
