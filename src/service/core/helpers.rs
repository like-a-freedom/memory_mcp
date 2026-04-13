//! Free utility functions for the memory service.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Value, json};

use crate::models::AccessContext;

/// Resolves a namespace from a scope string.
/// Returns `(namespace, fell_back)` where `fell_back` is true when the default
/// was used for an unknown scope.
pub(crate) fn resolve_namespace(
    namespaces: &[String],
    default: &str,
    scope: &str,
) -> (String, bool) {
    let scope_lower = scope.to_lowercase();

    if namespaces.contains(&scope_lower) {
        return (scope_lower, false);
    }

    const SCOPE_PREFIXES: &[(&str, &str)] = &[
        ("personal", "personal"),
        ("private", "private"),
        ("org", "org"),
    ];
    for (prefix, ns) in SCOPE_PREFIXES {
        if scope_lower.starts_with(prefix) && namespaces.iter().any(|n| n == *ns) {
            return (ns.to_string(), false);
        }
    }

    (default.to_string(), true)
}

/// Builds a structured log event for tool operations.
pub(crate) fn log_event(
    op: &str,
    args: Value,
    result: Value,
    access: Option<&AccessContext>,
    request_id: Option<&str>,
    duration_ms: Option<u64>,
) -> HashMap<String, Value> {
    let mut event = HashMap::new();
    event.insert("op".to_string(), Value::String(op.to_string()));
    event.insert("args".to_string(), args);
    event.insert("result".to_string(), result);
    if let Some(access) = access {
        event.insert("access".to_string(), serialize_access(access));
    }
    if let Some(rid) = request_id {
        event.insert("request_id".to_string(), Value::String(rid.to_string()));
    }
    if let Some(ms) = duration_ms {
        event.insert("duration_ms".to_string(), Value::Number(ms.into()));
    }
    event
}

pub(crate) fn serialize_access(access: &AccessContext) -> Value {
    json!({
        "caller_id": access.caller_id,
        "allowed_scopes": access.allowed_scopes,
        "allowed_tags": access.allowed_tags,
        "session_vars": access.session_vars,
        "transport": access.transport,
        "content_type": access.content_type,
        "cross_scope_allow": access.cross_scope_allow,
    })
}

/// Adds a `duration_ms` field to an args Value.
#[must_use]
pub(crate) fn log_args_with_duration(mut args: Value, duration: Duration) -> Value {
    let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    if let Some(map) = args.as_object_mut() {
        map.insert("duration_ms".to_string(), json!(duration_ms));
        args
    } else {
        json!({ "value": args, "duration_ms": duration_ms })
    }
}

/// Builds a log result for embedding operations.
#[must_use]
pub(crate) fn build_embedding_log_result(
    generated_embeddings: usize,
    dimension: Option<usize>,
) -> Value {
    let mut result = serde_json::Map::new();
    result.insert(
        "generated_embeddings".to_string(),
        json!(generated_embeddings),
    );
    if let Some(dimension) = dimension {
        result.insert("dimension".to_string(), json!(dimension));
    }
    Value::Object(result)
}

/// Extracts temporal index keys from content and `t_valid` date.
pub(crate) fn extract_temporal_index_keys(content: &str, t_valid: DateTime<Utc>) -> Vec<String> {
    static MONTH_YEAR_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(january|february|march|april|may|june|july|august|september|october|november|december)\s+\d{4}\b",
        )
        .expect("valid regex")
    });
    static ISO_DATE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b\d{4}-\d{2}(?:-\d{2})?\b").expect("valid regex"));

    let mut keys = HashSet::from([
        crate::service::normalize_text(&t_valid.format("%B %Y").to_string()),
        t_valid.format("%Y-%m").to_string(),
    ]);

    for capture in MONTH_YEAR_RE.find_iter(content) {
        keys.insert(crate::service::normalize_text(capture.as_str()));
    }
    for capture in ISO_DATE_RE.find_iter(content) {
        keys.insert(capture.as_str().to_lowercase());
    }

    let mut keys = keys
        .into_iter()
        .filter(|v| !v.trim().is_empty())
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

/// Builds an intro chain path from a next-hop map.
pub(crate) fn build_intro_chain_from_start(
    start_id: &str,
    target_id: &str,
    next_hop: &HashMap<String, String>,
) -> Option<Vec<String>> {
    let mut path = vec![start_id.to_string()];
    let mut current = start_id;

    while let Some(next) = next_hop.get(current) {
        path.push(next.clone());
        if next == target_id {
            return Some(path);
        }
        current = next;
    }

    None
}

/// BFS pathfinding for intro chain.
#[cfg(test)]
pub(crate) fn bfs_path(
    graph: &HashMap<String, Vec<String>>,
    start: &str,
    target: &str,
    max_hops: usize,
) -> Option<Vec<String>> {
    if start == target {
        return Some(vec![start.to_string()]);
    }

    let mut queue = std::collections::VecDeque::from([(vec![start.to_string()], 0usize)]);
    let mut visited = HashSet::from([start.to_string()]);

    while let Some((path, depth)) = queue.pop_front() {
        if depth >= max_hops {
            continue;
        }
        let current = path.last()?;
        for neighbor in graph.get(current).into_iter().flatten() {
            if neighbor == target {
                let mut result = path.clone();
                result.push(neighbor.clone());
                return Some(result);
            }
            if visited.insert(neighbor.clone()) {
                let mut new_path = path.clone();
                new_path.push(neighbor.clone());
                queue.push_back((new_path, depth + 1));
            }
        }
    }
    None
}
