use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::EvalError;

use super::scoring::{EvidenceRole, Observed, ObservedDisposition, SourceObservation};

pub(super) fn normalize_observation(
    responses: &[Value],
    bindings: &BTreeMap<String, String>,
) -> Result<Observed, EvalError> {
    let mut sources = Vec::new();
    let mut relation_outcomes = Vec::new();
    let mut guidance = None;

    for response in responses {
        let payload = tool_payload(response)?;
        if let Some(error) = payload.get("isError").and_then(Value::as_bool)
            && error
        {
            return Err(EvalError::InvalidInput(
                "MCP tool response reports isError=true".into(),
            ));
        }
        let body = payload
            .get("structuredContent")
            .or_else(|| payload.get("structured_content"))
            .ok_or_else(|| {
                EvalError::InvalidInput(
                    "MCP response lacks structuredContent; public output is ambiguous".into(),
                )
            })?;
        let result = body.get("result").ok_or_else(|| {
            EvalError::InvalidInput("assemble_context structured result is missing".into())
        })?;
        let items = result.as_array().ok_or_else(|| {
            EvalError::InvalidInput("assemble_context result is not an item array".into())
        })?;
        if let Some(value) = body.get("guidance").and_then(Value::as_str) {
            guidance = Some(value.to_string());
        }

        for item in items {
            if !item.is_object() {
                return Err(EvalError::InvalidInput(
                    "assemble_context item is not an object".into(),
                ));
            }
            let source_episode = item.get("source_episode").and_then(Value::as_str);
            let event_id = source_episode
                .and_then(|episode| bindings.get(episode))
                .cloned();
            let mut text = item
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(quote) = item.get("quote").and_then(Value::as_str)
                && !quote.is_empty()
            {
                text.push('\n');
                text.push_str(quote);
            }
            let (relation_role, outcomes) = normalize_relations(item.get("reconciliation"));
            let role = item
                .get("evidence_status")
                .and_then(Value::as_str)
                .map(normalize_evidence_status)
                .unwrap_or(relation_role);
            relation_outcomes.extend(outcomes);
            sources.push(SourceObservation {
                event_id,
                text,
                role,
            });
        }
    }

    relation_outcomes.sort();
    relation_outcomes.dedup();
    let disposition = if relation_outcomes
        .iter()
        .any(|outcome| outcome == "contradiction")
    {
        ObservedDisposition::Conflict
    } else if relation_outcomes
        .iter()
        .any(|outcome| matches!(outcome.as_str(), "correction" | "supersession"))
    {
        ObservedDisposition::Correction
    } else if sources.is_empty()
        && guidance.as_deref().is_some_and(|text| {
            text.to_lowercase()
                .contains("no sufficient matching context")
        })
    {
        ObservedDisposition::Unknown
    } else if !sources.is_empty() {
        ObservedDisposition::Supported
    } else {
        ObservedDisposition::Unobservable
    };
    let item_count = sources.len();

    Ok(Observed {
        sources,
        disposition,
        item_count,
        budget_used: item_count,
        relation_outcomes,
        guidance,
    })
}

fn normalize_evidence_status(status: &str) -> EvidenceRole {
    match status {
        "current" => EvidenceRole::Current,
        "historical" => EvidenceRole::Historical,
        "conflicting" => EvidenceRole::Conflicting,
        _ => EvidenceRole::Unknown,
    }
}

fn tool_payload(response: &Value) -> Result<&Value, EvalError> {
    if response.get("error").is_some() {
        return Err(EvalError::InvalidInput(
            "JSON-RPC response contains an error".into(),
        ));
    }
    response
        .get("result")
        .or_else(|| response.get("payload"))
        .ok_or_else(|| EvalError::InvalidInput("JSON-RPC response has no result".into()))
}

