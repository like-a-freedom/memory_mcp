use std::process::{Command, Output};

const PROBE_ENV: &str = "ANNO_INPUT_LIMIT_TEST_PROBE";
const EXPECT_LIMIT_ENV: &str = "ANNO_INPUT_LIMIT_TEST_EXPECT_LIMIT";
const EXPECT_ERROR_ENV: &str = "ANNO_INPUT_LIMIT_TEST_EXPECT_ERROR";

fn run_probe(
    selector: &str,
    input_limit: Option<&str>,
    expected_limit: Option<usize>,
    expected_error: Option<&str>,
) -> Output {
    let mut command = Command::new(std::env::current_exe().expect("test binary path is available"));
    command
        .env_clear()
        .env(PROBE_ENV, "1")
        .env("NER_EXTRACTOR", selector)
        .arg("--exact")
        .arg("anno_input_limit_probe");

    if let Some(limit) = input_limit {
        command.env("ANNO_MAX_INPUT_BYTES", limit);
    }
    if let Some(limit) = expected_limit {
        command.env(EXPECT_LIMIT_ENV, limit.to_string());
    }
    if let Some(variable) = expected_error {
        command.env(EXPECT_ERROR_ENV, variable);
    }

    command.output().expect("Anno config probe should start")
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
fn anno_input_limit_defaults_to_one_mib() {
    assert_probe_succeeds(run_probe("anno", None, Some(1_048_576), None));
}

#[test]
fn anno_input_limit_accepts_explicit_value() {
    assert_probe_succeeds(run_probe("anno", Some("65536"), Some(65_536), None));
}

#[test]
fn anno_input_limit_rejects_zero() {
    assert_probe_succeeds(run_probe(
        "anno",
        Some("0"),
        None,
        Some("ANNO_MAX_INPUT_BYTES"),
    ));
}

#[test]
fn anno_input_limit_rejects_above_one_mib() {
    assert_probe_succeeds(run_probe(
        "anno",
        Some("1048577"),
        None,
        Some("ANNO_MAX_INPUT_BYTES"),
    ));
}

#[test]
fn anno_input_limit_rejects_non_anno_selector() {
    assert_probe_succeeds(run_probe(
        "regex",
        Some("65536"),
        None,
        Some("ANNO_MAX_INPUT_BYTES"),
    ));
}

#[test]
fn anno_input_limit_probe() {
    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }

    let result = memory_mcp::config::NerConfig::from_env();
    if let Ok(variable) = std::env::var(EXPECT_ERROR_ENV) {
        let error = result.expect_err("invalid Anno limit config should fail");
        assert!(error.to_string().contains(&variable), "{error}");
        return;
    }

    let config = result.expect("Anno limit config should be valid");
    let expected = std::env::var(EXPECT_LIMIT_ENV)
        .expect("test supplies expected limit")
        .parse::<usize>()
        .expect("test supplies numeric expected limit");
    match config.extractor {
        memory_mcp::config::NerExtractorConfig::Anno { max_input_bytes } => {
            assert_eq!(max_input_bytes, expected);
        }
        _ => panic!("the Anno selector must produce its typed config"),
    }
}
