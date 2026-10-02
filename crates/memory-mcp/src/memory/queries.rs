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
