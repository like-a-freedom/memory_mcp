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

async fn claim_count(env: &PerfEnv) -> u64 {
    let rows = DbClient::query(
        env.db_client.as_ref(),
        "SELECT count() AS c FROM claim GROUP ALL",
        None,
        eval_harness::test_support::ACTIVE_NAMESPACE,
    )
    .await
    .expect("claim count query");
    rows[0]["c"].as_u64().expect("count is a number")
}

async fn claim_relation_count(env: &PerfEnv) -> u64 {
    let rows = DbClient::query(
        env.db_client.as_ref(),
        "SELECT count() AS c FROM claim_relation GROUP ALL",
        None,
        eval_harness::test_support::ACTIVE_NAMESPACE,
    )
    .await
    .expect("claim relation count query");
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
        ScenarioId::LatencyClaimWrite,
        ScenarioId::LatencyClaimAssemble,
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

#[tokio::test]
async fn claim_write_latency_scenario_times_the_production_claim_path() {
    let env = PerfEnv::connect_with_claim_stage("evidence").await;
    let spec = ScenarioSpec {
        id: ScenarioId::LatencyClaimWrite,
        driver: DriverKind::InProcess,
        scales: vec![3],
        iterations: 2,
        warmup: 0,
        queries: None,
    };
    let run = eval_harness::perf::scenarios::build(&spec);

    let first = run.run(&env, 0).await;
    let second = run.run(&env, 1).await;

    assert!(
        first.invalid_reasons.is_empty(),
        "{:?}",
        first.invalid_reasons
    );
    assert!(
        second.invalid_reasons.is_empty(),
        "{:?}",
        second.invalid_reasons
    );
    assert_eq!(first.latencies_us.len(), 1);
    assert_eq!(second.latencies_us.len(), 1);
    assert_eq!(
        claim_count(&env).await,
        5,
        "setup and timed writes project claims"
    );
    assert!(
        claim_relation_count(&env).await > 0,
        "Evidence writes should exercise relation persistence"
    );
}

#[tokio::test]
async fn claim_write_performance_scenario_can_run_as_shadow_control() {
    let env = PerfEnv::connect_with_claim_stage("shadow").await;
    let spec = ScenarioSpec {
        id: ScenarioId::LatencyClaimWrite,
        driver: DriverKind::InProcess,
        scales: vec![2],
        iterations: 1,
        warmup: 0,
        queries: None,
    };
    let run = eval_harness::perf::scenarios::build(&spec);
    let evidence = run.run(&env, 0).await;

    assert!(
        evidence.invalid_reasons.is_empty(),
        "{:?}",
        evidence.invalid_reasons
    );
    assert_eq!(claim_count(&env).await, 3);
    assert!(
        claim_relation_count(&env).await > 0,
        "Shadow persists relation decisions but does not expose them to readers"
    );
}

#[tokio::test]
async fn claim_assemble_scenario_measures_only_the_stage_gated_read_path() {
    let evidence_env = PerfEnv::connect_with_claim_stage("evidence").await;
    let shadow_env = PerfEnv::connect_with_claim_stage("shadow").await;
    let spec = ScenarioSpec {
        id: ScenarioId::LatencyClaimAssemble,
        driver: DriverKind::InProcess,
        scales: vec![2],
        iterations: 1,
        warmup: 0,
        queries: None,
    };
    let evidence_run = eval_harness::perf::scenarios::build(&spec);
    let shadow_run = eval_harness::perf::scenarios::build(&spec);
    let evidence = evidence_run.run(&evidence_env, 0).await;
    let shadow = shadow_run.run(&shadow_env, 0).await;

    assert!(
        evidence.invalid_reasons.is_empty(),
        "{:?}",
        evidence.invalid_reasons
    );
    assert!(
        shadow.invalid_reasons.is_empty(),
        "{:?}",
        shadow.invalid_reasons
    );
    assert_eq!(evidence.latencies_us.len(), 1);
    assert_eq!(shadow.latencies_us.len(), 1);
    assert!(claim_relation_count(&evidence_env).await > 0);

    let request = || AssembleContextRequest {
        query: "ARR".into(),
        fact_types: vec![],
        as_of: None,
        budget: 10,
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };
    let evidence_items = memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
        &evidence_env.service,
        request(),
    )
    .await
    .expect("Evidence claim-stage context assembles");
    let shadow_items = memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
        &shadow_env.service,
        request(),
    )
    .await
    .expect("Shadow claim-stage context assembles");

    assert!(
        !evidence_items.is_empty(),
        "Evidence fixture is retrievable"
    );
    assert!(!shadow_items.is_empty(), "Shadow fixture is retrievable");

    assert!(
        evidence_items
            .iter()
            .any(|item| item.reconciliation.is_some()),
        "Evidence must disclose persisted relation evidence"
    );
    assert!(
        shadow_items
            .iter()
            .all(|item| item.reconciliation.is_none()),
        "Shadow must withhold the same persisted relations"
    );
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
