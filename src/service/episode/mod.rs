//! Episode operations - extraction and record parsing.

mod communities;
mod edges;
mod record_parsing;
mod summary_parser;

pub(crate) use communities::{build_community_summary, update_communities};
#[cfg(test)]
pub(crate) use communities::{collect_connected_entity_component, find_overlapping_communities};
pub(crate) use edges::store_edge;
pub use record_parsing::{episode_from_record, fact_from_record};
pub(crate) use record_parsing::{fact_from_value_or_wrapper, fact_is_active, unwrap_record_string};
use summary_parser::{
    entity_links_for_fact_content, sanitized_content_for_entity_extraction,
    structured_summary_fact_candidates,
};

use serde_json::{Value, json};

use super::core::log_args_with_duration;
use super::error::MemoryError;
use super::statement_detection::{
    is_document_action_item, is_experience_statement, is_metric_statement, is_promise_statement,
    is_summary_like_note_candidate,
};
use crate::logging::LogLevel;
use crate::models::Episode;
use crate::models::{
    ContradictionWarning, EntityCandidate, ExtractResult, ExtractedEntity, ExtractedFact,
    ExtractedLink, FactType,
};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Extract entities from content.
///
/// # Arguments
///
/// * `service` - The memory service containing the entity extractor.
/// * `content` - The text content to extract entities from.
/// * `zero_shot_labels` - Optional custom entity labels for GLiNER extraction.
///   When provided, these labels override the default NER configuration.
pub async fn extract_entities(
    service: &crate::service::MemoryService,
    content: &str,
    zero_shot_labels: Option<&[String]>,
) -> Result<Vec<ExtractedEntity>, MemoryError> {
    let timer = Instant::now(); // ner.extract_candidates
    let provider = service.entity_extractor.provider_name();
    let content_chars = content.chars().count();

    let extraction_result = if ner_provider_uses_blocking_pool(provider) {
        let extractor = service.entity_extractor.clone();
        let content_owned = content.to_string();
        let zero_shot_labels = zero_shot_labels.map(<[String]>::to_vec);
        let handle = tokio::runtime::Handle::current();

        tokio::task::spawn_blocking(move || {
            handle.block_on(async move {
                match zero_shot_labels {
                    Some(labels) => {
                        extractor
                            .extract_candidates_with_labels(&content_owned, &labels)
                            .await
                    }
                    None => extractor.extract_candidates(&content_owned).await,
                }
            })
        })
        .await
        .map_err(|err| MemoryError::Storage(format!("entity extraction task panicked: {err}")))?
    } else {
        match zero_shot_labels {
            Some(labels) => {
                service
                    .entity_extractor
                    .extract_candidates_with_labels(content, labels)
                    .await
            }
            None => service.entity_extractor.extract_candidates(content).await,
        }
    };

    let candidates = match extraction_result {
        Ok(candidates) => candidates,
        Err(err) => {
            let label_count = zero_shot_labels.map(|labels| labels.len());
            log_ner_error(service, provider, content_chars, label_count, &err, timer);
            return Err(err);
        }
    };

    service.logger.log(
        super::log_event(
            "ner.extract.done",
            log_args_with_duration(json!({"content_chars": content_chars}), timer.elapsed()),
            build_ner_log_result(
                provider,
                candidates.len(),
                zero_shot_labels.map(|labels| labels.len()),
                None,
            ),
            None,
            None,
            None,
        ),
        LogLevel::Info,
    );

    let candidates = dedupe_entity_candidates(candidates);
    let mut entities = Vec::with_capacity(candidates.len());
    let mut seen_entity_ids = HashSet::new();

    for candidate in candidates {
        let entity_type = candidate.entity_type.clone();
        let canonical_name = candidate.canonical_name.clone();

        let entity_id = service
            .resolve(candidate.clone(), None)
            .await
            .inspect_err(|err| {
                service.logger.log(
                    super::log_event(
                        "ner.resolve.error",
                        json!({
                            "entity_type": &entity_type,
                            "canonical_name": &canonical_name,
                            "error": err.to_string(),
                        }),
                        json!({"provider": provider}),
                        None,
                        None,
                        None,
                    ),
                    LogLevel::Warn,
                );
            })?;

        if seen_entity_ids.insert(entity_id.clone()) {
            entities.push(ExtractedEntity {
                entity_id,
                entity_type,
                canonical_name,
            });
        }
    }

    Ok(entities)
}

