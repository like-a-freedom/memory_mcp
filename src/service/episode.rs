//! Episode operations - extraction and record parsing.

use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Value, json};

use super::core::log_args_with_duration;
use super::error::MemoryError;
use super::query::parse_iso;
use crate::logging::LogLevel;
use crate::models::Edge;
use crate::models::Episode;
use crate::models::{
    ContradictionWarning, ExtractResult, ExtractedEntity, ExtractedFact, ExtractedLink, FactType,
};
use std::time::Instant;

fn unwrap_string_value(v: &Value) -> Option<&str> {
    if let Some(s) = v.as_str() {
        Some(s)
    } else if let Some(obj) = v.as_object() {
        obj.get("String")
            .and_then(Value::as_str)
            .or_else(|| obj.get("Datetime").and_then(Value::as_str))
            .or_else(|| obj.get("Strand").and_then(Value::as_str))
            .or_else(|| {
                obj.get("Strand")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                obj.get("Datetime")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
            })
    } else {
        None
    }
}

fn unwrap_array_value(v: &Value) -> Option<&Vec<Value>> {
    if let Some(arr) = v.as_array() {
        Some(arr)
    } else if let Some(obj) = v.as_object() {
        obj.get("Array").and_then(Value::as_array)
    } else {
        None
    }
}

/// Parse an episode from a database record.
#[must_use]
pub fn episode_from_record(record: &serde_json::Map<String, Value>) -> Option<Episode> {
    Some(Episode {
        episode_id: unwrap_string_value(record.get("episode_id")?)?.to_string(),
        source_type: unwrap_string_value(record.get("source_type")?)?.to_string(),
        source_id: unwrap_string_value(record.get("source_id")?)?.to_string(),
        content: unwrap_string_value(record.get("content")?)?.to_string(),
        t_ref: parse_iso(unwrap_string_value(record.get("t_ref")?)?)?,
        t_ingested: parse_iso(unwrap_string_value(record.get("t_ingested")?)?)?,
        scope: unwrap_string_value(record.get("scope")?)?.to_string(),
        visibility_scope: record
            .get("visibility_scope")
            .and_then(unwrap_string_value)
            .unwrap_or_default()
            .to_string(),
        policy_tags: record
            .get("policy_tags")
            .and_then(unwrap_array_value)
            .map(|values| {
                values
                    .iter()
                    .filter_map(unwrap_string_value)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Extract a string field from a JSON map, handling SurrealDB String wrappers.
fn str_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(unwrap_string_value).map(String::from)
}

/// Extract a datetime field from a JSON map.
fn dt_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<DateTime<Utc>> {
    map.get(key)
        .and_then(unwrap_string_value)
        .and_then(parse_iso)
}

/// Extract an f64 field, handling SurrealDB Number/Float wrappers.
fn f64_field(map: &serde_json::Map<String, Value>, key: &str, default: f64) -> f64 {
    map.get(key)
        .and_then(|v| {
            v.as_f64().or_else(|| {
                v.as_object().and_then(|obj| {
                    obj.get("Number")
                        .or_else(|| obj.get("Float"))
                        .and_then(Value::as_f64)
                })
            })
        })
        .unwrap_or(default)
}

/// Extract an i64 field, handling SurrealDB Number wrappers.
fn i64_field(map: &serde_json::Map<String, Value>, key: &str, default: i64) -> i64 {
    map.get(key)
        .and_then(|v| {
            v.as_i64().or_else(|| {
                v.as_object()
                    .and_then(|o| o.get("Number").and_then(Value::as_i64))
            })
        })
        .unwrap_or(default)
}

/// Extract a string array field from a JSON map.
fn str_array_field(map: &serde_json::Map<String, Value>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(unwrap_array_value)
        .map(|values| {
            values
                .iter()
                .filter_map(unwrap_string_value)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a fact from a database record.
#[must_use]
pub fn fact_from_record(record: &Value) -> Option<crate::models::Fact> {
    let map = record.as_object()?;

    let t_valid = dt_field(map, "t_valid")?;

    Some(crate::models::Fact {
        fact_id: str_field(map, "fact_id")?,
        fact_type: str_field(map, "fact_type")?,
        content: str_field(map, "content")?,
        quote: str_field(map, "quote")?,
        source_episode: str_field(map, "source_episode")?,
        t_valid,
        t_ingested: dt_field(map, "t_ingested").unwrap_or(t_valid),
        t_invalid: dt_field(map, "t_invalid"),
        t_invalid_ingested: dt_field(map, "t_invalid_ingested"),
        confidence: f64_field(map, "confidence", 0.0),
        index_keys: str_array_field(map, "index_keys"),
        access_count: i64_field(map, "access_count", 0),
        last_accessed: dt_field(map, "last_accessed"),
        entity_links: str_array_field(map, "entity_links"),
        scope: str_field(map, "scope").unwrap_or_default(),
        policy_tags: str_array_field(map, "policy_tags"),
        provenance: map.get("provenance").cloned().unwrap_or(Value::Null),
        ft_score: f64_field(map, "ft_score", 0.0),
    })
}

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

    let extraction_result = match zero_shot_labels {
        Some(labels) => {
            service
                .entity_extractor
                .extract_candidates_with_labels(content, labels)
                .await
        }
        None => service.entity_extractor.extract_candidates(content).await,
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
        ),
        LogLevel::Info,
    );

    let mut entities = Vec::with_capacity(candidates.len());

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
                    ),
                    LogLevel::Warn,
                );
            })?;

        entities.push(ExtractedEntity {
            entity_id,
            entity_type,
            canonical_name,
        });
    }

    Ok(entities)
}

