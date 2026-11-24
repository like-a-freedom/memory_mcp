use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub(crate) struct DecisionCorpus {
    pub(crate) events: Vec<MemoryEvent>,
    pub(crate) probes: Vec<DecisionProbe>,
    pub(crate) oracles: BTreeMap<String, DecisionOracle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemoryEvent {
    pub(crate) id: String,
    pub(crate) source_id: String,
    pub(crate) source_type: String,
    pub(crate) content: String,
    pub(crate) t_ref: DateTime<Utc>,
    #[serde(default)]
    pub(crate) correction_of: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecisionProbe {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) family_id: Option<String>,
    pub(crate) event_ids: Vec<String>,
    #[serde(default)]
    pub(crate) knowledge_before_event_id: Option<String>,
    pub(crate) query: String,
    pub(crate) valid_at: DateTime<Utc>,
    #[serde(default)]
    pub(crate) as_of: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) temporal_diagnostic_as_of: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) requires_separate_time_cutoffs: bool,
    pub(crate) budget: usize,
    #[serde(default = "one_read")]
    pub(crate) repeat_read: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExpectedDisposition {
    Answerable,
    Conflict,
    Correction,
    Unknown,
    UnsupportedTemporal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecisionOracle {
    pub(crate) probe_id: String,
    pub(crate) disposition: ExpectedDisposition,
    pub(crate) evidence_alternatives: Vec<Vec<Vec<String>>>,
    pub(crate) expected_assertions: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) expected_roles: BTreeMap<String, String>,
    pub(crate) forbidden_event_ids: Vec<String>,
    #[serde(default)]
    pub(crate) relation_outcome: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventDocument {
    schema_version: String,
    events: Vec<MemoryEvent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeDocument {
    schema_version: String,
    probes: Vec<DecisionProbe>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OracleDocument {
    schema_version: String,
    oracles: Vec<DecisionOracle>,
}

fn one_read() -> usize {
    1
}

impl DecisionCorpus {
    pub(crate) fn load(root: &std::path::Path) -> Result<Self, crate::EvalError> {
        let events_path = root.join("events.json");
        let probes_path = root.join("probes.json");
        let oracle_path = root.join("oracle.json");
        let events =
            std::fs::read_to_string(&events_path).map_err(|source| crate::EvalError::Io {
                path: events_path,
                source,
            })?;
        let probes =
            std::fs::read_to_string(&probes_path).map_err(|source| crate::EvalError::Io {
                path: probes_path,
                source,
            })?;
        let oracle =
            std::fs::read_to_string(&oracle_path).map_err(|source| crate::EvalError::Io {
                path: oracle_path,
                source,
            })?;
        Self::parse(&events, &probes, &oracle)
    }

    pub(crate) fn parse(
        events: &str,
        probes: &str,
        oracle: &str,
    ) -> Result<Self, crate::EvalError> {
        let events: EventDocument = serde_json::from_str(events)?;
        let probes: ProbeDocument = serde_json::from_str(probes)?;
        let oracle: OracleDocument = serde_json::from_str(oracle)?;
        if events.schema_version != "memory-decisions-events/v1"
            || probes.schema_version != "memory-decisions-probes/v1"
            || !matches!(
                oracle.schema_version.as_str(),
                "memory-decisions-oracle/v1" | "memory-decisions-oracle/v2"
            )
        {
            return Err(crate::EvalError::InvalidConfig(
                "unsupported decision corpus schema version".into(),
            ));
        }
        let role_expectations_require_snapshot =
            oracle.schema_version == "memory-decisions-oracle/v2";
        let corpus = Self {
            events: events.events,
            probes: probes.probes,
            oracles: oracle
                .oracles
                .into_iter()
                .map(|row| (row.probe_id.clone(), row))
                .collect(),
        };
        corpus.validate(role_expectations_require_snapshot)?;
        Ok(corpus)
    }

    fn validate(&self, role_expectations_require_snapshot: bool) -> Result<(), crate::EvalError> {
        let mut event_ids = BTreeSet::new();
        for event in &self.events {
            if event.id.trim().is_empty()
                || event.source_id.trim().is_empty()
                || event.content.trim().is_empty()
                || event.source_type.trim().is_empty()
                || !event_ids.insert(event.id.as_str())
            {
                return Err(crate::EvalError::InvalidInput(
                    "event fields must be nonempty and event IDs unique".into(),
                ));
            }
        }
        let event_index: BTreeMap<_, _> = self
            .events
            .iter()
            .enumerate()
            .map(|(i, event)| (event.id.as_str(), i))
            .collect();
        for event in &self.events {
            if let Some(target) = event.correction_of.as_deref() {
                let target_index = event_index.get(target).ok_or_else(|| {
                    crate::EvalError::InvalidInput(format!(
                        "event `{}` corrects unknown event `{target}`",
                        event.id
                    ))
                })?;
                if target == event.id || *target_index >= event_index[event.id.as_str()] {
                    return Err(crate::EvalError::InvalidInput(format!(
                        "event `{}` has an invalid correction target",
                        event.id
                    )));
                }
            }
        }
        let mut probe_ids = BTreeSet::new();
        for probe in &self.probes {
            if probe.id.trim().is_empty()
                || probe.query.trim().is_empty()
                || probe.budget == 0
                || probe.repeat_read == 0
                || probe
                    .family_id
                    .as_deref()
                    .is_some_and(|family_id| family_id.trim().is_empty())
                || !probe_ids.insert(probe.id.as_str())
            {
                return Err(crate::EvalError::InvalidInput(
                    "probe fields must be valid, family IDs nonempty when supplied, and probe IDs unique".into(),
                ));
            }
            let mut visible = BTreeSet::new();
            for id in &probe.event_ids {
                if !event_index.contains_key(id.as_str()) || !visible.insert(id.as_str()) {
                    return Err(crate::EvalError::InvalidInput(format!(
                        "probe `{}` has an unknown or duplicate event",
                        probe.id
                    )));
                }
            }
            if let Some(boundary) = &probe.knowledge_before_event_id
                && !visible.contains(boundary.as_str())
            {
                return Err(crate::EvalError::InvalidInput(format!(
                    "probe `{}` knowledge boundary is outside its history",
                    probe.id
                )));
            }
            if probe.requires_separate_time_cutoffs && probe.temporal_diagnostic_as_of.is_none() {
                return Err(crate::EvalError::InvalidInput(format!(
                    "probe `{}` requires separate valid/known cutoffs without a one-cutoff diagnostic",
                    probe.id
                )));
            }
        }
        if self.oracles.len() != self.probes.len() || self.probes.is_empty() {
            return Err(crate::EvalError::InvalidInput(
                "every probe must have exactly one oracle".into(),
            ));
        }
        for probe in &self.probes {
            let oracle = self.oracles.get(&probe.id).ok_or_else(|| {
                crate::EvalError::InvalidInput(format!("missing oracle for probe `{}`", probe.id))
            })?;
            let boundary = probe
                .knowledge_before_event_id
                .as_ref()
                .and_then(|id| probe.event_ids.iter().position(|candidate| candidate == id))
                .unwrap_or(probe.event_ids.len());
            let known: BTreeSet<_> = probe.event_ids[..boundary]
                .iter()
                .map(String::as_str)
                .collect();
            if oracle.evidence_alternatives.iter().any(|alternative| {
                alternative.is_empty()
                    || alternative.iter().any(|group| group.is_empty())
                    || alternative
                        .iter()
                        .flatten()
                        .any(|id| !known.contains(id.as_str()))
            }) {
                return Err(crate::EvalError::InvalidInput(format!(
                    "oracle for `{}` requires evidence beyond knowledge boundary or has empty groups",
                    probe.id
                )));
            }
            if oracle
                .forbidden_event_ids
                .iter()
                .any(|id| !event_index.contains_key(id.as_str()))
            {
                return Err(crate::EvalError::InvalidInput(format!(
                    "oracle for `{}` references unknown forbidden event",
                    probe.id
                )));
            }
            let required: BTreeSet<_> = oracle
                .evidence_alternatives
                .iter()
                .flatten()
                .flatten()
                .map(String::as_str)
                .collect();
            if oracle
                .forbidden_event_ids
                .iter()
                .any(|id| required.contains(id.as_str()))
            {
                return Err(crate::EvalError::InvalidInput(format!(
                    "oracle for `{}` both requires and forbids evidence",
                    probe.id
                )));
            }
            if oracle
                .expected_assertions
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != required
            {
                return Err(crate::EvalError::InvalidInput(format!(
                    "oracle for `{}` must define one assertion per required source",
                    probe.id
                )));
            }
            if oracle
                .expected_assertions
                .values()
                .any(|value| value.trim().is_empty())
            {
                return Err(crate::EvalError::InvalidInput(format!(
                    "oracle for `{}` has an empty expected assertion",
                    probe.id
                )));
            }
            if oracle.expected_roles.iter().any(|(id, role)| {
                !known.contains(id.as_str())
                    || !matches!(role.as_str(), "current" | "historical" | "conflicting")
            }) {
                return Err(crate::EvalError::InvalidInput(format!(
                    "oracle for `{}` has an invalid expected evidence role",
                    probe.id
                )));
            }
            if role_expectations_require_snapshot
                && !oracle.expected_roles.is_empty()
                && !probe.requires_separate_time_cutoffs
            {
                return Err(crate::EvalError::InvalidInput(format!(
                    "oracle v2 roles for `{}` require a probe with separate time cutoffs",
                    probe.id
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DecisionCorpus;

    #[test]
    fn corpus_rejects_oracle_evidence_beyond_knowledge_boundary() {
        let events = r#"{"schema_version":"memory-decisions-events/v1","events":[{"id":"a","source_id":"a","source_type":"eval","content":"A","t_ref":"2026-05-01T00:00:00Z"},{"id":"b","source_id":"b","source_type":"eval","content":"B","t_ref":"2026-05-09T00:00:00Z"}]}"#;
        let probes = r#"{"schema_version":"memory-decisions-probes/v1","probes":[{"id":"p","event_ids":["a","b"],"knowledge_before_event_id":"b","query":"question","valid_at":"2026-05-01T00:00:00Z","budget":5}]}"#;
        let oracle = r#"{"schema_version":"memory-decisions-oracle/v1","oracles":[{"probe_id":"p","disposition":"answerable","evidence_alternatives":[[["b"]]],"expected_assertions":{"b":"B"},"forbidden_event_ids":[] }]}"#;

        let result = DecisionCorpus::parse(events, probes, oracle);

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("knowledge boundary")
        );
    }

    fn valid_corpus(oracle_suffix: &str) -> String {
        format!(
            r#"{{"schema_version":"memory-decisions-oracle/v1","oracles":[{{"probe_id":"p","disposition":"answerable","evidence_alternatives":[[["a"]]],"expected_assertions":{{"a":"North City"}},"forbidden_event_ids":[] {oracle_suffix}}}]}}"#
        )
    }

    fn events(two: bool) -> String {
        let second = if two {
            r#",{"id":"b","source_id":"source-b","source_type":"eval","content":"Correction to a","t_ref":"2026-05-09T00:00:00Z","correction_of":"a"}"#
        } else {
            ""
        };
        format!(
            r#"{{"schema_version":"memory-decisions-events/v1","events":[{{"id":"a","source_id":"source-a","source_type":"eval","content":"North City","t_ref":"2026-05-01T00:00:00Z"}}{second}]}}"#
        )
    }

    fn probes(two: bool) -> String {
        let ids = if two { r#"["a","b"]"# } else { r#"["a"]"# };
        format!(
            r#"{{"schema_version":"memory-decisions-probes/v1","probes":[{{"id":"p","event_ids":{ids},"query":"Where?","valid_at":"2026-05-01T00:00:00Z","budget":5}}]}}"#
        )
    }

    #[test]
    fn rejects_duplicate_event_ids() {
        let duplicated = events(false).replace(
            "\"events\":[{",
            "\"events\":[{\"id\":\"a\",\"source_id\":\"source-a\",\"source_type\":\"eval\",\"content\":\"North City\",\"t_ref\":\"2026-05-01T00:00:00Z\"},{",
        );
        let parsed = DecisionCorpus::parse(&duplicated, &probes(false), &valid_corpus(""));
        assert!(parsed.unwrap_err().to_string().contains("unique"));
    }

    #[test]
    fn rejects_unknown_correction_target() {
        let raw = events(false).replace(
            "\"t_ref\":\"2026-05-01T00:00:00Z\"}",
            "\"t_ref\":\"2026-05-01T00:00:00Z\",\"correction_of\":\"missing\"}",
        );
        let parsed = DecisionCorpus::parse(&raw, &probes(false), &valid_corpus(""));
        assert!(parsed.unwrap_err().to_string().contains("unknown event"));
    }

    #[test]
    fn rejects_required_source_also_declared_forbidden() {
        let oracle = valid_corpus("").replace(
            "\"forbidden_event_ids\":[]",
            "\"forbidden_event_ids\":[\"a\"]",
        );
        let parsed = DecisionCorpus::parse(&events(false), &probes(false), &oracle);
        assert!(
            parsed
                .unwrap_err()
                .to_string()
                .contains("requires and forbids")
        );
    }

    #[test]
    fn accepts_a_valid_full_history_and_prefix_boundary() {
        let probe = probes(true).replace(
            r#""event_ids":["a","b"]"#,
            r#""event_ids":["a","b"],"knowledge_before_event_id":"b""#,
        );
        let oracle = r#"{"schema_version":"memory-decisions-oracle/v1","oracles":[{"probe_id":"p","disposition":"answerable","evidence_alternatives":[[["a"]]],"expected_assertions":{"a":"North City"},"forbidden_event_ids":[] }]}"#;
        let parsed = DecisionCorpus::parse(&events(true), &probe, oracle);
        assert!(parsed.is_ok());
    }

    #[test]
    fn oracle_v2_rejects_role_expectations_without_a_temporal_snapshot() {
        let oracle = r#"{"schema_version":"memory-decisions-oracle/v2","oracles":[{"probe_id":"p","disposition":"answerable","evidence_alternatives":[[["a"]]],"expected_assertions":{"a":"North City"},"expected_roles":{"a":"current"},"forbidden_event_ids":[] }]}"#;

        let error = DecisionCorpus::parse(&events(false), &probes(false), oracle)
            .expect_err("plain current retrieval must not be graded as a snapshot");

        assert!(error.to_string().contains("separate time cutoffs"));
    }

    #[test]
    fn oracle_v2_accepts_role_expectations_for_a_temporal_snapshot() {
        let probe = probes(false).replace(
            r#""budget":5"#,
            r#""budget":5,"requires_separate_time_cutoffs":true,"temporal_diagnostic_as_of":"2026-05-01T00:00:00Z""#,
        );
        let oracle = r#"{"schema_version":"memory-decisions-oracle/v2","oracles":[{"probe_id":"p","disposition":"answerable","evidence_alternatives":[[["a"]]],"expected_assertions":{"a":"North City"},"expected_roles":{"a":"current"},"forbidden_event_ids":[] }]}"#;

        let parsed = DecisionCorpus::parse(&events(false), &probe, oracle);

        assert!(parsed.is_ok());
    }

    #[test]
    fn rejects_a_blank_explicit_history_family_id() {
        let probe = probes(false).replace(r#""id":"p""#, r#""id":"p","family_id":" ""#);
        let parsed = DecisionCorpus::parse(&events(false), &probe, &valid_corpus(""));

        assert!(
            parsed
                .unwrap_err()
                .to_string()
                .contains("family IDs nonempty")
        );
    }
}
