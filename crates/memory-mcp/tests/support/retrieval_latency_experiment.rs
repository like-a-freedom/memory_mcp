use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use memory_mcp::config::SurrealTargetConfig;
use memory_mcp::error::MemoryError;
use memory_mcp::knowledge::KnowledgeStoreClient;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde::Serialize;
use serde_json::Value;

use tokio::time::timeout;

use super::oracle;
use super::{
    ContainerEvidenceError, EffectiveResources, RunConfig, inspect_container_details, row_digest,
};

const WARMUP_COUNT: usize = 5;
const SAMPLE_COUNT: usize = 20;
const SAMPLE_LIMIT: usize = 40;
const TRIPLE_COUNT: usize = 5;

fn candidate_guarded_triple_sql() -> String {
    guarded_triple_sql_with_else(oracle::corrected_unguarded_triple_sql())
}

fn guarded_triple_sql_with_else(else_body: &str) -> String {
    format!(
        "IF array::len((SELECT VALUE source_fact_id FROM triple \
         WHERE (predicate CONTAINS $query OR object CONTAINS $query OR subject CONTAINS $query))) = 0 \
         {{ [] }} ELSE {{ {else_body} }}"
    )
}
const DOCKER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const DOCKER_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug)]
struct DockerEvidence {
    server_version: String,
    resources: EffectiveResources,
    host_architecture: String,
    image_architecture: String,
    emulation: bool,
}

const DOCKER_OVERRIDE_ENV: &[&str] = &["DOCKER_HOST", "DOCKER_CONTEXT"];

struct DatabaseSession {
    name: String,
    namespace: String,
    client: Arc<SurrealDbClient>,
    store: KnowledgeStoreClient,
    query_calls: Arc<AtomicUsize>,
}

#[derive(Clone, Copy)]
enum SampleCase {
    EmptyTable,
    PopulatedNoMatch,
    PositiveMatch,
}

