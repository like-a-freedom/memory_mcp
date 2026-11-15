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

#[derive(Debug)]
struct DockerEvidence {
    resources: EffectiveResources,
    host_architecture: String,
    image_architecture: String,
    emulation: bool,
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
enum CacheMode {
    Fresh,
    Warm,
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
    cache_mode: CacheMode,
    sample_pairs_ns: Vec<[u128; 2]>,
    max_noise_fraction: Option<f64>,
}

#[derive(Serialize)]
struct SampleRecord {
    case: &'static str,
    limit: usize,
    resource_mode: &'static str,
    cache_mode: CacheMode,
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
    let _docker = inspect_disposable_container(config)?;
    let session = open_database(config, true).await?;
    let result = async {
        verify_server_version(&session.client, &session.namespace).await?;
        for (query, limit, expected) in [
            ("subject-match", 40, &["fact:triple-fixture-0000"][..]),
            ("predicate-match", 40, &["fact:triple-fixture-0001"][..]),
            ("object-match", 40, &["fact:triple-fixture-0002"][..]),
            ("duplicate-match", 40, &["fact:triple-fixture-0000"][..]),
            ("quote 'snow' 日本語", 40, &[][..]),
            ("triple-fixture-no-match-query", 40, &[][..]),
            ("subject-match", 0, &[][..]),
        ] {
            let observation = measure_query(&session, query, limit, expected).await?;
            if observation.row_count != expected.len() {
                return Err(format!(
                    "remote compatibility case {query:?} returned an unexpected row count"
                ));
            }
        }
        Ok(())
    }
    .await;
    let cleanup = remove_database(&session).await;
    combine_result_and_cleanup(result, cleanup)
}

pub(super) async fn capture_remote_latency(config: &RunConfig) -> Result<(), String> {
    let docker = inspect_disposable_container(config)?;
    let version_session = open_database(config, false).await?;
    let server_version =
        verify_server_version(&version_session.client, &version_session.namespace).await;
    let version_cleanup = remove_database(&version_session).await;
    let server_version = combine_result_and_cleanup(server_version, version_cleanup)?;

    let mut samples = Vec::new();
    let mut calibration = Vec::new();
    for case in SampleCase::ALL {
        for limit in [0, 1, 3, 40] {
            let (sample_record, calibration_record) =
                capture_fresh_condition(config, case, limit).await?;
            samples.push(sample_record);
            calibration.push(calibration_record);
        }

        let warm_session = open_database(config, case.has_fixture()).await?;
        let warm_result = async {
            for limit in [0, 1, 3, 40] {
                let (sample_record, calibration_record) =
                    capture_warm_condition(config, &warm_session, case, limit).await?;
                samples.push(sample_record);
                calibration.push(calibration_record);
            }
            Ok(())
        }
        .await;
        let warm_cleanup = remove_database(&warm_session).await;
        combine_result_and_cleanup(warm_result, warm_cleanup)?;
    }

    let report = LatencyReport {
        schema: "memory-triple-latency/v1",
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

    Ok(DockerEvidence {
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
    run_docker_command(Path::new("docker"), args, DOCKER_COMMAND_TIMEOUT)
}

fn run_docker_command(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let mut stdout = tempfile::tempfile()
        .map_err(|_| "Docker CLI output buffer could not be created".to_owned())?;
    let child_stdout = stdout
        .try_clone()
        .map_err(|_| "Docker CLI output buffer could not be duplicated".to_owned())?;
    let mut child = Command::new(executable)
        .args(args)
        .stdout(Stdio::from(child_stdout))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Docker CLI could not be started".to_owned())?;

    let started_at = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started_at.elapsed() >= timeout => {
                terminate_docker_child(&mut child);
                return Err(
                    "Docker CLI command timed out while verifying disposable-container evidence"
                        .to_owned(),
                );
            }
            Ok(None) => {
                let remaining = timeout.saturating_sub(started_at.elapsed());
                thread::sleep(DOCKER_POLL_INTERVAL.min(remaining));
            }
            Err(_) => {
                terminate_docker_child(&mut child);
                return Err(
                    "Docker CLI process status could not be read while verifying disposable-container evidence"
                        .to_owned(),
                );
            }
        }
    };

    if !status.success() {
        return Err(
            "Docker CLI command failed while verifying disposable-container evidence".to_owned(),
        );
    }
    stdout
        .seek(SeekFrom::Start(0))
        .map_err(|_| "Docker CLI output could not be read".to_owned())?;
    let mut output = Vec::new();
    stdout
        .read_to_end(&mut output)
        .map_err(|_| "Docker CLI output could not be read".to_owned())?;
    String::from_utf8(output)
        .map_err(|_| "Docker CLI returned non-UTF-8 inspection output".to_owned())
}

fn terminate_docker_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

async fn open_database(config: &RunConfig, seed_fixture: bool) -> Result<DatabaseSession, String> {
    let namespace = super::DATABASE_NAMESPACE_PREFIX.to_owned();
    let name = next_database_name()?;
    let target = SurrealTargetConfig {
        url: config.url.clone(),
        username: "root".to_owned(),
        password: "root".to_owned(),
        database: name.clone(),
        namespace: namespace.clone(),
    };
    let client = match timeout(
        super::SETUP_TIMEOUT,
        SurrealDbClient::connect_bound(&target, "error"),
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
    let client = Arc::new(client);
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

    let db: Arc<dyn DbClient> = client.clone();
    let adapter =
        oracle::SqlOverrideDbClient::new(db, Some(oracle::corrected_unguarded_triple_sql()));
    let query_calls = adapter.query_counter();
    let db: Arc<dyn DbClient> = Arc::new(adapter);
    let store = KnowledgeStoreClient::new(db, namespace.clone());
    Ok(DatabaseSession {
        name,
        namespace,
        client,
        store,
        query_calls,
    })
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

async fn verify_server_version(
    client: &SurrealDbClient,
    namespace: &str,
) -> Result<String, String> {
    let version = client
        .server_version(namespace)
        .await
        .map_err(|_| "could not read the SurrealDB server version".to_owned())?
        .ok_or_else(|| "SurrealDB did not report a server version".to_owned())?;
    let expected = format!("SurrealDB {}", super::SERVER_VERSION);
    if version != expected {
        return Err("disposable SurrealDB server version does not match 3.3.5".to_owned());
    }
    Ok(version)
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
    let before = session.query_calls.load(Ordering::Relaxed);
    let started = Instant::now();
    let rows = timeout(
        super::QUERY_TIMEOUT,
        session
            .store
            .select_facts_by_triple(query, oracle::FIXED_CUTOFF, limit),
    )
    .await
    .map_err(|_| "triple query exceeded 10 seconds".to_owned())?
    .map_err(|_| "public triple lookup failed".to_owned())?;
    let elapsed_ns = started.elapsed().as_nanos();
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

async fn measure_fresh_query(
    config: &RunConfig,
    case: SampleCase,
    limit: usize,
) -> Result<Observation, String> {
    let session = open_database(config, case.has_fixture()).await?;
    let observation =
        measure_query(&session, case.query(), limit, case.expected_fact_ids(limit)).await;
    let cleanup = remove_database(&session).await;
    combine_result_and_cleanup(observation, cleanup)
}

async fn capture_fresh_condition(
    config: &RunConfig,
    case: SampleCase,
    limit: usize,
) -> Result<(SampleRecord, CalibrationRecord), String> {
    let mut pairs = Vec::with_capacity(5);
    for _ in 0..5 {
        let first = measure_fresh_query(config, case, limit).await?;
        let second = measure_fresh_query(config, case, limit).await?;
        if first.row_count != second.row_count || first.digest != second.digest {
            return Err("fresh A/A query results differed".to_owned());
        }
        pairs.push([first.elapsed_ns, second.elapsed_ns]);
    }
    let calibration =
        make_calibration_record(config.resource_mode, case, limit, CacheMode::Fresh, pairs);

    let mut observations = Vec::with_capacity(5);
    for _ in 0..5 {
        observations.push(measure_fresh_query(config, case, limit).await?);
    }
    let sample = make_sample_record(
        config.resource_mode,
        case,
        limit,
        CacheMode::Fresh,
        observations,
    )?;
    Ok((sample, calibration))
}

async fn capture_warm_condition(
    config: &RunConfig,
    session: &DatabaseSession,
    case: SampleCase,
    limit: usize,
) -> Result<(SampleRecord, CalibrationRecord), String> {
    for _ in 0..5 {
        measure_query(session, case.query(), limit, case.expected_fact_ids(limit)).await?;
    }

    let mut pairs = Vec::with_capacity(5);
    for _ in 0..5 {
        let first =
            measure_query(session, case.query(), limit, case.expected_fact_ids(limit)).await?;
        let second =
            measure_query(session, case.query(), limit, case.expected_fact_ids(limit)).await?;
        if first.row_count != second.row_count || first.digest != second.digest {
            return Err("warm A/A query results differed".to_owned());
        }
        pairs.push([first.elapsed_ns, second.elapsed_ns]);
    }
    let calibration =
        make_calibration_record(config.resource_mode, case, limit, CacheMode::Warm, pairs);

    let mut observations = Vec::with_capacity(5);
    for _ in 0..5 {
        observations.push(
            measure_query(session, case.query(), limit, case.expected_fact_ids(limit)).await?,
        );
    }
    let sample = make_sample_record(
        config.resource_mode,
        case,
        limit,
        CacheMode::Warm,
        observations,
    )?;
    Ok((sample, calibration))
}

fn make_calibration_record(
    resource_mode: ResourceMode,
    case: SampleCase,
    limit: usize,
    cache_mode: CacheMode,
    pairs: Vec<[u128; 2]>,
) -> CalibrationRecord {
    CalibrationRecord {
        case: case.label(),
        limit,
        resource_mode: resource_mode.label(),
        cache_mode,
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
    cache_mode: CacheMode,
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
        cache_mode,
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

    fn records_for_mode(mode: ResourceMode) -> (CalibrationRecord, SampleRecord) {
        let calibration = make_calibration_record(
            mode,
            SampleCase::EmptyTable,
            0,
            CacheMode::Fresh,
            vec![[100, 101]; 5],
        );
        let sample = make_sample_record(
            mode,
            SampleCase::EmptyTable,
            0,
            CacheMode::Fresh,
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
        let error = run_docker_command(&fake_docker, &[], Duration::from_millis(500))
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
            schema: "memory-triple-latency/v1",
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
                "cache_mode",
                "case",
                "limit",
                "max_noise_fraction",
                "resource_mode",
                "sample_pairs_ns",
            ]
        );
        assert_eq!(
            sorted_object_keys(&encoded["samples"][0]),
            vec![
                "cache_mode",
                "case",
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
