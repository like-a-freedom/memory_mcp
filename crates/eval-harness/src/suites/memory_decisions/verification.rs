use super::receipts::{ReplayIndex, sha256};
use crate::EvalError;
use serde_json::Value;
use std::path::Path;

/// Load a portable replay together with its raw receipts and rederive the
/// decision channel. Historical index-only comparison remains available.
pub fn load_verified_replay(path: &Path) -> Result<ReplayIndex, EvalError> {
    let invalid = |s: &str| EvalError::InvalidInput(s.into());
    let read = |path: &Path| -> Result<Vec<u8>, EvalError> {
        std::fs::read(path).map_err(|source| EvalError::Io {
            path: path.into(),
            source,
        })
    };
    let index: ReplayIndex = serde_json::from_slice(&read(path)?)?;
    super::compare_replays(&index, &index)?;
    let (primary_stage, configuration) =
        super::registered_configuration(&index.identity.config_sha256)
            .ok_or_else(|| invalid("replay uses an unregistered memory-decision configuration"))?;
    let root = path
        .parent()
        .ok_or_else(|| invalid("replay has no receipt directory"))?;
    for case in &index.cases {
        let receipt_path = Path::new(&case.receipt_path);
        if receipt_path.components().count() != 1
            || !matches!(
                receipt_path.components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(invalid("receipt must be a sibling filename"));
        }
        let raw: Value = serde_json::from_slice(&read(&root.join(receipt_path))?)?;
        let receipt_schema = raw["schema_version"].as_str();
        // v1 receipt hashes cover canonical pretty JSON, excluding the final newline.
        if sha256(&serde_json::to_vec_pretty(&raw)?) != case.receipt_sha256
            || !matches!(
                receipt_schema,
                Some("memory-decisions-receipt/v3" | "memory-decisions-receipt/v4")
            )
            || raw["case_id"] != case.case_id
            || raw["product_revision"] != index.identity.product_revision
            || raw["product_binary_sha256"] != index.identity.product_binary_sha256
            || raw["corpus_sha256"] != index.identity.corpus_sha256
            || raw["config_sha256"] != index.identity.config_sha256
            || raw["provider_configuration"] != configuration
            || raw["primary_claim_rollout_stage"] != primary_stage
        {
            return Err(invalid("receipt identity or hash differs from replay"));
        }
        let schemas = &raw["supported_api"]["input_schema"];
        if index.identity.input_schema_sha256.as_deref()
            != Some(sha256(schemas.to_string().as_bytes()).as_str())
        {
            return Err(invalid("receipt public schema differs from replay"));
        }
        let trace = &raw["evidence_enabled_diagnostic"];
        if trace["claim_rollout_stage"] != "evidence" {
            return Err(invalid("missing evidence-stage raw receipt"));
        }
        if receipt_schema == Some("memory-decisions-receipt/v4") {
            if trace["launch_profile"]["MEMORY_CLAIM_ROLLOUT_STAGE"] != "evidence" {
                return Err(invalid("evidence-stage launch override is not explicit"));
            }
            let launch_profile = &raw["primary_launch_profile"];
            match primary_stage {
                "default" if launch_profile.get("MEMORY_CLAIM_ROLLOUT_STAGE").is_some() => {
                    return Err(invalid(
                        "product-default capture explicitly overrides its rollout stage",
                    ));
                }
                "shadow" if launch_profile["MEMORY_CLAIM_ROLLOUT_STAGE"] != "shadow" => {
                    return Err(invalid(
                        "shadow control does not launch with the explicit rollback stage",
                    ));
                }
                _ => {}
            }
        }
        let bindings = serde_json::from_value(trace["episode_bindings"].clone())?;
        let observed = super::normalization::normalize_observation(
            std::slice::from_ref(&trace["temporal_revision_response"]),
            &bindings,
        )?;
        let request = &trace["temporal_revision_request"];
        let snapshot = trace["temporal_revision_response"]
            .pointer("/result/structuredContent/context/snapshot");
        let parse_time = |v: &Value| {
            v.as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        };
        let request_time = parse_time(&request["valid_at"]);
        let valid = request_time.is_some()
            && snapshot.is_some_and(|s| {
                parse_time(&s["valid_at"]) == request_time
                    && s["known_revision"].as_i64().is_some()
                    && s["known_revision"] == request["known_revision"]
            });
        let oracle = serde_json::from_value(raw["desired_goal"]["oracle"].clone())?;
        let budget = request["budget"]
            .as_u64()
            .ok_or_else(|| invalid("missing raw probe budget"))?;
        let score = super::scoring::score_probe(
            &oracle,
            &observed,
            usize::try_from(budget).map_err(|_| invalid("probe budget overflow"))?,
            valid,
        );
        if case.evidence_enabled_temporal_observation.as_ref()
            != Some(&super::replay_observation(&observed))
            || case.evidence_enabled_temporal_score.as_ref() != Some(&super::replay_score(&score))
            || raw["evidence_enabled_temporal_revision_score"] != super::replay_score(&score)
        {
            return Err(invalid(
                "decision observation or score is detached from raw receipt",
            ));
        }
        if receipt_schema == Some("memory-decisions-receipt/v4") {
            verify_primary_score(&raw, case)?;
        }
    }
    Ok(index)
}