impl SampleCase {
    const ALL: [Self; 3] = [
        Self::EmptyTable,
        Self::PopulatedNoMatch,
        Self::PositiveMatch,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::EmptyTable => "empty_table",
            Self::PopulatedNoMatch => "populated_no_match",
            Self::PositiveMatch => "positive_match",
        }
    }

    fn query(self) -> &'static str {
        match self {
            Self::EmptyTable => "triple-fixture-empty-table-query",
            Self::PopulatedNoMatch => "triple-fixture-no-match-query",
            Self::PositiveMatch => "subject-match",
        }
    }

    fn has_fixture(self) -> bool {
        !matches!(self, Self::EmptyTable)
    }

    fn expected_fact_ids(self, limit: usize) -> &'static [&'static str] {
        match self {
            Self::PositiveMatch if limit > 0 => &["fact:triple-fixture-0000"],
            Self::EmptyTable | Self::PopulatedNoMatch | Self::PositiveMatch => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Observation {
    elapsed_ns: u128,
    row_count: usize,
    digest: String,
}

#[derive(Serialize)]
struct FixtureMetadata {
    fact_count: usize,
    embedding_dimension: usize,
    triple_count: usize,
    cutoff: &'static str,
}

#[derive(Serialize)]
struct SampleRecord {
    case: &'static str,
    elapsed_ns: Vec<u128>,
    min_ns: u128,
    median_ns: u128,
    max_ns: u128,
    row_count: usize,
    sha256: String,
    query_call_count: usize,
}

#[derive(Serialize)]
struct LatencyReport {
    schema: &'static str,
    server_version: String,
    sdk_version: &'static str,
    image_id: String,
    storage_engine: &'static str,
    host_architecture: String,
    image_architecture: String,
    emulation: bool,
    resource_mode: &'static str,
    effective_resources: EffectiveResources,
    fixture: FixtureMetadata,
    connection_mode: &'static str,
    limit: usize,
    warmup_count: usize,
    sample_count: usize,
    samples: Vec<SampleRecord>,
}

#[derive(Serialize)]
struct GuardPairRecord {
    order: &'static str,
    baseline_ns: u128,
    candidate_ns: u128,
}

#[derive(Serialize)]
struct GuardCaseRecord {
    case: &'static str,
    row_count: usize,
    sha256: String,
    pairs: Vec<GuardPairRecord>,
}

#[derive(Serialize)]
struct GuardComparisonReport {
    schema: &'static str,
    server_version: String,
    sdk_version: &'static str,
    image_id: String,
    storage_engine: &'static str,
    host_architecture: String,
    image_architecture: String,
    emulation: bool,
    resource_mode: &'static str,
    effective_resources: EffectiveResources,
    fixture: FixtureMetadata,
    warmup_count: usize,
    pair_count: usize,
    cases: Vec<GuardCaseRecord>,
}

pub(super) async fn verify_remote_triple_compatibility(config: &RunConfig) -> Result<(), String> {
    inspect_disposable_container(config)?;
    let session = open_database(config, true).await?;
    let result = async {
        for (query, limit, expected) in [
            ("subject-match", 40, &["fact:triple-fixture-0000"][..]),
            ("predicate-match", 40, &["fact:triple-fixture-0001"][..]),
            ("object-match", 40, &["fact:triple-fixture-0002"][..]),
            ("duplicate-match", 40, &["fact:triple-fixture-0000"][..]),
            ("quote 'snow' 日本語", 40, &[][..]),
            ("triple-fixture-no-match-query", 40, &[][..]),
            ("subject-match", 0, &[][..]),
        ] {
            measure_query(&session, query, limit, expected).await?;
        }
        Ok(())
    }
    .await;
    let cleanup = remove_database(&session).await;
    combine_result_and_cleanup(result, cleanup)
}

pub(super) async fn verify_guard_candidate_compatibility(config: &RunConfig) -> Result<(), String> {
    inspect_disposable_container(config)?;
    let session = open_database(config, true).await?;
    let result = async {
        let candidate_sql = candidate_guarded_triple_sql();
        let (candidate_store, candidate_calls) = make_override_store(&session, &candidate_sql);

        for (query, expected) in [
            ("triple-fixture-no-match-query", &[][..]),
            ("subject-match", &["fact:triple-fixture-0000"][..]),
            ("predicate-match", &["fact:triple-fixture-0001"][..]),
            ("quote 'snow' 日本語", &[][..]),
        ] {
            let vars = oracle::corrected_triple_vars(query, oracle::FIXED_CUTOFF, 40);
            let raw = timeout(
                super::QUERY_TIMEOUT,
                session
                    .client
                    .query(&candidate_sql, Some(vars), &session.namespace),
            )
            .await
            .map_err(|_| "raw candidate query exceeded 10 seconds".to_owned())?
            .map_err(|_| "raw candidate query failed".to_owned())?;
            validate_raw_fact_rows(&raw, expected)?;

            let before = candidate_calls.load(Ordering::Relaxed);
            let candidate_rows = timeout(
                super::QUERY_TIMEOUT,
                candidate_store.select_facts_by_triple(query, oracle::FIXED_CUTOFF, 40),
            )
            .await
            .map_err(|_| "public candidate lookup exceeded 10 seconds".to_owned())?
            .map_err(|_| "public candidate lookup failed".to_owned())?;
            if candidate_calls.load(Ordering::Relaxed) - before != 1 {
                return Err(
                    "one public candidate lookup did not issue exactly one SQL call".to_owned(),
                );
            }
            validate_fact_rows(&candidate_rows, expected)?;

            let baseline_rows = timeout(
                super::QUERY_TIMEOUT,
                session
                    .store
                    .select_facts_by_triple(query, oracle::FIXED_CUTOFF, 40),
            )
            .await
            .map_err(|_| "public baseline lookup exceeded 10 seconds".to_owned())?
            .map_err(|_| "public baseline lookup failed".to_owned())?;
            if oracle::normalized_rows(&baseline_rows) != oracle::normalized_rows(&candidate_rows) {
                return Err("candidate changed eligible fact rows or projection".to_owned());
            }
        }

        let skipped_branch_sql =
            guarded_triple_sql_with_else("THROW 'candidate fact branch entered'");
        let (skipped_branch_store, _) = make_override_store(&session, &skipped_branch_sql);
        let skipped = timeout(
            super::QUERY_TIMEOUT,
            skipped_branch_store.select_facts_by_triple(
                "triple-fixture-no-match-query",
                oracle::FIXED_CUTOFF,
                40,
            ),
        )
        .await
        .map_err(|_| "skipped-branch query exceeded 10 seconds".to_owned())?
        .map_err(|_| "no-match unexpectedly entered the failing fact branch".to_owned())?;
        if !skipped.is_empty() {
            return Err(
                "skipping the failing fact branch did not return an empty array".to_owned(),
            );
        }
        let entered = timeout(
            super::QUERY_TIMEOUT,
            skipped_branch_store.select_facts_by_triple("subject-match", oracle::FIXED_CUTOFF, 40),
        )
        .await
        .map_err(|_| "entered-branch query exceeded 10 seconds".to_owned())?
        .expect_err("the selected fact branch deliberately throws");
        if !entered
            .to_string()
            .contains("candidate fact branch entered")
        {
            return Err("the selected fact branch error was not propagated".to_owned());
        }

        let malformed_sql = guarded_triple_sql_with_else("{ malformed: true }");
        let malformed = timeout(
            super::QUERY_TIMEOUT,
            session.client.query(
                &malformed_sql,
                Some(oracle::corrected_triple_vars(
                    "subject-match",
                    oracle::FIXED_CUTOFF,
                    40,
                )),
                &session.namespace,
            ),
        )
        .await
        .map_err(|_| "malformed-result query exceeded 10 seconds".to_owned())?
        .map_err(|_| "malformed-result query failed before producing a value".to_owned())?;
        if malformed.as_array().is_none_or(Vec::is_empty)
            || validate_raw_fact_rows(&malformed, &["fact:triple-fixture-0000"]).is_ok()
        {
            return Err(
                "malformed candidate output was indistinguishable from a valid empty result"
                    .to_owned(),
            );
        }

        timeout(
            super::QUERY_TIMEOUT,
            session
                .client
                .query("REMOVE TABLE triple", None, &session.namespace),
        )
        .await
        .map_err(|_| "removing the disposable triple table exceeded 10 seconds".to_owned())?
        .map_err(|_| "could not remove the disposable triple table".to_owned())?;
        for (label, rows) in [
            (
                "baseline",
                timeout(
                    super::QUERY_TIMEOUT,
                    session
                        .store
                        .select_facts_by_triple("subject-match", oracle::FIXED_CUTOFF, 40),
                )
                .await
                .map_err(|_| "missing-table baseline lookup exceeded 10 seconds".to_owned())?
                .map_err(|_| "baseline missing-table behavior changed".to_owned())?,
            ),
            (
                "candidate",
                timeout(
                    super::QUERY_TIMEOUT,
                    candidate_store.select_facts_by_triple(
                        "subject-match",
                        oracle::FIXED_CUTOFF,
                        40,
                    ),
                )
                .await
                .map_err(|_| "missing-table candidate lookup exceeded 10 seconds".to_owned())?
                .map_err(|_| "candidate missing-table behavior changed".to_owned())?,
            ),
        ] {
            if !rows.is_empty() {
                return Err(format!(
                    "{label} lookup returned rows after its triple table was removed"
                ));
            }
        }
        Ok(())
    }
    .await;
    let cleanup = remove_database(&session).await;
    combine_result_and_cleanup(result, cleanup)
}

pub(super) async fn compare_guard_candidate(config: &RunConfig) -> Result<(), String> {
    let docker = inspect_disposable_container(config)?;
    let session = open_database(config, true).await?;
    let result = async {
        let candidate_sql = candidate_guarded_triple_sql();
        let (candidate_store, candidate_calls) = make_override_store(&session, &candidate_sql);
        let mut cases = Vec::new();

        for (case, query, expected) in [
            (
                "populated_no_match",
                "triple-fixture-no-match-query",
                &[][..],
            ),
            (
                "positive_match",
                "subject-match",
                &["fact:triple-fixture-0000"][..],
            ),
        ] {
            for warmup in 0..WARMUP_COUNT {
                measure_guard_pair(
                    &session.store,
                    &session.query_calls,
                    &candidate_store,
                    &candidate_calls,
                    query,
                    expected,
                    warmup % 2 == 1,
                )
                .await?;
            }

            let mut pairs = Vec::with_capacity(SAMPLE_COUNT);
            let mut expected_digest = None;
            let mut expected_count = None;
            for pair in 0..SAMPLE_COUNT {
                let (baseline, candidate, order) = measure_guard_pair(
                    &session.store,
                    &session.query_calls,
                    &candidate_store,
                    &candidate_calls,
                    query,
                    expected,
                    pair % 2 == 1,
                )
                .await?;
                if baseline.digest != candidate.digest || baseline.row_count != candidate.row_count
                {
                    return Err("candidate changed rows during the paired comparison".to_owned());
                }
                if expected_digest
                    .as_ref()
                    .is_some_and(|digest| digest != &baseline.digest)
                    || expected_count.is_some_and(|count| count != baseline.row_count)
                {
                    return Err("query result varied across paired samples".to_owned());
                }
                expected_digest.get_or_insert_with(|| baseline.digest.clone());
                expected_count.get_or_insert(baseline.row_count);
                pairs.push(GuardPairRecord {
                    order,
                    baseline_ns: baseline.elapsed_ns,
                    candidate_ns: candidate.elapsed_ns,
                });
            }

            let row_count = expected_count
                .ok_or_else(|| "paired comparison did not collect any rows".to_owned())?;
            let sha256 = expected_digest
                .ok_or_else(|| "paired comparison did not collect a row digest".to_owned())?;
            cases.push(GuardCaseRecord {
                case,
                row_count,
                sha256,
                pairs,
            });
        }

        let report = GuardComparisonReport {
            schema: "memory-triple-guard-ab/v1",
            server_version: docker.server_version,
            sdk_version: super::SDK_VERSION,
            image_id: config.image_id.clone(),
            storage_engine: super::STORAGE_ENGINE,
            host_architecture: docker.host_architecture,
            image_architecture: docker.image_architecture,
            emulation: docker.emulation,
            resource_mode: config.resource_mode.label(),
            effective_resources: docker.resources,
            fixture: FixtureMetadata {
                fact_count: super::FACT_COUNT,
                embedding_dimension: super::EMBEDDING_DIMENSION,
                triple_count: TRIPLE_COUNT,
                cutoff: oracle::FIXED_CUTOFF,
            },
            warmup_count: WARMUP_COUNT,
            pair_count: SAMPLE_COUNT,
            cases,
        };
        write_json_report(&config.report, &report)
    }
    .await;
    let cleanup = remove_database(&session).await;
    combine_result_and_cleanup(result, cleanup)
}

pub(super) async fn capture_remote_latency(config: &RunConfig) -> Result<(), String> {
    let docker = inspect_disposable_container(config)?;
    let mut samples = Vec::new();
    for case in SampleCase::ALL {
        let session = open_database(config, case.has_fixture()).await?;
        let result = capture_case(&session, case).await;
        let cleanup = remove_database(&session).await;
        samples.push(combine_result_and_cleanup(result, cleanup)?);
    }
    let report = LatencyReport {
        schema: "memory-triple-latency/v4",
        server_version: docker.server_version,
        sdk_version: super::SDK_VERSION,
        image_id: config.image_id.clone(),
        storage_engine: super::STORAGE_ENGINE,
        host_architecture: docker.host_architecture,
        image_architecture: docker.image_architecture,
        emulation: docker.emulation,
        resource_mode: config.resource_mode.label(),
        effective_resources: docker.resources,
        fixture: FixtureMetadata {
            fact_count: super::FACT_COUNT,
            embedding_dimension: super::EMBEDDING_DIMENSION,
            triple_count: super::fixture_triples().len(),
            cutoff: oracle::FIXED_CUTOFF,
        },
        connection_mode: "reused_connection",
        limit: SAMPLE_LIMIT,
        warmup_count: WARMUP_COUNT,
        sample_count: SAMPLE_COUNT,
        samples,
    };
    write_report(config, &report)
}

async fn capture_case(session: &DatabaseSession, case: SampleCase) -> Result<SampleRecord, String> {
    for _ in 0..WARMUP_COUNT {
        measure_query(
            session,
            case.query(),
            SAMPLE_LIMIT,
            case.expected_fact_ids(SAMPLE_LIMIT),
        )
        .await?;
    }
    let mut observations = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        observations.push(
            measure_query(
                session,
                case.query(),
                SAMPLE_LIMIT,
                case.expected_fact_ids(SAMPLE_LIMIT),
            )
            .await?,
        );
    }
    make_sample_record(case, observations)
}

fn inspect_disposable_container(config: &RunConfig) -> Result<DockerEvidence, String> {
    let endpoint = resolve_docker_endpoint()?;
    let tag_id = docker(
        &endpoint,
        &["image", "inspect", "--format", "{{.Id}}", super::IMAGE_TAG],
    )?;
    if tag_id.trim() != config.image_id {
        return Err("configured image ID does not match the local SurrealDB 3.3.5 tag".to_owned());
    }

    let publish_filter = format!("publish={}", config.port);
    let container_ids = docker(
        &endpoint,
        &[
            "ps",
            "--filter",
            &publish_filter,
            "--filter",
            "status=running",
            "--format",
            "{{.ID}}",
        ],
    )?;
    let container_ids = container_ids
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if container_ids.len() != 1 {
        return Err(
            "Docker must report exactly one running container publishing the configured port"
                .to_owned(),
        );
    }

    let container_json = docker(&endpoint, &["inspect", container_ids[0]])?;
    let inspections: Value = serde_json::from_str(&container_json)
        .map_err(|_| "Docker returned invalid container inspection JSON".to_owned())?;
    let [inspection] = inspections
        .as_array()
        .ok_or_else(|| "Docker inspection did not return an array".to_owned())?
        .as_slice()
    else {
        return Err("Docker inspection must contain exactly one container".to_owned());
    };
    let long_id = inspection
        .get("Id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Docker inspection omitted the container ID".to_owned())?;
    if !long_id.starts_with(container_ids[0]) {
        return Err(
            "Docker inspected a container different from the published-port match".to_owned(),
        );
    }
    let resources = inspect_container_details(inspection, config)
        .map_err(|error: ContainerEvidenceError| error.to_string())?;

    let image_metadata = docker(
        &endpoint,
        &[
            "image",
            "inspect",
            "--format",
            "{{.Os}}|{{.Architecture}}",
            &config.image_id,
        ],
    )?;
    let (image_os, image_architecture) = image_metadata
        .trim()
        .split_once('|')
        .ok_or_else(|| "Docker image inspection omitted OS or architecture".to_owned())?;
    if image_os != "linux" {
        return Err("the disposable SurrealDB image must target Linux".to_owned());
    }

    let host_metadata = docker(
        &endpoint,
        &["info", "--format", "{{.OSType}}|{{.Architecture}}"],
    )?;
    let (host_os, host_architecture) = host_metadata
        .trim()
        .split_once('|')
        .ok_or_else(|| "Docker info omitted host OS or architecture".to_owned())?;
    if host_os != image_os {
        return Err("Docker host and image operating systems do not match".to_owned());
    }
    let emulation =
        canonical_architecture(host_architecture) != canonical_architecture(image_architecture);

    Ok(DockerEvidence {
        server_version: format!("SurrealDB {}", super::SERVER_VERSION),
        resources,
        host_architecture: host_architecture.to_owned(),
        image_architecture: image_architecture.to_owned(),
        emulation,
    })
}

fn canonical_architecture(architecture: &str) -> &str {
    match architecture {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    }
}

fn resolve_docker_endpoint() -> Result<String, String> {
    let docker_host = std::env::var_os("DOCKER_HOST");
    let docker_context = std::env::var_os("DOCKER_CONTEXT");
    if docker_host.is_some() && docker_context.is_some() {
        return Err(
            "DOCKER_HOST and DOCKER_CONTEXT conflict; set only one Docker override".to_owned(),
        );
    }

    let docker_host = docker_host
        .map(|value| {
            value
                .into_string()
                .map_err(|_| "DOCKER_HOST must be valid UTF-8".to_owned())
        })
        .transpose()?;
    let docker_context = docker_context
        .map(|value| {
            value
                .into_string()
                .map_err(|_| "DOCKER_CONTEXT must be valid UTF-8".to_owned())
        })
        .transpose()?;

    if let Some(host) = docker_host {
        return docker_endpoint_from_host(host);
    }

    let mut args = Vec::with_capacity(7);
    if let Some(context) = docker_context.as_deref() {
        if context.is_empty() {
            return Err("DOCKER_CONTEXT must name a Docker context".to_owned());
        }
        args.extend(["--context", context]);
    }
    args.extend([
        "context",
        "inspect",
        "--format",
        "{{.Endpoints.docker.Host}}",
    ]);
    if let Some(context) = docker_context.as_deref() {
        args.push(context);
    }

    let host = run_bounded_command_with_env(
        Path::new("docker"),
        &args,
        DOCKER_COMMAND_TIMEOUT,
        DOCKER_OVERRIDE_ENV,
    )?;
    docker_endpoint_from_host(host.trim().to_owned())
}

fn docker_endpoint_from_host(host: String) -> Result<String, String> {
    host.strip_prefix("unix://")
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| "preflight requires a local Unix socket Docker endpoint".to_owned())?;
    Ok(host)
}