fn dedupe_entity_candidates(candidates: Vec<EntityCandidate>) -> Vec<EntityCandidate> {
    #[derive(Debug, Default)]
    struct CandidateGroup {
        canonical_name: String,
        type_counts: HashMap<String, usize>,
        type_first_seen: HashMap<String, usize>,
        type_display_names: HashMap<String, String>,
        aliases: HashMap<String, String>,
    }

    let mut order = Vec::new();
    let mut groups = HashMap::<String, CandidateGroup>::new();

    for (index, candidate) in candidates.into_iter().enumerate() {
        let canonical_name = candidate.canonical_name.trim();
        let entity_type = candidate.entity_type.trim();
        if canonical_name.is_empty() || entity_type.is_empty() {
            continue;
        }

        let name_key = super::normalize_text(canonical_name);
        if name_key.is_empty() {
            continue;
        }

        let group = groups.entry(name_key.clone()).or_insert_with(|| {
            order.push(name_key.clone());
            CandidateGroup {
                canonical_name: canonical_name.to_string(),
                ..CandidateGroup::default()
            }
        });

        if canonical_name.len() > group.canonical_name.len() {
            group.canonical_name = canonical_name.to_string();
        }

        let entity_type_key = super::normalize_text(entity_type);
        *group
            .type_counts
            .entry(entity_type_key.clone())
            .or_default() += 1;
        group
            .type_first_seen
            .entry(entity_type_key.clone())
            .or_insert(index);
        group
            .type_display_names
            .entry(entity_type_key)
            .or_insert_with(|| entity_type.to_string());

        for alias in candidate.aliases {
            let alias = alias.trim();
            if alias.is_empty() {
                continue;
            }

            let alias_key = super::normalize_text(alias);
            if alias_key.is_empty() || alias_key == name_key {
                continue;
            }

            group
                .aliases
                .entry(alias_key)
                .or_insert_with(|| alias.to_string());
        }
    }

    let mut deduped = Vec::with_capacity(order.len());

    for key in order {
        let Some(mut group) = groups.remove(&key) else {
            continue;
        };

        let mut entity_types = group
            .type_counts
            .into_iter()
            .collect::<Vec<(String, usize)>>();
        entity_types.sort_by(|(left_key, left_count), (right_key, right_count)| {
            right_count
                .cmp(left_count)
                .then_with(|| {
                    group
                        .type_first_seen
                        .get(left_key)
                        .copied()
                        .unwrap_or(usize::MAX)
                        .cmp(
                            &group
                                .type_first_seen
                                .get(right_key)
                                .copied()
                                .unwrap_or(usize::MAX),
                        )
                })
                .then_with(|| left_key.cmp(right_key))
        });

        let Some((entity_type_key, _)) = entity_types.into_iter().next() else {
            continue;
        };

        let entity_type = group
            .type_display_names
            .remove(&entity_type_key)
            .unwrap_or(entity_type_key);

        let mut aliases = group.aliases.into_values().collect::<Vec<_>>();
        aliases.sort();

        deduped.push(EntityCandidate {
            entity_type,
            canonical_name: group.canonical_name,
            aliases,
        });
    }

    deduped
}

fn ner_provider_uses_blocking_pool(provider: &str) -> bool {
    matches!(provider, "anno" | "gliner")
}

#[derive(Debug, Default)]
pub(crate) struct FactExtractionOutcome {
    pub(crate) facts: Vec<ExtractedFact>,
    pub(crate) note_fallback_used: bool,
    pub(crate) structured_line_fact_count: usize,
}

/// Extract facts from an episode.
pub async fn extract_facts(
    service: &crate::service::MemoryService,
    episode: &Episode,
    entities: &[ExtractedEntity],
) -> Result<FactExtractionOutcome, MemoryError> {
    let structured_candidates = structured_summary_fact_candidates(&episode.content);
    if !structured_candidates.is_empty() {
        service.logger.log(
            super::log_event(
                "extract.structured_summary",
                json!({
                    "episode_id": episode.episode_id,
                    "source_type": episode.source_type,
                    "structured_line_fact_count": structured_candidates.len(),
                }),
                json!({
                    "content_chars": episode.content.chars().count(),
                }),
                None,
                None,
                None,
            ),
            LogLevel::Debug,
        );

        let mut facts = Vec::with_capacity(structured_candidates.len());
        for candidate in structured_candidates {
            let entity_links = entity_links_for_fact_content(&candidate.content, entities);
            facts.push(
                add_extracted_fact(
                    service,
                    episode,
                    &candidate.fact_type,
                    &candidate.content,
                    &candidate.quote,
                    &entity_links,
                    "structured_summary_line",
                )
                .await?,
            );
        }

        return Ok(FactExtractionOutcome {
            structured_line_fact_count: facts.len(),
            facts,
            note_fallback_used: false,
        });
    }

    let mut facts = Vec::new();
    let normalized = episode.content.to_lowercase();
    let entity_links = entities
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect::<Vec<_>>();

    if is_metric_statement(&episode.content) {
        facts.push(
            add_extracted_fact(
                service,
                episode,
                FactType::Metric.as_str(),
                &episode.content,
                &episode.content,
                &entity_links,
                "episode_heuristic",
            )
            .await?,
        );
    }

    if is_promise_statement(&normalized) || is_document_action_item(&episode.content) {
        facts.push(
            add_extracted_fact(
                service,
                episode,
                FactType::Promise.as_str(),
                &episode.content,
                &episode.content,
                &entity_links,
                "episode_heuristic",
            )
            .await?,
        );
    }

    if is_experience_statement(&episode.content) {
        facts.push(
            add_extracted_fact(
                service,
                episode,
                FactType::Experience.as_str(),
                &episode.content,
                &episode.content,
                &entity_links,
                "episode_heuristic",
            )
            .await?,
        );
    }

    let note_fallback_used = should_extract_note_fact(episode, &facts);
    if note_fallback_used {
        service.logger.log(
            super::log_event(
                "extract.note_fallback",
                json!({
                    "episode_id": episode.episode_id,
                    "source_type": episode.source_type,
                }),
                json!({
                    "content_chars": episode.content.chars().count(),
                }),
                None,
                None,
                None,
            ),
            LogLevel::Debug,
        );

        facts.push(
            add_extracted_fact(
                service,
                episode,
                FactType::Note.as_str(),
                &episode.content,
                &episode.content,
                &entity_links,
                "summary_note_fallback",
            )
            .await?,
        );
    }

    Ok(FactExtractionOutcome {
        facts,
        note_fallback_used,
        structured_line_fact_count: 0,
    })
}

