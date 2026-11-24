#![cfg(feature = "eval-support")]

#[allow(dead_code)]
#[path = "support/triple_retrieval_oracle.rs"]
mod oracle;

#[path = "support/retrieval_latency_experiment.rs"]
mod experiment;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

const ENV_URL: &str = "MEMORY_TRIPLE_TEST_URL";
const ENV_IMAGE_ID: &str = "MEMORY_TRIPLE_TEST_IMAGE_ID";
const ENV_RESOURCE_MODE: &str = "MEMORY_TRIPLE_TEST_RESOURCE_MODE";

const ENV_REPORT: &str = "MEMORY_TRIPLE_TEST_REPORT";
const IMAGE_ID: &str = "sha256:f40e356e40dd2094e5a743ad8b1353f646cf3a660541e92b9f6cce003d3be2dd";
const IMAGE_TAG: &str = "surrealdb/surrealdb:v3.3.5";
const SERVER_VERSION: &str = "3.3.5";
fn sdk_version() -> &'static str {
    include_str!("../../../Cargo.lock")
        .split("[[package]]")
        .find_map(|package| {
            let mut lines = package.trim_start().lines();
            if lines.next()? != r#"name = "surrealdb""# {
                return None;
            }
            lines
                .next()?
                .strip_prefix(r#"version = ""#)?
                .strip_suffix('"')
        })
        .expect("locked SurrealDB SDK version")
}

const STORAGE_ENGINE: &str = "RocksDB";
const FACT_COUNT: usize = 2191;
const EMBEDDING_DIMENSION: usize = 2048;
const DATABASE_NAMESPACE_PREFIX: &str = "memory_triple_latency";
const SETUP_TIMEOUT: Duration = Duration::from_secs(30);
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const RESOURCE_BYTES: u64 = 536_870_912;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceMode {
    DockerDefault,
    Memory512m,
}

impl ResourceMode {
    fn label(self) -> &'static str {
        match self {
            Self::DockerDefault => "docker-default",
            Self::Memory512m => "memory-512m",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RunConfig {
    url: String,
    host: String,
    port: u16,
    image_id: String,
    resource_mode: ResourceMode,

    report: PathBuf,
}

fn parse_config(values: &HashMap<String, String>) -> Result<RunConfig, String> {
    let required = |name: &str| {
        values
            .get(name)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .ok_or_else(|| format!("{name} is required"))
    };

    let url = required(ENV_URL)?;
    let (host, port) = parse_loopback_ws_url(&url)?;
    let image_id = required(ENV_IMAGE_ID)?;
    if !is_full_image_id(&image_id) {
        return Err(format!("{ENV_IMAGE_ID} must be a full sha256 image ID"));
    }
    let resource_mode = match required(ENV_RESOURCE_MODE)?.as_str() {
        "docker-default" => ResourceMode::DockerDefault,
        "memory-512m" => ResourceMode::Memory512m,
        _ => {
            return Err(format!(
                "{ENV_RESOURCE_MODE} must be docker-default or memory-512m"
            ));
        }
    };

    let report = PathBuf::from(required(ENV_REPORT)?);
    if report.exists() {
        return Err(format!("{ENV_REPORT} must not already exist"));
    }

    Ok(RunConfig {
        url,
        host,
        port,
        image_id,
        resource_mode,
        report,
    })
}

fn parse_loopback_ws_url(value: &str) -> Result<(String, u16), String> {
    let authority_and_path = value
        .strip_prefix("ws://")
        .ok_or_else(|| format!("{ENV_URL} must use ws://"))?;
    let authority_end = authority_and_path
        .find(['/', '?', '#'])
        .unwrap_or(authority_and_path.len());
    let authority = &authority_and_path[..authority_end];
    let path = &authority_and_path[authority_end..];
    if authority.is_empty()
        || authority.contains('@')
        || path.contains('?')
        || path.contains('#')
        || (!path.is_empty() && path != "/")
    {
        return Err(format!(
            "{ENV_URL} must not contain credentials, query, fragment, or path"
        ));
    }

    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (address, suffix) = bracketed
            .split_once(']')
            .ok_or_else(|| format!("{ENV_URL} has an invalid IPv6 host"))?;
        let port = suffix
            .strip_prefix(':')
            .ok_or_else(|| format!("{ENV_URL} must include an explicit port"))?;
        (address.to_owned(), parse_port(port)?)
    } else {
        let (host, port) = authority
            .rsplit_once(':')
            .ok_or_else(|| format!("{ENV_URL} must include an explicit port"))?;
        if host.contains(':') {
            return Err(format!("{ENV_URL} must bracket IPv6 hosts"));
        }
        (host.to_owned(), parse_port(port)?)
    };

    if host.eq_ignore_ascii_case("localhost") || _is_loopback_literal(&host) {
        Ok((host, port))
    } else {
        Err(format!(
            "{ENV_URL} must use localhost or a loopback IP address"
        ))
    }
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("{ENV_URL} must include a valid explicit port"))
}