fn docker(endpoint: &str, args: &[&str]) -> Result<String, String> {
    let mut pinned_args = Vec::with_capacity(args.len() + 2);
    pinned_args.extend(["--host", endpoint]);
    pinned_args.extend_from_slice(args);
    run_bounded_command_with_env(
        Path::new("docker"),
        &pinned_args,
        DOCKER_COMMAND_TIMEOUT,
        DOCKER_OVERRIDE_ENV,
    )
}

fn run_bounded_command(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    run_bounded_command_with_env(executable, args, timeout, &[])
}

fn run_bounded_command_with_env(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
    env_to_remove: &[&str],
) -> Result<String, String> {
    let mut stdout = tempfile::tempfile()
        .map_err(|_| "external command output buffer could not be created".to_owned())?;
    let child_stdout = stdout
        .try_clone()
        .map_err(|_| "external command output buffer could not be duplicated".to_owned())?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::null());
    for name in env_to_remove {
        command.env_remove(name);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "external command could not be started".to_owned())?;

    let started_at = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started_at.elapsed() >= timeout => {
                terminate_external_command(&mut child);
                return Err("external command timed out".to_owned());
            }
            Ok(None) => {
                let remaining = timeout.saturating_sub(started_at.elapsed());
                thread::sleep(DOCKER_POLL_INTERVAL.min(remaining));
            }
            Err(_) => {
                terminate_external_command(&mut child);
                return Err("external command status could not be read".to_owned());
            }
        }
    };

    if !status.success() {
        return Err("external command failed".to_owned());
    }
    stdout
        .seek(SeekFrom::Start(0))
        .map_err(|_| "external command output could not be read".to_owned())?;
    let mut output = Vec::new();
    stdout
        .read_to_end(&mut output)
        .map_err(|_| "external command output could not be read".to_owned())?;
    String::from_utf8(output).map_err(|_| "external command returned non-UTF-8 output".to_owned())
}