fn should_extract_note_fact(episode: &Episode, facts: &[ExtractedFact]) -> bool {
    if !facts.is_empty() {
        return false;
    }

    let supported_source_type = source_type_supports_summary_fallback(&episode.source_type);

    supported_source_type && is_summary_like_note_candidate(&episode.content)
}

pub(crate) fn build_extract_log_result(
    episode: Option<&Episode>,
    entities_len: usize,
    facts: &[ExtractedFact],
    links_len: usize,
    warnings_len: usize,
) -> Value {
    let note_fallback_used = episode.is_some_and(|episode| {
        facts.len() == 1
            && facts
                .iter()
                .all(|fact| fact.fact_type == FactType::Note.as_str())
            && source_type_supports_summary_fallback(&episode.source_type)
            && is_summary_like_note_candidate(&episode.content)
    });

    let structured_line_fact_count = episode
        .map(|episode| structured_summary_fact_candidates(&episode.content).len())
        .filter(|candidate_count| *candidate_count > 0 && *candidate_count == facts.len())
        .unwrap_or(0);

    build_extract_log_result_with_metadata(
        episode,
        entities_len,
        facts,
        links_len,
        warnings_len,
        note_fallback_used,
        structured_line_fact_count,
    )
}

pub(crate) fn build_extract_log_result_with_metadata(
    episode: Option<&Episode>,
    entities_len: usize,
    facts: &[ExtractedFact],
    links_len: usize,
    warnings_len: usize,
    note_fallback_used: bool,
    structured_line_fact_count: usize,
) -> Value {
    let mut result = serde_json::Map::from_iter([
        ("entities".to_string(), json!(entities_len)),
        ("facts".to_string(), json!(facts.len())),
        ("links".to_string(), json!(links_len)),
        ("warnings".to_string(), json!(warnings_len)),
        ("note_fallback_used".to_string(), json!(note_fallback_used)),
        (
            "structured_line_fact_count".to_string(),
            json!(structured_line_fact_count),
        ),
    ]);

    if let Some(episode) = episode {
        result.insert("source_type".to_string(), json!(episode.source_type));
        result.insert(
            "content_chars".to_string(),
            json!(episode.content.chars().count()),
        );
    }

    Value::Object(result)
}

fn source_type_supports_summary_fallback(source_type: &str) -> bool {
    matches!(
        source_type,
        "requirement" | "task_tracking" | "stakeholder_mapping" | "customer_engagement" | "email"
    ) || source_type.ends_with("_summary")
}

async fn add_extracted_fact(
    service: &crate::service::MemoryService,
    episode: &Episode,
    fact_type: &str,
    content: &str,
    quote: &str,
    entity_links: &[String],
    extraction_strategy: &str,
) -> Result<ExtractedFact, MemoryError> {
    use serde_json::json;

    let fact_id = service
        .add_fact(
            fact_type,
            content,
            quote,
            &episode.episode_id,
            episode.t_ref,
            &episode.scope,
            0.7,
            entity_links.to_vec(),
            Vec::new(),
            json!({
                "source_episode": episode.episode_id,
                "source_type": episode.source_type,
                "source_id": episode.source_id,
                "extraction_strategy": extraction_strategy,
            }),
        )
        .await?;

    Ok(ExtractedFact {
        fact_id,
        fact_type: fact_type.to_string(),
    })
}

/// Extract entities and facts from an episode.
pub async fn extract_from_episode(
    service: &crate::service::MemoryService,
    episode_id: &str,
    zero_shot_labels: Option<&[String]>,
) -> Result<ExtractResult, MemoryError> {
    use crate::models::Edge;

    let timer = Instant::now(); // extract_from_episode

    service.logger.log(
        super::log_event(
            "extract_from_episode.start",
            json!({"episode_id": episode_id}),
            json!({}),
            None,
            None,
            None,
        ),
        LogLevel::Info,
    );

    let (record, namespace) = service.find_episode_record(episode_id).await?;
    let namespace =
        namespace.ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;
    let record = record.ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;

    let episode = episode_from_record(&record)
        .ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;

    let entity_extraction_content = sanitized_content_for_entity_extraction(&episode.content);
    let entities = extract_entities(service, &entity_extraction_content, zero_shot_labels).await?;
    let fact_outcome = extract_facts(service, &episode, &entities).await?;
    let facts = fact_outcome.facts;
    let warnings = detect_contradiction_warnings(service, &episode, &facts, &namespace).await?;
    let mut links = Vec::new();
    let edge_ingested = super::query::now();

    for entity in &entities {
        links.push(ExtractedLink {
            entity_id: entity.entity_id.clone(),
            episode_id: episode_id.to_string(),
        });

        let edge = Edge {
            in_id: entity.entity_id.clone(),
            relation: "mentioned_in".to_string(),
            out_id: episode_id.to_string(),
            origin: crate::models::EdgeOrigin::Extracted,
            strength: 1.0,
            confidence: 0.9,
            provenance: json!({"source_episode": episode_id}),
            t_valid: episode.t_ref,
            t_ingested: edge_ingested,
            t_invalid: None,
            t_invalid_ingested: None,
        };
        store_edge(service, &edge, &namespace).await?;
    }

    for fact in &facts {
        let (fact_record, _) = service.find_fact_record(&fact.fact_id).await?;
        let Some(fact_record) = fact_record else {
            continue;
        };
        let Some(stored_fact) = fact_from_value_or_wrapper(&Value::Object(fact_record)) else {
            continue;
        };

        for entity_id in &stored_fact.entity_links {
            let edge = Edge {
                in_id: entity_id.clone(),
                relation: "involved_in".to_string(),
                out_id: fact.fact_id.clone(),
                origin: crate::models::EdgeOrigin::Extracted,
                strength: 0.8,
                confidence: 0.85,
                provenance: json!({"source_episode": episode_id}),
                t_valid: episode.t_ref,
                t_ingested: edge_ingested,
                t_invalid: None,
                t_invalid_ingested: None,
            };
            store_edge(service, &edge, &namespace).await?;
        }
    }

    let entity_ids: Vec<String> = entities
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect();

    update_communities(service, &entity_ids, &episode.scope).await?;

    service.logger.log(
        super::log_event(
            "extract_from_episode.done",
            log_args_with_duration(json!({"episode_id": episode_id}), timer.elapsed()),
            build_extract_log_result_with_metadata(
                Some(&episode),
                entities.len(),
                &facts,
                links.len(),
                warnings.len(),
                fact_outcome.note_fallback_used,
                fact_outcome.structured_line_fact_count,
            ),
            None,
            None,
            None,
        ),
        LogLevel::Info,
    );

    Ok(ExtractResult {
        episode_id: episode_id.to_string(),
        entities,
        facts,
        links,
        warnings,
    })
}