fn is_full_image_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_values() -> HashMap<String, String> {
    HashMap::from([
        (ENV_URL.to_owned(), "ws://127.0.0.1:8000".to_owned()),
        (ENV_IMAGE_ID.to_owned(), IMAGE_ID.to_owned()),
        (ENV_RESOURCE_MODE.to_owned(), "memory-512m".to_owned()),
        (
            ENV_REPORT.to_owned(),
            "target/assemble-context-latency-test.json".to_owned(),
        ),
    ])
}

#[test]
fn valid_experiment_configuration_is_accepted() {
    let config = parse_config(&valid_values()).expect("valid configuration should be accepted");

    assert_eq!(config.url, "ws://127.0.0.1:8000");
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 8000);
    assert_eq!(config.image_id, IMAGE_ID);
    assert_eq!(config.resource_mode, ResourceMode::Memory512m);

    assert_eq!(
        config.report,
        PathBuf::from("target/assemble-context-latency-test.json")
    );
}

#[test]
fn baseline_configuration_needs_only_endpoint_image_resource_and_report() {
    let values = HashMap::from([
        (ENV_URL.to_owned(), "ws://127.0.0.1:8000".to_owned()),
        (ENV_IMAGE_ID.to_owned(), IMAGE_ID.to_owned()),
        (ENV_RESOURCE_MODE.to_owned(), "docker-default".to_owned()),
        (
            ENV_REPORT.to_owned(),
            "target/descriptive-baseline-test.json".to_owned(),
        ),
    ]);
    assert!(parse_config(&values).is_ok());
}

#[test]
fn missing_url_is_rejected_with_its_variable_name() {
    let mut values = valid_values();
    values.remove(ENV_URL);

    assert!(parse_config(&values).is_err_and(|error| error.contains(ENV_URL)));
}

#[test]
fn missing_image_id_is_rejected_with_its_variable_name() {
    let mut values = valid_values();
    values.remove(ENV_IMAGE_ID);

    assert!(parse_config(&values).is_err_and(|error| error.contains(ENV_IMAGE_ID)));
}

#[test]
fn missing_resource_mode_is_rejected_with_its_variable_name() {
    let mut values = valid_values();
    values.remove(ENV_RESOURCE_MODE);

    assert!(parse_config(&values).is_err_and(|error| error.contains(ENV_RESOURCE_MODE)));
}

#[test]
fn missing_report_path_is_rejected_with_its_variable_name() {
    let mut values = valid_values();
    values.remove(ENV_REPORT);

    assert!(parse_config(&values).is_err_and(|error| error.contains(ENV_REPORT)));
}

#[test]
fn non_loopback_host_is_rejected_before_connection() {
    let mut values = valid_values();
    values.insert(ENV_URL.to_owned(), "ws://192.0.2.10:8000".to_owned());

    assert!(parse_config(&values).is_err());
}

