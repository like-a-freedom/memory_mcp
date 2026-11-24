//! In-process scenario execution: one capability call per measurement.
//!
//! `run()` performs exactly one iteration; the runner decides which
//! iterations are warm-up (discarded) and which are recorded. Scenarios never
//! exclude warm-up themselves.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_mcp::error::MemoryError;
use memory_mcp::eval_support::QueryObserver;
use memory_mcp::models::{
    AssembleContextRequest, EntityCandidate, ExplainItem, ExplainRequest, IngestRequest,
    InvalidateRequest,
};
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::{
    memory_capabilities_assemble_context::AssembleContextCapability,
    memory_capabilities_explain::ExplainCapability, memory_capabilities_extract::ExtractCapability,
    memory_capabilities_ingest::IngestCapability,
    memory_capabilities_invalidate::InvalidateCapability,
    memory_capabilities_resolve::ResolveCapability,
};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

use crate::perf::db_observer::{QueryLedger, QueryPhase};
use crate::perf::profile::{ScenarioId, ScenarioSpec};
use crate::perf::sampler::{ResourceSampler, self_sampler};
use crate::test_support::TestMemory;

/// One iteration's measurements for one `(scenario, scale)`.
#[derive(Debug, Clone)]
pub struct ScenarioEvidence {
    pub scenario: ScenarioId,
    pub scale: u32,
    pub latencies_us: Vec<u64>,
    pub invalid_reasons: Vec<String>,
}

impl ScenarioEvidence {
    fn empty(scenario: ScenarioId, scale: u32) -> Self {
        Self {
            scenario,
            scale,
            latencies_us: Vec::new(),
            invalid_reasons: Vec::new(),
        }
    }
}

/// One iteration of one scenario at one scale.
#[async_trait]
pub trait ScenarioRun: Send + Sync {
    fn id(&self) -> ScenarioId;
    fn scale(&self) -> u32;
    async fn run(&self, env: &PerfEnv, iteration: u32) -> ScenarioEvidence;
}

/// Everything a scenario may touch: the service under test, the SQL ledger
/// observer, and the resource sampler.
pub struct PerfEnv {
    pub service: MemoryService,
    pub db_client: Arc<SurrealDbClient>,
    pub ledger: Arc<QueryLedger>,
    pub sampler: Box<dyn ResourceSampler>,
}

impl PerfEnv {
    /// Connects the in-process stack: embedded SurrealDB + migrations +
    /// service, then installs the eval SQL observer. Migrations run before
    /// the observer exists, so setup SQL never pollutes a scenario ledger.
    pub async fn connect() -> Self {
        let claim_stage =
            std::env::var("MEMORY_CLAIM_ROLLOUT_STAGE").unwrap_or_else(|_| "evidence".to_owned());
        Self::connect_with_claim_stage(&claim_stage).await
    }

    /// Connects the in-process stack at a named claim stage. This seam lets a
    /// paired performance profile compare Shadow and Evidence without changing
    /// the global environment or process defaults.
    pub async fn connect_with_claim_stage(claim_stage: &str) -> Self {
        let memory = TestMemory::new_with_claim_stage(claim_stage).await;
        let ledger = QueryLedger::new();
        memory
            .db_client
            .set_eval_query_observer(Arc::clone(&ledger) as Arc<dyn QueryObserver>);
        Self {
            service: memory.service,
            db_client: memory.db_client,
            ledger,
            sampler: self_sampler(),
        }
    }
}

fn micros_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Deterministic per-iteration source id, unique across scenario, scale,
/// iteration and index so no iteration can be swallowed by dedup.
fn perf_source_id(scenario: ScenarioId, scale: u32, iteration: u32, i: u32) -> String {
    format!("perf-{}-{}-{}-{}", scenario.as_str(), scale, iteration, i)
}

