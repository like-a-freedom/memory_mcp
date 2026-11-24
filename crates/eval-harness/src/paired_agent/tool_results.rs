//! Normalize public memory-tool result envelopes into content-free receipts.

use std::collections::BTreeSet;

use serde_json::Value;

pub(crate) fn collect_source_episode_ids(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_source_episode_ids(value, found);
            }
        }
        Value::Object(fields) => {
            if let Some(id) = fields
                .get("source_episode")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
            {
                found.insert(id.to_string());
            }
            for value in fields.values() {
                collect_source_episode_ids(value, found);
            }
        }
        Value::String(text) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                collect_source_episode_ids(&parsed, found);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

pub(crate) fn collect_ingested_episode_id(value: &Value, found: &mut BTreeSet<String>) {
    if let Some(id) = episode_id_from_ingest_payload(value) {
        found.insert(id);
        return;
    }
    let Some(content) = value.get("content").and_then(Value::as_array) else {
        return;
    };
    for item in content {
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        if let Some(id) = episode_id_from_ingest_payload(&payload) {
            found.insert(id);
        }
    }
}

pub(crate) fn collect_inline_extracted_episode_id(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_inline_extracted_episode_id(value, found);
            }
        }
        Value::Object(fields) => {
            if let Some(id) = fields
                .get("episode_id")
                .and_then(Value::as_str)
                .filter(|id| id.starts_with("episode:") && id.len() > "episode:".len())
            {
                found.insert(id.to_owned());
            }
            for value in fields.values() {
                collect_inline_extracted_episode_id(value, found);
            }
        }
        Value::String(text) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                collect_inline_extracted_episode_id(&parsed, found);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

pub(crate) fn has_complete_fact_evidence(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(has_complete_fact_evidence),
        Value::Object(fields) => {
            let has_episode_id = fields
                .get("episode_id")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            let has_fact_fields = has_episode_id
                && fields
                    .get("facts")
                    .and_then(Value::as_array)
                    .is_some_and(|facts| {
                        facts.iter().any(|fact| {
                            let statement = fact
                                .get("statement")
                                .and_then(Value::as_str)
                                .is_some_and(|value| !value.trim().is_empty());
                            let source_quote = fact
                                .get("source_quote")
                                .and_then(Value::as_str)
                                .is_some_and(|value| !value.trim().is_empty());
                            statement && source_quote
                        })
                    });
            has_fact_fields || fields.values().any(has_complete_fact_evidence)
        }
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|parsed| has_complete_fact_evidence(&parsed)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn episode_id_from_ingest_payload(value: &Value) -> Option<String> {
    value
        .pointer("/structuredContent/result")
        .or_else(|| value.pointer("/result"))
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("episode:") && id.len() > "episode:".len())
        .map(str::to_owned)
}