#[test]
fn url_without_an_explicit_port_is_rejected() {
    let mut values = valid_values();
    values.insert(ENV_URL.to_owned(), "ws://localhost".to_owned());

    assert!(parse_config(&values).is_err());
}

#[test]
fn url_with_credentials_query_or_fragment_is_rejected() {
    let mut values = valid_values();
    values.insert(
        ENV_URL.to_owned(),
        "ws://root:root@localhost:8000/?database=unsafe#fragment".to_owned(),
    );

    assert!(parse_config(&values).is_err());
}

#[test]
fn unsupported_resource_mode_is_rejected() {
    let mut values = valid_values();
    values.insert(ENV_RESOURCE_MODE.to_owned(), "unlimited".to_owned());

    assert!(parse_config(&values).is_err());
}

#[test]
fn image_id_must_be_a_full_sha256_identifier() {
    let mut values = valid_values();
    values.insert(
        ENV_IMAGE_ID.to_owned(),
        "surrealdb/surrealdb:v3.3.5".to_owned(),
    );

    assert!(parse_config(&values).is_err());
}

#[allow(dead_code)]
fn _is_loopback_literal(host: &str) -> bool {
    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

fn fixture_facts(fact_count: usize, dimension: usize) -> Vec<serde_json::Value> {
    (0..fact_count)
        .map(|index| {
            let suffix = format!("{index:04}");
            let embedding = (0..dimension)
                .map(|axis| (((index * 31 + axis * 17) % 101) as f32) / 100.0)
                .collect::<Vec<_>>();
            serde_json::json!({
                "record_key": format!("triple-fixture-{suffix}"),
                "fact_id": format!("fact:triple-fixture-{suffix}"),
                "content": format!("synthetic triple fixture fact {suffix}"),
                "embedding": embedding,
            })
        })
        .collect()
}

fn fixture_triples() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!([
            "subject-match",
            "predicate-subject",
            "object-subject",
            "fact:triple-fixture-0000"
        ]),
        serde_json::json!([
            "subject-predicate",
            "predicate-match",
            "object-predicate",
            "fact:triple-fixture-0001"
        ]),
        serde_json::json!([
            "subject-object",
            "predicate-object",
            "object-match",
            "fact:triple-fixture-0002"
        ]),
        serde_json::json!([
            "duplicate-match",
            "predicate-duplicate",
            "object-duplicate",
            "fact:triple-fixture-0000"
        ]),
        serde_json::json!([
            "quote 'snow' 日本語",
            "predicate-dangling",
            "object-dangling",
            "fact:triple-fixture-missing"
        ]),
    ]
}

fn row_digest(rows: &[serde_json::Value]) -> String {
    let normalized = oracle::normalized_rows(rows);
    let encoded = serde_json::to_vec(&normalized)
        .expect("normalized JSON rows must have a deterministic encoding");
    hex::encode(Sha256::digest(encoded))
}

#[test]
fn fixture_contains_ordered_fact_ids_and_deterministic_2048_component_vectors() {
    let facts = fixture_facts(2191, 2048);

    assert_eq!(facts.len(), 2191);
    assert_eq!(facts[0]["fact_id"], "fact:triple-fixture-0000");
    assert_eq!(facts[2190]["fact_id"], "fact:triple-fixture-2190");
    assert_eq!(facts[0]["embedding"].as_array().map(Vec::len), Some(2048));
    assert_eq!(facts[0]["embedding"][0], 0.0);
    assert_eq!(facts[0]["embedding"][1].as_f64(), Some(f64::from(0.17_f32)));
    assert_eq!(facts[1]["embedding"][0].as_f64(), Some(f64::from(0.31_f32)));
    assert_eq!(facts[1]["embedding"][1].as_f64(), Some(f64::from(0.48_f32)));
}