async fn seed_claim_facts(env: &PerfEnv, scenario: ScenarioId, scale: u32) -> Result<(), String> {
    let lineage = format!("perf-claim-lineage-{}-{scale}", scenario.as_str());
    for i in 0..scale {
        let source_id = perf_source_id(scenario, scale, 0, i);
        let episode_id = format!("episode:{source_id}");
        let timestamp = fixture_t_ref().to_rfc3339();
        env.db_client
            .create(
                &episode_id,
                json!({
                    "episode_id": episode_id,
                    "source_type": "document",
                    "source_id": source_id,
                    "content": format!(
                        "subject: ARR; dimension: ARR; value: {}",
                        if i == 0 { "legacy" } else { "supersedes" }
                    ),
                    "t_ref": timestamp,
                    "t_ingested": timestamp,
                    "policy_tags": [],
                    "source_lineage": lineage,
                }),
                crate::test_support::ACTIVE_NAMESPACE,
                memory_mcp::memory::queries::EPISODE_TEMPORAL_FIELDS,
            )
            .await
            .map_err(|err| format!("claim seed episode {i}: {err}"))?;
        ExtractCapability::extract_from_service(&env.service, &episode_id, None, None)
            .await
            .map_err(|err| format!("claim seed extraction {i}: {err}"))?;
    }

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let rows = DbClient::query(
                env.db_client.as_ref(),
                "SELECT count() AS c FROM claim_relation GROUP ALL",
                None,
                crate::test_support::ACTIVE_NAMESPACE,
            )
            .await
            .map_err(|err| format!("read claim relation seed count: {err}"))?;
            if rows
                .as_array()
                .and_then(|rows| rows.first())
                .and_then(|row| row.get("c"))
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|count| count > 0)
            {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "claim seed did not produce a persisted relation within 5 seconds".to_owned())?
}

/// Measures the public fact-write path that runs deterministic claim
/// projection, inline reconciliation, and the configured persistence stage.
/// Seed claims are setup; each sample is one new fact against `scale` existing
/// same-slot claims so candidate-page work is represented in the measurement.
struct ClaimWrite {
    scale: u32,
    seeded: Mutex<bool>,
}

impl ClaimWrite {
    async fn seed(&self, env: &PerfEnv) -> Result<(), String> {
        crate::test_support::seed_entity(
            &env.db_client,
            "entity:perf-claim-subject",
            "person",
            "Alice Smith",
            &[],
        )
        .await;
        for i in 0..self.scale {
            let source_id = perf_source_id(self.id(), self.scale, 0, i);
            let episode_id = ingest_one(
                env,
                source_id,
                "claim performance seed".into(),
                fixture_t_ref(),
            )
            .await
            .map_err(|err| format!("claim seed ingest {i}: {err}"))?;
            env.service
                .add_fact(
                    "note",
                    &format!("subject: Alice Smith\ndimension: ARR\nvalue: {i}"),
                    "claim performance seed",
                    &episode_id,
                    fixture_t_ref(),
                    1.0,
                    vec!["entity:perf-claim-subject".into()],
                    vec![],
                    memory_mcp::models::Provenance::agent_observation(&episode_id),
                )
                .await
                .map_err(|err| format!("claim seed projection {i}: {err}"))?;
        }
        Ok(())
    }
}

/// Measures the stage-gated read path after deterministic claim setup. Each
/// iteration uses a distinct query to avoid reporting a context-cache hit.
struct ClaimAssemble {
    scale: u32,
    seeded: Mutex<bool>,
}

#[async_trait]
impl ScenarioRun for ClaimAssemble {
    fn id(&self) -> ScenarioId {
        ScenarioId::LatencyClaimAssemble
    }

    fn scale(&self) -> u32 {
        self.scale
    }

    async fn run(&self, env: &PerfEnv, iteration: u32) -> ScenarioEvidence {
        let mut ev = ScenarioEvidence::empty(self.id(), self.scale);
        env.ledger.set_phase(QueryPhase::Setup);
        let needs_seed = !*self.seeded.lock().expect("claim seed lock poisoned");
        if needs_seed {
            if let Err(reason) = seed_claim_facts(env, self.id(), self.scale).await {
                ev.invalid_reasons.push(reason);
                return ev;
            }
            *self.seeded.lock().expect("claim seed lock poisoned") = true;
        }

        let request = AssembleContextRequest {
            query: format!("ARR observation {iteration}"),
            fact_types: vec![],
            as_of: None,
            budget: 10,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        };
        env.ledger.set_phase(QueryPhase::Timed);
        timed(
            &mut ev,
            "claim-stage assemble",
            AssembleContextCapability::assemble_context_from_service(&env.service, request),
        )
        .await;
        ev
    }
}

#[async_trait]
impl ScenarioRun for ClaimWrite {
    fn id(&self) -> ScenarioId {
        ScenarioId::LatencyClaimWrite
    }