fn normalize_relations(reconciliation: Option<&Value>) -> (EvidenceRole, Vec<String>) {
    let Some(relations) = reconciliation
        .and_then(|value| value.get("relations"))
        .and_then(Value::as_array)
    else {
        return (EvidenceRole::Unknown, Vec::new());
    };
    let mut outcomes = Vec::new();
    let mut role = EvidenceRole::Unknown;
    for relation in relations {
        let Some(outcome) = relation.get("outcome").and_then(Value::as_str) else {
            continue;
        };
        outcomes.push(outcome.to_string());
        match outcome {
            "contradiction" => role = EvidenceRole::Conflicting,
            "correction" | "supersession" => {
                if relation
                    .get("superseded_by_fact_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty())
                {
                    role = EvidenceRole::Historical;
                } else if role != EvidenceRole::Conflicting {
                    role = EvidenceRole::Current;
                }
            }
            _ => {}
        }
    }
    (role, outcomes)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use super::normalize_observation;
    use crate::error::EvalError;
    use crate::suites::memory_decisions::scoring::{EvidenceRole, ObservedDisposition};

    fn bindings() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("episode:a".into(), "event-a".into()),
            ("episode:b".into(), "event-b".into()),
        ])
    }

    fn response(items: Value, guidance: &str) -> Value {
        json!({
            "jsonrpc":"2.0",
            "id":3,
            "result":{
                "structuredContent":{"status":"success","result":items,"guidance":guidance}
            }
        })
    }

    #[test]
    fn missing_source_citation_is_unbound_evidence() {
        let raw = response(json!([{"content":"North City"}]), "Call explain.");
        let actual = normalize_observation(&[raw], &bindings()).unwrap();

        assert_eq!(actual.sources[0].event_id, None);
    }

    #[test]
    fn citation_binds_only_through_the_observed_episode_id() {
        let raw = response(
            json!([{"source_episode":"episode:b","content":"North City"}]),
            "Call explain.",
        );
        let actual = normalize_observation(&[raw], &bindings()).unwrap();

        assert_eq!(actual.sources[0].event_id.as_deref(), Some("event-b"));
    }

    #[test]
    fn correction_relation_marks_superseded_fact_as_historical() {
        let raw = response(
            json!([{"source_episode":"episode:a","content":"North City","reconciliation":{"relations":[{"outcome":"correction","superseded_by_fact_id":"fact-b"}]}}]),
            "Call explain.",
        );
        let actual = normalize_observation(&[raw], &bindings()).unwrap();

        assert_eq!(actual.sources[0].role, EvidenceRole::Historical);
    }

    #[test]
    fn contradiction_relation_is_an_observable_conflict() {
        let raw = response(
            json!([{"source_episode":"episode:a","content":"North City","reconciliation":{"relations":[{"outcome":"contradiction"}]}}]),
            "Call explain.",
        );
        let actual = normalize_observation(&[raw], &bindings()).unwrap();

        assert_eq!(actual.disposition, ObservedDisposition::Conflict);
    }

    #[test]
    fn compact_response_without_relations_keeps_the_role_unknown() {
        let raw = response(
            json!([{"source_episode":"episode:a","content":"North City"}]),
            "Call explain.",
        );
        let actual = normalize_observation(&[raw], &bindings()).unwrap();

        assert_eq!(actual.sources[0].role, EvidenceRole::Unknown);
        assert_eq!(actual.disposition, ObservedDisposition::Supported);
    }

    #[test]
    fn snapshot_evidence_status_is_used_for_compact_temporal_items() {
        let raw = response(
            json!([{
                "source_episode":"episode:a",
                "content":"North City",
                "evidence_status":"current"
            }]),
            "Historical context is partial.",
        );
        let actual = normalize_observation(&[raw], &bindings()).unwrap();

        assert_eq!(actual.sources[0].role, EvidenceRole::Current);
    }

    #[test]
    fn explain_payload_does_not_replace_missing_retrieval_citations() {
        let assemble = response(
            json!([{"source_episode":"episode:a","content":"North City"}]),
            "Call explain.",
        );
        let explain = json!({
            "jsonrpc":"2.0","id":4,
            "result":{"structuredContent":{"result":[{"source_episode":"episode:b","content":"South City"}]}}
        });
        let actual = normalize_observation(&[assemble], &bindings()).unwrap();

        assert_eq!(actual.sources.len(), 1);
        assert_eq!(actual.sources[0].event_id.as_deref(), Some("event-a"));
        assert!(explain["result"]["structuredContent"]["result"].is_array());
    }

    #[test]
    fn malformed_structured_payload_is_not_scored_as_an_empty_answer() {
        let raw = response(json!({"not":"an array"}), "No sufficient matching context.");

        let actual = normalize_observation(&[raw], &bindings());

        assert!(matches!(actual, Err(EvalError::InvalidInput(_))));
    }

    #[test]
    fn cited_content_must_contain_the_returned_claim_text() {
        let raw = response(
            json!([{"source_episode":"episode:a","content":"North Depot is located in North City."}]),
            "Call explain.",
        );
        let actual = normalize_observation(&[raw], &bindings()).unwrap();

        assert!(actual.sources[0].text.contains("North City"));
        assert!(!actual.sources[0].text.contains("South City"));
    }
}