#[test]
fn fixture_contains_the_five_specified_triple_source_tuples() {
    assert_eq!(
        fixture_triples(),
        vec![
            serde_json::json!([
                "subject-match",
                "predicate-subject",
                "object-subject",
                "fact:triple-fixture-0000"
            ]),
            serde_json::json!([
                "subject-predicate",
                "predicate-match",
                "object-predicate",
                "fact:triple-fixture-0001"
            ]),
            serde_json::json!([
                "subject-object",
                "predicate-object",
                "object-match",
                "fact:triple-fixture-0002"
            ]),
            serde_json::json!([
                "duplicate-match",
                "predicate-duplicate",
                "object-duplicate",
                "fact:triple-fixture-0000"
            ]),
            serde_json::json!([
                "quote 'snow' 日本語",
                "predicate-dangling",
                "object-dangling",
                "fact:triple-fixture-missing"
            ]),
        ]
    );
}

#[test]
fn row_digest_is_stable_for_reordered_rows_and_object_keys() {
    let first = vec![
        serde_json::json!({"b": 2, "a": 1}),
        serde_json::json!({"c": 3}),
    ];
    let reordered = vec![
        serde_json::json!({"c": 3}),
        serde_json::json!({"a": 1, "b": 2}),
    ];

    assert_eq!(
        row_digest(&first),
        "d1eaecdce4199a5a9e8ef25d072b9c1de3c9b1c4c969975933216ccf46fcc999"
    );
    assert_eq!(row_digest(&first), row_digest(&reordered));
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct EffectiveResources {
    memory_limit_bytes: u64,
    memory_swap_limit_bytes: u64,
    cpu_quota_micros: i64,
    cpu_period_micros: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum ContainerEvidenceError {
    NotRunning,
    ImageIdMismatch,
    PortBindingMissing,
    PortBindingMismatch,
    StartupArgumentsMissing,
    StorageEngineMismatch,
    ResourceConfigurationMissing,
    ResourceConfigurationUnavailable,
    ResourceLimitsMismatch,
}

impl std::fmt::Display for ContainerEvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (category, description) = match self {
            Self::NotRunning => ("container_not_running", "matching container is not running"),
            Self::ImageIdMismatch => (
                "image_id_mismatch",
                "container image ID differs from configuration",
            ),
            Self::PortBindingMissing => (
                "port_binding_missing",
                "container does not publish its SurrealDB port",
            ),
            Self::PortBindingMismatch => (
                "port_binding_mismatch",
                "published port is not the configured loopback endpoint",
            ),
            Self::StartupArgumentsMissing => (
                "startup_arguments_missing",
                "container startup arguments are unavailable",
            ),
            Self::StorageEngineMismatch => (
                "storage_engine_mismatch",
                "container does not use the SurrealDB RocksDB storage path",
            ),
            Self::ResourceConfigurationMissing => (
                "resource_configuration_missing",
                "container has no effective resource configuration",
            ),
            Self::ResourceConfigurationUnavailable => (
                "resource_configuration_unavailable",
                "Docker did not report numeric resource limits",
            ),
            Self::ResourceLimitsMismatch => (
                "resource_limits_mismatch",
                "effective Docker resources differ from the selected mode",
            ),
        };
        write!(formatter, "{category}: {description}")
    }
}

