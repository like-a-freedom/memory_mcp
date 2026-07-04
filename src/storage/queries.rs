//! Query builders for SurrealDB operations.

use serde_json::{Value, json};

use super::types::GraphDirection;

const ACTIVE_EDGE_SCAN_BATCH_SIZE: i32 = 10_000;
const FACT_EMBEDDING_DIMENSION_PLACEHOLDER: &str = "__FACT_EMBEDDING_DIMENSION__";

/// Bi-temporal visibility filter: selects records visible as of a given cutoff timestamp.
/// Applied to both `fact` and `edge` tables with the same temporal semantics.
pub const BI_TEMPORAL_WHERE: &str = "t_valid <= type::datetime($cutoff) \
     AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) \
     AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff))";

pub(crate) fn build_fact_visibility_clause(cutoff_var: &str) -> String {
    format!(
        "t_valid <= type::datetime({cutoff}) \
         AND (t_ingested IS NONE OR t_ingested <= type::datetime({cutoff})) \
         AND (t_invalid IS NONE OR t_invalid > type::datetime({cutoff}) OR t_invalid_ingested > type::datetime({cutoff}))",
        cutoff = cutoff_var
    )
}

pub fn active_edge_scan_batch_size() -> i32 {
    ACTIVE_EDGE_SCAN_BATCH_SIZE
}

/// Backward-compatible alias for the active edge scan batch size.
pub fn active_edge_scan_limit() -> i32 {
    active_edge_scan_batch_size()
}

pub fn fact_embedding_dimension_placeholder() -> &'static str {
    FACT_EMBEDDING_DIMENSION_PLACEHOLDER
}

/// Build SQL query for selecting a single record.
pub fn build_select_one_query(record_id: &str) -> (String, Option<Value>) {
    let record_id = record_id.trim();
    if record_id.is_empty() {
        return ("SELECT * FROM none WHERE false".to_string(), None);
    }
    if let Some(idx) = record_id.find(':') {
        let table = &record_id[..idx];
        let id = &record_id[idx + 1..];
        if !table.trim().is_empty() && !id.trim().is_empty() {
            (format!("SELECT * FROM {table}:⟨{id}⟩"), None)
        } else {
            ("SELECT * FROM none WHERE false".to_string(), None)
        }
    } else if is_valid_table_name(record_id) {
        (format!("SELECT * FROM {record_id}"), None)
    } else {
        ("SELECT * FROM none WHERE false".to_string(), None)
    }
}

fn is_valid_table_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

/// Build SQL query for creating a record.
pub fn build_create_query(record_id: &str, content: Value) -> (String, Value) {
    let (table, id) = if let Some(idx) = record_id.find(':') {
        (&record_id[..idx], Some(&record_id[idx + 1..]))
    } else {
        (record_id, None)
    };

    let target = if let Some(record_id) = id {
        format!("{table}:⟨{record_id}⟩")
    } else {
        table.to_string()
    };

    let normalized = normalize_surreal_json(&content);
    if let Value::Object(map) = normalized {
        let (assignments, vars) = build_set_assignments(table, map);
        let sql = if assignments.is_empty() {
            format!("CREATE {target} RETURN *")
        } else {
            format!("CREATE {target} SET {} RETURN *", assignments.join(", "))
        };
        (sql, Value::Object(vars))
    } else {
        (
            format!("CREATE {target} CONTENT $content RETURN *"),
            json!({"content": normalized}),
        )
    }
}

/// Build SQL query for updating a record.
pub fn build_update_query(
    record_id: &str,
    content: Value,
) -> Result<(String, Value), crate::service::MemoryError> {
    use crate::service::MemoryError;

    let (table, id) = if let Some(idx) = record_id.find(':') {
        (&record_id[..idx], &record_id[idx + 1..])
    } else {
        return Err(MemoryError::Storage(format!(
            "Invalid record_id format: expected 'table:id', got '{record_id}'"
        )));
    };

    let content_for_update = if let Value::Object(mut map) = content {
        map.remove("id");
        Value::Object(map)
    } else {
        content
    };

    let normalized = normalize_surreal_json(&content_for_update);
    if let Value::Object(map) = normalized {
        let (assignments, vars) = build_set_assignments(table, map);
        let sql = if assignments.is_empty() {
            format!("UPDATE {table}:⟨{id}⟩ RETURN *")
        } else {
            format!(
                "UPDATE {table}:⟨{id}⟩ SET {} RETURN *",
                assignments.join(", ")
            )
        };
        Ok((sql, Value::Object(vars)))
    } else {
        let sql = format!("UPDATE {table}:⟨{id}⟩ MERGE $content RETURN *");
        Ok((sql, json!({"content": normalized})))
    }
}