pub fn load_registered_replay(path: &Path, corpus_root: &Path) -> Result<ReplayIndex, EvalError> {
    use std::collections::{BTreeMap, BTreeSet};
    let invalid = |s: &str| EvalError::InvalidInput(s.into());
    let index = load_verified_replay(path)?;
    let corpus = super::cases::DecisionCorpus::load(corpus_root)?;
    let Some((registered_stage, configuration)) =
        super::registered_configuration(&index.identity.config_sha256)
    else {
        return Err(invalid(
            "replay differs from registered corpus or configuration",
        ));
    };
    if index.identity.corpus_sha256 != super::receipts::hash_corpus(corpus_root)?
        || index
            .cases
            .iter()
            .map(|c| c.case_id.as_str())
            .collect::<BTreeSet<_>>()
            != corpus
                .probes
                .iter()
                .map(|p| p.id.as_str())
                .collect::<BTreeSet<_>>()
    {
        return Err(invalid(
            "replay differs from registered corpus or configuration",
        ));
    }
    let root = path
        .parent()
        .ok_or_else(|| invalid("missing receipt root"))?;
    for case in &index.cases {
        let raw: Value = serde_json::from_slice(
            &std::fs::read(root.join(&case.receipt_path)).map_err(|source| EvalError::Io {
                path: root.join(&case.receipt_path),
                source,
            })?,
        )?;
        let probe = corpus
            .probes
            .iter()
            .find(|p| p.id == case.case_id)
            .ok_or_else(|| invalid("unregistered probe"))?;
        let oracle = corpus
            .oracles
            .get(&case.case_id)
            .ok_or_else(|| invalid("unregistered oracle"))?;
        let events = probe
            .event_ids
            .iter()
            .map(|id| {
                corpus
                    .events
                    .iter()
                    .find(|e| e.id == *id)
                    .ok_or_else(|| invalid("unregistered event"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if raw["provider_configuration"] != configuration
            || raw["events"] != serde_json::to_value(&events)?
            || raw["desired_goal"]["oracle"] != serde_json::to_value(oracle)?
            || raw["desired_goal"]["valid_at"] != serde_json::to_value(probe.valid_at)?
            || raw["desired_goal"]["knowledge_before_event_id"]
                != serde_json::to_value(&probe.knowledge_before_event_id)?
        {
            return Err(invalid(
                "raw history, goal or oracle differs from registration",
            ));
        }
        if configuration.contains("scoring_protocol=temporal-cutoff-v2") {
            let expected_source = if probe.requires_separate_time_cutoffs {
                "temporal_revision"
            } else {
                "primary"
            };
            if raw["schema_version"] != "memory-decisions-receipt/v4"
                || raw["primary_claim_rollout_stage"] != registered_stage
                || raw["score_observation_source"] != expected_source
                || case.score_observation_source.as_deref() != Some(expected_source)
            {
                return Err(invalid(
                    "score source differs from registered temporal request semantics",
                ));
            }
            let primary_launch_profile = &raw["primary_launch_profile"];
            match registered_stage {
                "default"
                    if primary_launch_profile
                        .get("MEMORY_CLAIM_ROLLOUT_STAGE")
                        .is_some() =>
                {
                    return Err(invalid(
                        "default replay contains an explicit rollout override",
                    ));
                }
                "shadow" if primary_launch_profile["MEMORY_CLAIM_ROLLOUT_STAGE"] != "shadow" => {
                    return Err(invalid("shadow replay is missing its rollback override"));
                }
                _ => {}
            }
        }
        let trace = &raw["evidence_enabled_diagnostic"];
        let rows = |key: &str| {
            trace[key]
                .as_array()
                .filter(|v| v.len() == events.len())
                .ok_or_else(|| invalid("incomplete seed/checkpoint receipts"))
        };
        let ingests = rows("ingest_receipts")?;
        let extracts = rows("extract_receipts")?;
        let checkpoints = rows("revision_checkpoint_receipts")?;
        let mut bindings = BTreeMap::new();
        let mut revisions = BTreeMap::new();
        let mut previous = 0;
        for (i, event) in events.iter().enumerate() {
            let ingest = &ingests[i];
            let extract = &extracts[i];
            let checkpoint = &checkpoints[i];
            let episode = super::structured_result(&ingest["response"])
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid("unbound seed receipt"))?;
            if ingest["event_id"] != event.id
                || ingest["episode_id"] != episode
                || ingest["request"] != super::ingest_arguments(event)
                || extract["event_id"] != event.id
                || extract["request"] != serde_json::json!({"episode_id":episode})
                || checkpoint["after_event_id"] != event.id
                || checkpoint["request"]
                    != serde_json::json!({"query":"","valid_at":event.t_ref.to_rfc3339(),"budget":1,"compact":true})
            {
                return Err(invalid("seed order or inputs differ from registration"));
            }
            let snapshot = checkpoint["response"]
                .pointer("/result/structuredContent/context/snapshot")
                .ok_or_else(|| invalid("missing checkpoint snapshot"))?;
            let revision = snapshot["known_revision"]
                .as_i64()
                .filter(|r| *r > previous)
                .ok_or_else(|| invalid("nonmonotone checkpoint"))?;
            let valid = snapshot["valid_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            if checkpoint["known_revision"] != revision
                || valid.is_none_or(|v| v.with_timezone(&chrono::Utc) != event.t_ref)
                || bindings
                    .insert(episode.to_string(), event.id.clone())
                    .is_some()
            {
                return Err(invalid("checkpoint or seed identity mismatch"));
            }
            previous = revision;
            revisions.insert(event.id.clone(), revision);
        }
        if trace["episode_bindings"] != serde_json::to_value(bindings)? {
            return Err(invalid("event bindings differ from public seed receipts"));
        }
        let revision = super::temporal_revision_for_probe(probe, &revisions)
            .map_err(|e| invalid(&e.message))?;
        if trace["temporal_revision_request"]
            != serde_json::json!({"query":probe.query,"valid_at":probe.valid_at.to_rfc3339(),"known_revision":revision,"budget":probe.budget,"compact":true})
        {
            return Err(invalid(
                "actual probe differs from registered question, budget or checkpoint",
            ));
        }
        if configuration.contains("scoring_protocol=temporal-cutoff-v2") {
            let expected_current_request = super::assemble_arguments(probe, false, probe.as_of);
            if raw["temporal_revision_request"]
                != serde_json::json!({"query":probe.query,"valid_at":probe.valid_at.to_rfc3339(),"known_revision":revision,"budget":probe.budget,"compact":true})
                || (!probe.requires_separate_time_cutoffs
                    && raw["primary_request"] != expected_current_request)
            {
                return Err(invalid(
                    "scored request differs from the registered case semantics",
                ));
            }
        }
    }
    Ok(index)
}

fn verify_primary_score(raw: &Value, case: &super::receipts::ReplayCase) -> Result<(), EvalError> {
    let invalid = |s: &str| EvalError::InvalidInput(s.into());
    let requires_separate_cutoffs = raw["desired_goal"]["requires_separate_time_cutoffs"]
        .as_bool()
        .ok_or_else(|| invalid("missing temporal requirement for primary score"))?;
    let temporal_response = &raw["temporal_revision_response"];
    let expected_source = if requires_separate_cutoffs && temporal_response.is_object() {
        "temporal_revision"
    } else {
        "primary"
    };
    if raw["score_observation_source"] != expected_source
        || case.score_observation_source.as_deref() != Some(expected_source)
    {
        return Err(invalid("primary score source is detached from its request"));
    }

    let bindings = serde_json::from_value(raw["episode_bindings"].clone())?;
    let (observation, request, time_contract_supported) = match expected_source {
        "temporal_revision" => {
            let observed = super::normalization::normalize_observation(
                std::slice::from_ref(temporal_response),
                &bindings,
            )?;
            let request = &raw["temporal_revision_request"];
            let snapshot = temporal_response.pointer("/result/structuredContent/context/snapshot");
            let parse_time = |value: &Value| {
                value
                    .as_str()
                    .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
            };
            let request_time = parse_time(&request["valid_at"]);
            let snapshot_valid = request_time.is_some()
                && snapshot.is_some_and(|value| {
                    parse_time(&value["valid_at"]) == request_time
                        && value["known_revision"].as_i64().is_some()
                        && value["known_revision"] == request["known_revision"]
                });
            if raw["temporal_revision_snapshot_valid"] != snapshot_valid {
                return Err(invalid(
                    "temporal snapshot validity differs from raw response",
                ));
            }
            (observed, request, snapshot_valid)
        }
        _ => {
            let responses = raw["primary_compact_responses"]
                .as_array()
                .ok_or_else(|| invalid("missing raw primary responses"))?;
            let first_response = responses
                .first()
                .ok_or_else(|| invalid("missing raw primary response"))?;
            let observed = super::normalization::normalize_observation(
                std::slice::from_ref(first_response),
                &bindings,
            )?;
            (
                observed,
                &raw["primary_request"],
                !requires_separate_cutoffs,
            )
        }
    };
    let oracle = serde_json::from_value(raw["desired_goal"]["oracle"].clone())?;
    let budget = request["budget"]
        .as_u64()
        .ok_or_else(|| invalid("missing raw scoring budget"))?;
    let score = super::scoring::score_probe(
        &oracle,
        &observation,
        usize::try_from(budget).map_err(|_| invalid("scoring budget overflow"))?,
        time_contract_supported,
    );
    if raw["score"] != super::replay_score(&score)
        || case.evidence_success != Some(score.evidence_success)
        || case.decision_success != Some(score.decision_success)
        || case.probe_success != Some(score.probe_success)
        || case
            .score_observation
            .as_ref()
            .is_some_and(|saved| saved != &super::replay_observation(&observation))
        || case.source_order
            != observation
                .sources
                .iter()
                .map(|source| {
                    source
                        .event_id
                        .clone()
                        .unwrap_or_else(|| "<unbound>".into())
                })
                .collect::<Vec<_>>()
        || case.roles
            != observation
                .sources
                .iter()
                .map(|source| source.role.as_str().to_string())
                .collect::<Vec<_>>()
        || case.disposition != format!("{:?}", observation.disposition).to_lowercase()
        || case.reason_codes
            != score
                .reason_codes
                .iter()
                .map(|reason| reason.as_str().to_string())
                .collect::<Vec<_>>()
    {
        return Err(invalid("primary score is detached from raw observation"));
    }
    Ok(())
}