fn inspect_container_details(
    inspection: &serde_json::Value,
    config: &RunConfig,
) -> Result<EffectiveResources, ContainerEvidenceError> {
    if inspection
        .get("State")
        .and_then(|state| state.get("Running"))
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err(ContainerEvidenceError::NotRunning);
    }
    if inspection.get("Image").and_then(serde_json::Value::as_str) != Some(config.image_id.as_str())
    {
        return Err(ContainerEvidenceError::ImageIdMismatch);
    }

    let bindings = inspection
        .get("NetworkSettings")
        .and_then(|network| network.get("Ports"))
        .and_then(|ports| ports.get("8000/tcp"))
        .and_then(serde_json::Value::as_array)
        .ok_or(ContainerEvidenceError::PortBindingMissing)?;
    if bindings.len() != 1 {
        return Err(ContainerEvidenceError::PortBindingMismatch);
    }
    let binding = &bindings[0];
    let host_ip = binding
        .get("HostIp")
        .and_then(serde_json::Value::as_str)
        .ok_or(ContainerEvidenceError::PortBindingMismatch)?;
    let host_address = host_ip.parse::<IpAddr>().ok();
    let host_is_loopback = host_address.is_some_and(|address| address.is_loopback());
    let host_matches_configured_host = config.host.eq_ignore_ascii_case("localhost")
        || host_address == config.host.parse::<IpAddr>().ok();
    if !host_is_loopback
        || !host_matches_configured_host
        || binding.get("HostPort").and_then(serde_json::Value::as_str)
            != Some(config.port.to_string().as_str())
    {
        return Err(ContainerEvidenceError::PortBindingMismatch);
    }

    let command = inspection
        .get("Args")
        .and_then(serde_json::Value::as_array)
        .or_else(|| {
            inspection
                .get("Config")
                .and_then(|config| config.get("Cmd"))
                .and_then(serde_json::Value::as_array)
        })
        .ok_or(ContainerEvidenceError::StartupArgumentsMissing)?;
    let arguments = command
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    if !arguments.contains(&"start")
        || !arguments
            .iter()
            .any(|argument| argument.starts_with("rocksdb://"))
        || inspection
            .get("Path")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|path| !path.ends_with("surreal"))
    {
        return Err(ContainerEvidenceError::StorageEngineMismatch);
    }

    let host_config = inspection
        .get("HostConfig")
        .ok_or(ContainerEvidenceError::ResourceConfigurationMissing)?;
    let nano_cpus = host_config
        .get("NanoCpus")
        .and_then(serde_json::Value::as_u64)
        .ok_or(ContainerEvidenceError::ResourceConfigurationUnavailable)?;
    let cpuset_cpus = host_config
        .get("CpusetCpus")
        .and_then(serde_json::Value::as_str)
        .ok_or(ContainerEvidenceError::ResourceConfigurationUnavailable)?;
    let resources = EffectiveResources {
        memory_limit_bytes: host_config
            .get("Memory")
            .and_then(serde_json::Value::as_u64)
            .ok_or(ContainerEvidenceError::ResourceConfigurationUnavailable)?,
        memory_swap_limit_bytes: host_config
            .get("MemorySwap")
            .and_then(serde_json::Value::as_u64)
            .ok_or(ContainerEvidenceError::ResourceConfigurationUnavailable)?,
        cpu_quota_micros: host_config
            .get("CpuQuota")
            .and_then(serde_json::Value::as_i64)
            .ok_or(ContainerEvidenceError::ResourceConfigurationUnavailable)?,
        cpu_period_micros: host_config
            .get("CpuPeriod")
            .and_then(serde_json::Value::as_u64)
            .ok_or(ContainerEvidenceError::ResourceConfigurationUnavailable)?,
    };
    let expected_memory = match config.resource_mode {
        ResourceMode::DockerDefault => 0,
        ResourceMode::Memory512m => RESOURCE_BYTES,
    };
    if resources.memory_limit_bytes != expected_memory
        || resources.memory_swap_limit_bytes != expected_memory
        || resources.cpu_quota_micros != 0
        || resources.cpu_period_micros != 0
        || nano_cpus != 0
        || !cpuset_cpus.is_empty()
    {
        return Err(ContainerEvidenceError::ResourceLimitsMismatch);
    }

    Ok(resources)
}