pub fn build_select_facts_filtered_query(
    scope: &str,
    cutoff: &str,
    query_contains: Option<&str>,
    limit: i32,
) -> (String, Value) {
    build_select_facts_filtered_advanced_query(scope, cutoff, query_contains, limit, None, &[])
}

#[allow(clippy::too_many_arguments)]
pub fn build_select_facts_filtered_advanced_query(
    scope: &str,
    cutoff: &str,
    query_contains: Option<&str>,
    limit: i32,
    project: Option<&str>,
    fact_types: &[String],
) -> (String, Value) {
    let mut where_clauses = vec!["scope = $scope".to_string(), BI_TEMPORAL_WHERE.to_string()];

    let mut vars = serde_json::Map::from_iter([
        ("scope".to_string(), json!(scope)),
        ("cutoff".to_string(), json!(cutoff)),
        ("limit".to_string(), json!(limit)),
    ]);

    if let Some(project) = project.filter(|project| !project.trim().is_empty()) {
        vars.insert("project".to_string(), json!(project));
        where_clauses.push("project = $project".to_string());
    }

    if !fact_types.is_empty() {
        vars.insert("fact_types".to_string(), json!(fact_types));
        where_clauses.push("fact_type IN $fact_types".to_string());
    }

    let base_where = where_clauses.join(" AND ");

    let sql = if let Some(query) = query_contains.filter(|query| !query.trim().is_empty()) {
        vars.insert("query".to_string(), json!(query));

        format!(
            "SELECT *, search::score(1) AS ft_score FROM fact WHERE {base_where} AND (content @1@ $query OR index_keys @1@ $query) ORDER BY ft_score DESC, t_valid DESC, fact_id ASC LIMIT $limit"
        )
    } else {
        format!(
            "SELECT * FROM fact WHERE {base_where} ORDER BY t_valid DESC, fact_id ASC LIMIT $limit"
        )
    };

    (sql, Value::Object(vars))
}

pub fn build_select_facts_by_entity_links_query(
    scope: &str,
    cutoff: &str,
    entity_links: &[String],
    limit: i32,
) -> (String, Value) {
    (
        format!(
            "SELECT * FROM fact WHERE scope = $scope AND {BI_TEMPORAL_WHERE} AND entity_links CONTAINSANY $entity_links ORDER BY t_valid DESC LIMIT $limit"
        ),
        json!({
            "scope": scope,
            "cutoff": cutoff,
            "entity_links": entity_links,
            "limit": limit,
        }),
    )
}

/// Build a query to select facts linked via triples matching a search term.
///
/// Searches the `triple` table for rows whose subject, predicate, or object
/// matches `query`, then retrieves the linked `fact` records via `source_fact_id`.
/// Applies the standard bi-temporal visibility filter on the fact table.
pub fn build_select_facts_by_triple_query(
    cutoff: &str,
    query: &str,
    limit: usize,
) -> (String, Value) {
    let sql = format!(
        "SELECT * FROM fact \
         WHERE fact_id IN ( \
           SELECT source_fact_id FROM triple \
           WHERE namespace = $ns \
             AND (predicate CONTAINS $query OR object CONTAINS $query OR subject CONTAINS $query) \
         ) \
           AND {BI_TEMPORAL_WHERE} \
         LIMIT $limit"
    );
    (
        sql,
        json!({
            "ns":"__placeholder__",
            "query": query,
            "cutoff": cutoff,
            "limit": limit,
        }),
    )
}