fn terminate_external_command(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

async fn open_database(config: &RunConfig, seed_fixture: bool) -> Result<DatabaseSession, String> {
    let namespace = super::DATABASE_NAMESPACE_PREFIX.to_owned();
    let name = next_database_name()?;
    let client = connect_database(config, &namespace, &name).await?;
    let setup_client = Arc::clone(&client);
    let setup_namespace = namespace.clone();
    let setup = async move {
        setup_client.apply_migrations(&setup_namespace).await?;
        if seed_fixture {
            let db: Arc<dyn DbClient> = setup_client.clone();
            oracle::seed_triple_fixture(
                db.as_ref(),
                &setup_namespace,
                super::FACT_COUNT,
                super::EMBEDDING_DIMENSION,
            )
            .await?;
        }
        Ok::<(), MemoryError>(())
    };
    let setup_result = timeout(super::SETUP_TIMEOUT, setup).await;
    let setup_error = match setup_result {
        Ok(Ok(())) => None,
        Ok(Err(_)) => Some("database migration or fixture setup failed".to_owned()),
        Err(_) => Some("database migration or fixture setup exceeded 30 seconds".to_owned()),
    };
    if let Some(error) = setup_error {
        let cleanup = remove_database_parts(&client, &namespace, &name).await;
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(format!("{error}; cleanup also failed: {cleanup_error}")),
        };
    }

    Ok(make_database_session(name, namespace, client))
}

