use std::process::{Command, Output};

const PROBE_ENV: &str = "MEMORY_CACHE_LIMITS_TEST_PROBE";
const DEFAULT_CONTEXT_ENV: &str = "MEMORY_CACHE_LIMITS_TEST_DEFAULT_CONTEXT";
const EXPECT_CONTEXT_ENV: &str = "MEMORY_CACHE_LIMITS_TEST_EXPECT_CONTEXT";
const EXPECT_QUERY_ENV: &str = "MEMORY_CACHE_LIMITS_TEST_EXPECT_QUERY";
const EXPECT_ERROR_ENV: &str = "MEMORY_CACHE_LIMITS_TEST_EXPECT_ERROR";

fn run_probe(
    default_context_bytes: usize,
    expected_context_bytes: usize,
    expected_query_bytes: usize,
    context_override: Option<&str>,
    query_override: Option<&str>,
    expected_error_variable: Option<&str>,
) -> Output {
    let mut command = Command::new(std::env::current_exe().expect("test binary path is available"));
    command
        .env_clear()
        .env(PROBE_ENV, "1")
        .env(DEFAULT_CONTEXT_ENV, default_context_bytes.to_string())
        .env(EXPECT_CONTEXT_ENV, expected_context_bytes.to_string())
        .env(EXPECT_QUERY_ENV, expected_query_bytes.to_string())
        .arg("--exact")
        .arg("cache_limits_probe");

    if let Some(value) = context_override {
        command.env("MEMORY_CONTEXT_CACHE_BYTES", value);
    }
    if let Some(value) = query_override {
        command.env("MEMORY_QUERY_EMBEDDING_CACHE_BYTES", value);
    }
    if let Some(variable) = expected_error_variable {
        command.env(EXPECT_ERROR_ENV, variable);
    }

    command.output().expect("cache limits probe should start")
}

fn assert_probe_succeeds(output: Output) {
    assert!(
        output.status.success(),
        "probe failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cache_limits_use_http_and_stdio_defaults() {
    assert_probe_succeeds(run_probe(4_194_304, 4_194_304, 2_097_152, None, None, None));
    assert_probe_succeeds(run_probe(
        16_777_216, 16_777_216, 2_097_152, None, None, None,
    ));
}

#[test]
fn query_cache_budget_override_is_applied() {
    assert_probe_succeeds(run_probe(
        4_194_304,
        4_194_304,
        5_000_000,
        None,
        Some("5000000"),
        None,
    ));
}

#[test]
fn zero_context_cache_budget_fails_with_variable_name() {
    assert_probe_succeeds(run_probe(
        4_194_304,
        4_194_304,
        2_097_152,
        Some("0"),
        None,
        Some("MEMORY_CONTEXT_CACHE_BYTES"),
    ));
}

#[test]
fn zero_query_cache_budget_fails_with_variable_name() {
    assert_probe_succeeds(run_probe(
        4_194_304,
        4_194_304,
        2_097_152,
        None,
        Some("0"),
        Some("MEMORY_QUERY_EMBEDDING_CACHE_BYTES"),
    ));
}

#[test]
fn invalid_context_cache_budget_fails_with_variable_name() {
    assert_probe_succeeds(run_probe(
        4_194_304,
        4_194_304,
        2_097_152,
        Some("not-a-byte-count"),
        None,
        Some("MEMORY_CONTEXT_CACHE_BYTES"),
    ));
}

#[test]
fn invalid_query_cache_budget_fails_with_variable_name() {
    assert_probe_succeeds(run_probe(
        4_194_304,
        4_194_304,
        2_097_152,
        None,
        Some("not-a-byte-count"),
        Some("MEMORY_QUERY_EMBEDDING_CACHE_BYTES"),
    ));
}

#[test]
fn cache_limits_probe() {
    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }

    let default_context_bytes = std::env::var(DEFAULT_CONTEXT_ENV)
        .expect("test supplies the context default")
        .parse::<usize>()
        .expect("test supplies a numeric context default");
    let result = memory_mcp::config::CacheLimits::from_env(default_context_bytes);

    if let Ok(variable) = std::env::var(EXPECT_ERROR_ENV) {
        let error = result.expect_err("invalid cache budget should fail configuration");
        assert!(error.to_string().contains(&variable), "{error}");
        return;
    }

    let limits = result.expect("cache defaults and overrides should be valid");
    let expected_context_bytes = std::env::var(EXPECT_CONTEXT_ENV)
        .expect("test supplies the expected context budget")
        .parse::<usize>()
        .expect("test supplies a numeric context budget");
    let expected_query_bytes = std::env::var(EXPECT_QUERY_ENV)
        .expect("test supplies the expected query budget")
        .parse::<usize>()
        .expect("test supplies a numeric query budget");
    assert_eq!(limits.context_bytes.get(), expected_context_bytes);
    assert_eq!(limits.query_bytes.get(), expected_query_bytes);
}
