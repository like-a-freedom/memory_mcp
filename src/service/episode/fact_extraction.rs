use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::claim::ClaimRelationOutcome;
use crate::models::{
    ContradictionWarning, Edge, EdgeOrigin, Episode, ExtractResult, ExtractedEntity, ExtractedFact,
    ExtractedLink, FactType,
};
use crate::service::MemoryService;
use crate::service::episode::communities::update_communities;
use crate::service::episode::edges::store_edge;
use crate::service::episode::entity_extraction::extract_entities;
use crate::service::episode::record_parsing::{episode_from_record, fact_from_value_or_wrapper};
use crate::service::episode::summary_parser::{
    entity_links_for_fact_content, sanitized_content_for_entity_extraction,
    structured_summary_fact_candidates,
};
use crate::service::error::MemoryError;
use crate::service::query::now;
use crate::service::util::{
    is_document_action_item, is_experience_statement, is_metric_statement, is_promise_statement,
    is_summary_like_note_candidate,
};
use crate::service::{log_args_with_duration, log_event};

#[derive(Debug, Default)]
pub(crate) struct FactExtractionOutcome {
    pub(crate) facts: Vec<ExtractedFact>,
    pub(crate) note_fallback_used: bool,
    pub(crate) structured_line_fact_count: usize,
}

fn source_type_supports_summary_fallback(source_type: &str) -> bool {
    matches!(
        source_type,
        "requirement" | "task_tracking" | "stakeholder_mapping" | "customer_engagement" | "email"
    ) || source_type.ends_with("_summary")
}

pub(super) fn should_extract_note_fact(episode: &Episode, facts: &[ExtractedFact]) -> bool {
    if !facts.is_empty() {
        return false;
    }

    let supported_source_type = source_type_supports_summary_fallback(&episode.source_type);

    supported_source_type && is_summary_like_note_candidate(&episode.content)
}

pub(super) async fn add_extracted_fact(
    service: &MemoryService,
    episode: &Episode,
    fact_type: &str,
    content: &str,
    quote: &str,
    entity_links: &[String],
    extraction_strategy: &str,
) -> Result<ExtractedFact, MemoryError> {
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
            crate::models::Provenance::extraction(
                &episode.episode_id,
                &episode.source_type,
                &episode.source_id,
                extraction_strategy,
            ),
        )
        .await?;

    Ok(ExtractedFact {
        fact_id,
        fact_type: fact_type.to_string(),
    })
}

/// Extract facts from an episode.
pub async fn extract_facts(
    service: &MemoryService,
    episode: &Episode,
    entities: &[ExtractedEntity],
) -> Result<FactExtractionOutcome, MemoryError> {
    let structured_candidates = structured_summary_fact_candidates(&episode.content);
    if !structured_candidates.is_empty() {
        service.logger.log(
            log_event(
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
            log_event(
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

pub(super) async fn detect_contradiction_warnings(
    service: &MemoryService,
    facts: &[ExtractedFact],
    namespace: &str,
) -> Result<Vec<ContradictionWarning>, MemoryError> {
    claim_based_contradiction_warnings(service, facts, namespace).await
}

/// Query claim relations for active contradictions involving the extracted facts.
async fn claim_based_contradiction_warnings(
    service: &MemoryService,
    facts: &[ExtractedFact],
    namespace: &str,
) -> Result<Vec<ContradictionWarning>, MemoryError> {
    let fact_ids: Vec<_> = facts
        .iter()
        .map(|f| crate::models::FactId::from(f.fact_id.as_str()))
        .collect();
    if fact_ids.is_empty() {
        return Ok(Vec::new());
    }
    let query = crate::storage::claims::RelationsForFactsQuery {
        namespace,
        fact_ids: &fact_ids,
    };
    let relations = service
        .claim_service
        .store
        .select_relations_for_facts(query)
        .await?;
    let mut warnings = Vec::new();
    for rel in &relations {
        if rel.outcome != ClaimRelationOutcome::Contradiction {
            continue;
        }
        // Determine which side of the relation is the current fact (the one
        // being extracted) and which is the counterpart.
        let (current_fact_id, counterpart_id): (&str, &str) = if let Some(ref left) =
            rel.left_fact_id
            && fact_ids.iter().any(|f| f == left)
        {
            (
                left.as_ref(),
                rel.right_fact_id
                    .as_ref()
                    .map(|id| id.as_ref())
                    .unwrap_or(""),
            )
        } else if let Some(ref right) = rel.right_fact_id
            && fact_ids.iter().any(|f| f == right)
        {
            (
                right.as_ref(),
                rel.left_fact_id
                    .as_ref()
                    .map(|id| id.as_ref())
                    .unwrap_or(""),
            )
        } else {
            continue;
        };
        if counterpart_id.is_empty() {
            continue;
        }
        // Find the current extracted fact to read its type.
        let current_fact = facts.iter().find(|f| f.fact_id == current_fact_id);
        let fact_type = current_fact
            .map(|f| f.fact_type.clone())
            .unwrap_or_default();
        // Load both fact records to read their content.
        let current_record = service
            .find_fact_record(current_fact_id)
            .await
            .ok()
            .and_then(|(r, _)| {
                r.and_then(|record| {
                    let v = serde_json::Value::Object(record);
                    fact_from_value_or_wrapper(&v)
                })
            });
        let new_content = current_record
            .as_ref()
            .map(|f| f.content.clone())
            .unwrap_or_default();
        let counterpart = service
            .find_fact_record(counterpart_id)
            .await
            .ok()
            .and_then(|(r, _)| {
                r.and_then(|record| {
                    let v = serde_json::Value::Object(record);
                    fact_from_value_or_wrapper(&v)
                })
            });
        let counterpart_content = counterpart
            .as_ref()
            .map(|f| f.content.clone())
            .unwrap_or_default();
        warnings.push(ContradictionWarning {
            fact_type,
            new_fact_id: current_fact_id.to_string(),
            conflicting_fact_id: counterpart_id.to_string(),
            existing_content: counterpart_content,
            new_content,
            entity_ids: Vec::new(),
            reason: format!("claim_contradiction:{}", rel.reason_code),
        });
    }
    Ok(warnings)
}

/// Extract entities and facts from an episode.
pub async fn extract_from_episode(
    service: &MemoryService,
    episode_id: &str,
    zero_shot_labels: Option<&[String]>,
) -> Result<ExtractResult, MemoryError> {
    use std::time::Instant;

    let timer = Instant::now();

    service.logger.log(
        log_event(
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
    let warnings = detect_contradiction_warnings(service, &facts, &namespace).await?;
    let mut links = Vec::new();
    let edge_ingested = now();

    for entity in &entities {
        links.push(ExtractedLink {
            entity_id: entity.entity_id.clone(),
            episode_id: episode_id.to_string(),
        });

        let edge = Edge {
            in_id: entity.entity_id.clone(),
            relation: "mentioned_in".to_string(),
            out_id: episode_id.to_string(),
            origin: EdgeOrigin::Extracted,
            strength: 1.0,
            confidence: 0.9,
            provenance: crate::models::Provenance::agent_observation(episode_id),
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
                origin: EdgeOrigin::Extracted,
                strength: 0.8,
                confidence: 0.85,
                provenance: crate::models::Provenance::agent_observation(episode_id),
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
        log_event(
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
        reconciliation: None,
    })
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
