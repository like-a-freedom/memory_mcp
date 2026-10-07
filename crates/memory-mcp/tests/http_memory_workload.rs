use std::process::Command;

const ENV_PROBE: &str = "HTTP_MEMORY_TEST_ENV_PROBE";

struct StagingConfig {
    base_url: reqwest::Url,
    api_key: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ConfigError {
    MissingStagingAcknowledgement,
    MissingBaseUrl,
    InvalidBaseUrl,
    MissingApiKey,
    NonStagingEndpoint,
}

fn is_staging_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
        || host.ends_with(".test")
        || host
            .split('.')
            .any(|label| label.eq_ignore_ascii_case("staging"))
}

impl StagingConfig {
    fn from_environment() -> Result<Self, ConfigError> {
        let acknowledgement = std::env::var("HTTP_MEMORY_TEST_DESTRUCTIVE_STAGING").ok();
        let base_url = std::env::var("HTTP_MEMORY_TEST_BASE_URL").ok();
        let api_key = std::env::var("HTTP_MEMORY_TEST_API_KEY").ok();
        Self::from_values(
            acknowledgement.as_deref(),
            base_url.as_deref(),
            api_key.as_deref(),
        )
    }

    fn from_values(
        acknowledgement: Option<&str>,
        base_url: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<Self, ConfigError> {
        if acknowledgement != Some("1") {
            return Err(ConfigError::MissingStagingAcknowledgement);
        }
        let base_url = base_url.ok_or(ConfigError::MissingBaseUrl)?;
        let parsed_url = reqwest::Url::parse(base_url).map_err(|_| ConfigError::InvalidBaseUrl)?;
        if !matches!(parsed_url.scheme(), "http" | "https") {
            return Err(ConfigError::InvalidBaseUrl);
        }
        if parsed_url
            .host_str()
            .is_some_and(|host| !is_staging_host(host))
        {
            return Err(ConfigError::NonStagingEndpoint);
        }
        let api_key = api_key
            .filter(|key| !key.trim().is_empty())
            .ok_or(ConfigError::MissingApiKey)?;

        Ok(Self {
            base_url: parsed_url,
            api_key: api_key.to_owned(),
        })
    }
}

#[test]
fn external_workload_refuses_missing_staging_acknowledgement() {
    let result = StagingConfig::from_values(
        None,
        Some("https://staging.example.test/memory"),
        Some("synthetic-test-key"),
    );

    assert!(matches!(
        result,
        Err(ConfigError::MissingStagingAcknowledgement)
    ));
}

#[test]
fn external_workload_refuses_hosts_without_a_staging_marker() {
    let result = StagingConfig::from_values(
        Some("1"),
        Some("https://memory.example.com/memory"),
        Some("synthetic-test-key"),
    );

    assert!(matches!(result, Err(ConfigError::NonStagingEndpoint)));
}

#[test]
fn external_workload_accepts_only_complete_staging_configuration() {
    let config = StagingConfig::from_values(
        Some("1"),
        Some("https://staging.example.test/memory"),
        Some("synthetic-test-key"),
    )
    .expect("complete non-production staging settings should be accepted");

    assert_eq!(config.base_url.host_str(), Some("staging.example.test"));
    assert_eq!(config.api_key.len(), "synthetic-test-key".len());
}

#[test]
fn env_probe_child() {
    if std::env::var_os(ENV_PROBE).is_some() {
        let result = StagingConfig::from_environment();
        assert!(matches!(
            result,
            Err(ConfigError::MissingStagingAcknowledgement)
        ));
    }
}

#[test]
fn environment_parser_can_be_checked_in_an_isolated_subprocess() {
    let result = Command::new(std::env::current_exe().expect("test binary path is available"))
        .env_clear()
        .env(ENV_PROBE, "1")
        .arg("--exact")
        .arg("env_probe_child")
        .output()
        .expect("child test process should start");

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
