//! Independent fixed-query evidence scoring. Agent-selected reads are not inputs.
use crate::EvalError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeCapture {
    pub schema_version: String,
    pub launch_profile: std::collections::BTreeMap<String, String>,
    pub episode_id_to_event_id: std::collections::BTreeMap<String, String>,
    pub transport: String,
    pub extractor: String,
    pub embeddings_enabled: bool,
    pub observation: ProbeObservation,
    pub request: serde_json::Value,
    pub response: serde_json::Value,
    pub tool_schema_sha256: String,
    pub claim_rollout_stage: String,
    #[serde(default, skip_serializing_if = "ProbeApiProfile::is_revision_bound")]
    pub api_profile: ProbeApiProfile,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeApiProfile {
    #[default]
    RevisionBoundV1,
    LegacyAsOfV0,
}

impl ProbeApiProfile {
    const fn is_revision_bound(&self) -> bool {
        matches!(self, Self::RevisionBoundV1)
    }
}

impl ProbeCapture {
    /// Validate a recorded public read and re-derive source bindings before offline scoring.
    pub fn validate(
        &self,
        input: &ProbeInput,
        history: &[crate::paired_agent::SeedEpisode],
        product: &str,
        transport: &str,
        schema: &str,
    ) -> Result<(), EvalError> {
        let invalid = || EvalError::InvalidInput("independent capture provenance mismatch".into());
        input.validate()?;
        if !matches!(
            self.schema_version.as_str(),
            "memory-mcp-independent-probe-capture/v1" | "memory-mcp-independent-probe-capture/v2"
        ) || self.transport != transport
            || self.tool_schema_sha256 != schema
            || input.history_sha256 != hex::encode(Sha256::digest(serde_json::to_vec(history)?))
            || self.claim_rollout_stage != input.claim_rollout_stage
            || self.launch_profile.get("MEMORY_CLAIM_ROLLOUT_STAGE")
                != Some(&input.claim_rollout_stage)
            || self.launch_profile.get("NER_EXTRACTOR") != Some(&self.extractor)
            || self.launch_profile.get("EMBEDDINGS_ENABLED")
                != Some(&self.embeddings_enabled.to_string())
            || self.request != probe_request_for_profile(input, self.api_profile)?
        {
            return Err(invalid());
        }
        let valid_ids = history
            .iter()
            .map(|e| e.event_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let bound_ids = self
            .episode_id_to_event_id
            .values()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        if input.principal != "fixture-reader"
            || valid_ids.len() != history.len()
            || self.episode_id_to_event_id.len() != history.len()
            || bound_ids != valid_ids
            || self
                .episode_id_to_event_id
                .keys()
                .any(|id| !id.starts_with("episode:"))
        {
            return Err(invalid());
        }
        let mut derived = finish_capture(
            input,
            self.request.clone(),
            self.response.clone(),
            crate::paired_agent::SeedMemoryOutput {
                episode_id_to_event_id: self.episode_id_to_event_id.clone(),
                tool_schemas: serde_json::Value::Null,
                known_revision: None,
            },
            CaptureProfile {
                product_sha256: product,
                claim_rollout_stage: &self.claim_rollout_stage,
                transport,
                launch_profile: self.launch_profile.clone(),
            },
            self.api_profile,
        )?;
        // Capture v1 predates typed claim annotations. Preserve its historical
        // source-coverage interpretation while accepting the same raw response.
        if self.schema_version == "memory-mcp-independent-probe-capture/v1" {
            derived.observation.claim_annotations.clear();
        }
        if serde_json::to_value(&derived.observation)? != serde_json::to_value(&self.observation)? {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Capture a fixed public read in a newly created store, without an agent runner.
pub fn capture_stdio_probe(
    input: &ProbeInput,
    history: &[crate::paired_agent::SeedEpisode],
    binary: &std::path::Path,
    product_sha256: &str,
    store: &std::path::Path,
    claim_rollout_stage: &str,
) -> Result<ProbeCapture, EvalError> {
    prepare_capture(
        input,
        history,
        binary,
        product_sha256,
        store,
        claim_rollout_stage,
    )?;
    let mut session = crate::suites::memory_decisions::stdio::StdioSession::spawn_with_claim_stage(
        binary,
        store,
        claim_rollout_stage,
    )?;
    let seeded = crate::memory_fixture::seed_memory_in_session(&mut session, history, None)?;
    let read = read_public_probe(
        &mut session,
        input.query.as_str(),
        input.valid_at,
        input.context_budget,
        Some(input.known_revision),
    )?;
    session.shutdown()?;
    finish_capture(
        input,
        read.request,
        read.response,
        seeded,
        CaptureProfile {
            product_sha256,
            claim_rollout_stage,
            transport: "stdio",
            launch_profile: session.launch_profile.clone(),
        },
        read.api_profile,
    )
}

/// Capture through public HTTP MCP in a new isolated tenant, without an agent.
pub async fn capture_http_probe(
    input: &ProbeInput,
    history: &[crate::paired_agent::SeedEpisode],
    binary: &std::path::Path,
    product_sha256: &str,
    store: &std::path::Path,
    claim_rollout_stage: &str,
) -> Result<ProbeCapture, EvalError> {
    prepare_capture(
        input,
        history,
        binary,
        product_sha256,
        store,
        claim_rollout_stage,
    )?;
    let mut server = crate::paired_http::IsolatedHttpMemoryServer::spawn_deterministic(
        binary,
        store,
        claim_rollout_stage,
    )?;
    server.wait_ready().await?;
    let (episode_id_to_event_id, tool_schemas) = server.seed(history).await?;
    let request = probe_request(input);
    let response = server
        .call_tool("assemble_context", request.clone())
        .await?;
    let seeded = crate::paired_agent::SeedMemoryOutput {
        episode_id_to_event_id,
        tool_schemas,
        known_revision: None,
    };
    finish_capture(
        input,
        request,
        response,
        seeded,
        CaptureProfile {
            product_sha256,
            claim_rollout_stage,
            transport: "streamable_http",
            launch_profile: server.launch_profile.clone(),
        },
        ProbeApiProfile::RevisionBoundV1,
    )
}

fn prepare_capture(
    input: &ProbeInput,
    history: &[crate::paired_agent::SeedEpisode],
    binary: &std::path::Path,
    product_sha256: &str,
    store: &std::path::Path,
    claim_rollout_stage: &str,
) -> Result<(), EvalError> {
    input.validate()?;
    let mut event_ids = std::collections::BTreeSet::new();
    if history.iter().any(|episode| {
        episode.event_id.trim().is_empty() || !event_ids.insert(episode.event_id.as_str())
    }) {
        return Err(EvalError::InvalidInput(
            "independent history requires unique nonempty event IDs".into(),
        ));
    }
    if input.claim_rollout_stage != claim_rollout_stage {
        return Err(EvalError::InvalidInput(
            "independent capture stage differs from its fixed input".into(),
        ));
    }
    if input.principal != "fixture-reader" {
        return Err(EvalError::InvalidInput(
            "local probe supports only the isolated fixture-reader principal".into(),
        ));
    }
    let read = |path: &std::path::Path| {
        std::fs::read(path).map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })
    };
    if hex::encode(Sha256::digest(read(binary)?)) != product_sha256
        || hex::encode(Sha256::digest(serde_json::to_vec(history)?)) != input.history_sha256
    {
        return Err(EvalError::InvalidInput(
            "independent capture product or history fingerprint mismatch".into(),
        ));
    }
    std::fs::create_dir(store).map_err(|source| EvalError::Io {
        path: store.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn probe_request(input: &ProbeInput) -> serde_json::Value {
    serde_json::json!({
        "query": input.query, "valid_at": input.valid_at.to_rfc3339(),
        "known_revision": input.known_revision, "budget": input.context_budget, "compact": true,
    })
}

#[derive(Debug)]
pub(crate) struct PublicProbeRead {
    pub(crate) api_profile: ProbeApiProfile,
    pub(crate) request: serde_json::Value,
    pub(crate) response: serde_json::Value,
    pub(crate) source_episode_ids: Vec<String>,
    pub(crate) known_revision: Option<i64>,
}

/// Read a fixed query using only the snapshot fields advertised by the product.
/// The legacy profile is source-coverage-only and is valid only at revision zero;
/// temporal acceptance continues to require the revision-bound API.
pub(crate) fn read_public_probe(
    session: &mut crate::suites::memory_decisions::stdio::StdioSession,
    query: &str,
    valid_at: chrono::DateTime<chrono::Utc>,
    context_budget: usize,
    requested_revision: Option<i64>,
) -> Result<PublicProbeRead, EvalError> {
    let api_profile = probe_api_profile(session.tool_schemas())?;
    let request = match api_profile {
        ProbeApiProfile::RevisionBoundV1 => {
            let mut request = serde_json::json!({
                "query": query,
                "valid_at": valid_at.to_rfc3339(),
                "budget": context_budget,
                "compact": true,
            });
            if let Some(revision) = requested_revision {
                request["known_revision"] = serde_json::json!(revision);
            }
            request
        }
        ProbeApiProfile::LegacyAsOfV0 => {
            if requested_revision.is_some_and(|revision| revision != 0) {
                return Err(EvalError::InvalidInput(
                    "legacy as_of API cannot satisfy a revision-bound probe".into(),
                ));
            }
            serde_json::json!({
                "query": query,
                "as_of": valid_at.to_rfc3339(),
                "budget": context_budget,
                "compact": true,
            })
        }
    };
    let response = session.call_tool("assemble_context", request.clone())?;
    let snapshot = response.pointer("/result/structuredContent/context/snapshot");
    let known_revision = match api_profile {
        ProbeApiProfile::RevisionBoundV1 => {
            if snapshot
                .and_then(|value| value.get("valid_at"))
                .and_then(serde_json::Value::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .is_none_or(|observed| observed.with_timezone(&chrono::Utc) != valid_at)
            {
                return Err(EvalError::InvalidInput(
                    "independent probe response does not match its requested valid time".into(),
                ));
            }
            let revision = snapshot
                .and_then(|value| value.get("known_revision"))
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| {
                    EvalError::InvalidInput(
                        "revision-bound probe response lacks a known revision snapshot".into(),
                    )
                })?;
            if requested_revision.is_some_and(|requested| requested != revision) {
                return Err(EvalError::InvalidInput(
                    "independent probe response does not match its requested revision".into(),
                ));
            }
            Some(revision)
        }
        ProbeApiProfile::LegacyAsOfV0 => {
            if snapshot
                .and_then(|value| value.get("known_revision"))
                .and_then(serde_json::Value::as_i64)
                .is_some()
            {
                return Err(EvalError::InvalidInput(
                    "legacy as_of response unexpectedly claims revision-bound semantics".into(),
                ));
            }
            None
        }
    };
    let items = response
        .pointer("/result/structuredContent/result")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            EvalError::InvalidInput("independent probe lacks a public item array".into())
        })?;
    let source_episode_ids = items
        .iter()
        .map(|item| {
            item.get("source_episode")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    EvalError::InvalidInput("independent probe has an unbound source".into())
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PublicProbeRead {
        api_profile,
        request,
        response,
        source_episode_ids,
        known_revision,
    })
}

fn probe_api_profile(tool_schemas: &serde_json::Value) -> Result<ProbeApiProfile, EvalError> {
    let properties = tool_schemas
        .as_array()
        .and_then(|tools| {
            tools.iter().find(|tool| {
                tool.get("name").and_then(serde_json::Value::as_str) == Some("assemble_context")
            })
        })
        .and_then(|tool| tool.pointer("/inputSchema/properties"))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            EvalError::InvalidInput("assemble_context schema lacks input properties".into())
        })?;
    let revision_bound =
        properties.contains_key("valid_at") && properties.contains_key("known_revision");
    if revision_bound {
        return Ok(ProbeApiProfile::RevisionBoundV1);
    }
    let legacy_as_of = properties.contains_key("as_of")
        && !properties.contains_key("valid_at")
        && !properties.contains_key("known_revision");
    if legacy_as_of {
        return Ok(ProbeApiProfile::LegacyAsOfV0);
    }
    Err(EvalError::InvalidInput(
        "assemble_context schema does not match a supported probe API profile".into(),
    ))
}

fn probe_request_for_profile(
    input: &ProbeInput,
    profile: ProbeApiProfile,
) -> Result<serde_json::Value, EvalError> {
    match profile {
        ProbeApiProfile::RevisionBoundV1 => Ok(probe_request(input)),
        ProbeApiProfile::LegacyAsOfV0 if input.known_revision == 0 => Ok(serde_json::json!({
            "query": input.query,
            "as_of": input.valid_at.to_rfc3339(),
            "budget": input.context_budget,
            "compact": true,
        })),
        ProbeApiProfile::LegacyAsOfV0 => Err(EvalError::InvalidInput(
            "legacy as_of capture cannot claim a nonzero known revision".into(),
        )),
    }
}

struct CaptureProfile<'a> {
    product_sha256: &'a str,
    claim_rollout_stage: &'a str,
    transport: &'a str,
    launch_profile: std::collections::BTreeMap<String, String>,
}

fn finish_capture(
    input: &ProbeInput,
    request: serde_json::Value,
    response: serde_json::Value,
    seeded: crate::paired_agent::SeedMemoryOutput,
    profile: CaptureProfile<'_>,
    api_profile: ProbeApiProfile,
) -> Result<ProbeCapture, EvalError> {
    let CaptureProfile {
        product_sha256,
        claim_rollout_stage,
        transport,
        launch_profile,
    } = profile;
    if response.get("error").is_some()
        || response
            .pointer("/result/isError")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    {
        return Err(EvalError::InvalidInput(
            "independent probe response reports a tool or protocol error".into(),
        ));
    }
    if request != probe_request_for_profile(input, api_profile)? {
        return Err(EvalError::InvalidInput(
            "independent probe request does not match its registered API profile".into(),
        ));
    }
    let snapshot = response.pointer("/result/structuredContent/context/snapshot");
    match api_profile {
        ProbeApiProfile::RevisionBoundV1 => {
            if snapshot
                .and_then(|value| value.get("known_revision"))
                .and_then(serde_json::Value::as_i64)
                != Some(input.known_revision)
                || !snapshot
                    .and_then(|value| value.get("valid_at"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .is_some_and(|value| value.with_timezone(&chrono::Utc) == input.valid_at)
            {
                return Err(EvalError::InvalidInput(
                    "independent probe response does not match its requested snapshot".into(),
                ));
            }
        }
        ProbeApiProfile::LegacyAsOfV0 => {
            if input.known_revision != 0
                || snapshot
                    .and_then(|value| value.get("known_revision"))
                    .and_then(serde_json::Value::as_i64)
                    .is_some()
            {
                return Err(EvalError::InvalidInput(
                    "legacy as_of capture cannot claim revision-bound semantics".into(),
                ));
            }
        }
    }
    let items = response
        .pointer("/result/structuredContent/result")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            EvalError::InvalidInput("independent probe lacks a public item array".into())
        })?;
    let mut source_event_ids = Vec::with_capacity(items.len());
    let mut claim_annotations = Vec::new();
    for item in items {
        let source_episode = item
            .get("source_episode")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                EvalError::InvalidInput("independent probe has an unbound source".into())
            })?;
        let source_event_id = seeded
            .episode_id_to_event_id
            .get(source_episode)
            .cloned()
            .ok_or_else(|| {
                EvalError::InvalidInput("independent probe has an unbound source".into())
            })?;
        source_event_ids.push(source_event_id.clone());
        if let Some(annotation) = claim_annotation_from_item(item, source_event_id)? {
            claim_annotations.push(annotation);
        }
    }
    let observation = ProbeObservation {
        input_sha256: input.sha256()?,
        product_sha256: product_sha256.into(),
        source_event_ids,
        claim_annotations,
        budget_used: items.len(),
        completed: true,
    };
    Ok(ProbeCapture {
        schema_version: "memory-mcp-independent-probe-capture/v2".into(),
        transport: transport.into(),
        extractor: launch_profile
            .get("NER_EXTRACTOR")
            .cloned()
            .ok_or_else(|| {
                EvalError::InvalidInput("capture did not explicitly select an extractor".into())
            })?,
        embeddings_enabled: launch_profile
            .get("EMBEDDINGS_ENABLED")
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| {
                EvalError::InvalidInput(
                    "capture did not explicitly select embedding configuration".into(),
                )
            })?,
        launch_profile,
        episode_id_to_event_id: seeded.episode_id_to_event_id,
        observation,
        request,
        response,
        tool_schema_sha256: hex::encode(Sha256::digest(serde_json::to_vec(&seeded.tool_schemas)?)),
        claim_rollout_stage: claim_rollout_stage.into(),
        api_profile,
    })
}

