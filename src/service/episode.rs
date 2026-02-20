//! Episode operations - extraction and record parsing.

use regex::Regex;
use serde_json::Value;

use crate::models::Edge;
use crate::models::Episode;
use super::error::MemoryError;
use super::query::parse_iso;

/// Parse an episode from a database record.
#[must_use]
pub fn episode_from_record(record: &serde_json::Map<String, Value>) -> Option<Episode> {
    Some(Episode {
        episode_id: record.get("episode_id")?.as_str()?.to_string(),
        source_type: record.get("source_type")?.as_str()?.to_string(),
        source_id: record.get("source_id")?.as_str()?.to_string(),
        content: record.get("content")?.as_str()?.to_string(),
        t_ref: parse_iso(record.get("t_ref")?.as_str()?)?,
        t_ingested: parse_iso(record.get("t_ingested")?.as_str()?)?,
        scope: record.get("scope")?.as_str()?.to_string(),
        visibility_scope: record
            .get("visibility_scope")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        policy_tags: record
            .get("policy_tags")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
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
            obj.get("String").and_then(|s| s.as_str())
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
) -> Result<Vec<Value>, MemoryError> {
    use crate::models::EntityCandidate;
    use serde_json::json;

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

        entities.push(json!({
            "entity_id": entity_id,
            "type": entity_type,
            "canonical_name": candidate,
        }));
    }

    Ok(entities)
}

/// Extract facts from an episode.
pub async fn extract_facts(
    service: &crate::service::MemoryService,
    episode: &Episode,
) -> Result<Vec<Value>, MemoryError> {
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
        facts.push(json!({"fact_id": fact_id, "type": "metric"}));
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
        facts.push(json!({"fact_id": fact_id, "type": "promise"}));
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
) -> Result<Value, MemoryError> {
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
        links.push(json!({
            "entity_id": entity["entity_id"].clone(),
            "episode_id": episode_id,
        }));

        let edge = Edge {
            from_id: entity["entity_id"].as_str().unwrap_or("").to_string(),
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
                from_id: entity["entity_id"].as_str().unwrap_or("").to_string(),
                relation: "involved_in".to_string(),
                to_id: fact["fact_id"].as_str().unwrap_or("").to_string(),
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
        .filter_map(|entity| entity.get("entity_id").and_then(Value::as_str))
        .map(String::from)
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

    Ok(json!({
        "episode_id": episode_id,
        "entities": entities,
        "facts": facts,
        "links": links,
    }))
}

/// Store an edge in the database.
async fn store_edge(
    service: &crate::service::MemoryService,
    edge: &Edge,
    namespace: &str,
) -> Result<(), MemoryError> {
    use serde_json::json;

    let edge_id = super::ids::deterministic_edge_id(&edge.from_id, &edge.relation, &edge.to_id, edge.t_valid);

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
    payload_map.insert("provenance".to_string(), edge.provenance.clone());
    payload_map.insert("t_valid".to_string(), Value::String(super::normalize_dt(edge.t_valid)));
    payload_map.insert("t_ingested".to_string(), Value::String(super::normalize_dt(edge.t_ingested)));
    if let Some(t_invalid) = edge.t_invalid {
        payload_map.insert("t_invalid".to_string(), Value::String(super::normalize_dt(t_invalid)));
    }
    if let Some(t_invalid_ingested) = edge.t_invalid_ingested {
        payload_map.insert(
            "t_invalid_ingested".to_string(),
            Value::String(super::normalize_dt(t_invalid_ingested)),
        );
    }

    service.db_client
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
        entity_ids.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
    };

    let payload = json!({
        "community_id": community_id,
        "member_entities": entity_ids,
        "summary": summary,
        "updated_at": super::normalize_dt(super::query::now()),
    });

    let existing = service.db_client.select_one(&community_id, &namespace).await?;
    if existing.is_some() {
        service.db_client
            .update(&community_id, payload, &namespace)
            .await?;
    } else {
        service.db_client
            .create(&community_id, payload, &namespace)
            .await?;
    }

    Ok(())
}