async fn connect_database(
    config: &RunConfig,
    namespace: &str,
    name: &str,
) -> Result<Arc<SurrealDbClient>, String> {
    let target = SurrealTargetConfig {
        url: config.url.clone(),
        username: "root".to_owned(),
        password: "root".to_owned(),
        database: name.to_owned(),
        namespace: namespace.to_owned(),
    };
    let client = match timeout(
        super::SETUP_TIMEOUT,
        SurrealDbClient::connect_bound_with_dimension(&target, "error", super::EMBEDDING_DIMENSION),
    )
    .await
    {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => {
            let stage = match &error {
                MemoryError::Storage(message)
                    if message.starts_with("SurrealDB connect failed:") =>
                {
                    "connect"
                }
                MemoryError::Storage(message)
                    if message.starts_with("SurrealDB signin failed:") =>
                {
                    "signin"
                }
                MemoryError::Storage(message) if message.starts_with("SurrealDB use_failed:") => {
                    "namespace_database_binding"
                }
                _ => "unknown",
            };
            return Err(format!("connection setup failed at stage {stage}"));
        }
        Err(_) => return Err("connection setup timed out".to_owned()),
    };
    Ok(Arc::new(client))
}

fn make_override_store(
    session: &DatabaseSession,
    query: &str,
) -> (KnowledgeStoreClient, Arc<AtomicUsize>) {
    let db: Arc<dyn DbClient> = session.client.clone();
    let adapter = oracle::SqlOverrideDbClient::new(db, Some(query));
    let query_calls = adapter.query_counter();
    let db: Arc<dyn DbClient> = Arc::new(adapter);
    (
        KnowledgeStoreClient::new(db, session.namespace.clone()),
        query_calls,
    )
}

fn validate_raw_fact_rows(rows: &Value, expected_fact_ids: &[&str]) -> Result<(), String> {
    let rows = rows
        .as_array()
        .ok_or_else(|| "raw triple query result was not an array".to_owned())?;
    validate_fact_rows(rows, expected_fact_ids)
}

