//! Normalize public memory-tool result envelopes into content-free receipts.

use std::collections::BTreeSet;

use serde_json::Value;
use std::collections::BTreeMap;

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
    let count = read_payload(result)
        .and_then(|payload| payload.get("result"))
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

/// Fingerprint only the normalized valid-time axis, independently of a
/// knowledge-revision selector. RFC3339 offsets normalize to the same UTC hash.
pub(crate) fn valid_at_sha256(valid_at: chrono::DateTime<chrono::Utc>) -> String {
    super::sha256(
        valid_at
            .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
            .as_bytes(),
    )
}

pub(crate) fn requested_valid_at_sha256(arguments: &Value) -> Option<String> {
    let timestamp = arguments
        .get("valid_at")
        .and_then(Value::as_str)
        .or_else(|| arguments.get("as_of").and_then(Value::as_str))?;
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| valid_at_sha256(timestamp.with_timezone(&chrono::Utc)))
}

fn returned_valid_at_sha256(payload: &Value) -> Option<String> {
    let timestamp = payload
        .pointer("/context/snapshot/valid_at")
        .and_then(Value::as_str)?;
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| valid_at_sha256(timestamp.with_timezone(&chrono::Utc)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadRequestFingerprints {
    pub query_sha256: Option<String>,
    pub non_query_arguments_sha256: String,
    pub selector_arguments_sha256: BTreeMap<String, String>,
}

pub(crate) const ASSEMBLE_CONTEXT_SELECTOR_FIELDS: [&str; 10] = [
    "fact_types",
    "as_of",
    "valid_at",
    "known_at",
    "known_revision",
    "budget",
    "view_mode",
    "window_start",
    "window_end",
    "compact",
];

/// Retain only digests that let an offline evaluator prove a retry changed
/// the query while preserving every selector and budget argument.
pub(crate) fn read_request_fingerprints(
    arguments: &Value,
) -> Result<ReadRequestFingerprints, crate::EvalError> {
    let mut non_query_arguments = arguments.clone();
    let query = if let Value::Object(fields) = &mut non_query_arguments {
        fields.remove("query")
    } else {
        None
    };
    let query_sha256 = query
        .map(|query| serde_json::to_vec(&query).map(|bytes| super::sha256(&bytes)))
        .transpose()?;
    let non_query_arguments_sha256 = super::sha256(&serde_json::to_vec(&non_query_arguments)?);
    Ok(ReadRequestFingerprints {
        query_sha256,
        non_query_arguments_sha256,
        selector_arguments_sha256: selector_argument_fingerprints(arguments)?,
    })
}

pub(crate) fn expected_selector_argument_fingerprints(
    arguments: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, String>, crate::EvalError> {
    let value = Value::Object(
        arguments
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    selector_argument_fingerprints(&value)
}

pub(crate) fn validate_expected_selector_arguments(
    arguments: &BTreeMap<String, Value>,
) -> Result<(), crate::EvalError> {
    use crate::EvalError;

    for (field, value) in arguments {
        if !ASSEMBLE_CONTEXT_SELECTOR_FIELDS.contains(&field.as_str()) {
            return Err(EvalError::InvalidInput(
                "registered read selector contains an unsupported field".into(),
            ));
        }
        let valid = match field.as_str() {
            "fact_types" => value
                .as_array()
                .is_some_and(|values| values.iter().all(Value::is_string)),
            "as_of" => value.as_str().is_some_and(|timestamp| {
                timestamp.trim().is_empty()
                    || chrono::DateTime::parse_from_rfc3339(timestamp).is_ok()
            }),
            "valid_at" | "known_at" => {
                value.is_null()
                    || value.as_str().is_some_and(|timestamp| {
                        chrono::DateTime::parse_from_rfc3339(timestamp).is_ok()
                    })
            }
            "window_start" | "window_end" => {
                value.is_null()
                    || value.as_str().is_some_and(|timestamp| {
                        memory_mcp::tools::parsers::parse_datetime(timestamp).is_some()
                    })
            }
            "known_revision" => {
                value.is_null() || value.as_i64().is_some_and(|revision| revision >= 0)
            }
            "budget" => value
                .as_i64()
                .is_some_and(|budget| i32::try_from(budget).is_ok()),
            "view_mode" => value.is_null() || value.is_string(),
            "compact" => value.is_boolean(),
            _ => false,
        };
        if !valid {
            return Err(EvalError::InvalidInput(
                "registered read selector has an invalid value".into(),
            ));
        }
    }

    let as_of = arguments
        .get("as_of")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    if as_of
        && ["valid_at", "known_at", "known_revision"]
            .iter()
            .any(|field| arguments.get(*field).is_some_and(|value| !value.is_null()))
        || arguments
            .get("known_at")
            .is_some_and(|value| !value.is_null())
            && arguments
                .get("known_revision")
                .is_some_and(|value| !value.is_null())
    {
        return Err(EvalError::InvalidInput(
            "registered read selector combines incompatible temporal axes".into(),
        ));
    }
    Ok(())
}

fn selector_argument_fingerprints(
    arguments: &Value,
) -> Result<BTreeMap<String, String>, crate::EvalError> {
    let mut fingerprints = BTreeMap::new();
    for field in ASSEMBLE_CONTEXT_SELECTOR_FIELDS {
        let normalized = effective_selector_value(field, arguments.get(field));
        fingerprints.insert(
            field.to_owned(),
            super::sha256(&serde_json::to_vec(&normalized)?),
        );
    }
    Ok(fingerprints)
}

fn effective_selector_value(field: &str, value: Option<&Value>) -> Value {
    use serde_json::json;

    match field {
        "fact_types" => match value {
            None => json!([]),
            Some(Value::Array(values)) => {
                let mut normalized = values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                normalized.sort();
                normalized.dedup();
                json!(normalized)
            }
            Some(value) => value.clone(),
        },
        "as_of" => {
            let Some(value) = value else {
                return Value::Null;
            };
            if value.as_str().is_some_and(|value| value.trim().is_empty()) {
                return Value::Null;
            }
            normalized_timestamp_value(value)
        }
        "valid_at" | "known_at" => value.map(normalized_timestamp_value).unwrap_or(Value::Null),
        "window_start" | "window_end" => value
            .and_then(Value::as_str)
            .and_then(memory_mcp::tools::parsers::parse_datetime)
            .map(|timestamp| {
                Value::String(timestamp.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
            })
            .unwrap_or(Value::Null),
        "known_revision" => value
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(Value::Null),
        "view_mode" => match value {
            None | Some(Value::Null) => Value::Null,
            Some(Value::String(value)) => {
                let trimmed = value.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("current") {
                    Value::Null
                } else {
                    Value::String(trimmed.to_owned())
                }
            }
            Some(value) => value.clone(),
        },
        "budget" => match value {
            None => json!(5),
            Some(value) => value
                .as_i64()
                .map(|budget| json!(budget.clamp(1, 250)))
                .unwrap_or_else(|| value.clone()),
        },
        "compact" => value.cloned().unwrap_or(Value::Bool(true)),
        _ => Value::Null,
    }
}

fn normalized_timestamp_value(value: &Value) -> Value {
    let Some(timestamp) = value.as_str() else {
        return value.clone();
    };
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| {
            Value::String(
                timestamp
                    .with_timezone(&chrono::Utc)
                    .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
            )
        })
        .unwrap_or_else(|| value.clone())
}

/// Fingerprint guidance when present without persisting its natural-language text.
pub(crate) fn response_guidance_sha256(result: &Value) -> Option<String> {
    let guidance = read_payload(result)
        .and_then(|payload| payload.get("guidance"))
        .or_else(|| result.get("guidance"))
        .and_then(Value::as_str)?;
    Some(super::sha256(guidance.as_bytes()))
}

pub(crate) fn read_evidence(
    result: &Value,
    requested_cutoff_sha256: Option<String>,
) -> Option<super::MemoryReadEvidence> {
    let payload = read_payload(result)?;
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
        returned_valid_at_sha256: returned_valid_at_sha256(payload),
    })
}

fn read_payload(result: &Value) -> Option<&Value> {
    let tool_result = result
        .get("result")
        .or_else(|| result.get("payload"))
        .unwrap_or(result);
    tool_result
        .get("structuredContent")
        .or_else(|| tool_result.get("structured_content"))
}

pub(crate) fn read_fact_source_pairs(result: &Value) -> Vec<(String, String)> {
    let Some(items) = read_payload(result)
        .and_then(|payload| payload.get("result"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let fact_id = item.get("fact_id")?.as_str()?.trim();
            let source_episode = item.get("source_episode")?.as_str()?.trim();
            (!fact_id.is_empty() && !source_episode.is_empty())
                .then(|| (fact_id.to_owned(), source_episode.to_owned()))
        })
        .collect()
}

pub(crate) fn successful_read_fact_source_pairs(result: &Value) -> Vec<(String, String)> {
    let tool_result = result
        .get("result")
        .or_else(|| result.get("payload"))
        .unwrap_or(result);
    if result.get("isError").and_then(Value::as_bool) == Some(true)
        || tool_result.get("isError").and_then(Value::as_bool) == Some(true)
    {
        return Vec::new();
    }
    let Some(payload) = read_payload(result) else {
        return Vec::new();
    };
    if payload
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status != "success")
        || read_result(result).0 != super::MemoryReadOutcome::Nonempty
    {
        return Vec::new();
    }
    read_fact_source_pairs(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn selector_fingerprints(arguments: &Value) -> BTreeMap<String, String> {
        read_request_fingerprints(arguments)
            .expect("request fingerprints")
            .selector_arguments_sha256
    }

    use super::{
        read_evidence, read_request_fingerprints, read_result, requested_valid_at_sha256,
        response_guidance_sha256, successful_read_fact_source_pairs,
    };
    use crate::paired_agent::{MemoryReadOutcome, ReadCompleteness};

    #[test]
    fn reads_structured_evidence_from_json_rpc_tool_result_envelope() {
        let response = json!({
            "jsonrpc":"2.0",
            "id":7,
            "result":{"structuredContent":{
                "result":[{"source_episode":"episode:one","content":"Quartz-31"}],
                "context":{"completeness":"partial"}
            }}
        });

        let (outcome, count) = read_result(&response);
        let evidence = read_evidence(&response, None).expect("structured evidence");

        assert_eq!(outcome, MemoryReadOutcome::Nonempty);
        assert_eq!(count, Some(1));
        assert_eq!(evidence.source_episode_ids, ["episode:one"]);
        assert_eq!(evidence.completeness, Some(ReadCompleteness::Partial));
    }

    #[test]
    fn successful_fact_source_pairs_require_a_successful_nonempty_tool_result() {
        let response = json!({
            "result": {"structuredContent": {
                "status":"success",
                "result":[{"fact_id":"fact:one","source_episode":"episode:one"}]
            }}
        });

        assert_eq!(
            successful_read_fact_source_pairs(&response),
            [("fact:one".into(), "episode:one".into())]
        );
    }

    #[test]
    fn failed_tool_result_cannot_supply_exact_invalidation_evidence() {
        let response = json!({
            "isError":true,
            "result": {"structuredContent": {
                "status":"success",
                "result":[{"fact_id":"fact:one","source_episode":"episode:one"}]
            }}
        });

        assert!(successful_read_fact_source_pairs(&response).is_empty());
    }

    #[test]
    fn reads_structured_evidence_from_adapter_payload_envelope() {
        let response = json!({
            "payload":{"structured_content":{
                "result":[{"source_episode":"episode:two","content":"Harbor-91"}],
                "context":{"completeness":"complete"}
            }}
        });

        let (outcome, count) = read_result(&response);
        let evidence = read_evidence(&response, None).expect("structured evidence");

        assert_eq!(outcome, MemoryReadOutcome::Nonempty);
        assert_eq!(count, Some(1));
        assert_eq!(evidence.source_episode_ids, ["episode:two"]);
        assert_eq!(evidence.completeness, Some(ReadCompleteness::Complete));
    }

    #[test]
    fn reads_empty_result_from_supported_response_envelope() {
        let empty_response = json!({
            "structuredContent":{"result":[],"context":{"completeness":"partial"}}
        });

        assert_eq!(
            read_result(&empty_response),
            (MemoryReadOutcome::Empty, Some(0))
        );
    }

    #[test]
    fn extracts_fact_to_source_pairs_from_structured_read() {
        let response = json!({
            "result": {"structuredContent": {"result": [
                {"fact_id":"fact:old","source_episode":"episode:old"},
                {"fact_id":"fact:new","source_episode":"episode:new"}
            ]}}
        });

        assert_eq!(
            super::read_fact_source_pairs(&response),
            vec![
                ("fact:old".to_owned(), "episode:old".to_owned()),
                ("fact:new".to_owned(), "episode:new".to_owned())
            ]
        );
    }

    #[test]
    fn reports_unknown_for_unrecognized_response_envelope() {
        let unrecognized_response = json!({"content":[{"type":"text","text":"opaque"}]});

        assert_eq!(
            read_result(&unrecognized_response),
            (MemoryReadOutcome::Unknown, None)
        );
    }

    #[test]
    fn read_request_fingerprints_change_only_when_query_changes() {
        let first = json!({
            "query":"where is the archive key?",
            "valid_at":"2026-11-21T12:00:00Z",
            "fact_types":["decision"],
            "budget":2,
            "compact":true
        });
        let retry = json!({
            "query":"which key is stored at the archive?",
            "valid_at":"2026-11-21T12:00:00Z",
            "fact_types":["decision"],
            "budget":2,
            "compact":true
        });
        let broadened = json!({
            "query":"which key is stored at the archive?",
            "valid_at":"2026-11-22T12:00:00Z",
            "fact_types":["decision"],
            "budget":4,
            "compact":true
        });

        let first = read_request_fingerprints(&first).expect("fingerprint first request");
        let retry = read_request_fingerprints(&retry).expect("fingerprint retry request");
        let broadened =
            read_request_fingerprints(&broadened).expect("fingerprint broadened request");

        assert_ne!(first.query_sha256, retry.query_sha256);
        assert_eq!(
            first.non_query_arguments_sha256,
            retry.non_query_arguments_sha256
        );
        assert_ne!(
            retry.non_query_arguments_sha256,
            broadened.non_query_arguments_sha256
        );
        assert_eq!(
            first.selector_arguments_sha256["budget"],
            retry.selector_arguments_sha256["budget"]
        );
        assert_ne!(
            retry.selector_arguments_sha256["budget"],
            broadened.selector_arguments_sha256["budget"]
        );
    }

    #[test]
    fn selector_fingerprints_normalize_default_values() {
        let defaults = selector_fingerprints(&json!({}));
        let explicit_defaults = selector_fingerprints(&json!({
            "fact_types": [],
            "budget": 5,
            "compact": true
        }));
        assert_eq!(defaults, explicit_defaults);
    }

    #[test]
    fn selector_fingerprints_clamp_budget_to_server_minimum() {
        let minimum_budget = selector_fingerprints(&json!({"budget": 0}));
        let explicit_minimum = selector_fingerprints(&json!({"budget": 1}));
        assert_eq!(minimum_budget["budget"], explicit_minimum["budget"]);
    }

    #[test]
    fn selector_fingerprints_clamp_budget_to_server_maximum() {
        let maximum_budget = selector_fingerprints(&json!({"budget": 251}));
        let explicit_maximum = selector_fingerprints(&json!({"budget": 250}));
        assert_eq!(maximum_budget["budget"], explicit_maximum["budget"]);
    }

    #[test]
    fn selector_fingerprints_treat_blank_legacy_time_as_unset() {
        let defaults = selector_fingerprints(&json!({}));
        let blank_legacy_time = selector_fingerprints(&json!({"as_of": "  "}));

        assert_eq!(defaults["as_of"], blank_legacy_time["as_of"]);
    }

    #[test]
    fn blank_legacy_time_does_not_conflict_with_valid_time_registration() {
        let blank_legacy_axis = std::collections::BTreeMap::from([
            ("as_of".to_owned(), json!("  ")),
            ("valid_at".to_owned(), json!("2026-05-01T10:00:00Z")),
        ]);
        validate_expected_selector_arguments(&blank_legacy_axis)
            .expect("whitespace-only legacy selector is unset");
    }

    #[test]
    fn selector_fingerprints_treat_current_view_as_default() {
        let defaults = selector_fingerprints(&json!({}));
        let current_view = selector_fingerprints(&json!({"view_mode": " CURRENT "}));

        assert_eq!(defaults["view_mode"], current_view["view_mode"]);
    }

    #[test]
    fn selector_window_timestamps_accept_the_server_minute_precision() {
        let minute_precision = selector_fingerprints(&json!({
            "window_start":"2026-05-01T11:00Z"
        }));
        let second_precision = selector_fingerprints(&json!({
            "window_start":"2026-05-01T11:00:00Z"
        }));

        assert_eq!(
            minute_precision["window_start"],
            second_precision["window_start"]
        );
    }

    #[test]
    fn registered_selector_rejects_negative_knowledge_revision() {
        let selectors =
            std::collections::BTreeMap::from([("known_revision".to_owned(), json!(-1))]);

        assert!(validate_expected_selector_arguments(&selectors).is_err());
    }

    #[test]
    fn response_guidance_is_fingerprinted_without_retaining_its_text() {
        let response = json!({
            "result":{"structuredContent":{
                "result":[],
                "guidance":"No sufficient matching context was found. Refine the query or verify primary sources."
            }}
        });

        let digest = response_guidance_sha256(&response).expect("guidance digest");

        assert_eq!(digest.len(), 64);
        assert!(!digest.contains("Refine"));
    }

    #[test]
    fn valid_at_fingerprints_normalize_offsets_and_accept_legacy_as_of() {
        let valid_at = json!({"valid_at":"2026-05-01T11:00:00+01:00"});
        let legacy_as_of = json!({"as_of":"2026-05-01T10:00:00Z"});

        assert_eq!(
            requested_valid_at_sha256(&valid_at),
            requested_valid_at_sha256(&legacy_as_of)
        );
        assert_eq!(
            requested_valid_at_sha256(&valid_at).as_deref(),
            Some("092c00268b464dfd95c1624f4019859171028b88271f5c69b9bb29536ec8b33f")
        );
    }

    #[test]
    fn invalid_or_absent_requested_valid_at_has_no_fingerprint() {
        assert_eq!(
            requested_valid_at_sha256(&json!({"valid_at":"not a timestamp"})),
            None
        );
        assert_eq!(requested_valid_at_sha256(&json!({"query":"no time"})), None);
    }

    #[test]
    fn read_evidence_fingerprints_snapshot_valid_at_without_retaining_timestamp() {
        let response = json!({
            "structuredContent": {
                "result": [{"source_episode":"episode:one"}],
                "context": {"snapshot": {"valid_at":"2026-05-01T10:00:00Z"}}
            }
        });
        let evidence = read_evidence(&response, None).expect("structured evidence");
        let serialized = serde_json::to_string(&evidence).expect("serialized evidence");

        assert_eq!(
            evidence.returned_valid_at_sha256.as_deref(),
            Some("092c00268b464dfd95c1624f4019859171028b88271f5c69b9bb29536ec8b33f")
        );
        assert!(!serialized.contains("2026-05-01"));
    }
}
