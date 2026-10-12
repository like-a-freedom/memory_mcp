use serde_json::{Value, json};

use super::statement_detection::{
    is_document_action_item, is_experience_statement, is_metric_statement, is_promise_statement,
    is_summary_like_note_candidate,
};
use crate::error::MemoryError;
use crate::knowledge::fact_parsing::fact_from_value_or_wrapper;
use crate::logging::LogLevel;
use crate::memory::capabilities::deps::ExtractDeps;
use crate::memory::episode::communities::update_communities;
use crate::memory::episode::edges::store_edge;
use crate::memory::episode::entity_extraction::extract_entities;
use crate::memory::episode::summary_parser::{
    entity_links_for_fact_content, sanitized_content_for_entity_extraction,
    structured_summary_fact_candidates,
};
use crate::memory::record_parsing::episode_from_record;
use crate::models::claim::ClaimRelationOutcome;
use crate::models::{
    ContradictionWarning, Edge, EdgeOrigin, Episode, ExtractResult, ExtractedEntity, ExtractedFact,
    ExtractedLink, FactType,
};
use crate::platform::log_event::{duration_ms, log_event};
use crate::shared::temporal::now;

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

/// Whether the note-fallback fact may be extracted for this episode.
///
/// The reporting predicate in [`build_extract_log_result`] mirrors the
/// source-type/content checks but adds its own precondition (exactly one
/// Note fact); the two are deliberately not merged — extraction gates on
/// "no facts yet", reporting on "the one fact is a note".
pub(super) fn should_extract_note_fact(episode: &Episode, facts: &[ExtractedFact]) -> bool {
    if !facts.is_empty() {
        return false;
    }

    let supported_source_type = source_type_supports_summary_fallback(&episode.source_type);

    supported_source_type && is_summary_like_note_candidate(&episode.content)
}