fn disposable_container_inspection() -> serde_json::Value {
    serde_json::json!({
        "Id": "container-id",
        "Image": IMAGE_ID,
        "Path": "/surreal",
        "State": { "Running": true },
        "Config": {
            "Entrypoint": ["/surreal"],
            "Cmd": [
                "start", "--log", "warn", "--user", "root", "--pass", "root",
                "--bind", "0.0.0.0:8000", "rocksdb:///var/lib/surrealdb"
            ]
        },
        "Args": [
            "start", "--log", "warn", "--user", "root", "--pass", "root",
            "--bind", "0.0.0.0:8000", "rocksdb:///var/lib/surrealdb"
        ],
        "HostConfig": {
            "Memory": 536870912,
            "MemorySwap": 536870912,
            "CpuQuota": 0,
            "CpuPeriod": 0,
            "NanoCpus": 0,
            "CpusetCpus": ""
        },
        "NetworkSettings": {
            "Ports": {
                "8000/tcp": [{ "HostIp": "127.0.0.1", "HostPort": "8000" }]
            }
        }
    })
}

#[test]
fn container_evidence_accepts_the_configured_image_loopback_rocksdb_and_limits() {
    let config = parse_config(&valid_values()).expect("valid configuration");
    let observed = inspect_container_details(&disposable_container_inspection(), &config)
        .expect("matching container evidence should be accepted");

    assert_eq!(
        observed,
        EffectiveResources {
            memory_limit_bytes: RESOURCE_BYTES,
            memory_swap_limit_bytes: RESOURCE_BYTES,
            cpu_quota_micros: 0,
            cpu_period_micros: 0,
        }
    );
}

#[test]
fn container_evidence_accepts_unlimited_resources_in_docker_default_mode() {
    let mut values = valid_values();
    values.insert(ENV_RESOURCE_MODE.to_owned(), "docker-default".to_owned());
    let config = parse_config(&values).expect("docker-default configuration");
    let mut inspection = disposable_container_inspection();
    inspection["HostConfig"] = serde_json::json!({
        "Memory": 0,
        "MemorySwap": 0,
        "CpuQuota": 0,
        "CpuPeriod": 0,
        "NanoCpus": 0,
        "CpusetCpus": ""
    });

    let observed = inspect_container_details(&inspection, &config)
        .expect("unlimited Docker defaults should be accepted");

    assert_eq!(
        observed,
        EffectiveResources {
            memory_limit_bytes: 0,
            memory_swap_limit_bytes: 0,
            cpu_quota_micros: 0,
            cpu_period_micros: 0,
        }
    );
}

#[test]
fn container_evidence_classifies_a_configured_image_id_mismatch() {
    let config = parse_config(&valid_values()).expect("valid configuration");
    let mut inspection = disposable_container_inspection();
    inspection["Image"] = serde_json::Value::String("sha256:wrong".to_owned());

    assert_inspection_failure_category(
        &inspection,
        &config,
        ContainerEvidenceError::ImageIdMismatch,
    );
}

#[test]
fn container_evidence_rejects_a_loopback_binding_that_differs_from_the_configured_host() {
    let mut values = valid_values();
    values.insert(ENV_URL.to_owned(), "ws://127.0.0.2:8000".to_owned());
    let config = parse_config(&values).expect("loopback configuration");
    let inspection = disposable_container_inspection();

    assert_inspection_failure_category(
        &inspection,
        &config,
        ContainerEvidenceError::PortBindingMismatch,
    );
}

#[test]
fn container_evidence_classifies_a_non_loopback_published_port() {
    let config = parse_config(&valid_values()).expect("valid configuration");
    let mut inspection = disposable_container_inspection();
    inspection["NetworkSettings"]["Ports"]["8000/tcp"][0]["HostIp"] =
        serde_json::Value::String("0.0.0.0".to_owned());

    assert_inspection_failure_category(
        &inspection,
        &config,
        ContainerEvidenceError::PortBindingMismatch,
    );
}