    fn scale(&self) -> u32 {
        self.scale
    }

    async fn run(&self, env: &PerfEnv, iteration: u32) -> ScenarioEvidence {
        let mut ev = ScenarioEvidence::empty(self.id(), self.scale);
        env.ledger.set_phase(QueryPhase::Setup);
        let needs_seed = !*self.seeded.lock().expect("claim seed lock poisoned");
        if needs_seed {
            if let Err(reason) = self.seed(env).await {
                ev.invalid_reasons.push(reason);
                return ev;
            }
            *self.seeded.lock().expect("claim seed lock poisoned") = true;
        }

        let episode_id = match ingest_one(
            env,
            perf_source_id(self.id(), self.scale, iteration, self.scale),
            "claim performance sample".into(),
            fixture_t_ref(),
        )
        .await
        {
            Ok(episode_id) => episode_id,
            Err(err) => {
                ev.invalid_reasons
                    .push(format!("claim sample ingest: {err}"));
                return ev;
            }
        };

        env.ledger.set_phase(QueryPhase::Timed);
        let content = format!(
            "subject: Alice Smith\ndimension: ARR\nvalue: {}",
            self.scale + iteration
        );
        timed(
            &mut ev,
            "claim projection and inline reconciliation",
            env.service.add_fact(
                "note",
                &content,
                &content,
                &episode_id,
                fixture_t_ref(),
                1.0,
                vec!["entity:perf-claim-subject".into()],
                vec![],
                memory_mcp::models::Provenance::agent_observation(&episode_id),
            ),
        )
        .await;
        ev
    }
}

/// Times one capability call: records the latency on success, or the
/// `"{label}: {err}"` reason on failure.
async fn timed<T>(
    ev: &mut ScenarioEvidence,
    label: &str,
    fut: impl Future<Output = Result<T, MemoryError>>,
) {
    let start = Instant::now();
    match fut.await {
        Ok(_) => ev.latencies_us.push(micros_since(start)),
        Err(err) => ev.invalid_reasons.push(format!("{label}: {err}")),
    }
}