#[must_use]
fn build_ner_log_result(
    provider: &str,
    entity_count: usize,
    zero_shot_label_count: Option<usize>,
    error: Option<&str>,
) -> Value {
    let mut result = serde_json::Map::new();
    result.insert("provider".to_string(), json!(provider));
    result.insert("entity_count".to_string(), json!(entity_count));
    if let Some(zero_shot_label_count) = zero_shot_label_count {
        result.insert(
            "zero_shot_label_count".to_string(),
            json!(zero_shot_label_count),
        );
    }
    if let Some(error) = error {
        result.insert("error".to_string(), json!(error));
    }
    Value::Object(result)
}

fn log_ner_error(
    service: &crate::service::MemoryService,
    provider: &str,
    content_chars: usize,
    zero_shot_label_count: Option<usize>,
    err: &MemoryError,
    timer: Instant,
) {
    service.logger.log(
        super::log_event(
            "ner.extract.error",
            log_args_with_duration(json!({"content_chars": content_chars}), timer.elapsed()),
            build_ner_log_result(provider, 0, zero_shot_label_count, Some(&err.to_string())),
            None,
            None,
            None,
        ),
        LogLevel::Warn,
    );
}

async fn detect_contradiction_warnings(
    service: &crate::service::MemoryService,
    episode: &Episode,
    facts: &[ExtractedFact],
    namespace: &str,
) -> Result<Vec<ContradictionWarning>, MemoryError> {
    let cutoff = super::query::now();
    let mut warnings = Vec::new();
    let mut seen_conflicts = std::collections::HashSet::new();
    let active_facts = service
        .db_client
        .select_active_facts(namespace, 500)
        .await?
        .into_iter()
        .filter_map(|record| fact_from_value_or_wrapper(&record))
        .filter(|fact| fact.scope == episode.scope)
        .collect::<Vec<_>>();

    for extracted_fact in facts {
        let (record, _) = service.find_fact_record(&extracted_fact.fact_id).await?;
        let Some(record) = record else {
            continue;
        };
        let Some(new_fact) = fact_from_value_or_wrapper(&Value::Object(record)) else {
            continue;
        };
        if new_fact.entity_links.is_empty() {
            continue;
        }

        let new_content = super::normalize_text(&new_fact.content);

        for existing_fact in &active_facts {
            if existing_fact.fact_id == new_fact.fact_id
                || existing_fact.source_episode == episode.episode_id
                || existing_fact.fact_type != new_fact.fact_type
                || !fact_is_active(existing_fact, cutoff)
                || !has_meaningful_entity_overlap(
                    &existing_fact.entity_links,
                    &new_fact.entity_links,
                )
                || super::normalize_text(&existing_fact.content) == new_content
            {
                continue;
            }

            let conflict_key = format!("{}->{}", new_fact.fact_id, existing_fact.fact_id);
            if !seen_conflicts.insert(conflict_key) {
                continue;
            }

            warnings.push(ContradictionWarning {
                fact_type: new_fact.fact_type.clone(),
                new_fact_id: new_fact.fact_id.clone(),
                conflicting_fact_id: existing_fact.fact_id.clone(),
                existing_content: existing_fact.content.clone(),
                new_content: new_fact.content.clone(),
                entity_ids: new_fact.entity_links.clone(),
                reason: "active fact with the same fact_type and entity set has different content"
                    .to_string(),
            });
        }
    }

    Ok(warnings)
}