fn validate_fact_rows(rows: &[Value], expected_fact_ids: &[&str]) -> Result<(), String> {
    let mut fact_ids = rows
        .iter()
        .map(|row| {
            row.get("fact_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "triple lookup returned a row without a fact ID".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if rows.iter().any(|row| row.get("embedding").is_some()) {
        return Err("triple projection unexpectedly returned embeddings".to_owned());
    }
    fact_ids.sort();
    let mut expected = expected_fact_ids
        .iter()
        .map(|fact_id| (*fact_id).to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    if fact_ids != expected {
        return Err("triple result differed from independently specified fixture IDs".to_owned());
    }
    Ok(())
}

fn make_database_session(
    name: String,
    namespace: String,
    client: Arc<SurrealDbClient>,
) -> DatabaseSession {
    let db: Arc<dyn DbClient> = client.clone();
    let adapter =
        oracle::SqlOverrideDbClient::new(db, Some(oracle::corrected_unguarded_triple_sql()));
    let query_calls = adapter.query_counter();
    let db: Arc<dyn DbClient> = Arc::new(adapter);
    let store = KnowledgeStoreClient::new(db, namespace.clone());
    DatabaseSession {
        name,
        namespace,
        client,
        store,
        query_calls,
    }
}

fn next_database_name() -> Result<String, String> {
    static NEXT_DATABASE: AtomicUsize = AtomicUsize::new(1);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_owned())?
        .as_nanos();
    let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
    Ok(format!(
        "{}_{}_{}",
        super::DATABASE_NAMESPACE_PREFIX,
        timestamp,
        sequence
    ))
}

async fn remove_database(session: &DatabaseSession) -> Result<(), String> {
    remove_database_parts(&session.client, &session.namespace, &session.name).await
}

async fn remove_database_parts(
    client: &SurrealDbClient,
    namespace: &str,
    database: &str,
) -> Result<(), String> {
    let sql = format!("REMOVE DATABASE {database}");
    timeout(super::SETUP_TIMEOUT, client.query(&sql, None, namespace))
        .await
        .map_err(|_| "database cleanup exceeded 30 seconds".to_owned())?
        .map_err(|error| format!("could not remove generated database: {error}"))?;
    Ok(())
}

fn combine_result_and_cleanup<T>(
    result: Result<T, String>,
    cleanup: Result<(), String>,
) -> Result<T, String> {
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
        (Err(error), Err(cleanup_error)) => {
            Err(format!("{error}; cleanup also failed: {cleanup_error}"))
        }
    }
}

async fn measure_query(
    session: &DatabaseSession,
    query: &str,
    limit: usize,
    expected_fact_ids: &[&str],
) -> Result<Observation, String> {
    measure_store_query(
        &session.store,
        &session.query_calls,
        query,
        limit,
        expected_fact_ids,
    )
    .await
}

async fn measure_store_query(
    store: &KnowledgeStoreClient,
    query_calls: &AtomicUsize,
    query: &str,
    limit: usize,
    expected_fact_ids: &[&str],
) -> Result<Observation, String> {
    let before = query_calls.load(Ordering::Relaxed);
    let started = Instant::now();
    let query_result = timeout(
        super::QUERY_TIMEOUT,
        store.select_facts_by_triple(query, oracle::FIXED_CUTOFF, limit),
    )
    .await;
    let elapsed_ns = started.elapsed().as_nanos();
    let rows = query_result
        .map_err(|_| "triple query exceeded 10 seconds".to_owned())?
        .map_err(|_| "public triple lookup failed".to_owned())?;
    let query_call_count = query_calls.load(Ordering::Relaxed) - before;
    if query_call_count != 1 {
        return Err(format!(
            "one public triple lookup issued {query_call_count} SQL calls"
        ));
    }

    validate_fact_rows(&rows, expected_fact_ids)?;

    Ok(Observation {
        elapsed_ns,
        row_count: rows.len(),
        digest: row_digest(&rows),
    })
}

async fn measure_guard_pair(
    baseline_store: &KnowledgeStoreClient,
    baseline_calls: &AtomicUsize,
    candidate_store: &KnowledgeStoreClient,
    candidate_calls: &AtomicUsize,
    query: &str,
    expected_fact_ids: &[&str],
    candidate_first: bool,
) -> Result<(Observation, Observation, &'static str), String> {
    let (baseline, candidate, order) = if candidate_first {
        let candidate = measure_store_query(
            candidate_store,
            candidate_calls,
            query,
            SAMPLE_LIMIT,
            expected_fact_ids,
        )
        .await?;
        let baseline = measure_store_query(
            baseline_store,
            baseline_calls,
            query,
            SAMPLE_LIMIT,
            expected_fact_ids,
        )
        .await?;
        (baseline, candidate, "BA")
    } else {
        let baseline = measure_store_query(
            baseline_store,
            baseline_calls,
            query,
            SAMPLE_LIMIT,
            expected_fact_ids,
        )
        .await?;
        let candidate = measure_store_query(
            candidate_store,
            candidate_calls,
            query,
            SAMPLE_LIMIT,
            expected_fact_ids,
        )
        .await?;
        (baseline, candidate, "AB")
    };
    if baseline.row_count != candidate.row_count || baseline.digest != candidate.digest {
        return Err("candidate changed rows during the paired comparison".to_owned());
    }
    Ok((baseline, candidate, order))
}

fn make_sample_record(
    case: SampleCase,
    observations: Vec<Observation>,
) -> Result<SampleRecord, String> {
    if observations.len() != SAMPLE_COUNT {
        return Err("descriptive capture requires exactly 20 timed samples".to_owned());
    }
    let first = &observations[0];
    if observations.iter().any(|observation| {
        observation.row_count != first.row_count || observation.digest != first.digest
    }) {
        return Err("timed query result rows varied across samples".to_owned());
    }
    let elapsed_ns = observations
        .iter()
        .map(|observation| observation.elapsed_ns)
        .collect::<Vec<_>>();
    let mut sorted = elapsed_ns.clone();
    sorted.sort_unstable();
    let lower = sorted[SAMPLE_COUNT / 2 - 1];
    let upper = sorted[SAMPLE_COUNT / 2];
    Ok(SampleRecord {
        case: case.label(),
        min_ns: sorted[0],
        median_ns: lower + (upper - lower) / 2,
        max_ns: sorted[SAMPLE_COUNT - 1],
        elapsed_ns,
        row_count: first.row_count,
        sha256: first.digest.clone(),
        query_call_count: observations.len(),
    })
}

fn write_report(config: &RunConfig, report: &LatencyReport) -> Result<(), String> {
    write_json_report(&config.report, report)
}

fn write_json_report<T: Serialize>(path: &Path, report: &T) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(report)
        .map_err(|_| "experiment report serialization failed".to_owned())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| {
            "report path could not be created without overwriting an existing file".to_owned()
        })?;
    if file.write_all(&encoded).is_err() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err("experiment report could not be written".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptive_samples_preserve_raw_order_and_report_median_and_range() {
        let observations = (1..=20)
            .rev()
            .map(|index| Observation {
                elapsed_ns: index * 10,
                row_count: 1,
                digest: "same-rows".to_owned(),
            })
            .collect();
        let record = make_sample_record(SampleCase::PositiveMatch, observations)
            .expect("complete descriptive sample");
        let encoded = serde_json::to_value(record).expect("serialize samples");
        assert_eq!(encoded["min_ns"], 10);
        assert_eq!(encoded["median_ns"], 105);
        assert_eq!(encoded["max_ns"], 200);
        assert_eq!(encoded["elapsed_ns"][0], 200);
        assert_eq!(encoded["elapsed_ns"][19], 10);
        assert_eq!(encoded["query_call_count"], 20);
    }

    #[test]
    fn descriptive_samples_reject_an_incomplete_capture() {
        let observations = vec![
            Observation {
                elapsed_ns: 1000,
                row_count: 0,
                digest: "empty".to_owned()
            };
            19
        ];
        assert!(make_sample_record(SampleCase::EmptyTable, observations).is_err());
    }

    #[test]
    fn descriptive_samples_reject_changed_result_rows() {
        let mut observations = vec![
            Observation {
                elapsed_ns: 1000,
                row_count: 1,
                digest: "first".to_owned()
            };
            20
        ];
        observations[19].digest = "different".to_owned();
        assert!(make_sample_record(SampleCase::PositiveMatch, observations).is_err());
    }

    #[test]
    fn descriptive_samples_keep_large_outliers_without_failing_the_capture() {
        let mut observations = vec![
            Observation {
                elapsed_ns: 1000,
                row_count: 0,
                digest: "empty".to_owned()
            };
            20
        ];
        observations[0].elapsed_ns = 1_000_000;
        let record = make_sample_record(SampleCase::EmptyTable, observations)
            .expect("outliers are descriptive, not a failure");
        let encoded = serde_json::to_value(record).expect("serialize samples");
        assert_eq!(encoded["median_ns"], 1000);
        assert_eq!(encoded["max_ns"], 1_000_000);
        assert_eq!(encoded["elapsed_ns"][0], 1_000_000);
    }

    #[cfg(unix)]
    #[test]
    fn docker_remote_host_override_is_rejected_before_any_docker_command() {
        if run_docker_endpoint_test_child("remote-host") {
            return;
        }

        spawn_docker_endpoint_test_child(
            "remote-host",
            "experiment::tests::docker_remote_host_override_is_rejected_before_any_docker_command",
            Some("tcp://remote.example:2376"),
            None,
        );
    }

    #[cfg(unix)]
    #[test]
    fn conflicting_docker_host_and_context_overrides_are_rejected_before_cli_use() {
        if run_docker_endpoint_test_child("conflicting-overrides") {
            return;
        }

        spawn_docker_endpoint_test_child(
            "conflicting-overrides",
            "experiment::tests::conflicting_docker_host_and_context_overrides_are_rejected_before_cli_use",
            Some("unix:///tmp/docker-host.sock"),
            Some("local"),
        );
    }

    #[cfg(unix)]
    #[test]
    fn docker_host_override_is_pinned_and_removed_from_child_environment() {
        if run_docker_endpoint_test_child("pinned-host") {
            return;
        }

        spawn_docker_endpoint_test_child(
            "pinned-host",
            "experiment::tests::docker_host_override_is_pinned_and_removed_from_child_environment",
            Some("unix:///tmp/resolved-docker.sock"),
            None,
        );
    }

    #[cfg(unix)]
    #[test]
    fn docker_context_is_resolved_before_inspection_and_then_pinned() {
        if run_docker_endpoint_test_child("pinned-context") {
            return;
        }

        spawn_docker_endpoint_test_child(
            "pinned-context",
            "experiment::tests::docker_context_is_resolved_before_inspection_and_then_pinned",
            None,
            Some("local"),
        );
    }

    #[cfg(unix)]
    #[test]
    fn docker_remote_context_is_rejected_before_daemon_inspection() {
        if run_docker_endpoint_test_child("remote-context") {
            return;
        }

        spawn_docker_endpoint_test_child(
            "remote-context",
            "experiment::tests::docker_remote_context_is_rejected_before_daemon_inspection",
            None,
            Some("remote"),
        );
    }

    #[cfg(unix)]
    fn run_docker_endpoint_test_child(mode: &str) -> bool {
        const CHILD_MODE: &str = "MEMORY_TRIPLE_DOCKER_TEST_CHILD";
        let Ok(child_mode) = std::env::var(CHILD_MODE) else {
            return false;
        };
        assert_eq!(child_mode, mode);

        let config = crate::parse_config(&crate::valid_values()).expect("valid test configuration");
        let result = inspect_disposable_container(&config);
        let log_path =
            std::env::var("MEMORY_TRIPLE_DOCKER_TEST_LOG").expect("child Docker log path");

        match mode {
            "remote-host" => {
                let error = result.expect_err("remote Docker hosts must be rejected");
                assert!(
                    error.contains("local Unix socket"),
                    "unexpected error: {error}"
                );
                assert!(!Path::new(&log_path).exists(), "Docker CLI must not run");
            }
            "conflicting-overrides" => {
                let error = result.expect_err("conflicting Docker overrides must be rejected");
                assert!(error.contains("conflict"), "unexpected error: {error}");
                assert!(!Path::new(&log_path).exists(), "Docker CLI must not run");
            }
            "pinned-host" => {
                assert!(result.is_err(), "the fake Docker CLI should stop preflight");
                let log = std::fs::read_to_string(log_path).expect("Docker CLI invocation log");
                assert!(
                    log.contains(
                        "host=;context=;args=--host unix:///tmp/resolved-docker.sock image inspect"
                    ),
                    "Docker CLI was not pinned to the resolved host: {log}"
                );
            }
            "pinned-context" => {
                assert!(result.is_err(), "the fake Docker CLI should stop preflight");
                let log = std::fs::read_to_string(log_path).expect("Docker CLI invocation log");
                assert!(
                    log.contains(
                        "--context local context inspect --format {{.Endpoints.docker.Host}} local"
                    ),
                    "selected context was not resolved explicitly: {log}"
                );
                assert!(
                    log.contains(
                        "host=;context=;args=--host unix:///tmp/context.sock image inspect"
                    ),
                    "Docker CLI was not pinned to the resolved context endpoint: {log}"
                );
            }
            "remote-context" => {
                let error = result.expect_err("remote Docker contexts must be rejected");
                assert!(
                    error.contains("local Unix socket"),
                    "unexpected error: {error}"
                );
                let log = std::fs::read_to_string(log_path).expect("context inspection log");
                assert!(log.contains("--context remote context inspect"), "{log}");
                assert!(
                    !log.contains("image inspect"),
                    "daemon inspection must not run: {log}"
                );
            }
            _ => panic!("unexpected child mode: {mode}"),
        }
        true
    }

    #[cfg(unix)]
    fn spawn_docker_endpoint_test_child(
        mode: &str,
        test_name: &str,
        docker_host: Option<&str>,
        docker_context: Option<&str>,
    ) {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_MODE: &str = "MEMORY_TRIPLE_DOCKER_TEST_CHILD";
        const DOCKER_LOG: &str = "MEMORY_TRIPLE_DOCKER_TEST_LOG";
        let directory = tempfile::tempdir().expect("temporary Docker test directory");
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).expect("create fake Docker bin directory");
        let fake_docker = bin.join("docker");
        std::fs::write(
            &fake_docker,
            r#"#!/bin/sh
printf 'host=%s;context=%s;args=%s\n' "${DOCKER_HOST-}" "${DOCKER_CONTEXT-}" "$*" >> "$MEMORY_TRIPLE_DOCKER_TEST_LOG"
case "$*" in
  "--context remote context inspect --format {{.Endpoints.docker.Host}} remote") printf '%s\n' 'tcp://remote.example:2376'; exit 0 ;;
  "--context local context inspect --format {{.Endpoints.docker.Host}} local") printf '%s\n' 'unix:///tmp/context.sock'; exit 0 ;;