pub(super) async fn add_extracted_fact(
    service: &ExtractDeps,
    episode: &Episode,
    fact_type: &str,
    content: &str,
    quote: &str,
    entity_links: &[String],
    extraction_strategy: &str,
) -> Result<ExtractedFact, MemoryError> {
    let fact_id = service
        .fact_service
        .add_fact(
            service,
            fact_type,
            content,
            quote,
            &episode.episode_id,
            episode.t_ref,
            0.7,
            entity_links.to_vec(),
            episode.policy_tags.clone(),
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

/// Push a heuristic fact for the episode content when `enabled` is true.
/// Content/quote are always the full episode text with the
/// `episode_heuristic` strategy — the three heuristics differ only in type.
async fn push_heuristic(
    service: &ExtractDeps,
    episode: &Episode,
    facts: &mut Vec<ExtractedFact>,
    entity_links: &[String],
    fact_type: &str,
    enabled: bool,
) -> Result<(), MemoryError> {
    if !enabled {
        return Ok(());
    }
    facts.push(
        add_extracted_fact(
            service,
            episode,
            fact_type,
            &episode.content,
            &episode.content,
            entity_links,
            "episode_heuristic",
        )
        .await?,
    );
    Ok(())
}

/// Shared debug event for the two extract-path decisions that carry the
/// episode's source type and content length (structured-summary detection
/// and note fallback). `structured_line_fact_count` is present only on the
/// structured-summary event.
fn log_extraction_decision(
    service: &ExtractDeps,
    op: &str,
    episode: &Episode,
    structured_line_fact_count: Option<usize>,
) {
    let mut args = serde_json::Map::from_iter([
        ("episode_id".to_string(), json!(episode.episode_id)),
        ("source_type".to_string(), json!(episode.source_type)),
    ]);
    if let Some(count) = structured_line_fact_count {
        args.insert("structured_line_fact_count".to_string(), json!(count));
    }
    service.logger.log(
        log_event(
            op,
            Value::Object(args),
            json!({
                "content_chars": episode.content.chars().count(),
            }),
            None,
            None,
            None,
        ),
        LogLevel::Debug,
    );
}

/// Extract facts from an episode.
pub async fn extract_facts(
    service: &ExtractDeps,
    episode: &Episode,
    entities: &[ExtractedEntity],
) -> Result<FactExtractionOutcome, MemoryError> {
    let structured_candidates = structured_summary_fact_candidates(&episode.content);
    if !structured_candidates.is_empty() {
        log_extraction_decision(
            service,
            "extract.structured_summary",
            episode,
            Some(structured_candidates.len()),
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

    push_heuristic(
        service,
        episode,
        &mut facts,
        &entity_links,
        FactType::Metric.as_str(),
        is_metric_statement(&episode.content),
    )
    .await?;
    push_heuristic(
        service,
        episode,
        &mut facts,
        &entity_links,
        FactType::Promise.as_str(),
        is_promise_statement(&normalized) || is_document_action_item(&episode.content),
    )
    .await?;
    push_heuristic(
        service,
        episode,
        &mut facts,
        &entity_links,
        FactType::Experience.as_str(),
        is_experience_statement(&episode.content),
    )
    .await?;

    let note_fallback_used = should_extract_note_fact(episode, &facts);
    if note_fallback_used {
        log_extraction_decision(service, "extract.note_fallback", episode, None);

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
    service: &ExtractDeps,
    facts: &[ExtractedFact],
) -> Result<Vec<ContradictionWarning>, MemoryError> {
    claim_based_contradiction_warnings(service, facts).await
}

/// Load claim relations for the given fact ids. Error handling is the caller's
/// concern (one path propagates, the reconciliation path swallows).
async fn claim_relations_for_facts(
    service: &ExtractDeps,
    fact_ids: &[crate::models::FactId],
) -> Result<Vec<crate::models::claim::ClaimRelation>, MemoryError> {
    let query = crate::knowledge::claims::RelationsForFactsQuery { fact_ids };
    service
        .claim_service
        .store
        .select_relations_for_facts(query)
        .await
}

/// Load a stored fact record and parse it, collapsing fetch/parse errors to
/// `None` (the contradiction-warning path treats a missing fact as empty
/// content rather than failing the extraction).
async fn fact_content_from_record(
    service: &ExtractDeps,
    fact_id: &str,
) -> Option<crate::models::Fact> {
    service
        .find_fact_record(fact_id)
        .await
        .ok()
        .flatten()
        .and_then(|record| fact_from_value_or_wrapper(&Value::Object(record)))
}

/// Query claim relations for active contradictions involving the extracted facts.
async fn claim_based_contradiction_warnings(
    service: &ExtractDeps,
    facts: &[ExtractedFact],
) -> Result<Vec<ContradictionWarning>, MemoryError> {
    let fact_ids: Vec<_> = facts
        .iter()
        .map(|f| crate::models::FactId::from(f.fact_id.as_str()))
        .collect();
    if fact_ids.is_empty() {
        return Ok(Vec::new());
    }
    let relations = claim_relations_for_facts(service, &fact_ids).await?;
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
        let current_record = fact_content_from_record(service, current_fact_id).await;
        let new_content = current_record
            .as_ref()
            .map(|f| f.content.clone())
            .unwrap_or_default();
        let counterpart = fact_content_from_record(service, counterpart_id).await;
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

/// Write `mentioned_in` edges from each extracted entity to the episode and
/// collect the corresponding extract links.
async fn store_mentioned_in(
    service: &ExtractDeps,
    episode_id: &str,
    episode: &Episode,
    entities: &[ExtractedEntity],
    links: &mut Vec<ExtractedLink>,
    t_ingested: chrono::DateTime<chrono::Utc>,
) -> Result<(), MemoryError> {
    for entity in entities {
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
            t_ingested,
            t_invalid: None,
            t_invalid_ingested: None,
        };
        store_edge(service, &edge).await?
    }
    Ok(())
}

/// Write `involved_in` edges from each stored fact's entity links to the fact.
async fn store_involved_in(
    service: &ExtractDeps,
    episode_id: &str,
    episode: &Episode,
    facts: &[ExtractedFact],
    t_ingested: chrono::DateTime<chrono::Utc>,
) -> Result<(), MemoryError> {
    for fact in facts {
        let fact_record = service.find_fact_record(&fact.fact_id).await?;
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
                t_ingested,
                t_invalid: None,
                t_invalid_ingested: None,
            };
            store_edge(service, &edge).await?
        }
    }
    Ok(())
}

/// Extract entities and facts from an episode.
pub async fn extract_from_episode(
    service: &ExtractDeps,
    episode_id: &str,
    zero_shot_labels: Option<&[String]>,
) -> Result<ExtractResult, MemoryError> {
    use std::time::Instant;

    let timer = Instant::now();

    service.logger.log(
        log_event(
            "extract.from_episode.start",
            json!({"episode_id": episode_id}),
            json!({}),
            None,
            None,
            None,
        ),
        LogLevel::Info,
    );

    let record = service.find_episode_record(episode_id).await?;
    let record = record.ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;

    let episode = episode_from_record(&record)
        .ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;

    crate::knowledge::api::validate_entity_extraction_input(
        service.entity_extractor.as_ref(),
        &episode.content,
    )?;
    let entity_extraction_content = sanitized_content_for_entity_extraction(&episode.content);
    let entities = extract_entities(
        service,
        episode_id,
        &entity_extraction_content,
        zero_shot_labels,
    )
    .await?;
    let fact_outcome = extract_facts(service, &episode, &entities).await?;
    let facts = fact_outcome.facts;
    let warnings = detect_contradiction_warnings(service, &facts).await?;
    let mut links = Vec::new();
    let edge_ingested = now();

    store_mentioned_in(
        service,
        episode_id,
        &episode,
        &entities,
        &mut links,
        edge_ingested,
    )
    .await?;
    store_involved_in(service, episode_id, &episode, &facts, edge_ingested).await?;

    let entity_ids: Vec<String> = entities
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect();

    let mut community_new_nodes = Vec::with_capacity(1 + facts.len());
    community_new_nodes.push(episode_id.to_string());
    community_new_nodes.extend(facts.iter().map(|fact| fact.fact_id.clone()));

    update_communities(service, &entity_ids, &community_new_nodes).await?;

    service.logger.log(
        log_event(
            "extract.from_episode.done",
            json!({"episode_id": episode_id}),
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
            Some(duration_ms(timer.elapsed())),
        ),
        LogLevel::Info,
    );

    // Build reconciliation summary when evidence exposure is enabled.
    // Shadow and Relations stages project claims and persist relations,
    // but only Evidence exposes authorized metadata to the extract API.
    // Queries active claim relations for the extracted facts.
    let reconciliation = if service
        .claim_service
        .config
        .rollout_stage
        .exposes_evidence()
    {
        let fact_ids: Vec<_> = facts
            .iter()
            .map(|f| crate::models::FactId::from(f.fact_id.as_str()))
            .collect();
        let relations = claim_relations_for_facts(service, &fact_ids)
            .await
            .unwrap_or_default();
        let active_relations = relations
            .iter()
            .filter(|r| r.t_invalid_ingested.is_none())
            .count();
        let reason_codes: Vec<String> = relations
            .iter()
            .map(|r| r.reason_code.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        Some(crate::models::ReconciliationSummary {
            status: crate::models::ReconciliationStatus::Complete,
            claims_projected: facts.len(),
            active_relations,
            reason_codes,
        })
    } else {
        None
    };

    // Facts, entities, links and the edges between them were all written by
    // the time control reaches here; an earlier `?` would have returned before
    // this line, so the knowledge clock is stamped only for an extraction that
    // actually persisted.
    //
    // Like the episode write in `memory::ingestion`, this is the shared funnel
    // — the `extract` tool, the filesystem watcher and the projection pass all
    // extract through this function — which is why the freshness gauge can be
    // read at all on a deployment that never calls an MCP tool.
    crate::observability::record_knowledge_write();

    Ok(ExtractResult {
        episode_id: episode_id.to_owned(),
        entities,
        facts,
        links,
        warnings,
        reconciliation,
    })
}

/// Build the extract log result, deriving `note_fallback_used` and the
/// structured-line fact count from the episode content.
///
/// The note-fallback reporting predicate here mirrors the
/// source-type/content checks of [`should_extract_note_fact`] but adds
/// its own precondition (exactly one Note fact); the two are
/// deliberately not merged — extraction gates on "no facts yet",
/// reporting on "the one fact is a note".
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Episode, ExtractedFact};
    use chrono::Utc;
    use serde_json::Value;

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
            source_lineage: None,
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
            source_lineage: None,
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
}