/// Build a query to find nearest-neighbor facts via vector similarity (ANN).
pub fn build_select_facts_ann_query(
    scope: &str,
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
         WHERE scope = $scope \
           AND embedding IS NOT NONE \
           AND embedding IS NOT NULL \
           AND {BI_TEMPORAL_WHERE} \
           AND embedding <|{ann_limit}, {ef_search}|> $query_vec \
         ORDER BY sem_score DESC \
         LIMIT $limit"
    );
    (
        sql,
        json!({
            "scope": scope,
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

pub fn build_count_facts_needing_reembed_query(target_signature: &str) -> (String, Value) {
    (
        "SELECT count() AS count FROM fact WHERE embedding_signature IS NONE OR embedding_signature IS NULL OR embedding_signature != $target_signature".to_string(),
        json!({"target_signature": target_signature}),
    )
}

pub fn build_select_facts_needing_reembed_query(
    target_signature: &str,
    last_completed_fact_id: Option<&str>,
    limit: i32,
) -> (String, Value) {
    let cursor_clause = if last_completed_fact_id.is_some() {
        " AND fact_id > $last_completed_fact_id"
    } else {
        ""
    };

    (
        format!(
            "SELECT * FROM fact WHERE (embedding_signature IS NONE OR embedding_signature IS NULL OR embedding_signature != $target_signature){cursor_clause} ORDER BY fact_id ASC LIMIT $limit"
        ),
        json!({
            "target_signature": target_signature,
            "last_completed_fact_id": last_completed_fact_id,
            "limit": limit,
        }),
    )
}

pub fn build_select_episodes_for_archival_query(cutoff: &str, limit: i32) -> (String, Value) {
    (
        "SELECT * FROM episode WHERE status != 'archived' AND t_ref < type::datetime($cutoff) ORDER BY t_ref ASC LIMIT $limit".to_string(),
        json!({"cutoff": cutoff, "limit": limit}),
    )
}

pub fn build_select_active_facts_by_episode_query(
    episode_id: &str,
    cutoff: &str,
    limit: i32,
) -> (String, Value) {
    let visibility = build_fact_visibility_clause("$cutoff");
    (
        format!(
            "SELECT * FROM fact WHERE source_episode = $episode_id AND {visibility} LIMIT $limit"
        ),
        json!({"episode_id": episode_id, "cutoff": cutoff, "limit": limit}),
    )
}

pub fn build_select_episodes_by_content_query(
    scope: &str,
    cutoff: &str,
    query_contains: Option<&str>,
    limit: i32,
) -> (String, Value) {
    build_select_episodes_by_content_advanced_query(scope, cutoff, query_contains, limit, None)
}

pub fn build_select_episodes_by_content_advanced_query(
    scope: &str,
    cutoff: &str,
    query_contains: Option<&str>,
    limit: i32,
    project: Option<&str>,
) -> (String, Value) {
    let mut where_clauses = vec![
        "scope = $scope AND t_ref <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff))".to_string(),
    ];

    let mut vars = serde_json::Map::from_iter([
        ("scope".to_string(), json!(scope)),
        ("cutoff".to_string(), json!(cutoff)),
        ("limit".to_string(), json!(limit)),
    ]);

    if let Some(project) = project.filter(|project| !project.trim().is_empty()) {
        vars.insert("project".to_string(), json!(project));
        where_clauses.push("project = $project".to_string());
    }

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

pub fn filter_records_by_project_and_fact_types(
    records: Vec<Value>,
    project: Option<&str>,
    fact_types: &[String],
) -> Vec<Value> {
    records
        .into_iter()
        .filter(|record| record_matches_project(record, project))
        .filter(|record| record_matches_fact_type(record, fact_types))
        .collect()
}

pub fn filter_records_by_project(records: Vec<Value>, project: Option<&str>) -> Vec<Value> {
    records
        .into_iter()
        .filter(|record| record_matches_project(record, project))
        .collect()
}

fn record_matches_project(record: &Value, project: Option<&str>) -> bool {
    let Some(project) = project.filter(|project| !project.trim().is_empty()) else {
        return true;
    };

    record_object(record)
        .and_then(|map| map.get("project"))
        .and_then(crate::service::value_helpers::json_string)
        .is_some_and(|value| value == project)
}

fn record_matches_fact_type(record: &Value, fact_types: &[String]) -> bool {
    if fact_types.is_empty() {
        return true;
    }

    record_object(record)
        .and_then(|map| map.get("fact_type"))
        .and_then(crate::service::value_helpers::json_string)
        .is_some_and(|value| fact_types.iter().any(|fact_type| fact_type == value))
}

fn record_object(record: &Value) -> Option<&serde_json::Map<String, Value>> {
    if let Some(map) = record.as_object() {
        Some(map)
    } else {
        record.get("Object").and_then(Value::as_object)
    }
}

pub fn build_select_edges_filtered_query(cutoff: &str) -> (String, Value) {
    build_select_edges_filtered_page_query(cutoff, active_edge_scan_batch_size() as usize, 0)
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

pub fn build_select_entity_lookup_canonical_query(normalized_name: &str) -> (String, Value) {
    (
        "SELECT * FROM entity WHERE canonical_name_normalized = $name LIMIT 1".to_string(),
        json!({"name": normalized_name}),
    )
}

pub fn build_select_entity_lookup_alias_query(normalized_name: &str) -> (String, Value) {
    (
        "SELECT * FROM entity WHERE aliases CONTAINS $name LIMIT 1".to_string(),
        json!({"name": normalized_name}),
    )
}

pub fn build_select_communities_matching_summary_query(query: &str) -> (String, Value) {
    (
        "SELECT *, search::score(1) AS ft_score FROM community WHERE summary @1@ $query ORDER BY ft_score DESC, summary ASC LIMIT 25".to_string(),
        json!({"query": query}),
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

    (
        format!(
            "SELECT * FROM edge WHERE {node_field} = <record> $node_id AND {BI_TEMPORAL_WHERE} ORDER BY in ASC, out ASC, t_valid DESC"
        ),
        json!({"node_id": node_id, "cutoff": cutoff}),
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

    if let Value::Object(map) = normalized {
        let (assignments, mut vars) = build_set_assignments("edge", map);
        let all_assignments = assignments;
        vars.insert("edge_id".to_string(), json!(edge_id));
        vars.insert("in_id".to_string(), json!(from_id));
        vars.insert("out_id".to_string(), json!(to_id));

        (
            format!(
                "LET $in = <record> $in_id; LET $out = <record> $out_id; RELATE $in -> {edge_record_literal} -> $out SET {} RETURN *",
                all_assignments.join(", ")
            ),
            Value::Object(vars),
        )
    } else {
        (
            format!(
                "LET $in = <record> $in_id; LET $out = <record> $out_id; RELATE $in -> {edge_record_literal} -> $out SET content = $content RETURN *"
            ),
            json!({
                "edge_id": edge_id,
                "in_id": from_id,
                "out_id": to_id,
                "content": normalized,
            }),
        )
    }
}

fn record_literal(record_id: &str) -> String {
    record_id.split_once(':').map_or_else(
        || record_id.to_string(),
        |(table, key)| format!("{table}:⟨{key}⟩"),
    )
}

fn temporal_field_names_for_table(table: &str) -> &'static [&'static str] {
    match table {
        "episode" => &["t_ref", "t_ingested", "archived_at"],
        "fact" | "edge" => &[
            "t_valid",
            "t_ingested",
            "t_invalid",
            "t_invalid_ingested",
            "last_accessed",
            "embedding_updated_at",
        ],
        "community" => &["updated_at"],
        "event_log" => &["ts"],
        "task" => &["due_date"],
        "script_migration" => &["executed_at"],
        "embedding_state" => &["updated_at"],
        "embedding_job" => &["requested_at", "started_at", "updated_at", "finished_at"],
        _ => &[],
    }
}

fn build_set_assignments(
    table: &str,
    map: serde_json::Map<String, Value>,
) -> (Vec<String>, serde_json::Map<String, Value>) {
    let temporal_fields = temporal_field_names_for_table(table);
    let mut entries: Vec<(String, Value)> = map.into_iter().collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut assignments = Vec::with_capacity(entries.len());
    let mut vars = serde_json::Map::new();

    for (key, value) in entries {
        match value {
            Value::Null => assignments.push(format!("{key} = NONE")),
            Value::String(raw) if temporal_fields.contains(&key.as_str()) => {
                vars.insert(key.clone(), Value::String(raw));
                assignments.push(format!("{key} = type::datetime(${key})"));
            }
            other => {
                vars.insert(key.clone(), other);
                assignments.push(format!("{key} = ${key}"));
            }
        }
    }

    (assignments, vars)
}

fn normalize_surreal_json(v: &Value) -> Value {
    use serde_json::Value as J;

    match v {
        J::Object(map) if map.len() == 1 => {
            let Some((k, val)) = map.iter().next() else {
                return J::Object(map.clone());
            };
            match k.as_str() {
                "None" => v.clone(),
                "Array" => val
                    .as_array()
                    .map(|items| J::Array(items.iter().map(normalize_surreal_json).collect()))
                    .unwrap_or_else(|| val.clone()),
                "Object" => val
                    .as_object()
                    .map(|inner| {
                        J::Object(
                            inner
                                .iter()
                                .map(|(ik, iv)| (ik.clone(), normalize_surreal_json(iv)))
                                .collect(),
                        )
                    })
                    .unwrap_or_else(|| val.clone()),
                "Strand" | "String" => val
                    .as_object()
                    .and_then(|inner| inner.get("String").cloned())
                    .unwrap_or_else(|| val.clone()),
                "Datetime" => val
                    .as_object()
                    .and_then(|inner| inner.get("String").cloned())
                    .unwrap_or_else(|| val.clone()),
                "Number" | "Float" | "Int" | "Decimal" => normalize_surreal_json(val),
                _ => J::Object(
                    map.iter()
                        .map(|(ik, iv)| (ik.clone(), normalize_surreal_json(iv)))
                        .collect(),
                ),
            }
        }
        J::Object(map) => J::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), normalize_surreal_json(v)))
                .collect(),
        ),
        J::Null => J::Null,
        J::Array(arr) => J::Array(arr.iter().map(normalize_surreal_json).collect()),
        _ => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_select_one_query_empty_string_returns_safe_noop() {
        let (sql, bind) = build_select_one_query("");
        assert_eq!(sql, "SELECT * FROM none WHERE false");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_table_with_id() {
        let (sql, bind) = build_select_one_query("episode:abc123");
        assert_eq!(sql, "SELECT * FROM episode:⟨abc123⟩");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_table_only_returns_safe_noop() {
        let (sql, bind) = build_select_one_query("episode:");
        assert_eq!(sql, "SELECT * FROM none WHERE false");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_plain_table() {
        let (sql, bind) = build_select_one_query("episode");
        assert_eq!(sql, "SELECT * FROM episode");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_bare_hex_returns_safe_noop() {
        let (sql, bind) = build_select_one_query("474b2d8b81b3feabf832ef08");
        assert_eq!(sql, "SELECT * FROM none WHERE false");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_bare_hex_with_letters_returns_safe_noop() {
        let (sql, bind) = build_select_one_query("072d682d0d467aa94aad684d");
        assert_eq!(sql, "SELECT * FROM none WHERE false");
        assert!(bind.is_none());
    }

    #[test]
    fn build_select_one_query_fact_with_id() {
        let (sql, bind) = build_select_one_query("fact:52f9d92d20d829840f24294f");
        assert_eq!(sql, "SELECT * FROM fact:⟨52f9d92d20d829840f24294f⟩");
        assert!(bind.is_none());
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
    fn build_select_active_facts_by_episode_query_uses_full_bitemporal_visibility() {
        let (sql, vars) =
            build_select_active_facts_by_episode_query("episode:1", "2026-05-13T00:00:00Z", 1);

        assert!(sql.contains("t_valid <= type::datetime($cutoff)"));
        assert!(sql.contains("t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)"));
        assert!(sql.contains("t_invalid_ingested > type::datetime($cutoff)"));
        assert_eq!(vars["episode_id"], "episode:1");
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
