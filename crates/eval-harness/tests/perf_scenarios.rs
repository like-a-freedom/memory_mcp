use eval_harness::perf::profile::{DriverKind, ScenarioId, ScenarioSpec};
use eval_harness::perf::scenarios::{PerfEnv, ScenarioEvidence, ScenarioRun};
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::storage::DbClient;

async fn episode_count(env: &PerfEnv) -> u64 {
    let rows = DbClient::query(
        env.db_client.as_ref(),
        "SELECT count() AS c FROM episode GROUP ALL",
        None,
        eval_harness::test_support::ACTIVE_NAMESPACE,
    )
    .await
    .expect("episode count query");
    rows[0]["c"].as_u64().expect("count is a number")
}

fn ingest_scale_spec(scales: Vec<u32>, iterations: u32) -> ScenarioSpec {
    ScenarioSpec {
        id: ScenarioId::IngestScale,
        driver: DriverKind::InProcess,
        scales,
        iterations,
        warmup: 0,
        queries: None,
    }
}

#[tokio::test]
async fn ingest_scale_unique_ids_write_every_episode_and_are_observed() {
    let env = PerfEnv::connect().await;
    let run: Box<dyn ScenarioRun> =
        eval_harness::perf::scenarios::build(&ingest_scale_spec(vec![2], 2));

    let e0: ScenarioEvidence = run.run(&env, 0).await;
    let e1: ScenarioEvidence = run.run(&env, 1).await;

    assert!(
        e0.invalid_reasons.is_empty(),
        "iteration 0 failed: {:?}",
        e0.invalid_reasons
    );
    assert!(
        e1.invalid_reasons.is_empty(),
        "iteration 1 failed: {:?}",
        e1.invalid_reasons
    );
    assert_eq!(e0.scale, 2);
    assert_eq!(e1.scenario, ScenarioId::IngestScale);

    let latencies: Vec<u64> = e0
        .latencies_us
        .iter()
        .chain(e1.latencies_us.iter())
        .copied()
        .collect();
    assert_eq!(latencies.len(), 4, "2 ingests x 2 iterations");
    assert!(
        latencies.iter().all(|&d| d > 0),
        "every latency must be measured: {latencies:?}"
    );

    assert_eq!(
        episode_count(&env).await,
        4,
        "unique source ids perf-scenario-scale-iteration-i: dedup must not swallow iterations"
    );

    let records = env.ledger.snapshot();
    let episode_writes: u32 = records
        .iter()
        .filter(|r| r.class == "EpisodeWrite")
        .map(|r| r.count)
        .sum();
    assert_eq!(
        episode_writes, 4,
        "exactly one observed write per episode across templates"
    );
}

#[tokio::test]
async fn build_covers_every_scenario_id() {
    for id in [
        ScenarioId::IngestScale,
        ScenarioId::AssembleScale,
        ScenarioId::LatencyExtract,
        ScenarioId::LatencyResolve,
        ScenarioId::LatencyExplain,
        ScenarioId::LatencyInvalidate,
    ] {
        let spec = ScenarioSpec {
            id,
            driver: DriverKind::InProcess,
            scales: vec![10],
            iterations: 1,
            warmup: 0,
            queries: Some(2),
        };
        let run = eval_harness::perf::scenarios::build(&spec);
        assert_eq!(run.id(), id);
        assert_eq!(run.scale(), 10);
    }
}

/// The assemble-scale corpus must be fact-producing and its queries must
/// defeat the warm context cache: the weekly gate claims to measure
/// `assemble_context` retrieval, so a recorded sample has to contain the
/// pipeline (SQL channels), not a cache hit over an empty result set.
#[tokio::test]
async fn assemble_scale_corpus_is_fact_producing_and_retrievable() {
    let env = PerfEnv::connect().await;
    let spec = ScenarioSpec {
        id: ScenarioId::AssembleScale,
        driver: DriverKind::InProcess,
        scales: vec![3],
        iterations: 1,
        warmup: 0,
        queries: Some(2),
    };
    let run = eval_harness::perf::scenarios::build(&spec);
    let ev = run.run(&env, 0).await;
    assert!(ev.invalid_reasons.is_empty(), "{:?}", ev.invalid_reasons);
    assert_eq!(ev.latencies_us.len(), 2, "two assemble calls per iteration");

    let items =
        memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
            &env.service,
            AssembleContextRequest {
                query: "cold retrieval check: ARR grew by Friday".into(),
                fact_types: vec![],
                as_of: None,
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
                compact: false,
            },
        )
        .await
        .expect("cold assemble");
    assert!(
        !items.is_empty(),
        "scenario corpus must yield retrievable facts"
    );
}
