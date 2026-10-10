//! Alias expansion for query broadening.

use std::collections::HashSet;

use serde_json::Value;

use super::query_mode::query_phrase_candidates;

/// Resolve only the entire subject of a bare/profile query. Attribute words
/// must remain part of the subject, so a partial entity match cannot broaden it.
pub(super) async fn resolve_entity_subject(
    service: &crate::memory::retrieval::RetrievalContext,
    query: &str,
) -> Result<(Vec<String>, Vec<String>), crate::MemoryError> {
    let lowered = query.trim().to_lowercase();
    let subject = [
        "what do you know about ",
        "tell me about ",
        "profile of ",
        "everything about ",
    ]
    .iter()
    .find_map(|prefix| lowered.strip_prefix(prefix))
    .unwrap_or(&lowered)
    .trim_matches(|c: char| !c.is_alphanumeric());
    let normalized = crate::shared::search::normalize_text(subject);
    if normalized.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut lookup_names = query_phrase_candidates(query)
        .into_iter()
        .flat_map(|phrase| {
            let possessive_subject = phrase
                .split_whitespace()
                .map(|word| {
                    word.strip_suffix("'s")
                        .or_else(|| word.strip_suffix("’s"))
                        .unwrap_or(word)
                })
                .collect::<Vec<_>>()
                .join(" ");
            [
                crate::shared::search::normalize_text(&phrase),
                crate::shared::search::normalize_text(&possessive_subject),
            ]
        })
        .collect::<Vec<_>>();
    lookup_names.push(normalized.clone());
    lookup_names.sort();
    lookup_names.dedup();
    let rows = service
        .knowledge_store()
        .select_entities_batch(&lookup_names)
        .await?;
    let mut subjects = Vec::new();
    let mut mentioned = Vec::new();
    for row in rows {
        let Some(object) = row.as_object() else {
            continue;
        };
        let Some(canonical) = object.get("canonical_name").and_then(Value::as_str) else {
            continue;
        };
        let names = std::iter::once(canonical)
            .chain(
                object
                    .get("aliases")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            )
            .collect::<Vec<_>>();
        if names
            .iter()
            .any(|name| crate::shared::search::normalize_text(name) == normalized)
            && let Some(id) = object.get("entity_id").and_then(Value::as_str)
        {
            subjects.push(id.to_owned());
        }
        mentioned.extend(
            names
                .into_iter()
                .filter(|name| lookup_names.contains(&crate::shared::search::normalize_text(name)))
                .map(str::to_owned),
        );
    }
    subjects.sort();
    subjects.dedup();
    mentioned.sort();
    mentioned.dedup();
    Ok((subjects, mentioned))
}

/// Expands a search query with entity aliases for broader recall.
///
/// Looks up entities whose canonical names appear in the query,
/// and returns additional query terms derived from their aliases.
pub(crate) async fn expand_query_with_aliases(
    service: &crate::memory::retrieval::RetrievalContext,
    query: &str,
) -> Vec<String> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let phrase_entries = query_phrase_candidates(query)
        .into_iter()
        .filter_map(|phrase| {
            let (position, phrase_len) = {
                let phrase_terms = phrase.split_whitespace().collect::<Vec<_>>();
                let phrase_len = phrase_terms.len();
                let position = terms
                    .windows(phrase_len)
                    .position(|window| window == phrase_terms.as_slice());
                (position, phrase_len)
            };

            position.map(|start| (phrase, start, start + phrase_len))
        })
        .collect::<Vec<_>>();

    // Deduplicate normalized names for batch lookup
    let normalized_names: Vec<String> = phrase_entries
        .iter()
        .map(|(phrase, _, _)| crate::shared::search::normalize_text(phrase))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Single batch query instead of O(N²) individual lookups
    let entities = service
        .knowledge_store()
        .select_entities_batch(&normalized_names)
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
                    .map(crate::shared::search::normalize_text)
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
        let normalized = crate::shared::search::normalize_text(phrase);
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

#[cfg(test)]
mod tests {
    use super::expand_query_with_aliases;
    use crate::storage::DbClient;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn expand_query_with_aliases_supports_multi_word_entities() {
        let db_name = format!(
            "alias_expansion_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory_with_namespaces(
                &db_name,
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory db"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");

        // Seed the multi-word entity the mock used to return from `select_entities_batch`.
        db_client
            .create(
                "entity:alice_smith",
                json!({
                    "entity_id": "entity:alice_smith",
                    "entity_type": "person",
                    "canonical_name": "Alice Smith",
                    "canonical_name_normalized": "alice smith",
                    "aliases": ["alice s."],
                }),
                "org",
                crate::knowledge::queries::ENTITY_TEMPORAL_FIELDS,
            )
            .await
            .expect("seed entity");

        let service = crate::service::MemoryService::new(
            db_client,
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let retrieval = crate::memory::retrieval_deps::AssembleContextDeps::from(&service);
        let expanded = expand_query_with_aliases(&retrieval, "alice smith atlas").await;

        assert!(
            expanded.iter().any(|query| query == "alice s. atlas"),
            "multi-word entity alias should expand the full phrase, got: {expanded:?}"
        );
    }
}