esac
exit 1
"#,
        )
        .expect("write fake Docker executable");
        let mut permissions = std::fs::metadata(&fake_docker)
            .expect("fake Docker metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_docker, permissions)
            .expect("make fake Docker executable runnable");

        let log_path = directory.path().join("docker.log");
        let mut command = Command::new(std::env::current_exe().expect("test executable path"));
        command
            .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
            .env(CHILD_MODE, mode)
            .env(DOCKER_LOG, &log_path)
            .env("PATH", &bin)
            .env_remove("DOCKER_HOST")
            .env_remove("DOCKER_CONTEXT");
        if let Some(docker_host) = docker_host {
            command.env("DOCKER_HOST", docker_host);
        }
        if let Some(docker_context) = docker_context {
            command.env("DOCKER_CONTEXT", docker_context);
        }

        let output = command.output().expect("run isolated Docker test child");
        assert!(
            output.status.success(),
            "child test failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[cfg(unix)]
    #[test]
    fn docker_cli_times_out_and_stops_the_child_process() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary Docker directory");
        let fake_docker = directory.path().join("docker");
        std::fs::write(
            &fake_docker,
            "#!/bin/sh\nprintf '%s' \"$$\" > \"${0}.pid\"\nexec sleep 10\n",
        )
        .expect("write fake Docker executable");
        let mut permissions = std::fs::metadata(&fake_docker)
            .expect("fake Docker metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_docker, permissions)
            .expect("make fake Docker executable runnable");

        let started_at = Instant::now();
        let error = run_bounded_command(&fake_docker, &[], Duration::from_millis(500))
            .expect_err("a long-running Docker CLI must time out");
        assert!(error.contains("timed out"), "unexpected error: {error}");
        assert!(started_at.elapsed() < Duration::from_secs(3));

        let process_id = std::fs::read_to_string(fake_docker.with_extension("pid"))
            .expect("fake Docker process ID")
            .trim()
            .to_owned();
        let still_running = Command::new("/bin/kill")
            .args(["-0", &process_id])
            .stderr(Stdio::null())
            .status()
            .expect("check fake Docker process state");
        assert!(!still_running.success(), "timed-out child is still running");
    }

    fn sample_report() -> LatencyReport {
        let sample = make_sample_record(
            SampleCase::EmptyTable,
            vec![
                Observation {
                    elapsed_ns: 100,
                    row_count: 0,
                    digest: "empty-digest".to_owned()
                };
                20
            ],
        )
        .expect("complete samples");
        LatencyReport {
            schema: "memory-triple-latency/v4",
            server_version: "SurrealDB 3.3.5".to_owned(),
            sdk_version: "3.3.5",
            image_id: "sha256:test".to_owned(),
            storage_engine: "RocksDB",
            host_architecture: "aarch64".to_owned(),
            image_architecture: "arm64".to_owned(),
            emulation: false,
            resource_mode: "memory-512m",
            effective_resources: EffectiveResources {
                memory_limit_bytes: 536_870_912,
                memory_swap_limit_bytes: 536_870_912,
                cpu_quota_micros: 0,
                cpu_period_micros: 0,
            },
            fixture: FixtureMetadata {
                fact_count: 2191,
                embedding_dimension: 2048,
                triple_count: 5,
                cutoff: "2026-11-13T15:52:35.269Z",
            },
            connection_mode: "reused_connection",
            limit: 40,
            warmup_count: 5,
            sample_count: 20,
            samples: vec![sample],
        }
    }

    #[test]
    fn latency_report_serializes_only_allowlisted_fields() {
        let encoded = serde_json::to_value(sample_report()).expect("serialize report");
        assert_eq!(encoded["schema"], "memory-triple-latency/v4");
        assert_eq!(encoded["connection_mode"], "reused_connection");
        assert_eq!(encoded["sample_count"], 20);
        assert_eq!(
            sorted_object_keys(&encoded),
            vec![
                "connection_mode",
                "effective_resources",
                "emulation",
                "fixture",
                "host_architecture",
                "image_architecture",
                "image_id",
                "limit",
                "resource_mode",
                "sample_count",
                "samples",
                "schema",
                "sdk_version",
                "server_version",
                "storage_engine",
                "warmup_count"
            ]
        );
        assert_eq!(
            sorted_object_keys(&encoded["effective_resources"]),
            vec![
                "cpu_period_micros",
                "cpu_quota_micros",
                "memory_limit_bytes",
                "memory_swap_limit_bytes"
            ]
        );
        assert_eq!(
            sorted_object_keys(&encoded["fixture"]),
            vec![
                "cutoff",
                "embedding_dimension",
                "fact_count",
                "triple_count"
            ]
        );
        assert_eq!(
            sorted_object_keys(&encoded["samples"][0]),
            vec![
                "case",
                "elapsed_ns",
                "max_ns",
                "median_ns",
                "min_ns",
                "query_call_count",
                "row_count",
                "sha256"
            ]
        );
    }

    #[test]
    fn latency_report_does_not_overwrite_an_existing_artifact() {
        let directory = tempfile::tempdir().expect("temporary report directory");
        let path = directory.path().join("report.json");
        std::fs::write(&path, "previous evidence").expect("existing artifact");
        let mut config = crate::parse_config(&crate::valid_values()).expect("valid configuration");
        config.report = path.clone();
        assert!(write_report(&config, &sample_report()).is_err());
        assert_eq!(
            std::fs::read_to_string(path).expect("preserved evidence"),
            "previous evidence"
        );
    }

    fn sorted_object_keys(value: &Value) -> Vec<String> {
        let mut keys = value
            .as_object()
            .expect("report fields should serialize as JSON objects")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }
}
