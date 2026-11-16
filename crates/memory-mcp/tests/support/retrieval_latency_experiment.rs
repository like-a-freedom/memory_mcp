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
    ContainerEvidenceError, EffectiveResources, ResourceMode, RunConfig, inspect_container_details,
    row_digest,
};

const DOCKER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const DOCKER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(60);
const QUIESCENCE_MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
const QUIESCENCE_STABLE_INTERVALS: usize = 5;
const QUIESCENCE_MAX_CPU_PERCENT: u128 = 1;
const QUIESCENCE_MAX_IO_BYTES_PER_SECOND: u128 = 1024 * 1024;

fn unix_timestamp_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn emit_diagnostic_event(phase: &str, details: &str) {
    eprintln!(
        "memory_triple_latency_diagnostic ts_unix_ns={} phase={phase} {details}",
        unix_timestamp_ns()
    );
}

fn emit_query_timing_event(
    started_at_unix_ns: u128,
    ended_at_unix_ns: u128,
    elapsed_ns: u128,
    context: &str,
) {
    eprintln!(
        "memory_triple_latency_diagnostic ts_start_unix_ns={started_at_unix_ns} ts_end_unix_ns={ended_at_unix_ns} elapsed_ns={elapsed_ns} phase=query {context}"
    );
}

#[derive(Debug)]
struct DockerEvidence {
    server_version: String,
    container_id: String,
    docker_socket_path: String,
    resources: EffectiveResources,
    host_architecture: String,
    image_architecture: String,
    emulation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContainerActivity {
    cpu_time_ns: u64,
    read_bytes: u64,
    write_bytes: u64,
}

fn parse_container_activity(stats: &Value) -> Result<ContainerActivity, String> {
    let cpu_time_ns = stats
        .pointer("/cpu_stats/cpu_usage/total_usage")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Docker stats omitted the container CPU counter".to_owned())?;
    let io_entries = stats
        .pointer("/blkio_stats/io_service_bytes_recursive")
        .and_then(Value::as_array)
        .ok_or_else(|| "Docker stats omitted recursive block I/O counters".to_owned())?;

    let mut activity = ContainerActivity {
        cpu_time_ns,
        read_bytes: 0,
        write_bytes: 0,
    };
    let mut found_io_counter = false;
    for entry in io_entries {
        let operation = entry
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| "Docker block I/O counter omitted its operation".to_owned())?;
        let value = entry
            .get("value")
            .and_then(Value::as_u64)
            .ok_or_else(|| "Docker block I/O counter omitted its byte value".to_owned())?;
        let counter = if operation.eq_ignore_ascii_case("read") {
            &mut activity.read_bytes
        } else if operation.eq_ignore_ascii_case("write") {
            &mut activity.write_bytes
        } else {
            continue;
        };
        *counter = counter
            .checked_add(value)
            .ok_or_else(|| "Docker block I/O counter overflowed".to_owned())?;
        found_io_counter = true;
    }
    if !found_io_counter {
        return Err("Docker stats contained no read or write counters".to_owned());
    }
    Ok(activity)
}