pub(crate) fn claim_annotation_from_item(
    item: &serde_json::Value,
    source_event_id: String,
) -> Result<Option<ProbeClaimAnnotation>, EvalError> {
    let evidence_status = item
        .get("evidence_status")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let relations = item
        .pointer("/reconciliation/relations")
        .and_then(serde_json::Value::as_array)
        .map(|relations| {
            relations
                .iter()
                .map(|relation| {
                    let outcome = relation
                        .get("outcome")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            EvalError::InvalidInput("probe relation lacks an outcome".into())
                        })?;
                    let role = relation
                        .get("role")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            EvalError::InvalidInput("probe relation lacks a typed role".into())
                        })?;
                    Ok(ProbeRelationAnnotation {
                        outcome: outcome.to_owned(),
                        role: role.to_owned(),
                    })
                })
                .collect::<Result<Vec<_>, EvalError>>()
        })
        .transpose()?
        .unwrap_or_default();
    if evidence_status.is_none() && relations.is_empty() {
        return Ok(None);
    }
    let annotation = ProbeClaimAnnotation {
        source_event_id,
        evidence_status,
        relations,
    };
    validate_claim_annotation(&annotation)?;
    Ok(Some(annotation))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeInput {
    pub schema_version: String,
    pub query: String,
    pub principal: String,
    pub valid_at: chrono::DateTime<chrono::Utc>,
    pub known_revision: i64,
    pub context_budget: usize,
    pub history_sha256: String,
    pub claim_rollout_stage: String,
}
impl ProbeInput {
    pub(crate) fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version != "memory-mcp-independent-probe/v1" {
            return Err(EvalError::InvalidInput(
                "unsupported independent probe schema".into(),
            ));
        }
        if self.known_revision < 0 {
            return Err(EvalError::InvalidInput(
                "independent probe knowledge revision must be nonnegative".into(),
            ));
        }
        Ok(())
    }
    pub fn sha256(&self) -> Result<String, EvalError> {
        Ok(Sha256::digest(serde_json::to_vec(self)?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeOracle {
    pub sufficient_evidence_alternatives: Vec<Vec<Vec<String>>>,
    pub forbidden_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_claim_annotations: Vec<ProbeClaimAnnotation>,
    /// OR across complete requirement sets; every annotation within a set is required.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_claim_annotation_alternatives: Vec<Vec<ProbeClaimAnnotation>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeObservation {
    pub input_sha256: String,
    pub product_sha256: String,
    pub source_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claim_annotations: Vec<ProbeClaimAnnotation>,
    pub budget_used: usize,
    pub completed: bool,
}

/// Stable, content-free decision metadata exposed with a retrieved fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeClaimAnnotation {
    pub source_event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<ProbeRelationAnnotation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeRelationAnnotation {
    pub outcome: String,
    pub role: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeScore {
    pub schema_version: String,
    pub input_sha256: String,
    pub oracle_sha256: String,
    pub observation_sha256: String,
    pub product_sha256: String,
    pub evidence_success: bool,
}
pub fn score_probe(
    input: &ProbeInput,
    oracle: &ProbeOracle,
    observation: Option<&ProbeObservation>,
    expected_product_sha256: &str,
) -> Result<ProbeScore, EvalError> {
    if expected_product_sha256.len() != 64
        || !expected_product_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EvalError::InvalidInput(
            "independent probe requires a lowercase SHA-256 product fingerprint".into(),
        ));
    }
    input.validate()?;
    if oracle
        .sufficient_evidence_alternatives
        .iter()
        .any(|alternative| {
            alternative.is_empty()
                || alternative.iter().any(|group| {
                    group.is_empty()
                        || group
                            .iter()
                            .all(|source| oracle.forbidden_event_ids.contains(source))
                })
        })
    {
        return Err(EvalError::InvalidInput(
            "independent probe oracle has an impossible evidence set".into(),
        ));
    }
    let observation = observation.ok_or_else(|| {
        EvalError::InvalidInput("independent probe observation is missing".into())
    })?;
    if observation.source_event_ids.len() > observation.budget_used {
        return Err(EvalError::InvalidInput(
            "independent probe has more source bindings than returned items".into(),
        ));
    }
    if observation.claim_annotations.iter().any(|annotation| {
        validate_claim_annotation(annotation).is_err()
            || !observation
                .source_event_ids
                .contains(&annotation.source_event_id)
    }) {
        return Err(EvalError::InvalidInput(
            "independent probe contains an invalid or unreturned claim annotation".into(),
        ));
    }
    if !oracle.required_claim_annotations.is_empty()
        && !oracle.required_claim_annotation_alternatives.is_empty()
    {
        return Err(EvalError::InvalidInput(
            "probe oracle cannot mix conjunctive claim annotations and annotation alternatives"
                .into(),
        ));
    }
    for annotation in &oracle.required_claim_annotations {
        validate_claim_annotation(annotation)?;
        validate_annotation_evidence_binding(annotation, oracle)?;
    }
    for alternative in &oracle.required_claim_annotation_alternatives {
        if alternative.is_empty() {
            return Err(EvalError::InvalidInput(
                "probe oracle claim annotation alternatives cannot be empty".into(),
            ));
        }
        for annotation in alternative {
            validate_claim_annotation(annotation)?;
            validate_annotation_evidence_binding(annotation, oracle)?;
        }
    }
    if observation.input_sha256 != input.sha256()?
        || observation.product_sha256 != expected_product_sha256
    {
        return Err(EvalError::InvalidInput(
            "independent probe identity mismatch".into(),
        ));
    }
    let sources = &observation.source_event_ids;
    let evidence_complete = if oracle.sufficient_evidence_alternatives.is_empty() {
        sources.is_empty() && observation.budget_used == 0
    } else {
        oracle
            .sufficient_evidence_alternatives
            .iter()
            .any(|alternative| {
                !alternative.is_empty()
                    && alternative
                        .iter()
                        .all(|group| group.iter().any(|source| sources.contains(source)))
            })
    };
    let annotations_complete = oracle.required_claim_annotations.iter().all(|required| {
        observation
            .claim_annotations
            .iter()
            .any(|actual| claim_annotation_matches(required, actual))
    }) && (oracle.required_claim_annotation_alternatives.is_empty()
        || oracle
            .required_claim_annotation_alternatives
            .iter()
            .any(|alternative| {
                alternative.iter().all(|required| {
                    observation
                        .claim_annotations
                        .iter()
                        .any(|actual| claim_annotation_matches(required, actual))
                })
            }));
    Ok(ProbeScore {
        schema_version: "memory-mcp-independent-probe-score/v1".into(),
        input_sha256: input.sha256()?,
        oracle_sha256: Sha256::digest(serde_json::to_vec(oracle)?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        product_sha256: expected_product_sha256.into(),
        observation_sha256: hex::encode(Sha256::digest(serde_json::to_vec(observation)?)),
        evidence_success: observation.completed
            && observation.budget_used <= input.context_budget
            && evidence_complete
            && annotations_complete
            && !oracle
                .forbidden_event_ids
                .iter()
                .any(|source| sources.contains(source)),
    })
}

fn validate_annotation_evidence_binding(
    annotation: &ProbeClaimAnnotation,
    oracle: &ProbeOracle,
) -> Result<(), EvalError> {
    if !oracle
        .sufficient_evidence_alternatives
        .iter()
        .flatten()
        .flatten()
        .any(|event_id| event_id == &annotation.source_event_id)
    {
        return Err(EvalError::InvalidInput(
            "probe claim annotation must be bound to required source evidence".into(),
        ));
    }
    Ok(())
}

pub(crate) fn claim_annotation_matches(
    required: &ProbeClaimAnnotation,
    actual: &ProbeClaimAnnotation,
) -> bool {
    actual.source_event_id == required.source_event_id
        && required
            .evidence_status
            .as_ref()
            .is_none_or(|status| actual.evidence_status.as_ref() == Some(status))
        && required
            .relations
            .iter()
            .all(|relation| actual.relations.contains(relation))
}

pub(crate) fn validate_claim_annotation(
    annotation: &ProbeClaimAnnotation,
) -> Result<(), EvalError> {
    let valid_status = annotation.evidence_status.as_deref().is_none_or(|status| {
        matches!(status, "current" | "historical" | "conflicting" | "unknown")
    });
    let valid_relation = annotation.relations.iter().all(|relation| {
        matches!(
            (relation.outcome.as_str(), relation.role.as_str()),
            ("duplicate", "duplicate")
                | ("supersession", "superseded")
                | ("supersession", "relation_participant")
                | ("correction", "relation_participant")
                | ("contradiction", "conflict")
                | ("temporal_ambiguity", "temporally_ambiguous")
        )
    });
    if annotation.source_event_id.trim().is_empty()
        || (annotation.evidence_status.is_none() && annotation.relations.is_empty())
        || !valid_status
        || !valid_relation
    {
        return Err(EvalError::InvalidInput(
            "probe claim annotation has an unsupported status, relation, role, or source".into(),
        ));
    }
    Ok(())
}