#[test]
fn container_evidence_classifies_a_non_rocksdb_startup_path() {
    let config = parse_config(&valid_values()).expect("valid configuration");
    let mut inspection = disposable_container_inspection();
    let arguments = inspection["Args"]
        .as_array_mut()
        .expect("fixture startup arguments are an array");
    *arguments
        .last_mut()
        .expect("fixture includes a storage path") = serde_json::json!("mem://");

    assert_inspection_failure_category(
        &inspection,
        &config,
        ContainerEvidenceError::StorageEngineMismatch,
    );
}

#[test]
fn container_evidence_rejects_a_nanocpus_limit() {
    let config = parse_config(&valid_values()).expect("memory-512m configuration");
    let mut inspection = disposable_container_inspection();
    inspection["HostConfig"]["NanoCpus"] = serde_json::json!(1_000_000_000_u64);

    assert_inspection_failure_category(
        &inspection,
        &config,
        ContainerEvidenceError::ResourceLimitsMismatch,
    );
}

#[test]
fn container_evidence_rejects_a_cpu_set_restriction() {
    let config = parse_config(&valid_values()).expect("memory-512m configuration");
    let mut inspection = disposable_container_inspection();
    inspection["HostConfig"]["CpusetCpus"] = serde_json::json!("0");

    assert_inspection_failure_category(
        &inspection,
        &config,
        ContainerEvidenceError::ResourceLimitsMismatch,
    );
}

#[test]
fn container_evidence_classifies_resource_limits_that_do_not_match_the_selected_mode() {
    let config = parse_config(&valid_values()).expect("memory-512m configuration");
    let mut inspection = disposable_container_inspection();
    inspection["HostConfig"]["Memory"] = serde_json::json!(0);

    assert_inspection_failure_category(
        &inspection,
        &config,
        ContainerEvidenceError::ResourceLimitsMismatch,
    );
}

fn assert_inspection_failure_category(
    inspection: &serde_json::Value,
    config: &RunConfig,
    expected: ContainerEvidenceError,
) {
    let error = inspect_container_details(inspection, config)
        .expect_err("unsafe or mismatched container evidence must be rejected");
    assert_eq!(error, expected);
}

fn environment_values() -> HashMap<String, String> {
    [ENV_URL, ENV_IMAGE_ID, ENV_RESOURCE_MODE, ENV_REPORT]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), value))
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires a disposable loopback SurrealDB 3.3.5 Docker container"]
async fn remote_triple_compatibility() {
    let config =
        parse_config(&environment_values()).expect("valid remote experiment configuration");
    let result = experiment::verify_remote_triple_compatibility(&config).await;
    if let Err(error) = &result
        && error.starts_with("connection setup failed")
    {
        assert!(
            error.contains(" at stage "),
            "remote connection failures should report a safe failure stage"
        );
    }
    result.expect("remote triple compatibility contract should hold");
}

#[tokio::test]
#[ignore = "requires a disposable loopback SurrealDB 3.3.5 Docker container"]
async fn remote_guard_candidate_compatibility() {
    let config =
        parse_config(&environment_values()).expect("valid remote experiment configuration");
    experiment::verify_guard_candidate_compatibility(&config)
        .await
        .expect("guard candidate must preserve remote triple contracts");
}

#[tokio::test]
#[ignore = "requires a disposable loopback SurrealDB 3.3.5 Docker container"]
async fn remote_guard_candidate_paired_comparison() {
    let config =
        parse_config(&environment_values()).expect("valid remote experiment configuration");
    experiment::compare_guard_candidate(&config)
        .await
        .expect("paired candidate comparison should preserve results and write raw timings");
}

#[tokio::test]
#[ignore = "requires a disposable loopback SurrealDB 3.3.5 Docker container"]
async fn remote_latency_capture() {
    let config =
        parse_config(&environment_values()).expect("valid remote experiment configuration");
    experiment::capture_remote_latency(&config)
        .await
        .expect("descriptive baseline should complete with correct results");
}
