//! Alias expansion for query broadening.

use std::collections::HashSet;

use serde_json::Value;

use super::query_mode::query_phrase_candidates;

/// Expands a search query with entity aliases for broader recall.
///
/// Looks up entities whose canonical names appear in the query,
/// and returns additional query terms derived from their aliases.
pub(crate) async fn expand_query_with_aliases(
    service: &crate::service::service_context::ServiceContext,
    query: &str,
    namespace: &str,
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
        .map(|(phrase, _, _)| crate::service::normalize_text(phrase))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Single batch query instead of O(N²) individual lookups
    let entities = service
        .context_store()
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
    service: &crate::service::service_context::ServiceContext,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    expand_query_with_aliases(service, query, namespace).await
}

#[cfg(test)]
mod tests {
    use super::expand_query_with_aliases_for_test;
    use crate::service::error::MemoryError;
    use crate::storage::{DbClient, GraphDirection};
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::Arc;

    #[tokio::test]
    async fn expand_query_with_aliases_supports_multi_word_entities() {
        struct MultiWordAliasDbClient;

        #[async_trait]
        impl DbClient for MultiWordAliasDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                if normalized_name == "alice smith" {
                    return Ok(Some(json!({
                        "entity_id": "entity:alice_smith",
                        "aliases": ["alice s."]
                    })));
                }

                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                let mut results = Vec::new();
                for name in names {
                    if name == "alice smith" {
                        results.push(json!({
                            "entity_id": "entity:alice_smith",
                            "canonical_name_normalized": "alice smith",
                            "aliases": ["alice s."]
                        }));
                    }
                }
                Ok(results)
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }

            async fn select_facts_by_triple(
                &self,
                _namespace: &str,
                _query_text: &str,
                _cutoff: &str,
                _limit: usize,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entities_by_ids(
                &self,
                _namespace: &str,
                _entity_ids: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn select_edges_for_triple(
                &self,
                _namespace: &str,
                _in_id: &str,
                _relation: &str,
                _out_id: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn count_facts_needing_reembed(
                &self,
                _namespace: &str,
                _target_signature: &str,
            ) -> Result<usize, MemoryError> {
                Ok(0)
            }

            async fn select_facts_needing_reembed(
                &self,
                _namespace: &str,
                _target_signature: &str,
                _last_completed_fact_id: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(MultiWordAliasDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let expanded = expand_query_with_aliases_for_test(
            &service.build_context(),
            "alice smith atlas",
            "org",
        )
        .await;

        assert!(
            expanded.iter().any(|query| query == "alice s. atlas"),
            "multi-word entity alias should expand the full phrase, got: {expanded:?}"
        );
    }
}