async fn ingest_one(
    env: &PerfEnv,
    source_id: String,
    content: String,
    t_ref: DateTime<Utc>,
) -> Result<String, MemoryError> {
    IngestCapability::ingest_from_service(
        &env.service,
        IngestRequest {
            source_type: "perf".into(),
            source_id: source_id.clone(),
            content,
            t_ref,
            t_ingested: None,
            source_context: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
}

/// N ingests with ids unique across scenario, scale, iteration and index, so
/// no iteration can be swallowed by the dedup path (Review Focus #5).
struct IngestScale {
    scale: u32,
}

#[async_trait]
impl ScenarioRun for IngestScale {
    fn id(&self) -> ScenarioId {
        ScenarioId::IngestScale
    }

    fn scale(&self) -> u32 {
        self.scale
    }

    async fn run(&self, env: &PerfEnv, iteration: u32) -> ScenarioEvidence {
        // Every ingest in this scenario is part of the measurement.
        env.ledger.set_phase(QueryPhase::Timed);
        let mut ev = ScenarioEvidence::empty(self.id(), self.scale);
        for i in 0..self.scale {
            let source_id = perf_source_id(self.id(), self.scale, iteration, i);
            let content =
                format!("Perf ingest {source_id}: Ada Lovelace worked on the Analytical Engine.");
            timed(
                &mut ev,
                &format!("ingest {source_id}"),
                ingest_one(env, source_id, content, Utc::now()),
            )
            .await;
        }
        ev
    }
}

/// Ingest N, then `queries` assemble calls over deterministic questions.
/// The recorded latencies are the assemble calls only; the ingests are
/// setup (their SQL still shows up in the ledger) and their failures land
/// in `invalid_reasons`.
///
/// The corpus is fact-producing and extracted in setup, and every assemble
/// call uses a unique query: the context cache is keyed by query, so a
/// repeated question would record cache-hit latencies (~tens of µs) while
/// claiming to gate the retrieval pipeline. See the weekly-gate honesty
/// note in `docs/performance/PERF_HARNESS.md`.
struct AssembleScale {
    scale: u32,
    queries: u32,
}

#[async_trait]
impl ScenarioRun for AssembleScale {
    fn id(&self) -> ScenarioId {
        ScenarioId::AssembleScale
    }

    fn scale(&self) -> u32 {
        self.scale
    }

    async fn run(&self, env: &PerfEnv, iteration: u32) -> ScenarioEvidence {
        env.ledger.set_phase(QueryPhase::Setup);
        let mut ev = ScenarioEvidence::empty(self.id(), self.scale);
        for i in 0..self.scale {
            let source_id = perf_source_id(self.id(), self.scale, iteration, i);
            let content = format!(
                "Perf assemble {source_id}: ARR grew to ${}M. I will send the update by Friday.",
                10 + i
            );
            let episode_id = match ingest_one(env, source_id.clone(), content, Utc::now()).await {
                Ok(id) => id,
                Err(err) => {
                    ev.invalid_reasons
                        .push(format!("ingest {source_id}: {err}"));
                    continue;
                }
            };
            if let Err(reason) =
                ExtractCapability::extract_from_service(&env.service, &episode_id, None, None)
                    .await
                    .map(|_| ())
                    .map_err(|err| format!("assemble setup extract {episode_id}: {err}"))
            {
                ev.invalid_reasons.push(reason);
            }
        }
        env.ledger.set_phase(QueryPhase::Timed);
        for q in 0..self.queries {
            let request = AssembleContextRequest {
                query: format!("perf query {iteration}-{q}"),
                fact_types: vec![],
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
                compact: false,
            };
            timed(
                &mut ev,
                "assemble",
                AssembleContextCapability::assemble_context_from_service(&env.service, request),
            )
            .await;
        }
        ev
    }
}

/// Shared corpus: `scale` episodes ingested at one fixed `t_ref`, then
/// extracted so their facts exist for explain/invalidate.
#[derive(Clone)]
struct CorpusFixture {
    episode_ids: Vec<String>,
    fact_ids: Vec<String>,
}

fn fixture_t_ref() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .expect("fixture t_ref is a valid RFC-3339 timestamp")
        .with_timezone(&Utc)
}

async fn corpus_fixture(
    env: &PerfEnv,
    scenario: ScenarioId,
    scale: u32,
) -> Result<CorpusFixture, String> {
    let mut episode_ids = Vec::new();
    let mut fact_ids = Vec::new();
    for i in 0..scale {
        let source_id = format!("perf-fixture-{}-{}-{}", scenario.as_str(), scale, i);
        // Fact heuristics (metric/promise), not NER, produce the fact ids that
        // explain/invalidate need; entity-only prose yields an empty corpus.
        let content = format!(
            "Perf fixture {i}: ARR grew to ${}M. I will send the update by Friday.",
            3 + i
        );
        let episode_id = ingest_one(env, source_id.clone(), content, fixture_t_ref())
            .await
            .map_err(|err| format!("ingest {source_id}: {err}"))?;
        episode_ids.push(episode_id);
    }
    for episode_id in &episode_ids {
        let extracted =
            ExtractCapability::extract_from_service(&env.service, episode_id, None, None)
                .await
                .map_err(|err| format!("fixture extract {episode_id}: {err}"))?;
        for fact in extracted.facts {
            fact_ids.push(fact.fact_id);
        }
    }
    Ok(CorpusFixture {
        episode_ids,
        fact_ids,
    })
}

/// One capability call per iteration over the shared corpus fixture. The
/// fixture is built on first use and cached, so only the timed call is
/// recorded after warm-up.
struct LatencyScenario {
    id: ScenarioId,
    scale: u32,
    fixture: Mutex<Option<CorpusFixture>>,
}

impl LatencyScenario {
    fn new(id: ScenarioId, scale: u32) -> Self {
        Self {
            id,
            scale,
            fixture: Mutex::new(None),
        }
    }

    async fn corpus(&self, env: &PerfEnv) -> Result<CorpusFixture, String> {
        let cached = self.fixture.lock().expect("fixture mutex poisoned").clone();
        if let Some(fixture) = cached {
            return Ok(fixture);
        }
        let fixture = corpus_fixture(env, self.id, self.scale).await?;
        *self.fixture.lock().expect("fixture mutex poisoned") = Some(fixture.clone());
        Ok(fixture)
    }
}

#[async_trait]
impl ScenarioRun for LatencyScenario {
    fn id(&self) -> ScenarioId {
        self.id
    }

    fn scale(&self) -> u32 {
        self.scale
    }

    async fn run(&self, env: &PerfEnv, iteration: u32) -> ScenarioEvidence {
        let mut ev = ScenarioEvidence::empty(self.id, self.scale);
        env.ledger.set_phase(QueryPhase::Setup);
        let fixture = match self.corpus(env).await {
            Ok(fixture) => fixture,
            Err(reason) => {
                ev.invalid_reasons.push(reason);
                return ev;
            }
        };
        if fixture.episode_ids.is_empty() {
            ev.invalid_reasons
                .push("fixture produced no episodes".into());
            return ev;
        }
        env.ledger.set_phase(QueryPhase::Timed);
        match self.id {
            ScenarioId::LatencyExtract => {
                let episode_id =
                    &fixture.episode_ids[(iteration as usize) % fixture.episode_ids.len()];
                timed(
                    &mut ev,
                    "extract",
                    ExtractCapability::extract_from_service(&env.service, episode_id, None, None),
                )
                .await;
            }
            ScenarioId::LatencyResolve => {
                let candidate = EntityCandidate {
                    entity_type: "person".into(),
                    canonical_name: format!("Perf Candidate {}", iteration % self.scale.max(1)),
                    aliases: vec![],
                };
                timed(
                    &mut ev,
                    "resolve",
                    ResolveCapability::resolve_from_service(&env.service, candidate, None),
                )
                .await;
            }
            ScenarioId::LatencyExplain => {
                if fixture.fact_ids.is_empty() {
                    ev.invalid_reasons.push("fixture produced no facts".into());
                    return ev;
                }
                let context_pack: Vec<ExplainItem> = fixture
                    .fact_ids
                    .iter()
                    .enumerate()
                    .map(|(i, fact_id)| ExplainItem {
                        fact_id: Some(fact_id.clone()),
                        content: format!("Perf fixture fact {i}"),
                        quote: format!("Perf fixture fact {i}"),
                        source_episode: fixture
                            .episode_ids
                            .get(i % fixture.episode_ids.len())
                            .cloned()
                            .unwrap_or_default(),
                        ..ExplainItem::default()
                    })
                    .collect();
                let request = ExplainRequest {
                    context_pack,
                    compact: false,
                };
                timed(
                    &mut ev,
                    "explain",
                    ExplainCapability::explain_from_service(&env.service, request, None),
                )
                .await;
            }
            ScenarioId::LatencyInvalidate => {
                if fixture.fact_ids.is_empty() {
                    ev.invalid_reasons.push("fixture produced no facts".into());
                    return ev;
                }
                let fact_id =
                    fixture.fact_ids[(iteration as usize) % fixture.fact_ids.len()].clone();
                let request = InvalidateRequest {
                    fact_id,
                    reason: "perf latency scenario".into(),
                    t_invalid: Utc::now(),
                };
                timed(
                    &mut ev,
                    "invalidate",
                    InvalidateCapability::invalidate_from_service(&env.service, request, None),
                )
                .await;
            }
            ScenarioId::IngestScale
            | ScenarioId::AssembleScale
            | ScenarioId::LatencyClaimWrite
            | ScenarioId::LatencyClaimAssemble => {
                ev.invalid_reasons
                    .push("latency scenario built with a scale scenario id".into());
            }
        }
        ev
    }
}

/// Builds the scenario for `spec`. Multi-scale profiles are expanded by the
/// runner into one spec per scale, so the first scale is authoritative here.
pub fn build(spec: &ScenarioSpec) -> Box<dyn ScenarioRun> {
    let scale = spec.scales.first().copied().unwrap_or(1);
    if spec.id == ScenarioId::LatencyClaimWrite {
        return Box::new(ClaimWrite {
            scale,
            seeded: Mutex::new(false),
        });
    }
    if spec.id == ScenarioId::LatencyClaimAssemble {
        return Box::new(ClaimAssemble {
            scale,
            seeded: Mutex::new(false),
        });
    }
    match spec.id {
        ScenarioId::IngestScale => Box::new(IngestScale { scale }),
        ScenarioId::AssembleScale => Box::new(AssembleScale {
            scale,
            queries: spec.queries.unwrap_or(0),
        }),
        id => Box::new(LatencyScenario::new(id, scale)),
    }
}