/// Extract facts from an episode.
pub async fn extract_facts(
    service: &crate::service::MemoryService,
    episode: &Episode,
    entities: &[ExtractedEntity],
) -> Result<Vec<ExtractedFact>, MemoryError> {
    let mut facts = Vec::new();
    let normalized = episode.content.to_lowercase();
    let entity_links = entities
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect::<Vec<_>>();

    if is_metric_statement(&episode.content) {
        facts.push(
            add_extracted_fact(service, episode, FactType::Metric.as_str(), &entity_links).await?,
        );
    }

    if is_promise_statement(&normalized) || is_document_action_item(&episode.content) {
        facts.push(
            add_extracted_fact(service, episode, FactType::Promise.as_str(), &entity_links).await?,
        );
    }

    if is_experience_statement(&episode.content) {
        facts.push(
            add_extracted_fact(
                service,
                episode,
                FactType::Experience.as_str(),
                &entity_links,
            )
            .await?,
        );
    }

    Ok(facts)
}

async fn add_extracted_fact(
    service: &crate::service::MemoryService,
    episode: &Episode,
    fact_type: &str,
    entity_links: &[String],
) -> Result<ExtractedFact, MemoryError> {
    use serde_json::json;

    let fact_id = service
        .add_fact(
            fact_type,
            &episode.content,
            &episode.content,
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
            }),
        )
        .await?;

    Ok(ExtractedFact {
        fact_id,
        fact_type: fact_type.to_string(),
    })
}

/// Check if content contains a promise statement.
#[must_use]
pub fn is_promise_statement(content: &str) -> bool {
    static PROMISE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(i will|i'll|will\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule)|going to\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule))\b")
            .expect("promise regex is valid")
    });
    PROMISE_RE.is_match(content)
}

/// Detects metric-related content using word-boundary matching.
///
/// Matches financial metrics (ARR, MRR, NRR, revenue, churn) and dollar amounts.
/// Avoids false positives on words like "barrel", "narrative", "arrive".
pub fn is_metric_statement(content: &str) -> bool {
    static METRIC_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(ARR|MRR|NRR|revenue|churn|ROI|LTV|CAC|NPS|EBITDA)\b|\$\d")
            .expect("metric regex is valid")
    });
    METRIC_RE.is_match(content)
}

/// Detects preference/profile statements that should be stored as experience facts.
#[must_use]
pub fn is_experience_statement(content: &str) -> bool {
    static EXPERIENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(prefer|prefers|dislike|dislikes|enjoy|enjoys|love|loves|hate|hates|value|values)\b")
            .expect("experience regex is valid")
    });
    let normalized = content.to_lowercase();
    EXPERIENCE_RE.is_match(&normalized)
}