fn container_activity_is_quiet(
    previous: ContainerActivity,
    current: ContainerActivity,
    elapsed: Duration,
) -> bool {
    let elapsed_ns = elapsed.as_nanos();
    let Some(cpu_delta_ns) = current.cpu_time_ns.checked_sub(previous.cpu_time_ns) else {
        return false;
    };
    let Some(read_delta_bytes) = current.read_bytes.checked_sub(previous.read_bytes) else {
        return false;
    };
    let Some(write_delta_bytes) = current.write_bytes.checked_sub(previous.write_bytes) else {
        return false;
    };
    let io_delta_bytes = u128::from(read_delta_bytes) + u128::from(write_delta_bytes);

    elapsed_ns >= Duration::from_millis(500).as_nanos()
        && u128::from(cpu_delta_ns) * 100 <= elapsed_ns * QUIESCENCE_MAX_CPU_PERCENT
        && io_delta_bytes * 1_000_000_000 <= elapsed_ns * QUIESCENCE_MAX_IO_BYTES_PER_SECOND
}

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

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ConnectionMode {
    NewConnection,
    ReusedConnection,
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
struct CalibrationRecord {
    case: &'static str,
    limit: usize,
    resource_mode: &'static str,
    connection_mode: ConnectionMode,
    sample_pairs_ns: Vec<[u128; 2]>,
    max_noise_fraction: Option<f64>,
}

#[derive(Serialize)]
struct SampleRecord {
    case: &'static str,
    limit: usize,
    resource_mode: &'static str,
    connection_mode: ConnectionMode,
    elapsed_ns: Vec<u128>,
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
    measurement_resolution_ns: u128,
    calibration: Vec<CalibrationRecord>,
    samples: Vec<SampleRecord>,
}

pub(super) async fn verify_remote_triple_compatibility(config: &RunConfig) -> Result<(), String> {
    let docker = inspect_disposable_container(config)?;
    emit_diagnostic_event("fixture_setup_start", "case=compatibility");
    let session = open_database(config, true).await?;
    emit_diagnostic_event("fixture_setup_end", "case=compatibility");
    let result = async {
        wait_for_container_quiescence(&docker, "compatibility")?;
        for (query_index, (query, limit, expected)) in [
            ("subject-match", 40, &["fact:triple-fixture-0000"][..]),
            ("predicate-match", 40, &["fact:triple-fixture-0001"][..]),
            ("object-match", 40, &["fact:triple-fixture-0002"][..]),
            ("duplicate-match", 40, &["fact:triple-fixture-0000"][..]),
            ("quote 'snow' 日本語", 40, &[][..]),
            ("triple-fixture-no-match-query", 40, &[][..]),
            ("subject-match", 0, &[][..]),
        ]
        .into_iter()
        .enumerate()
        {
            let context = format!(
                "case=compatibility limit={limit} mode=compatibility query_index={query_index}"
            );
            let observation = measure_query(&session, query, limit, expected, &context).await?;
            if observation.row_count != expected.len() {
                return Err(format!(
                    "remote compatibility case {query:?} returned an unexpected row count"
                ));
            }
        }
        Ok(())
    }
    .await;
    emit_diagnostic_event("cleanup_start", "case=compatibility");
    let cleanup = remove_database(&session).await;
    emit_diagnostic_event("cleanup_end", "case=compatibility");
    combine_result_and_cleanup(result, cleanup)
}

pub(super) async fn capture_remote_latency(config: &RunConfig) -> Result<(), String> {
    let docker = inspect_disposable_container(config)?;
    let server_version = docker.server_version.clone();

    let mut samples = Vec::new();
    let mut calibration = Vec::new();
    for case in SampleCase::ALL {
        emit_diagnostic_event("fixture_setup_start", &format!("case={}", case.label()));
        let session = open_database(config, case.has_fixture()).await?;
        emit_diagnostic_event("fixture_setup_end", &format!("case={}", case.label()));
        let capture_result = async {
            wait_for_container_quiescence(&docker, case.label())?;
            for limit in [0, 1, 3, 40] {
                let (sample_record, calibration_record) =
                    capture_new_connection_condition(config, &session, case, limit).await?;
                samples.push(sample_record);
                calibration.push(calibration_record);
            }

            for limit in [0, 1, 3, 40] {
                let (sample_record, calibration_record) =
                    capture_reused_connection_condition(config, &session, case, limit).await?;
                samples.push(sample_record);
                calibration.push(calibration_record);
            }
            Ok(())
        }
        .await;
        emit_diagnostic_event("cleanup_start", &format!("case={}", case.label()));
        let cleanup = remove_database(&session).await;
        emit_diagnostic_event("cleanup_end", &format!("case={}", case.label()));
        combine_result_and_cleanup(capture_result, cleanup)?;
    }

    let report = LatencyReport {
        schema: "memory-triple-latency/v2",
        server_version,
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
        measurement_resolution_ns: measurement_resolution_ns(),
        calibration,
        samples,
    };
    write_report(config, &report)?;

    let calibration_is_valid = report
        .calibration
        .iter()
        .all(|record| record.max_noise_fraction.is_some_and(|noise| noise <= 0.03));
    if !calibration_is_valid {
        return Err("latency report was written, but at least one A/A calibration was invalid or exceeded 3% noise".to_owned());
    }

    Ok(())
}

fn inspect_disposable_container(config: &RunConfig) -> Result<DockerEvidence, String> {
    let tag_id = docker(&["image", "inspect", "--format", "{{.Id}}", super::IMAGE_TAG])?;
    if tag_id.trim() != config.image_id {
        return Err("configured image ID does not match the local SurrealDB 3.3.5 tag".to_owned());
    }

    let publish_filter = format!("publish={}", config.port);
    let container_ids = docker(&[
        "ps",
        "--filter",
        &publish_filter,
        "--filter",
        "status=running",
        "--format",
        "{{.ID}}",
    ])?;
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

    let container_json = docker(&["inspect", container_ids[0]])?;
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

    let image_metadata = docker(&[
        "image",
        "inspect",
        "--format",
        "{{.Os}}|{{.Architecture}}",
        &config.image_id,
    ])?;
    let (image_os, image_architecture) = image_metadata
        .trim()
        .split_once('|')
        .ok_or_else(|| "Docker image inspection omitted OS or architecture".to_owned())?;
    if image_os != "linux" {
        return Err("the disposable SurrealDB image must target Linux".to_owned());
    }

    let host_metadata = docker(&["info", "--format", "{{.OSType}}|{{.Architecture}}"])?;
    let (host_os, host_architecture) = host_metadata
        .trim()
        .split_once('|')
        .ok_or_else(|| "Docker info omitted host OS or architecture".to_owned())?;
    if host_os != image_os {
        return Err("Docker host and image operating systems do not match".to_owned());
    }
    let emulation =
        canonical_architecture(host_architecture) != canonical_architecture(image_architecture);
    let docker_endpoint = docker(&[
        "context",
        "inspect",
        "--format",
        "{{.Endpoints.docker.Host}}",
    ])?;
    let docker_socket_path = docker_endpoint
        .trim()
        .strip_prefix("unix://")
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "exact container stats require a local Unix Docker socket".to_owned())?
        .to_owned();

    Ok(DockerEvidence {
        server_version: format!("SurrealDB {}", super::SERVER_VERSION),
        container_id: long_id.to_owned(),
        docker_socket_path,
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

fn docker(args: &[&str]) -> Result<String, String> {
    run_bounded_command(Path::new("docker"), args, DOCKER_COMMAND_TIMEOUT)
}

fn read_container_activity(
    docker_socket_path: &str,
    container_id: &str,
) -> Result<ContainerActivity, String> {
    let stats_url = format!("http://localhost/v1.41/containers/{container_id}/stats?stream=false");
    let args = [
        "--silent",
        "--show-error",
        "--fail",
        "--max-time",
        "8",
        "--unix-socket",
        docker_socket_path,
        stats_url.as_str(),
    ];
    let response = run_bounded_command(Path::new("curl"), &args, DOCKER_COMMAND_TIMEOUT)?;
    let stats: Value = serde_json::from_str(&response)
        .map_err(|_| "Docker Engine returned invalid container stats JSON".to_owned())?;
    parse_container_activity(&stats)
}

fn wait_for_container_quiescence(docker: &DockerEvidence, case: &str) -> Result<(), String> {
    emit_diagnostic_event(
        "quiescence_start",
        &format!(
            "case={case} stable_intervals={QUIESCENCE_STABLE_INTERVALS} max_cpu_percent={QUIESCENCE_MAX_CPU_PERCENT} max_io_bytes_per_second={QUIESCENCE_MAX_IO_BYTES_PER_SECOND} timeout_seconds={}",
            QUIESCENCE_TIMEOUT.as_secs()
        ),
    );

    let started_at = Instant::now();
    let mut previous: Option<(ContainerActivity, Instant)> = None;
    let mut stable_intervals = 0;
    loop {
        if started_at.elapsed() >= QUIESCENCE_TIMEOUT {
            return Err(format!(
                "SurrealDB container did not remain below the quiescence thresholds for {QUIESCENCE_STABLE_INTERVALS} intervals within {} seconds",
                QUIESCENCE_TIMEOUT.as_secs()
            ));
        }

        let request_started = Instant::now();
        let current = read_container_activity(&docker.docker_socket_path, &docker.container_id)?;
        let sampled_at = Instant::now();
        if let Some((prior, prior_sampled_at)) = previous {
            let elapsed = sampled_at.duration_since(prior_sampled_at);
            let quiet = container_activity_is_quiet(prior, current, elapsed);
            stable_intervals = if quiet { stable_intervals + 1 } else { 0 };
            emit_diagnostic_event(
                "quiescence_sample",
                &format!(
                    "case={case} interval_ns={} cpu_delta_ns={} read_delta_bytes={} write_delta_bytes={} quiet={quiet} stable_intervals={stable_intervals}",
                    elapsed.as_nanos(),
                    current.cpu_time_ns.saturating_sub(prior.cpu_time_ns),
                    current.read_bytes.saturating_sub(prior.read_bytes),
                    current.write_bytes.saturating_sub(prior.write_bytes)
                ),
            );
            if stable_intervals >= QUIESCENCE_STABLE_INTERVALS {
                emit_diagnostic_event(
                    "quiescence_end",
                    &format!("case={case} stable_intervals={stable_intervals}"),
                );
                return Ok(());
            }
        } else {
            emit_diagnostic_event(
                "quiescence_sample",
                &format!("case={case} interval_ns=0 quiet=false stable_intervals=0"),
            );
        }
        previous = Some((current, sampled_at));

        let remaining_interval =
            QUIESCENCE_MIN_SAMPLE_INTERVAL.saturating_sub(request_started.elapsed());
        thread::sleep(remaining_interval);
    }
}

fn run_bounded_command(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut stdout = tempfile::tempfile()
        .map_err(|_| "external command output buffer could not be created".to_owned())?;
    let child_stdout = stdout
        .try_clone()
        .map_err(|_| "external command output buffer could not be duplicated".to_owned())?;
    let mut child = Command::new(executable)
        .args(args)
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::null())
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

async fn reconnect_database(
    config: &RunConfig,
    database: &DatabaseSession,
) -> Result<DatabaseSession, String> {
    let client = connect_database(config, &database.namespace, &database.name).await?;
    Ok(make_database_session(
        database.name.clone(),
        database.namespace.clone(),
        client,
    ))
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
    context: &str,
) -> Result<Observation, String> {
    let before = session.query_calls.load(Ordering::Relaxed);
    let started_at_unix_ns = unix_timestamp_ns();
    let started = Instant::now();
    let query_result = timeout(
        super::QUERY_TIMEOUT,
        session
            .store
            .select_facts_by_triple(query, oracle::FIXED_CUTOFF, limit),
    )
    .await;
    let elapsed_ns = started.elapsed().as_nanos();
    let ended_at_unix_ns = unix_timestamp_ns();
    emit_query_timing_event(started_at_unix_ns, ended_at_unix_ns, elapsed_ns, context);
    let rows = query_result
        .map_err(|_| "triple query exceeded 10 seconds".to_owned())?
        .map_err(|_| "public triple lookup failed".to_owned())?;
    let query_call_count = session.query_calls.load(Ordering::Relaxed) - before;
    if query_call_count != 1 {
        return Err(format!(
            "one public triple lookup issued {query_call_count} SQL calls"
        ));
    }

    let mut fact_ids = rows
        .iter()
        .map(|row| {
            row.get("fact_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "triple lookup returned a row without a fact ID".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    fact_ids.sort();
    let mut expected = expected_fact_ids
        .iter()
        .map(|fact_id| (*fact_id).to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    if fact_ids != expected {
        return Err(
            "remote triple result did not match the independently specified fixture IDs".to_owned(),
        );
    }
    if rows.iter().any(|row| row.get("embedding").is_some()) {
        return Err("triple projection unexpectedly returned embeddings".to_owned());
    }

    Ok(Observation {
        elapsed_ns,
        row_count: rows.len(),
        digest: row_digest(&rows),
    })
}

async fn measure_new_connection_query(
    config: &RunConfig,
    database: &DatabaseSession,
    case: SampleCase,
    limit: usize,
    context: &str,
) -> Result<Observation, String> {
    let session = reconnect_database(config, database).await?;
    measure_query(
        &session,
        case.query(),
        limit,
        case.expected_fact_ids(limit),
        context,
    )
    .await
}

async fn capture_new_connection_condition(
    config: &RunConfig,
    database: &DatabaseSession,
    case: SampleCase,
    limit: usize,
) -> Result<(SampleRecord, CalibrationRecord), String> {
    let mut pairs = Vec::with_capacity(5);
    for pair_index in 0..5 {
        let first_context = format!(
            "case={} limit={limit} mode=new_connection stage=calibration pair={pair_index} side=a",
            case.label()
        );
        let second_context = format!(
            "case={} limit={limit} mode=new_connection stage=calibration pair={pair_index} side=b",
            case.label()
        );
        let first =
            measure_new_connection_query(config, database, case, limit, &first_context).await?;
        let second =
            measure_new_connection_query(config, database, case, limit, &second_context).await?;
        if first.row_count != second.row_count || first.digest != second.digest {
            return Err("new-connection A/A query results differed".to_owned());
        }
        pairs.push([first.elapsed_ns, second.elapsed_ns]);
    }
    let calibration = make_calibration_record(
        config.resource_mode,
        case,
        limit,
        ConnectionMode::NewConnection,
        pairs,
    );

    let mut observations = Vec::with_capacity(5);
    for sample_index in 0..5 {
        let context = format!(
            "case={} limit={limit} mode=new_connection stage=sample index={sample_index}",
            case.label()
        );
        observations
            .push(measure_new_connection_query(config, database, case, limit, &context).await?);
    }
    let sample = make_sample_record(
        config.resource_mode,
        case,
        limit,
        ConnectionMode::NewConnection,
        observations,
    )?;
    Ok((sample, calibration))
}

async fn capture_reused_connection_condition(
    config: &RunConfig,
    session: &DatabaseSession,
    case: SampleCase,
    limit: usize,
) -> Result<(SampleRecord, CalibrationRecord), String> {
    for warmup_index in 0..5 {
        let context = format!(
            "case={} limit={limit} mode=reused_connection stage=warmup index={warmup_index}",
            case.label()
        );
        measure_query(
            session,
            case.query(),
            limit,
            case.expected_fact_ids(limit),
            &context,
        )
        .await?;
    }

    let mut pairs = Vec::with_capacity(5);
    for pair_index in 0..5 {
        let first_context = format!(
            "case={} limit={limit} mode=reused_connection stage=calibration pair={pair_index} side=a",
            case.label()
        );
        let second_context = format!(
            "case={} limit={limit} mode=reused_connection stage=calibration pair={pair_index} side=b",
            case.label()
        );
        let first = measure_query(
            session,
            case.query(),
            limit,
            case.expected_fact_ids(limit),
            &first_context,
        )
        .await?;
        let second = measure_query(
            session,
            case.query(),
            limit,
            case.expected_fact_ids(limit),
            &second_context,
        )
        .await?;
        if first.row_count != second.row_count || first.digest != second.digest {
            return Err("reused-connection A/A query results differed".to_owned());
        }
        pairs.push([first.elapsed_ns, second.elapsed_ns]);
    }
    let calibration = make_calibration_record(
        config.resource_mode,
        case,
        limit,
        ConnectionMode::ReusedConnection,
        pairs,
    );

    let mut observations = Vec::with_capacity(5);
    for sample_index in 0..5 {
        let context = format!(
            "case={} limit={limit} mode=reused_connection stage=sample index={sample_index}",
            case.label()
        );
        observations.push(
            measure_query(
                session,
                case.query(),
                limit,
                case.expected_fact_ids(limit),
                &context,
            )
            .await?,
        );
    }
    let sample = make_sample_record(
        config.resource_mode,
        case,
        limit,
        ConnectionMode::ReusedConnection,
        observations,
    )?;
    Ok((sample, calibration))
}

fn make_calibration_record(
    resource_mode: ResourceMode,
    case: SampleCase,
    limit: usize,
    connection_mode: ConnectionMode,
    pairs: Vec<[u128; 2]>,
) -> CalibrationRecord {
    CalibrationRecord {
        case: case.label(),
        limit,
        resource_mode: resource_mode.label(),
        connection_mode,
        max_noise_fraction: max_calibration_noise(&pairs),
        sample_pairs_ns: pairs,
    }
}

fn max_calibration_noise(pairs: &[[u128; 2]]) -> Option<f64> {
    pairs
        .iter()
        .map(|[first, second]| {
            let maximum = (*first).max(*second);
            (maximum != 0).then(|| first.abs_diff(*second) as f64 / maximum as f64)
        })
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .reduce(f64::max)
}

fn make_sample_record(
    resource_mode: ResourceMode,
    case: SampleCase,
    limit: usize,
    connection_mode: ConnectionMode,
    observations: Vec<Observation>,
) -> Result<SampleRecord, String> {
    let first = observations
        .first()
        .ok_or_else(|| "no timed samples were collected".to_owned())?;
    if observations.iter().any(|observation| {
        observation.row_count != first.row_count || observation.digest != first.digest
    }) {
        return Err("timed query result rows varied across samples".to_owned());
    }
    Ok(SampleRecord {
        case: case.label(),
        limit,
        resource_mode: resource_mode.label(),
        connection_mode,
        elapsed_ns: observations
            .iter()
            .map(|observation| observation.elapsed_ns)
            .collect(),
        row_count: first.row_count,
        sha256: first.digest.clone(),
        query_call_count: observations.len(),
    })
}

fn measurement_resolution_ns() -> u128 {
    let mut previous = Instant::now();
    let mut smallest = None;
    for _ in 0..10_000 {
        let current = Instant::now();
        let elapsed = current.duration_since(previous).as_nanos();
        if elapsed > 0 {
            smallest = Some(smallest.map_or(elapsed, |value: u128| value.min(elapsed)));
        }
        previous = current;
    }
    smallest.unwrap_or(0)
}

fn write_report(config: &RunConfig, report: &LatencyReport) -> Result<(), String> {
    let encoded = serde_json::to_vec_pretty(report)
        .map_err(|_| "latency report serialization failed".to_owned())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config.report)
        .map_err(|_| {
            "report path could not be created without overwriting an existing file".to_owned()
        })?;
    if file.write_all(&encoded).is_err() {
        drop(file);
        let _ = fs::remove_file(&config.report);
        return Err("latency report could not be written".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_stats_parser_sums_read_and_write_bytes_by_operation() {
        let stats = serde_json::json!({
            "cpu_stats": {
                "cpu_usage": {"total_usage": 1234}
            },
            "blkio_stats": {
                "io_service_bytes_recursive": [
                    {"op": "read", "value": 100},
                    {"op": "write", "value": 200},
                    {"op": "read", "value": 300},
                    {"op": "sync", "value": 400}
                ]
            }
        });

        assert_eq!(
            parse_container_activity(&stats).expect("valid Docker stats"),
            ContainerActivity {
                cpu_time_ns: 1234,
                read_bytes: 400,
                write_bytes: 200,
            }
        );
    }

    #[test]
    fn container_quiescence_accepts_low_cpu_and_io_rates() {
        let previous = ContainerActivity {
            cpu_time_ns: 1_000_000_000,
            read_bytes: 10_000_000,
            write_bytes: 20_000_000,
        };
        let current = ContainerActivity {
            cpu_time_ns: 1_005_000_000,
            read_bytes: 10_100_000,
            write_bytes: 20_100_000,
        };

        assert!(container_activity_is_quiet(
            previous,
            current,
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn container_quiescence_rejects_cpu_above_one_percent() {
        let previous = ContainerActivity {
            cpu_time_ns: 1_000_000_000,
            read_bytes: 10_000_000,
            write_bytes: 20_000_000,
        };
        let current = ContainerActivity {
            cpu_time_ns: 1_011_000_000,
            read_bytes: 10_000_000,
            write_bytes: 20_000_000,
        };

        assert!(!container_activity_is_quiet(
            previous,
            current,
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn container_quiescence_rejects_io_above_one_mib_per_second() {
        let previous = ContainerActivity {
            cpu_time_ns: 1_000_000_000,
            read_bytes: 10_000_000,
            write_bytes: 20_000_000,
        };
        let current = ContainerActivity {
            cpu_time_ns: 1_001_000_000,
            read_bytes: 10_600_000,
            write_bytes: 20_600_000,
        };

        assert!(!container_activity_is_quiet(
            previous,
            current,
            Duration::from_secs(1)
        ));
    }

    fn records_for_mode(mode: ResourceMode) -> (CalibrationRecord, SampleRecord) {
        let calibration = make_calibration_record(
            mode,
            SampleCase::EmptyTable,
            0,
            ConnectionMode::NewConnection,
            vec![[100, 101]; 5],
        );
        let sample = make_sample_record(
            mode,
            SampleCase::EmptyTable,
            0,
            ConnectionMode::NewConnection,
            vec![
                Observation {
                    elapsed_ns: 100,
                    row_count: 0,
                    digest: "empty-digest".to_owned(),
                };
                5
            ],
        )
        .expect("five timed observations should form a sample record");
        (calibration, sample)
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

    #[test]
    fn latency_records_name_connection_reuse_instead_of_cache_state() {
        let (calibration, sample) = records_for_mode(ResourceMode::DockerDefault);
        let calibration =
            serde_json::to_value(calibration).expect("calibration record should serialize");
        let sample = serde_json::to_value(sample).expect("sample record should serialize");

        assert_eq!(calibration["connection_mode"], "new_connection");
        assert_eq!(sample["connection_mode"], "new_connection");
    }

    #[test]
    fn latency_records_report_memory_512m_resource_mode() {
        let (calibration, sample) = records_for_mode(ResourceMode::Memory512m);

        assert_eq!(calibration.resource_mode, "memory-512m");
        assert_eq!(sample.resource_mode, "memory-512m");
        assert_eq!(sample.query_call_count, 5);
    }

    #[test]
    fn latency_records_report_docker_default_resource_mode() {
        let (calibration, sample) = records_for_mode(ResourceMode::DockerDefault);

        assert_eq!(calibration.resource_mode, "docker-default");
        assert_eq!(sample.resource_mode, "docker-default");
    }

    #[test]
    fn latency_report_serializes_only_allowlisted_fields() {
        let (calibration, sample) = records_for_mode(ResourceMode::Memory512m);
        let report = LatencyReport {
            schema: "memory-triple-latency/v2",
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
            measurement_resolution_ns: 1,
            calibration: vec![calibration],
            samples: vec![sample],
        };

        let encoded = serde_json::to_value(report).expect("latency report should serialize");
        assert_eq!(encoded["schema"], "memory-triple-latency/v2");
        assert_eq!(
            sorted_object_keys(&encoded),
            vec![
                "calibration",
                "effective_resources",
                "emulation",
                "fixture",
                "host_architecture",
                "image_architecture",
                "image_id",
                "measurement_resolution_ns",
                "resource_mode",
                "samples",
                "schema",
                "sdk_version",
                "server_version",
                "storage_engine",
            ]
        );
        assert_eq!(
            sorted_object_keys(&encoded["effective_resources"]),
            vec![
                "cpu_period_micros",
                "cpu_quota_micros",
                "memory_limit_bytes",
                "memory_swap_limit_bytes",
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
            sorted_object_keys(&encoded["calibration"][0]),
            vec![
                "case",
                "connection_mode",
                "limit",
                "max_noise_fraction",
                "resource_mode",
                "sample_pairs_ns",
            ]
        );
        assert_eq!(
            sorted_object_keys(&encoded["samples"][0]),
            vec![
                "case",
                "connection_mode",
                "elapsed_ns",
                "limit",
                "query_call_count",
                "resource_mode",
                "row_count",
                "sha256",
            ]
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
