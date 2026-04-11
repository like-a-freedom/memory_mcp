//! Alias expansion for query broadening.

use std::collections::HashSet;

use serde_json::Value;

/// Expands a search query with entity aliases for broader recall.
///
/// Looks up entities whose canonical names appear in the query,
/// and returns additional query terms derived from their aliases.
pub(crate) async fn expand_query_with_aliases(
    service: &crate::service::MemoryService,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }

    // Collect all n-gram phrases and their positions
    let mut phrase_entries: Vec<(String, usize, usize)> = Vec::new();
    for span_len in (1..=terms.len()).rev() {
        for start in 0..=terms.len().saturating_sub(span_len) {
            let end = start + span_len;
            let phrase = terms[start..end].join(" ");
            if phrase.len() >= 2 {
                phrase_entries.push((phrase, start, end));
            }
        }
    }

    // Deduplicate normalized names for batch lookup
    let normalized_names: Vec<String> = phrase_entries
        .iter()
        .map(|(phrase, _, _)| crate::service::normalize_text(phrase))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Single batch query instead of O(N²) individual lookups
    let entities = service
        .db_client
        .select_entities_batch(namespace, &normalized_names)
        .await
        .unwrap_or_default();

    // Build lookup map: normalized_name → aliases
    let mut entity_aliases: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for entity in &entities {
        let obj = match entity.as_object() {
            Some(obj) => obj,
            None => continue,
        };
        let canonical_norm = obj
            .get("canonical_name_normalized")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| {
                obj.get("canonical_name")
                    .and_then(Value::as_str)
                    .map(crate::service::normalize_text)
            })
            .unwrap_or_default();
        let aliases: Vec<String> = obj
            .get("aliases")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        if !canonical_norm.is_empty() && !aliases.is_empty() {
            entity_aliases.entry(canonical_norm).or_insert(aliases);
        }
    }

    // Expand queries using matched entities
    let mut expanded = HashSet::new();
    for (phrase, start, end) in &phrase_entries {
        let normalized = crate::service::normalize_text(phrase);
        if let Some(aliases) = entity_aliases.get(&normalized) {
            for alias_str in aliases {
                let mut parts: Vec<String> = terms[..*start]
                    .iter()
                    .map(|term| (*term).to_string())
                    .collect();
                parts.push(alias_str.clone());
                parts.extend(terms[*end..].iter().map(|term| (*term).to_string()));
                let alias_expanded = parts.join(" ");

                if alias_expanded != query {
                    expanded.insert(alias_expanded);
                }
            }
        }
    }

    expanded.into_iter().collect()
}

#[allow(dead_code)]
pub(crate) async fn expand_query_with_aliases_for_test(
    service: &crate::service::MemoryService,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    expand_query_with_aliases(service, query, namespace).await
}