fn has_meaningful_entity_overlap(lhs: &[String], rhs: &[String]) -> bool {
    let lhs = lhs
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let rhs = rhs
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();

    if lhs.is_empty() || rhs.is_empty() {
        return false;
    }

    let overlap = lhs.intersection(&rhs).count();
    let smaller_set = lhs.len().min(rhs.len());

    overlap > 0 && overlap * 2 >= smaller_set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EntityCandidate;
    use crate::service::EntityExtractor;
    use crate::storage::{DbClient, SurrealDbClient};
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn episode_from_record_parses_full_record() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));
        record.insert("source_type".to_string(), json!("email"));
        record.insert("source_id".to_string(), json!("msg-123"));
        record.insert("content".to_string(), json!("Test content"));
        record.insert("t_ref".to_string(), json!("2024-01-15T10:30:00Z"));
        record.insert("t_ingested".to_string(), json!("2024-01-15T10:31:00Z"));
        record.insert("scope".to_string(), json!("org"));
        record.insert("visibility_scope".to_string(), json!("org"));
        record.insert("policy_tags".to_string(), json!(["tag1", "tag2"]));

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.episode_id, "episode:test123");
        assert_eq!(episode.source_type, "email");
        assert_eq!(episode.source_id, "msg-123");
        assert_eq!(episode.content, "Test content");
        assert_eq!(episode.scope, "org");
        assert_eq!(episode.visibility_scope, "org");
        assert_eq!(episode.policy_tags, vec!["tag1", "tag2"]);
    }

    #[test]
    fn episode_from_record_returns_none_for_missing_required_field() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));

        assert!(episode_from_record(&record).is_none());
    }

    #[test]
    fn episode_from_record_handles_wrapped_string_values() {
        let mut record = serde_json::Map::new();
        record.insert(
            "episode_id".to_string(),
            json!({"String": "episode:test123"}),
        );
        record.insert("source_type".to_string(), json!({"String": "email"}));
        record.insert("source_id".to_string(), json!({"String": "msg-123"}));
        record.insert("content".to_string(), json!({"String": "Test"}));
        record.insert(
            "t_ref".to_string(),
            json!({"String": "2024-01-15T10:30:00Z"}),
        );
        record.insert(
            "t_ingested".to_string(),
            json!({"String": "2024-01-15T10:31:00Z"}),
        );
        record.insert("scope".to_string(), json!({"String": "org"}));
        record.insert(
            "policy_tags".to_string(),
            json!({"Array": [{"String": "tag1"}]}),
        );

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.episode_id, "episode:test123");
        assert_eq!(episode.policy_tags, vec!["tag1"]);
    }

    #[test]
    fn episode_from_record_uses_defaults_for_optional_fields() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));
        record.insert("source_type".to_string(), json!("email"));
        record.insert("source_id".to_string(), json!("msg-123"));
        record.insert("content".to_string(), json!("Test"));
        record.insert("t_ref".to_string(), json!("2024-01-15T10:30:00Z"));
        record.insert("t_ingested".to_string(), json!("2024-01-15T10:31:00Z"));
        record.insert("scope".to_string(), json!("org"));

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.visibility_scope, "");
        assert!(episode.policy_tags.is_empty());
    }

    #[test]
    fn fact_from_record_parses_full_record() {
        let record = json!({
            "fact_id": "fact:test123",
            "fact_type": "note",
            "content": "Test fact",
            "quote": "Test quote",
            "source_episode": "episode:abc",
            "t_valid": "2024-01-15T10:30:00Z",
            "t_ingested": "2024-01-15T10:31:00Z",
            "t_invalid": "2024-01-16T10:30:00Z",
            "confidence": 0.95,
            "entity_links": ["entity:1", "entity:2"],
            "scope": "org",
            "policy_tags": ["tag1"],
            "provenance": {"source": "test"}
        });

        let fact = fact_from_record(&record).unwrap();
        assert_eq!(fact.fact_id, "fact:test123");
        assert_eq!(fact.fact_type, "note");
        assert_eq!(fact.content, "Test fact");
        assert_eq!(fact.quote, "Test quote");
        assert_eq!(fact.source_episode, "episode:abc");
        assert!((fact.confidence - 0.95).abs() < f64::EPSILON);
        assert_eq!(fact.entity_links, vec!["entity:1", "entity:2"]);
        assert_eq!(fact.scope, "org");
        assert_eq!(fact.policy_tags, vec!["tag1"]);
    }

    #[test]
    fn fact_from_record_handles_optional_fields() {
        let record = json!({
            "fact_id": "fact:test123",
            "fact_type": "note",
            "content": "Test",
            "quote": "Quote",
            "source_episode": "episode:abc",
            "t_valid": "2024-01-15T10:30:00Z",
            "scope": "org"
        });

        let fact = fact_from_record(&record).unwrap();
        assert!(fact.t_invalid.is_none());
        assert!(fact.t_invalid_ingested.is_none());
        assert!(fact.entity_links.is_empty());
        assert!(fact.policy_tags.is_empty());
        assert!((fact.confidence).abs() < f64::EPSILON);
    }

    #[test]
    fn fact_from_record_returns_none_for_invalid_record() {
        let record = json!({"invalid": "data"});
        assert!(fact_from_record(&record).is_none());
    }

    #[test]
    fn unwrap_record_string_handles_record_id() {
        let value = json!({"RecordId": {"table": "entity", "key": "alice"}});
        assert_eq!(
            unwrap_record_string(&value),
            Some("entity:alice".to_string())
        );
    }

    #[test]
    fn is_promise_statement_detects_promise_patterns() {
        assert!(is_promise_statement("i will finish this task"));
        assert!(is_promise_statement("i'll deliver the report tomorrow"));
        assert!(is_promise_statement("will complete the project"));
        assert!(is_promise_statement("going to implement the feature"));
        assert!(is_promise_statement("I will do this tomorrow"));
    }

    #[test]
    fn is_promise_statement_rejects_non_promise_patterns() {
        assert!(!is_promise_statement("this is just a note"));
        assert!(!is_promise_statement("meeting scheduled for tomorrow"));
        assert!(!is_promise_statement("review the document"));
        assert!(!is_promise_statement("the task is complete"));
    }

    #[test]
    fn is_promise_statement_detects_lowercase_variations() {
        assert!(is_promise_statement("i will finish this"));
        assert!(is_promise_statement("i'll deliver"));
        assert!(is_promise_statement("will complete the task"));
    }

    #[test]
    fn is_experience_statement_detects_preference_patterns() {
        assert!(is_experience_statement(
            "Alice Smith prefers weekly launch updates over ad-hoc pings."
        ));
        assert!(is_experience_statement("I enjoy quiet deep-work mornings."));
        assert!(is_experience_statement(
            "I tend to avoid high-rise buildings for accommodations."
        ));
        assert!(is_experience_statement(
            "I have a strong aversion to beachfront resorts."
        ));
        assert!(is_experience_statement(
            "I do not enjoy casinos or gaming environments."
        ));
    }

    #[test]
    fn is_experience_statement_rejects_non_preference_patterns() {
        assert!(!is_experience_statement("Atlas budget is $2M."));
        assert!(!is_experience_statement("I will send the deck tomorrow."));
    }

    #[test]
    fn is_document_action_item_detects_email_style_bullets() {
        assert!(is_document_action_item(
            "Subject: Atlas follow-up\n\nAction items:\n- Alice Smith: send revised deck by Friday\n- Bob Jones: review launch checklist by Monday"
        ));
    }

    #[test]
    fn is_document_action_item_rejects_plain_notes() {
        assert!(!is_document_action_item(
            "Meeting notes: Alice shared the deck."
        ));
        assert!(!is_document_action_item(
            "Action items: this section is empty for now"
        ));
    }

    #[test]
    fn is_summary_like_note_candidate_detects_dense_summary_content() {
        assert!(is_summary_like_note_candidate(
            "July 2025 planning summary: platform integrations ready, stakeholder approvals pending, response workflow scoped."
        ));
    }

    #[test]
    fn is_summary_like_note_candidate_rejects_short_content() {
        assert!(!is_summary_like_note_candidate("Short note only"));
    }

    #[test]
    fn should_extract_note_fact_requires_supported_source_type_and_no_existing_facts() {
        let episode = Episode {
            episode_id: "episode:test".to_string(),
            source_type: "requirement".to_string(),
            source_id: "summary-1".to_string(),
            content: "July 2025 planning summary: platform integrations ready, stakeholder approvals pending, response workflow scoped.".to_string(),
            t_ref: Utc::now(),
            t_ingested: Utc::now(),
            scope: "org".to_string(),
            visibility_scope: String::new(),
            policy_tags: Vec::new(),
        };

        assert!(should_extract_note_fact(&episode, &[]));
        assert!(!should_extract_note_fact(
            &Episode {
                source_type: "meeting".to_string(),
                ..episode.clone()
            },
            &[]
        ));
        assert!(should_extract_note_fact(
            &Episode {
                source_type: "meeting_summary".to_string(),
                ..episode.clone()
            },
            &[]
        ));
        assert!(!should_extract_note_fact(
            &episode,
            &[ExtractedFact {
                fact_id: "fact:test".to_string(),
                fact_type: "promise".to_string(),
            }]
        ));
    }

    #[test]
    fn structured_summary_fact_candidates_extract_labeled_and_heading_scoped_lines() {
        let candidates = structured_summary_fact_candidates(
            "Project decision summary:\n\n- Decision: Approve the cross-platform activation policy.\n- Decision: Keep legacy on-premise licenses separate.\n\nDocumentation facts:\n- Fact: Docs team needs final terminology for supported languages.",
        );

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].fact_type, FactType::Decision.as_str());
        assert_eq!(
            candidates[0].content,
            "Approve the cross-platform activation policy."
        );
        assert_eq!(candidates[1].fact_type, FactType::Decision.as_str());
        assert_eq!(
            candidates[1].content,
            "Keep legacy on-premise licenses separate."
        );
        assert_eq!(candidates[2].fact_type, FactType::Note.as_str());
        assert_eq!(
            candidates[2].content,
            "Docs team needs final terminology for supported languages."
        );
    }

    #[test]
    fn structured_summary_fact_candidates_extract_markdown_headings_without_colons() {
        let candidates = structured_summary_fact_candidates(
            "# September 2025 program summary\n\n## Decisions Made\n1. Regional launch in South market approved for September 30.\n2. Response logging rollout approved for September 30.\n\n## Pending Items\n1. Complete global launch follow-up.\n2. Continue platform 1.5 development.",
        );

        assert_eq!(candidates.len(), 4);
        assert_eq!(candidates[0].fact_type, FactType::Decision.as_str());
        assert_eq!(
            candidates[0].content,
            "Regional launch in South market approved for September 30."
        );
        assert_eq!(candidates[1].fact_type, FactType::Decision.as_str());
        assert_eq!(
            candidates[1].content,
            "Response logging rollout approved for September 30."
        );
        assert_eq!(candidates[2].fact_type, FactType::Note.as_str());
        assert_eq!(candidates[2].content, "Complete global launch follow-up.");
        assert_eq!(candidates[3].fact_type, FactType::Note.as_str());
        assert_eq!(candidates[3].content, "Continue platform 1.5 development.");
    }

    #[test]
    fn structured_summary_fact_candidates_extract_thematic_heading_lines_with_heading_context() {
        let candidates = structured_summary_fact_candidates(
            "# Monthly coordination summary\n\n## Release Activities\n- Finalize phased rollout checklist.\n- Publish support handoff notes.\n\n## Capacity Planning\n- Prepare archive review for next quarter.",
        );

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].fact_type, FactType::Note.as_str());
        assert_eq!(
            candidates[0].content,
            "Release Activities: Finalize phased rollout checklist."
        );
        assert_eq!(candidates[0].quote, "Finalize phased rollout checklist.");
        assert_eq!(
            candidates[1].content,
            "Release Activities: Publish support handoff notes."
        );
        assert_eq!(
            candidates[2].content,
            "Capacity Planning: Prepare archive review for next quarter."
        );
    }

    #[test]
    fn sanitized_content_for_entity_extraction_strips_structural_labels() {
        let sanitized = sanitized_content_for_entity_extraction(
            "Architecture decisions:\n- Decision: Platform becomes the umbrella product name.\n- Fact: Legacy bridge remains active during rollout.",
        );

        assert!(!sanitized.contains("Decision:"));
        assert!(!sanitized.contains("Fact:"));
        assert!(!sanitized.contains("Architecture decisions:"));
        assert!(sanitized.contains("Platform becomes the umbrella product name."));
        assert!(sanitized.contains("Legacy bridge remains active during rollout."));
    }

    #[test]
    fn sanitized_content_for_entity_extraction_strips_thematic_section_headings() {
        let sanitized = sanitized_content_for_entity_extraction(
            "Release Activities:\n- Finalize phased rollout checklist.\n- Publish support handoff notes.",
        );

        assert!(!sanitized.contains("Release Activities:"));
        assert!(sanitized.contains("Finalize phased rollout checklist."));
        assert!(sanitized.contains("Publish support handoff notes."));
    }

    #[test]
    fn dedupe_entity_candidates_merges_duplicate_names_and_aliases() {
        use crate::models::EntityCandidate;
        use std::collections::BTreeSet;

        let candidates = dedupe_entity_candidates(vec![
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Avery Stone".to_string(),
                aliases: vec!["A. Stone".to_string()],
            },
            EntityCandidate {
                entity_type: "company".to_string(),
                canonical_name: "Avery Stone".to_string(),
                aliases: vec!["Stone Group".to_string()],
            },
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Avery Stone".to_string(),
                aliases: vec!["Avery S.".to_string()],
            },
            EntityCandidate {
                entity_type: "organization".to_string(),
                canonical_name: "Operations Forum".to_string(),
                aliases: vec![],
            },
        ]);

        assert_eq!(candidates.len(), 2);

        let avery = candidates
            .iter()
            .find(|candidate| candidate.canonical_name == "Avery Stone")
            .expect("deduped person candidate");
        assert_eq!(avery.entity_type, "person");
        assert_eq!(
            avery.aliases.iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "A. Stone".to_string(),
                "Avery S.".to_string(),
                "Stone Group".to_string(),
            ])
        );
    }

    #[test]
    fn build_ner_log_result_includes_provider_entity_count_and_zero_shot_count() {
        let result = build_ner_log_result("gliner", 3, Some(6), None);

        assert_eq!(
            result.get("provider").and_then(Value::as_str),
            Some("gliner")
        );
        assert_eq!(result.get("entity_count").and_then(Value::as_u64), Some(3));
        assert_eq!(
            result.get("zero_shot_label_count").and_then(Value::as_u64),
            Some(6)
        );
        assert!(
            result.get("error").is_none(),
            "error field should be omitted when not provided"
        );
    }

    #[test]
    fn build_ner_log_result_omits_zero_shot_label_count_when_none() {
        let result = build_ner_log_result("regex", 0, None, None);

        assert_eq!(
            result.get("provider").and_then(Value::as_str),
            Some("regex")
        );
        assert_eq!(result.get("entity_count").and_then(Value::as_u64), Some(0));
        assert!(
            result.get("zero_shot_label_count").is_none(),
            "zero_shot_label_count should be omitted when None"
        );
    }

    #[test]
    fn build_ner_log_result_includes_error_when_provided() {
        let result = build_ner_log_result("gliner", 0, Some(3), Some("tokenization failed"));

        assert_eq!(
            result.get("error").and_then(Value::as_str),
            Some("tokenization failed")
        );
        assert_eq!(result.get("entity_count").and_then(Value::as_u64), Some(0));
    }

    #[test]
    fn build_extract_log_result_includes_episode_metadata_and_note_fallback_usage() {
        let episode = Episode {
            episode_id: "episode:test".to_string(),
            source_type: "requirement".to_string(),
            source_id: "summary-1".to_string(),
            content: "July 2025 planning summary: platform integrations ready.".to_string(),
            t_ref: Utc::now(),
            t_ingested: Utc::now(),
            scope: "org".to_string(),
            visibility_scope: String::new(),
            policy_tags: Vec::new(),
        };

        let result = build_extract_log_result_with_metadata(
            Some(&episode),
            2,
            &[ExtractedFact {
                fact_id: "fact:test".to_string(),
                fact_type: "note".to_string(),
            }],
            3,
            1,
            true,
            0,
        );

        assert_eq!(result.get("entities").and_then(Value::as_u64), Some(2));
        assert_eq!(result.get("facts").and_then(Value::as_u64), Some(1));
        assert_eq!(result.get("links").and_then(Value::as_u64), Some(3));
        assert_eq!(result.get("warnings").and_then(Value::as_u64), Some(1));
        assert_eq!(
            result.get("source_type").and_then(Value::as_str),
            Some("requirement")
        );
        assert_eq!(
            result.get("content_chars").and_then(Value::as_u64),
            Some(56)
        );
        assert_eq!(
            result.get("note_fallback_used").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            result
                .get("structured_line_fact_count")
                .and_then(Value::as_u64),
            Some(0)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extract_entities_does_not_block_runtime_for_local_gliner_provider() {
        struct BlockingGlinerExtractor;

        #[async_trait::async_trait]
        impl EntityExtractor for BlockingGlinerExtractor {
            fn provider_name(&self) -> &'static str {
                "gliner"
            }

            async fn extract_candidates(
                &self,
                _content: &str,
            ) -> Result<Vec<EntityCandidate>, MemoryError> {
                std::thread::sleep(Duration::from_millis(250));
                Ok(Vec::new())
            }
        }

        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory("episode-test", "org", "warn")
                .await
                .expect("connect in memory"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");

        let mut service = crate::service::MemoryService::new(
            db_client,
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("create service");
        service.entity_extractor = Arc::new(BlockingGlinerExtractor);

        let ticker = tokio::spawn(async move {
            let start = Instant::now();
            tokio::time::sleep(Duration::from_millis(50)).await;
            start.elapsed()
        });
        tokio::task::yield_now().await;

        let _ = extract_entities(&service, "Atlas project status", None)
            .await
            .expect("extract entities");
        let tick_elapsed = ticker.await.expect("join ticker");

        assert!(
            tick_elapsed < Duration::from_millis(150),
            "local gliner extraction blocked the runtime for {:?}",
            tick_elapsed
        );
    }

    #[tokio::test]
    async fn collect_connected_entity_component_uses_neighbor_queries_instead_of_edge_scan() {
        use crate::storage::{DbClient, GraphDirection};
        use std::sync::Arc;

        struct NeighborOnlyDbClient;

        #[async_trait::async_trait]
        impl DbClient for NeighborOnlyDbClient {
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

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                panic!("community traversal should not scan the full edge table")
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                node_id: &str,
                _cutoff: &str,
                direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                let mk = |from_id: &str, relation: &str, to_id: &str| {
                    json!({
                        "edge_id": format!("edge:{from_id}:{relation}:{to_id}"),
                        "in": from_id,
                        "relation": relation,
                        "out": to_id,
                        "t_valid": "2024-01-01T00:00:00Z",
                        "t_ingested": "2024-01-01T00:00:00Z"
                    })
                };

                Ok(match (node_id, direction) {
                    ("entity:alice", GraphDirection::Outgoing) => {
                        vec![mk("entity:alice", "mentioned_in", "episode:shared")]
                    }
                    ("episode:shared", GraphDirection::Incoming) => vec![
                        mk("entity:alice", "mentioned_in", "episode:shared"),
                        mk("entity:bob", "mentioned_in", "episode:shared"),
                    ],
                    ("entity:bob", GraphDirection::Outgoing) => {
                        vec![mk("entity:bob", "involved_in", "fact:joint")]
                    }
                    ("fact:joint", GraphDirection::Incoming) => vec![
                        mk("entity:bob", "involved_in", "fact:joint"),
                        mk("entity:carol", "involved_in", "fact:joint"),
                    ],
                    _ => vec![],
                })
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
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
        }

        let service = crate::service::MemoryService::new(
            Arc::new(NeighborOnlyDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let connected =
            collect_connected_entity_component(&service, &["entity:alice".to_string()], "org")
                .await
                .unwrap();

        assert_eq!(
            connected,
            vec![
                "entity:alice".to_string(),
                "entity:bob".to_string(),
                "entity:carol".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn find_overlapping_communities_uses_index_based_lookup() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        static SELECT_COMMUNITIES_BY_MEMBER_CALLED: AtomicBool = AtomicBool::new(false);
        static SELECT_TABLE_CALLED: AtomicBool = AtomicBool::new(false);

        #[derive(Clone)]
        struct IndexLookupDbClient;

        #[async_trait::async_trait]
        impl crate::storage::DbClient for IndexLookupDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<serde_json::Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                SELECT_TABLE_CALLED.store(true, Ordering::SeqCst);
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: crate::storage::GraphDirection,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<serde_json::Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                SELECT_COMMUNITIES_BY_MEMBER_CALLED.store(true, Ordering::SeqCst);
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: serde_json::Value,
            ) -> Result<serde_json::Value, MemoryError> {
                Ok(serde_json::Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: serde_json::Value,
                _namespace: &str,
            ) -> Result<serde_json::Value, MemoryError> {
                Ok(serde_json::Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: serde_json::Value,
                _namespace: &str,
            ) -> Result<serde_json::Value, MemoryError> {
                Ok(serde_json::Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<serde_json::Value>,
                _namespace: &str,
            ) -> Result<serde_json::Value, MemoryError> {
                Ok(serde_json::Value::Null)
            }

            async fn select_entities_by_ids(
                &self,
                _namespace: &str,
                _ids: &[String],
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(IndexLookupDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let _ = find_overlapping_communities(
            &service,
            "org",
            &["entity:alice".to_string(), "entity:bob".to_string()],
        )
        .await;

        assert!(
            SELECT_COMMUNITIES_BY_MEMBER_CALLED.load(Ordering::SeqCst),
            "find_overlapping_communities should call select_communities_by_member_entities"
        );
        assert!(
            !SELECT_TABLE_CALLED.load(Ordering::SeqCst),
            "find_overlapping_communities should NOT call select_table (full scan)"
        );
    }
}
