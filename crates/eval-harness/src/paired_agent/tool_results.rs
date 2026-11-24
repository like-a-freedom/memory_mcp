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

pub(crate) fn read_result(result: &Value) -> (super::MemoryReadOutcome, Option<u32>) {
    use super::MemoryReadOutcome;
    let count = result
        .pointer("/structuredContent/result")
        .and_then(Value::as_array)
        .and_then(|items| u32::try_from(items.len()).ok());
    let outcome = match count {
        Some(0) => MemoryReadOutcome::Empty,
        Some(_) => MemoryReadOutcome::Nonempty,
        None => MemoryReadOutcome::Unknown,
    };
    (outcome, count)
}

pub(crate) fn cutoff_digest(
    valid_at: chrono::DateTime<chrono::Utc>,
    revision: Option<i64>,
) -> Result<String, crate::EvalError> {
    Ok(super::sha256(&serde_json::to_vec(&(
        valid_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        revision,
    ))?))
}

pub(crate) fn requested_cutoff(arguments: &Value) -> Option<String> {
    let valid_at = arguments.get("valid_at")?.as_str()?.parse().ok()?;
    cutoff_digest(
        valid_at,
        arguments.get("known_revision").and_then(Value::as_i64),
    )
    .ok()
}

pub(crate) fn read_evidence(
    result: &Value,
    requested_cutoff_sha256: Option<String>,
) -> Option<super::MemoryReadEvidence> {
    let payload = result.get("structuredContent")?;
    let items = payload.get("result")?.as_array()?;
    let mut source_episode_ids = Vec::with_capacity(items.len());
    let mut claim_annotations = Vec::new();
    for item in items {
        let source = item.get("source_episode")?.as_str()?.to_owned();
        if source.trim().is_empty() {
            return None;
        }
        if let Some(annotation) =
            crate::independent_probe::claim_annotation_from_item(item, source.clone()).ok()?
        {
            claim_annotations.push(annotation);
        }
        source_episode_ids.push(source);
    }
    Some(super::MemoryReadEvidence {
        source_episode_ids,
        claim_annotations,
        completeness: payload
            .pointer("/context/completeness")
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        requested_cutoff_sha256,
        returned_cutoff_sha256: payload
            .pointer("/context/snapshot")
            .filter(|snapshot| {
                snapshot
                    .get("known_revision")
                    .and_then(Value::as_i64)
                    .is_some()
            })
            .and_then(requested_cutoff),
    })
}