/// Detects document-style action items (for example from emails) as promise-like commitments.
#[must_use]
pub fn is_document_action_item(content: &str) -> bool {
    static ACTION_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*(action items?|next steps|follow-?ups?|todo)\s*:")
            .expect("action-item header regex is valid")
    });
    static ACTION_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*(?:[-*]|\d+\.)\s+[a-z]+(?:\s+[a-z]+){0,2}\s*(?::|-)\s*(?:send|review|share|update|prepare|schedule|confirm|draft|deliver|complete|close|fix|follow(?:\s+|-)?up)\b")
            .expect("action-item line regex is valid")
    });
    let normalized = content.to_lowercase();
    ACTION_HEADER_RE.is_match(&normalized) && ACTION_LINE_RE.is_match(&normalized)
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
        ),
        LogLevel::Info,
    );

    let (record, namespace) = service.find_episode_record(episode_id).await?;
    let namespace =
        namespace.ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;
    let record = record.ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;

    let episode = episode_from_record(&record)
        .ok_or_else(|| MemoryError::NotFound("episode_id not found".into()))?;

    let entities = extract_entities(service, &episode.content, zero_shot_labels).await?;
    let facts = extract_facts(service, &episode, &entities).await?;
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
        for entity in &entities {
            let edge = Edge {
                in_id: entity.entity_id.clone(),
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
            json!({"entities": entities.len(), "facts": facts.len(), "warnings": warnings.len()}),
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
                || !fact_is_active_for_warning(existing_fact, cutoff)
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

fn fact_from_value_or_wrapper(value: &Value) -> Option<crate::models::Fact> {
    fact_from_record(value).or_else(|| value.get("Object").and_then(fact_from_record))
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

fn fact_is_active_for_warning(
    fact: &crate::models::Fact,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> bool {
    if fact.t_valid > cutoff || fact.t_ingested > cutoff {
        return false;
    }

    match (fact.t_invalid, fact.t_invalid_ingested) {
        (None, _) => true,
        (Some(invalidated_at), _) if invalidated_at > cutoff => true,
        (_, Some(invalidated_ingested_at)) if invalidated_ingested_at > cutoff => true,
        _ => false,
    }
}

/// Build a JSON payload map from an edge for database storage.
fn build_edge_payload(edge: &Edge, edge_id: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("edge_id".to_string(), Value::String(edge_id.to_string()));
    m.insert("in".to_string(), Value::String(edge.in_id.clone()));
    m.insert("relation".to_string(), Value::String(edge.relation.clone()));
    m.insert("out".to_string(), Value::String(edge.out_id.clone()));
    m.insert("origin".to_string(), json!(edge.origin));
    m.insert("strength".to_string(), json!(edge.strength));
    m.insert("confidence".to_string(), json!(edge.confidence));
    m.insert("provenance".to_string(), edge.provenance.clone());
    m.insert(
        "t_valid".to_string(),
        Value::String(super::normalize_dt(edge.t_valid)),
    );
    m.insert(
        "t_ingested".to_string(),
        Value::String(super::normalize_dt(edge.t_ingested)),
    );
    if let Some(t_invalid) = edge.t_invalid {
        m.insert(
            "t_invalid".to_string(),
            Value::String(super::normalize_dt(t_invalid)),
        );
    }
    if let Some(t_invalid_ingested) = edge.t_invalid_ingested {
        m.insert(
            "t_invalid_ingested".to_string(),
            Value::String(super::normalize_dt(t_invalid_ingested)),
        );
    }
    m
}

/// Store an edge in the database.
pub(crate) async fn store_edge(
    service: &crate::service::MemoryService,
    edge: &Edge,
    namespace: &str,
) -> Result<(), MemoryError> {
    let edge_id =
        super::ids::deterministic_edge_id(&edge.in_id, &edge.relation, &edge.out_id, edge.t_valid);

    let existing = service.db_client.select_one(&edge_id, namespace).await?;
    if existing.is_some() {
        return Ok(());
    }

    invalidate_conflicting_edges(service, edge, namespace).await?;

    let payload = build_edge_payload(edge, &edge_id);

    service
        .db_client
        .relate_edge(
            namespace,
            &edge_id,
            &edge.in_id,
            &edge.out_id,
            Value::Object(payload),
        )
        .await?;

    Ok(())
}

#[derive(Debug)]
struct StoredEdgeVersion {
    edge_id: String,
    in_id: String,
    relation: String,
    out_id: String,
    t_valid: chrono::DateTime<chrono::Utc>,
    t_ingested: chrono::DateTime<chrono::Utc>,
    t_invalid: Option<chrono::DateTime<chrono::Utc>>,
    t_invalid_ingested: Option<chrono::DateTime<chrono::Utc>>,
}

async fn invalidate_conflicting_edges(
    service: &crate::service::MemoryService,
    new_edge: &Edge,
    namespace: &str,
) -> Result<(), MemoryError> {
    let existing_edges = service
        .db_client
        .select_edges_for_triple(
            namespace,
            &new_edge.in_id,
            &new_edge.relation,
            &new_edge.out_id,
        )
        .await?;

    for existing in existing_edges
        .iter()
        .filter_map(stored_edge_version_from_record)
        .filter(|existing| edge_versions_conflict(existing, new_edge))
    {
        service
            .db_client
            .update(
                &existing.edge_id,
                serde_json::json!({
                    "t_invalid": super::normalize_dt(new_edge.t_valid),
                    "t_invalid_ingested": super::normalize_dt(new_edge.t_ingested),
                }),
                namespace,
            )
            .await?;
    }

    Ok(())
}

/// In the current flat-edge model, only active versions of the same logical
/// edge triple conflict. Broader semantic contradictions (for example,
/// relation-specific exclusivity across different targets) are deferred until
/// Task 5 introduces graph-native relation semantics.
///
/// Conflict requires BOTH timestamps to be <=: an existing edge is invalidated
/// only when the new edge is strictly newer in both t_valid AND t_ingested.
/// This is intentional: if t_valid is older but t_ingested is newer (retroactive
/// data entry), the edge should NOT be invalidated. Using OR would incorrectly
/// invalidate edges in such scenarios.
fn edge_versions_conflict(existing: &StoredEdgeVersion, new_edge: &Edge) -> bool {
    existing.in_id == new_edge.in_id
        && existing.relation == new_edge.relation
        && existing.out_id == new_edge.out_id
        && existing.t_valid <= new_edge.t_valid
        && existing.t_ingested <= new_edge.t_ingested
        && existing
            .t_invalid
            .is_none_or(|t_invalid| t_invalid > new_edge.t_valid)
        && existing
            .t_invalid_ingested
            .is_none_or(|t_invalid_ingested| t_invalid_ingested > new_edge.t_ingested)
}

fn stored_edge_version_from_record(record: &Value) -> Option<StoredEdgeVersion> {
    let map = record.as_object()?;

    let edge_id = map
        .get("edge_id")
        .and_then(unwrap_record_string)
        .or_else(|| map.get("id").and_then(unwrap_record_string))?;

    Some(StoredEdgeVersion {
        edge_id,
        in_id: map.get("in").and_then(unwrap_record_string)?,
        relation: map.get("relation").and_then(unwrap_record_string)?,
        out_id: map.get("out").and_then(unwrap_record_string)?,
        t_valid: map
            .get("t_valid")
            .and_then(unwrap_record_string)
            .as_deref()
            .and_then(parse_iso)?,
        t_ingested: map
            .get("t_ingested")
            .and_then(unwrap_record_string)
            .as_deref()
            .and_then(parse_iso)?,
        t_invalid: map
            .get("t_invalid")
            .and_then(unwrap_record_string)
            .as_deref()
            .and_then(parse_iso),
        t_invalid_ingested: map
            .get("t_invalid_ingested")
            .and_then(unwrap_record_string)
            .as_deref()
            .and_then(parse_iso),
    })
}

pub(crate) fn unwrap_record_string(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        Some(value.to_string())
    } else if let Some(object) = value.as_object() {
        object
            .get("String")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                object
                    .get("Datetime")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                object
                    .get("Strand")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                object
                    .get("Strand")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                object
                    .get("Datetime")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .or_else(|| {
                object.get("RecordId").and_then(|record_id| {
                    let record_id = record_id.as_object()?;
                    let table = record_id.get("table")?.as_str()?;
                    let key = record_id.get("key")?.as_str()?;
                    Some(format!("{table}:{key}"))
                })
            })
    } else {
        None
    }
}

/// Update community memberships.
async fn update_communities(
    service: &crate::service::MemoryService,
    entity_ids: &[String],
    scope: &str,
) -> Result<(), MemoryError> {
    use serde_json::json;

    if entity_ids.len() < 2 {
        return Ok(());
    }

    let namespace = service.namespace_for_scope(scope);
    let member_entities =
        collect_connected_entity_component(service, entity_ids, &namespace).await?;
    if member_entities.len() < 2 {
        return Ok(());
    }

    let summary = build_community_summary(service, &namespace, &member_entities).await?;
    let overlapping = find_overlapping_communities(service, &namespace, &member_entities).await?;
    let community_id = overlapping
        .iter()
        .map(|community| community.community_id.clone())
        .min()
        .unwrap_or_else(|| super::ids::deterministic_community_id(&member_entities));

    let payload = json!({
        "community_id": community_id,
        "member_entities": member_entities,
        "summary": summary,
        "updated_at": super::normalize_dt(super::query::now()),
    });

    let existing = service
        .db_client
        .select_one(&community_id, &namespace)
        .await?;
    if existing.is_some() {
        service
            .db_client
            .update(&community_id, payload, &namespace)
            .await?;
    } else {
        service
            .db_client
            .create(&community_id, payload, &namespace)
            .await?;
    }

    for stale in overlapping
        .into_iter()
        .filter(|community| community.community_id != community_id)
    {
        service
            .db_client
            .query(
                "DELETE type::record($community_id);",
                Some(json!({"community_id": stale.community_id})),
                &namespace,
            )
            .await?;
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct StoredCommunity {
    community_id: String,
    member_entities: Vec<String>,
}

/// Collects all entities connected via edges to the given seed entities.
///
/// Uses BFS traversal over the active edge set (bounded by `ACTIVE_EDGE_SCAN_LIMIT = 10_000`).
/// If the edge table exceeds this limit, the traversal will be incomplete.
/// A warning is logged when the limit is hit (see `db.select_edges_filtered.limit_hit`).
async fn collect_connected_entity_component(
    service: &crate::service::MemoryService,
    entity_ids: &[String],
    namespace: &str,
) -> Result<Vec<String>, MemoryError> {
    let cutoff = super::normalize_dt(super::query::now());
    let mut visited = std::collections::BTreeSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut traversed_nodes = std::collections::HashSet::new();

    for entity_id in entity_ids
        .iter()
        .filter(|entity_id| is_entity_id(entity_id))
    {
        if visited.insert(entity_id.clone()) {
            queue.push_back(entity_id.clone());
        }
    }

    while let Some(current) = queue.pop_front() {
        if !traversed_nodes.insert(current.clone()) {
            continue;
        }

        for direction in [
            crate::storage::GraphDirection::Incoming,
            crate::storage::GraphDirection::Outgoing,
        ] {
            let edges = service
                .db_client
                .select_edge_neighbors(namespace, &current, &cutoff, direction)
                .await?;

            for edge in edges.iter().filter_map(stored_edge_version_from_record) {
                let neighbor = match direction {
                    crate::storage::GraphDirection::Incoming => edge.in_id,
                    crate::storage::GraphDirection::Outgoing => edge.out_id,
                };

                if is_entity_id(&neighbor) {
                    if visited.insert(neighbor.clone()) {
                        queue.push_back(neighbor);
                    }
                    continue;
                }

                if is_traversable_context_node(&neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    Ok(visited.into_iter().collect())
}

pub(crate) async fn build_community_summary(
    service: &crate::service::MemoryService,
    namespace: &str,
    member_entities: &[String],
) -> Result<String, MemoryError> {
    let records = service
        .db_client
        .select_entities_by_ids(namespace, member_entities)
        .await?;
    let mut names = records
        .iter()
        .filter_map(|record| record.as_object())
        .filter_map(|record| {
            record
                .get("canonical_name")
                .and_then(unwrap_record_string)
                .or_else(|| {
                    record
                        .get("entity_id")
                        .and_then(unwrap_record_string)
                        .or_else(|| record.get("id").and_then(unwrap_record_string))
                })
        })
        .collect::<Vec<_>>();

    names.sort();
    names.dedup();

    let labels = if names.is_empty() {
        let mut fallback = member_entities.to_vec();
        fallback.sort();
        fallback.dedup();
        fallback
    } else {
        names
    };

    Ok(condense_community_labels(&labels))
}

fn condense_community_labels(labels: &[String]) -> String {
    let preview = labels
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let remaining = labels.len().saturating_sub(3);

    if remaining > 0 {
        format!("{preview} (+{remaining} more)")
    } else {
        preview
    }
}

async fn find_overlapping_communities(
    service: &crate::service::MemoryService,
    namespace: &str,
    member_entities: &[String],
) -> Result<Vec<StoredCommunity>, MemoryError> {
    let member_set: std::collections::HashSet<_> = member_entities.iter().cloned().collect();

    // Use index-based lookup via CONTAINSANY instead of full table scan.
    let communities = service
        .db_client
        .select_communities_by_member_entities(namespace, member_entities)
        .await?;

    Ok(communities
        .iter()
        .filter_map(stored_community_from_record)
        .filter(|community| {
            community
                .member_entities
                .iter()
                .any(|member| member_set.contains(member))
        })
        .collect())
}

fn stored_community_from_record(record: &Value) -> Option<StoredCommunity> {
    let map = record.as_object()?;
    let community_id = map
        .get("community_id")
        .and_then(unwrap_record_string)
        .or_else(|| map.get("id").and_then(unwrap_record_string))?;
    let member_entities = map
        .get("member_entities")
        .and_then(unwrap_record_array)
        .map(|values| {
            values
                .iter()
                .filter_map(unwrap_record_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(StoredCommunity {
        community_id,
        member_entities,
    })
}

fn unwrap_record_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        Some(array)
    } else if let Some(object) = value.as_object() {
        object.get("Array").and_then(Value::as_array)
    } else {
        None
    }
}

fn is_entity_id(record_id: &str) -> bool {
    record_id.starts_with("entity:")
}

fn is_traversable_context_node(record_id: &str) -> bool {
    record_id.starts_with("episode:") || record_id.starts_with("fact:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
