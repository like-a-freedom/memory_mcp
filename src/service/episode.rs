//! Episode operations - extraction and record parsing.

use regex::Regex;
use serde_json::Value;

use super::error::MemoryError;
use super::query::parse_iso;
use crate::models::Edge;
use crate::models::Episode;
use crate::models::{ExtractResult, ExtractedEntity, ExtractedFact, ExtractedLink};

/// Parse an episode from a database record.
#[must_use]
pub fn episode_from_record(record: &serde_json::Map<String, Value>) -> Option<Episode> {
    fn unwrap_string(v: &Value) -> Option<&str> {
        if let Some(s) = v.as_str() {
            Some(s)
        } else if let Some(obj) = v.as_object() {
            obj.get("String")
                .and_then(Value::as_str)
                .or_else(|| obj.get("Strand").and_then(Value::as_str))
                .or_else(|| {
                    obj.get("Strand")
                        .and_then(|inner| inner.get("String"))
                        .and_then(Value::as_str)
                })
        } else {
            None
        }
    }

    fn unwrap_array(v: &Value) -> Option<&Vec<Value>> {
        if let Some(arr) = v.as_array() {
            Some(arr)
        } else if let Some(obj) = v.as_object() {
            obj.get("Array").and_then(Value::as_array)
        } else {
            None
        }
    }

    Some(Episode {
        episode_id: unwrap_string(record.get("episode_id")?)?.to_string(),
        source_type: unwrap_string(record.get("source_type")?)?.to_string(),
        source_id: unwrap_string(record.get("source_id")?)?.to_string(),
        content: unwrap_string(record.get("content")?)?.to_string(),
        t_ref: parse_iso(unwrap_string(record.get("t_ref")?)?)?,
        t_ingested: parse_iso(unwrap_string(record.get("t_ingested")?)?)?,
        scope: unwrap_string(record.get("scope")?)?.to_string(),
        visibility_scope: record
            .get("visibility_scope")
            .and_then(unwrap_string)
            .unwrap_or_default()
            .to_string(),
        policy_tags: record
            .get("policy_tags")
            .and_then(unwrap_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(unwrap_string)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Parse a fact from a database record.
#[must_use]
pub fn fact_from_record(record: &Value) -> Option<crate::models::Fact> {
    let map = record.as_object()?;

    fn unwrap_string(v: &Value) -> Option<&str> {
        if let Some(s) = v.as_str() {
            Some(s)
        } else if let Some(obj) = v.as_object() {
            obj.get("String")
                .and_then(Value::as_str)
                .or_else(|| obj.get("Strand").and_then(Value::as_str))
                .or_else(|| {
                    obj.get("Strand")
                        .and_then(|inner| inner.get("String"))
                        .and_then(Value::as_str)
                })
        } else {
            None
        }
    }

    fn unwrap_array(v: &Value) -> Option<&Vec<Value>> {
        if let Some(arr) = v.as_array() {
            Some(arr)
        } else if let Some(obj) = v.as_object() {
            obj.get("Array").and_then(|a| a.as_array())
        } else {
            None
        }
    }

    let t_valid_str = unwrap_string(map.get("t_valid")?)?;
    let t_valid = parse_iso(t_valid_str)?;
    let t_ingested = map
        .get("t_ingested")
        .and_then(unwrap_string)
        .and_then(parse_iso)
        .unwrap_or(t_valid);

    let fact_id = unwrap_string(map.get("fact_id")?)?.to_string();
    let fact_type = unwrap_string(map.get("fact_type")?)?.to_string();
    let content = unwrap_string(map.get("content")?)?.to_string();
    let quote = unwrap_string(map.get("quote")?)?.to_string();
    let source_episode = unwrap_string(map.get("source_episode")?)?.to_string();
    let scope = unwrap_string(map.get("scope")?)
        .unwrap_or_default()
        .to_string();

    Some(crate::models::Fact {
        fact_id,
        fact_type,
        content,
        quote,
        source_episode,
        t_valid,
        t_ingested,
        t_invalid: map
            .get("t_invalid")
            .and_then(unwrap_string)
            .and_then(parse_iso),
        t_invalid_ingested: map
            .get("t_invalid_ingested")
            .and_then(unwrap_string)
            .and_then(parse_iso),
        confidence: map
            .get("confidence")
            .and_then(|v| {
                if let Some(f) = v.as_f64() {
                    Some(f)
                } else if let Some(obj) = v.as_object() {
                    obj.get("Number")
                        .and_then(|n| n.as_f64())
                        .or_else(|| obj.get("Float").and_then(|n| n.as_f64()))
                } else {
                    None
                }
            })
            .unwrap_or(0.0),
        entity_links: map
            .get("entity_links")
            .and_then(unwrap_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(unwrap_string)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        scope,
        policy_tags: map
            .get("policy_tags")
            .and_then(unwrap_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(unwrap_string)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        provenance: map.get("provenance").cloned().unwrap_or(Value::Null),
    })
}

/// Extract entities from content.
pub async fn extract_entities(
    service: &crate::service::MemoryService,
    content: &str,
) -> Result<Vec<ExtractedEntity>, MemoryError> {
    use crate::models::EntityCandidate;

    let candidates: std::collections::HashSet<_> = service
        .name_regex
        .find_iter(content)
        .map(|mat| mat.as_str().to_string())
        .collect();

    let mut entities = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        let entity_type = if candidate.contains("Corp") || candidate.contains("Inc") {
            "company"
        } else {
            "person"
        };

        let entity_id = service
            .resolve(
                EntityCandidate {
                    entity_type: entity_type.to_string(),
                    canonical_name: candidate.clone(),
                    aliases: Vec::new(),
                },
                None,
            )
            .await?;

        entities.push(ExtractedEntity {
            entity_id,
            entity_type: entity_type.to_string(),
            canonical_name: candidate,
        });
    }

    Ok(entities)
}

/// Extract facts from an episode.
pub async fn extract_facts(
    service: &crate::service::MemoryService,
    episode: &Episode,
) -> Result<Vec<ExtractedFact>, MemoryError> {
    use serde_json::json;

    let mut facts = Vec::new();
    let normalized = episode.content.to_lowercase();

    // Detect metric facts
    if normalized.contains("arr") || episode.content.contains('$') {
        let fact_id = service
            .add_fact(
                "metric",
                &episode.content,
                &episode.content,
                &episode.episode_id,
                episode.t_ref,
                &episode.scope,
                0.7,
                Vec::new(),
                Vec::new(),
                json!({"source_episode": episode.episode_id}),
            )
            .await?;
        facts.push(ExtractedFact {
            fact_id,
            fact_type: "metric".to_string(),
        });
    }

    // Detect promise facts
    if is_promise_statement(&normalized) {
        let fact_id = service
            .add_fact(
                "promise",
                &episode.content,
                &episode.content,
                &episode.episode_id,
                episode.t_ref,
                &episode.scope,
                0.7,
                Vec::new(),
                Vec::new(),
                json!({"source_episode": episode.episode_id}),
            )
            .await?;
        facts.push(ExtractedFact {
            fact_id,
            fact_type: "promise".to_string(),
        });
    }

    Ok(facts)
}

/// Check if content contains a promise statement.
#[must_use]
pub fn is_promise_statement(content: &str) -> bool {
    static PROMISE_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let promise_re = PROMISE_RE.get_or_init(|| {
        Regex::new(r"\b(i will|i'll|will\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule)|going to\s+(?:finish|deliver|do|close|complete|implement|deploy|ship|fix|provide|send|schedule))\b")
            .expect("promise regex is valid")
    });
    content.contains("сделаю") || promise_re.is_match(content)
}

/// Extract entities and facts from an episode.
pub async fn extract_from_episode(
    service: &crate::service::MemoryService,
    episode_id: &str,
) -> Result<ExtractResult, MemoryError> {
    use crate::logging::LogLevel;
    use crate::models::Edge;
    use serde_json::json;

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

    let entities = extract_entities(service, &episode.content).await?;
    let facts = extract_facts(service, &episode).await?;
    let mut links = Vec::new();
    let edge_ingested = super::query::now();

    // Create entity-episode edges
    for entity in &entities {
        links.push(ExtractedLink {
            entity_id: entity.entity_id.clone(),
            episode_id: episode_id.to_string(),
        });

        let edge = Edge {
            from_id: entity.entity_id.clone(),
            relation: "mentioned_in".to_string(),
            to_id: episode_id.to_string(),
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

    // Create entity-fact edges
    for fact in &facts {
        for entity in &entities {
            let edge = Edge {
                from_id: entity.entity_id.clone(),
                relation: "involved_in".to_string(),
                to_id: fact.fact_id.clone(),
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

    // Update communities
    let entity_ids: Vec<String> = entities
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect();

    update_communities(service, &entity_ids, &episode.scope).await?;

    service.logger.log(
        super::log_event(
            "extract_from_episode.done",
            json!({"episode_id": episode_id}),
            json!({"entities": entities.len(), "facts": facts.len()}),
            None,
        ),
        LogLevel::Info,
    );

    Ok(ExtractResult {
        episode_id: episode_id.to_string(),
        entities,
        facts,
        links,
    })
}

/// Store an edge in the database.
pub(crate) async fn store_edge(
    service: &crate::service::MemoryService,
    edge: &Edge,
    namespace: &str,
) -> Result<(), MemoryError> {
    use serde_json::json;

    let edge_id =
        super::ids::deterministic_edge_id(&edge.from_id, &edge.relation, &edge.to_id, edge.t_valid);

    let existing = service.db_client.select_one(&edge_id, namespace).await?;
    if existing.is_some() {
        return Ok(());
    }

    let mut payload_map = serde_json::Map::new();
    payload_map.insert("edge_id".to_string(), Value::String(edge_id.clone()));
    payload_map.insert("from_id".to_string(), Value::String(edge.from_id.clone()));
    payload_map.insert("relation".to_string(), Value::String(edge.relation.clone()));
    payload_map.insert("to_id".to_string(), Value::String(edge.to_id.clone()));
    payload_map.insert("strength".to_string(), json!(edge.strength));
    payload_map.insert("confidence".to_string(), json!(edge.confidence));
    payload_map.insert("provenance".to_string(), json!({}));
    payload_map.insert(
        "t_valid".to_string(),
        Value::String(super::normalize_dt(edge.t_valid)),
    );
    payload_map.insert(
        "t_ingested".to_string(),
        Value::String(super::normalize_dt(edge.t_ingested)),
    );
    if let Some(t_invalid) = edge.t_invalid {
        payload_map.insert(
            "t_invalid".to_string(),
            Value::String(super::normalize_dt(t_invalid)),
        );
    }
    if let Some(t_invalid_ingested) = edge.t_invalid_ingested {
        payload_map.insert(
            "t_invalid_ingested".to_string(),
            Value::String(super::normalize_dt(t_invalid_ingested)),
        );
    }

    service
        .db_client
        .create(&edge_id, Value::Object(payload_map), namespace)
        .await?;

    Ok(())
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

    let community_id = super::ids::deterministic_community_id(entity_ids);
    let namespace = service.namespace_for_scope(scope);

    let mut names = Vec::new();
    let records = service
        .db_client
        .select_table("entity", &service.default_namespace)
        .await?;

    for record in records {
        if let Value::Object(map) = record {
            let entity_id = map
                .get("entity_id")
                .and_then(Value::as_str)
                .or_else(|| map.get("id").and_then(Value::as_str))
                .unwrap_or("");

            if entity_ids.contains(&entity_id.to_string()) {
                names.push(
                    map.get("canonical_name")
                        .and_then(Value::as_str)
                        .unwrap_or(entity_id)
                        .to_string(),
                );
            }
        }
    }

    let summary = if !names.is_empty() {
        names.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
    } else {
        entity_ids
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };

    let payload = json!({
        "community_id": community_id,
        "member_entities": entity_ids,
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

    Ok(())
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
        // Missing source_type

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
        // No visibility_scope or policy_tags

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
    fn is_promise_statement_detects_promise_patterns() {
        assert!(is_promise_statement("i will finish this task"));
        assert!(is_promise_statement("i'll deliver the report tomorrow"));
        assert!(is_promise_statement("will complete the project"));
        assert!(is_promise_statement("going to implement the feature"));
        assert!(is_promise_statement("I сделаю это завтра"));
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
}
