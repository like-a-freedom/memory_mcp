//! In-process scenario execution: one capability call per measurement.
//!
//! `run()` performs exactly one iteration; the runner (Task 9) decides which
//! iterations are warm-up (discarded) and which are recorded. Scenarios never
//! exclude warm-up themselves.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
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
use memory_mcp::storage::SurrealDbClient;

use crate::perf::db_observer::QueryLedger;
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
        let memory = TestMemory::new().await;
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

async fn ingest_one(
    env: &PerfEnv,
    source_id: String,
    content: String,
    t_ref: DateTime<Utc>,
) -> Result<String, String> {
    IngestCapability::ingest_from_service(
        &env.service,
        IngestRequest {
            source_type: "perf".into(),
            source_id: source_id.clone(),
            content,
            t_ref,
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .map_err(|err| format!("ingest {source_id}: {err}"))
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
        let mut ev = ScenarioEvidence::empty(self.id(), self.scale);
        for i in 0..self.scale {
            let source_id = format!(
                "perf-{}-{}-{}-{}",
                self.id().as_str(),
                self.scale,
                iteration,
                i
            );
            let content =
                format!("Perf ingest {source_id}: Ada Lovelace worked on the Analytical Engine.");
            let start = Instant::now();
            match ingest_one(env, source_id, content, Utc::now()).await {
                Ok(_) => ev.latencies_us.push(micros_since(start)),
                Err(reason) => ev.invalid_reasons.push(reason),
            }
        }
        ev
    }
}

/// Ingest N, then `queries` assemble calls over deterministic questions.
/// The recorded latencies are the assemble calls only; the ingests are
/// setup (their SQL still shows up in the ledger) and their failures land
/// in `invalid_reasons`.
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
        let mut ev = ScenarioEvidence::empty(self.id(), self.scale);
        for i in 0..self.scale {
            let source_id = format!(
                "perf-{}-{}-{}-{}",
                self.id().as_str(),
                self.scale,
                iteration,
                i
            );
            let content =
                format!("Perf assemble {source_id}: Ada Lovelace worked on the Analytical Engine.");
            if let Err(reason) = ingest_one(env, source_id, content, Utc::now()).await {
                ev.invalid_reasons.push(reason);
            }
        }
        for q in 0..self.queries {
            let request = AssembleContextRequest {
                query: format!("perf query {q}"),
                fact_types: vec![],
                as_of: Some(Utc::now()),
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
                compact: false,
            };
            let start = Instant::now();
            match AssembleContextCapability::assemble_context_from_service(&env.service, request)
                .await
            {
                Ok(_) => ev.latencies_us.push(micros_since(start)),
                Err(err) => ev.invalid_reasons.push(format!("assemble: {err}")),
            }
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
        let content = format!("Perf fixture {i}: Alice from Berlin founded Acme Corp with Bob.");
        let episode_id = ingest_one(env, source_id, content, fixture_t_ref()).await?;
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
        match self.id {
            ScenarioId::LatencyExtract => {
                let episode_id =
                    &fixture.episode_ids[(iteration as usize) % fixture.episode_ids.len()];
                let start = Instant::now();
                match ExtractCapability::extract_from_service(&env.service, episode_id, None, None)
                    .await
                {
                    Ok(_) => ev.latencies_us.push(micros_since(start)),
                    Err(err) => ev.invalid_reasons.push(format!("extract: {err}")),
                }
            }
            ScenarioId::LatencyResolve => {
                let candidate = EntityCandidate {
                    entity_type: "person".into(),
                    canonical_name: format!("Perf Candidate {}", iteration % self.scale.max(1)),
                    aliases: vec![],
                };
                let start = Instant::now();
                match ResolveCapability::resolve_from_service(&env.service, candidate, None).await {
                    Ok(_) => ev.latencies_us.push(micros_since(start)),
                    Err(err) => ev.invalid_reasons.push(format!("resolve: {err}")),
                }
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
                let start = Instant::now();
                match ExplainCapability::explain_from_service(&env.service, request, None).await {
                    Ok(_) => ev.latencies_us.push(micros_since(start)),
                    Err(err) => ev.invalid_reasons.push(format!("explain: {err}")),
                }
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
                let start = Instant::now();
                match InvalidateCapability::invalidate_from_service(&env.service, request, None)
                    .await
                {
                    Ok(_) => ev.latencies_us.push(micros_since(start)),
                    Err(err) => ev.invalid_reasons.push(format!("invalidate: {err}")),
                }
            }
            ScenarioId::IngestScale | ScenarioId::AssembleScale => {
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
    match spec.id {
        ScenarioId::IngestScale => Box::new(IngestScale { scale }),
        ScenarioId::AssembleScale => Box::new(AssembleScale {
            scale,
            queries: spec.queries.unwrap_or(0),
        }),
        id => Box::new(LatencyScenario::new(id, scale)),
    }
}
