#![cfg(all(
    feature = "streamable-http",
    feature = "mcp-apps",
    feature = "eval-support",
    feature = "test-fixtures"
))]

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::http_storage_memory::{validate_http_memory_evidence, validate_http_memory_suite};
use serde_json::{Value, json};
use tempfile::TempDir;

fn valid_run() -> Value {
    json!({
        "schema_version": 4,
        "case": {
            "id": "http-remote384-cold-decay-r1",
            "composition": "http",
            "mode": { "kind": "remote", "dimension": 384 },
            "scenario": "cold_decay",
            "transition": null,
            "phase": "steady",
            "workload": null,
            "repetition": 1
        },
        "image": {
            "platform": "linux/amd64",
            "runner_platform": "linux/amd64",
            "emulated": false,
            "image_id": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "oci_digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "source_bundle_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "expected_oci_digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "expected_source_bundle_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "glibc_version": "2.41-12+deb13u4",
            "app_features": ["streamable-http", "mcp-apps", "fs-watch"]
        },
        "process": {
            "binary_name": "memory_mcp_http",
            "transport": "streamable_http",
            "pid": 42,
            "start_time_ticks": 901,
            "observed_pid": 42,
            "observed_start_time_ticks": 901,
            "sampler_exit_code": 0,
            "sampler_started_before_app": true,
            "cold_start_observed": true,
            "steady_window_ms": 60000,
            "measurement_finished_ms": null
        },
        "sample_count": 2,
        "samples_file": "samples.jsonl",
        "expected_case_ids": ["http-remote384-cold-decay-r1"],
        "executed_case_ids": ["http-remote384-cold-decay-r1"],
        "expected_faults": [],
        "requests": [
            { "request_id": "decay-first", "outcome": "success", "category": null },
            { "request_id": "decay-repeat", "outcome": "success", "category": null }
        ],
        "idle_checkpoints": [],
        "provider": {
            "embedding_enabled": true,
            "provider_id": "openai-compatible",
            "model_revision": "test-model@v1",
            "configured_dimension": 384,
            "provider_dimension": 384,
            "schema_dimension": 384,
            "configured_signature": "sig:test-model@v1:d384",
            "persisted_embeddings": [
                { "dimension": 384, "signature": "sig:test-model@v1:d384" }
            ],
            "observed_signatures": [],
            "resolved_dimension": 384,
            "stored_dimensions": [384],
            "observed_dimensions": [384],
            "index_dimension": 384,
            "requests": 0,
            "ann_queries": 0,
            "trap_listener_started": false,
            "trap_requests": 0
        },
        "db_reads": { "events": [] },
        "decay": {
            "completion_signals": ["first", "repeat"],
            "invalidated_fact_ids": ["fact:storage_memory_decay_old"],
            "closed_claim_ids": [
                "claim:storage_memory_decay_subject",
                "claim:storage_memory_decay_object"
            ]
        },
        "cgroup": {
            "memory_peak_bytes": 200000000,
            "memory_events": { "oom_delta": 0, "oom_kill_delta": 0 }
        }
    })
}

fn seeded_rewrite_fact_ids() -> Vec<String> {
    (0..205)
        .map(|index| format!("fact:storage_memory_{index:04}"))
        .collect::<Vec<_>>()
}

fn valid_complete_rewrite_evidence() -> Value {
    let fact_ids = seeded_rewrite_fact_ids();
    json!({
        "requested_facts": 205,
        "completed_facts": 205,
        "batch_sizes": [100, 100, 5],
        "source_fact_ids": fact_ids.clone(),
        "final_fact_ids": fact_ids,
        "last_completed_fact_id": "fact:storage_memory_0204",
        "interrupted_after_facts": null,
        "resume_after_fact_id": null,
        "restarted": false,
        "failed_fact_ids": [],
        "state": "completed"
    })
}

fn valid_running_rewrite_evidence() -> Value {
    let fact_ids = seeded_rewrite_fact_ids();
    json!({
        "requested_facts": 205,
        "completed_facts": 100,
        "batch_sizes": [100],
        "source_fact_ids": fact_ids.clone(),
        "final_fact_ids": fact_ids,
        "last_completed_fact_id": "fact:storage_memory_0099",
        "interrupted_after_facts": null,
        "resume_after_fact_id": null,
        "restarted": false,
        "failed_fact_ids": [],
        "state": "running"
    })
}

fn valid_interrupted_resume_evidence() -> Value {
    let mut evidence = valid_complete_rewrite_evidence();
    evidence["batch_sizes"] = json!([100, 1, 100, 4]);
    evidence["interrupted_after_facts"] = json!(101);
    evidence["resume_after_fact_id"] = json!("fact:storage_memory_0100");
    evidence["restarted"] = json!(true);
    evidence
}

fn set_remote_identity(
    run: &mut Value,
    dimension: usize,
    model_revision: &str,
    persisted: &[(usize, &str)],
) {
    let configured_signature = format!("sig:{model_revision}:d{dimension}");
    let mut stored_dimensions = persisted
        .iter()
        .map(|(stored_dimension, _)| *stored_dimension)
        .collect::<Vec<_>>();
    stored_dimensions.sort_unstable();
    stored_dimensions.dedup();
    run["provider"]["embedding_enabled"] = json!(true);
    run["provider"]["provider_id"] = json!("openai-compatible");
    run["provider"]["model_revision"] = json!(model_revision);
    run["provider"]["configured_dimension"] = json!(dimension);
    run["provider"]["provider_dimension"] = json!(dimension);
    run["provider"]["schema_dimension"] = json!(dimension);
    run["provider"]["configured_signature"] = json!(configured_signature);
    run["provider"]["persisted_embeddings"] = json!(
        persisted
            .iter()
            .map(|(stored_dimension, signature)| json!({
                "dimension": stored_dimension,
                "signature": signature
            }))
            .collect::<Vec<_>>()
    );
    run["provider"]["observed_signatures"] = json!([]);
    run["provider"]["resolved_dimension"] = json!(dimension);
    run["provider"]["stored_dimensions"] = json!(stored_dimensions);
    run["provider"]["index_dimension"] = json!(dimension);
}

fn set_disabled_identity(run: &mut Value, dimension: usize, stored_dimensions: &[usize]) {
    let mut sorted_dimensions = stored_dimensions.to_vec();
    sorted_dimensions.sort_unstable();
    sorted_dimensions.dedup();
    run["provider"]["embedding_enabled"] = json!(false);
    run["provider"]["provider_id"] = Value::Null;
    run["provider"]["model_revision"] = Value::Null;
    run["provider"]["configured_dimension"] = json!(dimension);
    run["provider"]["provider_dimension"] = Value::Null;
    run["provider"]["schema_dimension"] = json!(dimension);
    run["provider"]["configured_signature"] = Value::Null;
    run["provider"]["persisted_embeddings"] = json!(
        stored_dimensions
            .iter()
            .map(|stored_dimension| json!({
                "dimension": stored_dimension,
                "signature": format!("sig:legacy:d{stored_dimension}")
            }))
            .collect::<Vec<_>>()
    );
    run["provider"]["observed_signatures"] = json!([]);
    run["provider"]["resolved_dimension"] = json!(dimension);
    run["provider"]["stored_dimensions"] = json!(sorted_dimensions);
    run["provider"]["index_dimension"] = json!(dimension);
}

fn local_native_run() -> Value {
    let mut run = valid_run();
    run["case"]["composition"] = json!("stdio");
    run["process"]["binary_name"] = json!("memory_mcp");
    run["process"]["transport"] = json!("stdio_mcp");
    run["case"]["mode"] = json!({ "kind": "local_native", "dimension": 384 });
    run["image"]["app_features"] = json!(["mcp-apps", "fs-watch"]);
    set_remote_identity(
        &mut run,
        384,
        "test-model@v1",
        &[(384, "sig:test-model@v1:d384")],
    );
    run["provider"]["provider_id"] = json!("local-candle");
    run
}

fn cli_rewrite_run() -> Value {
    let mut run = valid_run();
    run["case"]["id"] = json!("cli-remote2048-reembed-before-r1");
    run["case"]["composition"] = json!("cli");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 2048 });
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 2048 });
    run["case"]["phase"] = json!("before");
    run["expected_case_ids"] = json!(["cli-remote2048-reembed-before-r1"]);
    run["executed_case_ids"] = json!(["cli-remote2048-reembed-before-r1"]);
    run["image"]["app_features"] = json!(["fs-watch", "mcp-apps"]);
    run["process"]["binary_name"] = json!("memory_mcp");
    run["process"]["transport"] = json!("cli");
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[(384, "sig:test-model@v1:d384")],
    );
    run["provider"]["observed_dimensions"] = json!([384, 2048]);
    run["rewrite"] = json!({
        "requested_facts": 205,
        "completed_facts": 0,
        "batch_sizes": [],
        "source_fact_ids": seeded_rewrite_fact_ids(),
        "final_fact_ids": seeded_rewrite_fact_ids(),
        "last_completed_fact_id": null,
        "interrupted_after_facts": null,
        "resume_after_fact_id": null,
        "restarted": false,
        "failed_fact_ids": [],
        "state": "ready"
    });
    run
}

fn write_evidence(run: &Value, samples: &str) -> TempDir {
    let directory = tempfile::tempdir().expect("temporary evidence directory should be available");
    fs::write(
        directory.path().join("run.json"),
        serde_json::to_vec_pretty(run).expect("run evidence should serialize"),
    )
    .expect("run evidence should be written");
    write_sample_evidence(&directory.path().join("samples.jsonl"), run, samples);
    directory
}

fn write_sample_evidence(path: &std::path::Path, run: &Value, samples: &str) {
    let measured_binary = run["process"]["binary_name"]
        .as_str()
        .expect("process evidence should name the measured binary");
    let mut output = String::new();
    for line in samples.lines() {
        let mut sample: Value =
            serde_json::from_str(line).expect("synthetic sampler records should be valid JSON");
        if sample["binary_name"] == "__measured_binary__" {
            sample["binary_name"] = json!(measured_binary);
        }
        output.push_str(
            &serde_json::to_string(&sample).expect("synthetic sampler record should serialize"),
        );
        output.push('\n');
    }
    fs::write(path, output).expect("sample evidence should be written");
}

fn write_suite(cases: &[(&str, Value)]) -> TempDir {
    let directory = tempfile::tempdir().expect("temporary suite directory should be available");
    let cases_dir = directory.path().join("cases");
    fs::create_dir_all(&cases_dir).expect("suite cases directory should be created");
    let case_ids = cases
        .iter()
        .map(|(case_id, _)| (*case_id).to_string())
        .collect::<Vec<_>>();
    for (directory_id, run) in cases {
        let case_dir = cases_dir.join(directory_id);
        fs::create_dir_all(&case_dir).expect("case artifact directory should be created");
        let mut run = run.clone();
        let embedded_id = run["case"]["id"]
            .as_str()
            .expect("case evidence should include its embedded ID")
            .to_owned();
        run["expected_case_ids"] = json!([embedded_id]);
        run["executed_case_ids"] = json!([embedded_id]);
        fs::write(
            case_dir.join("run.json"),
            serde_json::to_vec_pretty(&run).expect("run evidence should serialize"),
        )
        .expect("run evidence should be written");
        write_sample_evidence(&case_dir.join("samples.jsonl"), &run, VALID_SAMPLES);
    }
    fs::write(
        directory.path().join("suite.json"),
        serde_json::to_vec_pretty(&json!({
        "schema_version": 4,
            "expected_case_ids": case_ids,
            "executed_case_ids": case_ids
        }))
        .expect("suite evidence should serialize"),
    )
    .expect("suite evidence should be written");
    directory
}

const VALID_SAMPLES: &str = concat!(
    "{\"binary_name\":\"__measured_binary__\",\"process_alive\":true,\"elapsed_ms\":0,\"rss_kib\":100000,\"swap_kib\":0,\"hwm_kib\":100000,\"cgroup_current_bytes\":120000000,\"cgroup_swap_bytes\":0,\"cgroup_memory_peak_bytes\":200000000,\"cgroup_oom\":0,\"cgroup_oom_kill\":0}\n",
    "{\"binary_name\":\"__measured_binary__\",\"process_alive\":true,\"elapsed_ms\":60000,\"rss_kib\":101000,\"swap_kib\":0,\"hwm_kib\":101000,\"cgroup_current_bytes\":121000000,\"cgroup_swap_bytes\":0,\"cgroup_memory_peak_bytes\":200000000,\"cgroup_oom\":0,\"cgroup_oom_kill\":0}\n"
);

fn retrieval_plateau_evidence(first_rss_kib: u64, last_rss_kib: u64) -> (Value, String) {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("retrieval_misses");
    run["case"]["workload"] = json!("wake_up");
    run["process"]["measurement_finished_ms"] = json!(60_000);

    let requests = (0..120)
        .map(|index| {
            let phase = if index < 20 { "warmup" } else { "measured" };
            let request_id = if index < 20 {
                format!("warmup-{index}")
            } else {
                format!("lookup-{}", index - 20)
            };
            let started_ms = index * 100;
            json!({
                "request_id": request_id,
                "outcome": "success",
                "category": null,
                "phase": phase,
                "started_ms": started_ms,
                "finished_ms": started_ms + 10
            })
        })
        .collect::<Vec<_>>();
    let measured_ids = (0..100)
        .map(|index| format!("lookup-{index}"))
        .collect::<Vec<_>>();
    run["requests"] = json!(requests);
    run["db_reads"]["events"] = json!(
        measured_ids
            .iter()
            .map(|request_id| json!({
                "request_id": request_id,
                "foreground": true,
                "statement_kind": "SELECT",
                "cache_hit": false
            }))
            .collect::<Vec<_>>()
    );

    let checkpoint_measurements = (0..100)
        .filter(|index| *index < 20 || *index >= 80)
        .map(|measured_index| {
            let request_index = measured_index + 20;
            let request_id = format!("lookup-{measured_index}");
            let elapsed_ms = request_index * 100 + 20;
            let rss_kib = if measured_index < 20 {
                first_rss_kib
            } else {
                last_rss_kib
            };
            (
                json!({
                    "request_id": request_id,
                    "elapsed_ms": elapsed_ms,
                    "inflight_requests": 0
                }),
                (elapsed_ms, rss_kib),
            )
        })
        .collect::<Vec<_>>();
    run["idle_checkpoints"] = json!(
        checkpoint_measurements
            .iter()
            .map(|(checkpoint, _)| checkpoint.clone())
            .collect::<Vec<_>>()
    );

    let peak_hwm = first_rss_kib.max(last_rss_kib);
    let sample_line = |elapsed_ms: u64, rss_kib: u64, hwm_kib: u64| {
        serde_json::to_string(&json!({
            "binary_name": "__measured_binary__",
            "process_alive": true,
            "elapsed_ms": elapsed_ms,
            "rss_kib": rss_kib,
            "swap_kib": 0,
            "hwm_kib": hwm_kib,
            "cgroup_current_bytes": rss_kib * 1024,
            "cgroup_swap_bytes": 0,
            "cgroup_memory_peak_bytes": 200_000_000,
            "cgroup_oom": 0,
            "cgroup_oom_kill": 0
        }))
        .expect("synthetic sample should serialize")
    };
    let mut samples = vec![sample_line(0, first_rss_kib, first_rss_kib)];
    for (elapsed_ms, rss_kib) in checkpoint_measurements
        .iter()
        .map(|(_, measurement)| measurement)
    {
        samples.push(sample_line(*elapsed_ms, *rss_kib, peak_hwm));
    }
    samples.push(sample_line(60_000, first_rss_kib, peak_hwm));
    run["sample_count"] = json!(samples.len());
    (run, format!("{}\n", samples.join("\n")))
}

fn assert_rejected(run: Value, samples: &str, expected_category: &str) {
    let directory = write_evidence(&run, samples);
    let error = validate_http_memory_evidence(directory.path())
        .expect_err("invalid memory evidence must fail closed");
    assert!(
        error.contains(expected_category),
        "expected error category {expected_category:?}, got {error:?}"
    );
}

#[test]
fn missing_samples_fail_the_run() {
    assert_rejected(valid_run(), "", "samples");
}

#[test]
fn version_three_memory_evidence_is_rejected_after_checkpoint_schema_change() {
    let mut run = valid_run();
    run["schema_version"] = json!(3);

    assert_rejected(run, VALID_SAMPLES, "schema");
}

#[test]
fn a_single_sample_cannot_prove_a_steady_memory_plateau() {
    let samples = concat!(
        "{\"binary_name\":\"__measured_binary__\",\"elapsed_ms\":0,\"rss_kib\":100000,\"swap_kib\":0,\"hwm_kib\":100000,\"cgroup_current_bytes\":120000000,\"cgroup_swap_bytes\":0}\n",
        "{\"binary_name\":\"__measured_binary__\",\"elapsed_ms\":120000,\"rss_kib\":101000,\"swap_kib\":0,\"hwm_kib\":101000,\"cgroup_current_bytes\":121000000,\"cgroup_swap_bytes\":0}\n"
    );

    assert_rejected(valid_run(), samples, "samples");
}

#[test]
fn arm_image_is_not_accepted_as_amd64_evidence() {
    let mut run = valid_run();
    run["image"]["platform"] = json!("linux/arm64");

    assert_rejected(run, VALID_SAMPLES, "platform");
}

#[test]
fn amd64_image_running_on_arm_or_under_emulation_is_not_accepted() {
    let mut run = valid_run();
    run["image"]["runner_platform"] = json!("linux/arm64");
    run["image"]["emulated"] = json!(true);

    assert_rejected(run, VALID_SAMPLES, "platform");
}

#[test]
fn cache_hits_do_not_close_miss_gate() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("retrieval_misses");
    run["case"]["workload"] = json!("lexical_fallback");
    run["db_reads"]["events"] = json!(
        (0..100)
            .map(|index| json!({
                "request_id": format!("lookup-{index}"),
                "foreground": true,
                "statement_kind": "SELECT",
                "cache_hit": true
            }))
            .collect::<Vec<_>>()
    );

    assert_rejected(run, VALID_SAMPLES, "cache-miss");
}

#[test]
fn no_cache_wake_up_workload_requires_repeated_foreground_reads() {
    let (run, samples) = retrieval_plateau_evidence(100_000, 100_000);
    let directory = write_evidence(&run, &samples);

    let report = validate_http_memory_evidence(directory.path())
        .expect("wake-up is measured by repeated real foreground database reads");

    assert_eq!(report.db_read_delta, 100);
}

#[test]
fn error_release_case_cannot_pass_without_an_injected_failure() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("error_release");
    run["requests"] = json!([
        { "request_id": "recovered", "outcome": "success", "category": null }
    ]);
    let directory = write_evidence(&run, VALID_SAMPLES);

    let error = validate_http_memory_evidence(directory.path())
        .expect_err("ErrorRelease must prove a fault followed by successful recovery");

    assert!(error.contains("request"), "{error}");
}

#[test]
fn error_release_accepts_exact_faults_followed_by_successful_recovery() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("error_release");
    run["expected_faults"] = json!([
        "provider_unavailable",
        "request_cancelled",
        "storage_failure"
    ]);
    run["requests"] = json!([
        { "request_id": "provider-fault", "outcome": "expected_fault", "category": "provider_unavailable" },
        { "request_id": "cancelled", "outcome": "expected_fault", "category": "request_cancelled" },
        { "request_id": "storage-fault", "outcome": "expected_fault", "category": "storage_failure" },
        { "request_id": "recovered", "outcome": "success", "category": null }
    ]);
    let directory = write_evidence(&run, VALID_SAMPLES);

    let report = validate_http_memory_evidence(directory.path())
        .expect("all required faults followed by recovery should validate");

    assert_eq!(report.expected_fault_outcomes, 3);
    assert_eq!(report.unexpected_failed_requests, 0);
}

#[test]
fn sampler_exit_before_decay_is_failure() {
    let mut run = valid_run();
    run["process"]["sampler_exit_code"] = json!(1);
    run["decay"]["completion_signals"] = json!(["first"]);

    assert_rejected(run, VALID_SAMPLES, "sampler");
}

#[test]
fn application_error_is_not_a_successful_request() {
    let mut run = valid_run();
    run["requests"][0]["outcome"] = json!("application_error");
    run["requests"][0]["category"] = json!("storage");

    assert_rejected(run, VALID_SAMPLES, "request");
}

#[test]
fn cgroup_without_peak_or_events_is_incomplete() {
    let mut run = valid_run();
    run["cgroup"]["memory_peak_bytes"] = Value::Null;
    run["cgroup"]["memory_events"] = Value::Null;
    let samples = VALID_SAMPLES
        .replace("120000000", "null")
        .replace("121000000", "null");

    assert_rejected(run, &samples, "cgroup");
}

#[test]
fn cgroup_peak_must_match_the_raw_sampled_peak() {
    let samples = concat!(
        "{\"binary_name\":\"memory_mcp_http\",\"process_alive\":true,\"elapsed_ms\":0,\"rss_kib\":100000,\"swap_kib\":0,\"hwm_kib\":100000,\"cgroup_current_bytes\":120000000,\"cgroup_swap_bytes\":0,\"cgroup_memory_peak_bytes\":140000000,\"cgroup_oom\":5,\"cgroup_oom_kill\":2}\n",
        "{\"binary_name\":\"memory_mcp_http\",\"process_alive\":true,\"elapsed_ms\":60000,\"rss_kib\":101000,\"swap_kib\":0,\"hwm_kib\":101000,\"cgroup_current_bytes\":121000000,\"cgroup_swap_bytes\":0,\"cgroup_memory_peak_bytes\":150000000,\"cgroup_oom\":5,\"cgroup_oom_kill\":2}\n"
    );

    let directory = write_evidence(&valid_run(), samples);
    let error = validate_http_memory_evidence(directory.path())
        .expect_err("a mismatched cgroup peak must fail closed");
    assert!(error.starts_with("[cgroup]"), "{error}");
}

#[test]
fn cgroup_oom_delta_must_match_the_raw_sampled_counters() {
    let samples = concat!(
        "{\"binary_name\":\"memory_mcp_http\",\"process_alive\":true,\"elapsed_ms\":0,\"rss_kib\":100000,\"swap_kib\":0,\"hwm_kib\":100000,\"cgroup_current_bytes\":120000000,\"cgroup_swap_bytes\":0,\"cgroup_memory_peak_bytes\":150000000,\"cgroup_oom\":5,\"cgroup_oom_kill\":2}\n",
        "{\"binary_name\":\"memory_mcp_http\",\"process_alive\":true,\"elapsed_ms\":60000,\"rss_kib\":101000,\"swap_kib\":0,\"hwm_kib\":101000,\"cgroup_current_bytes\":121000000,\"cgroup_swap_bytes\":0,\"cgroup_memory_peak_bytes\":150000000,\"cgroup_oom\":6,\"cgroup_oom_kill\":2}\n"
    );

    let directory = write_evidence(&valid_run(), samples);
    let error = validate_http_memory_evidence(directory.path())
        .expect_err("an unreported cgroup OOM must fail closed");
    assert!(error.starts_with("[cgroup]"), "{error}");
}

#[test]
fn valid_evidence_returns_measured_values() {
    let directory = write_evidence(&valid_run(), VALID_SAMPLES);

    let report = validate_http_memory_evidence(directory.path())
        .expect("complete synthetic evidence should validate");

    assert_eq!(report.peak_rss_bytes, 101_000 * 1024);
    assert_eq!(report.steady_rss_plus_swap_bytes, Some(101_000 * 1024));
    assert_eq!(report.max_swap_bytes, 0);
    assert_eq!(report.cgroup_peak_bytes, 200_000_000);
    assert_eq!(report.oom_delta, 0);
    assert_eq!(report.oom_kill_delta, 0);
    assert_eq!(report.plateau_growth_bytes, None);
    assert_eq!(report.unexpected_failed_requests, 0);
    assert_eq!(report.samples, 2);
}

#[test]
fn cold_decay_requires_the_seeded_fact_and_all_derived_claims_to_be_closed() {
    let mut run = valid_run();
    run["decay"]["invalidated_fact_ids"] = json!([]);
    run["decay"]["closed_claim_ids"] = json!([]);

    assert_rejected(run, VALID_SAMPLES, "lifecycle");
}

#[test]
fn cold_decay_rejects_a_partial_derived_claim_close() {
    let mut run = valid_run();
    run["decay"]["closed_claim_ids"] = json!(["claim:storage_memory_decay_subject"]);

    assert_rejected(run, VALID_SAMPLES, "lifecycle");
}

#[test]
fn concurrent_maintenance_requires_two_tenants_and_overlapping_requests() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("concurrent_maintenance");
    run["requests"] = json!(
        (0..8)
            .map(|index| {
                let tenant = if index < 4 { "tenant-a" } else { "tenant-b" };
                let start = if index < 4 {
                    index * 10
                } else {
                    100 + index * 10
                };
                json!({
                    "request_id": format!("maintenance-{index}"),
                    "outcome": "success",
                    "category": null,
                    "tenant_id": tenant,
                    "started_ms": start,
                    "finished_ms": start + 5
                })
            })
            .collect::<Vec<_>>()
    );

    assert_rejected(run, VALID_SAMPLES, "concurrency");
}

#[test]
fn concurrent_maintenance_accepts_eight_successful_overlapping_requests_across_two_tenants() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("concurrent_maintenance");
    run["maintenance"] = json!({ "started_ms": 10, "finished_ms": 20 });
    run["requests"] = json!(
        (0..8)
            .map(|index| {
                let tenant = if index % 2 == 0 {
                    "tenant-a"
                } else {
                    "tenant-b"
                };
                json!({
                    "request_id": format!("maintenance-{index}"),
                    "outcome": "success",
                    "category": null,
                    "tenant_id": tenant,
                    "started_ms": index,
                    "finished_ms": 100 + index
                })
            })
            .collect::<Vec<_>>()
    );
    let directory = write_evidence(&run, VALID_SAMPLES);

    let report = validate_http_memory_evidence(directory.path())
        .expect("the observed cross-tenant overlap should pass");

    assert_eq!(report.unexpected_failed_requests, 0);
}

#[test]
fn concurrent_maintenance_rejects_missing_maintenance_interval_evidence() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("concurrent_maintenance");
    run["requests"] = json!(
        (0..8)
            .map(|index| json!({
                "request_id": format!("maintenance-{index}"),
                "outcome": "success",
                "category": null,
                "tenant_id": if index % 2 == 0 { "tenant-a" } else { "tenant-b" },
                "started_ms": index,
                "finished_ms": 100 + index
            }))
            .collect::<Vec<_>>()
    );
    let directory = write_evidence(&run, VALID_SAMPLES);

    let error = validate_http_memory_evidence(directory.path())
        .expect_err("concurrent foreground traffic must overlap measured maintenance work");

    assert!(error.starts_with("[concurrency]"), "{error}");
}

#[test]
fn concurrent_maintenance_rejects_maintenance_outside_the_shared_request_window() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("concurrent_maintenance");
    run["maintenance"] = json!({ "started_ms": 200, "finished_ms": 250 });
    run["requests"] = json!(
        (0..8)
            .map(|index| json!({
                "request_id": format!("maintenance-{index}"),
                "outcome": "success",
                "category": null,
                "tenant_id": if index % 2 == 0 { "tenant-a" } else { "tenant-b" },
                "started_ms": index,
                "finished_ms": 100 + index
            }))
            .collect::<Vec<_>>()
    );

    assert_rejected(run, VALID_SAMPLES, "concurrency");
}

#[test]
fn concurrent_maintenance_rejects_one_overlapping_pair_without_eight_way_overlap() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("concurrent_maintenance");
    let intervals = [
        ("tenant-a", 0, 20),
        ("tenant-b", 10, 30),
        ("tenant-a", 40, 50),
        ("tenant-b", 60, 70),
        ("tenant-a", 80, 90),
        ("tenant-b", 100, 110),
        ("tenant-a", 120, 130),
        ("tenant-b", 140, 150),
    ];
    run["requests"] = json!(
        intervals
            .iter()
            .enumerate()
            .map(|(index, (tenant_id, started_ms, finished_ms))| json!({
                "request_id": format!("maintenance-{index}"),
                "outcome": "success",
                "category": null,
                "tenant_id": tenant_id,
                "started_ms": started_ms,
                "finished_ms": finished_ms
            }))
            .collect::<Vec<_>>()
    );

    assert_rejected(run, VALID_SAMPLES, "concurrency");
}

#[test]
fn remote_peak_rss_over_budget_fails_evidence_validation() {
    let samples = VALID_SAMPLES
        .replace("100000", "300000")
        .replace("101000", "301000");
    assert_rejected(valid_run(), &samples, "budget");
}

#[test]
fn remote_steady_memory_over_budget_fails_evidence_validation() {
    let samples = VALID_SAMPLES
        .replace("100000", "130000")
        .replace("101000", "132000");
    assert_rejected(valid_run(), &samples, "budget");
}

#[test]
fn remote_swap_usage_fails_evidence_validation() {
    let samples = VALID_SAMPLES
        .replace("\"swap_kib\":0", "\"swap_kib\":1")
        .replace("\"cgroup_swap_bytes\":0", "\"cgroup_swap_bytes\":4096");
    assert_rejected(valid_run(), &samples, "budget");
}

#[test]
fn remote_cgroup_peak_over_budget_fails_evidence_validation() {
    let mut run = valid_run();
    run["cgroup"]["memory_peak_bytes"] = json!(335_544_320);
    let samples = VALID_SAMPLES.replace("200000000", "335544320");
    assert_rejected(run, &samples, "budget");
}

#[test]
fn remote_plateau_growth_over_budget_fails_evidence_validation() {
    let (run, samples) = retrieval_plateau_evidence(100_000, 110_500);
    assert_rejected(run, &samples, "budget");
}

#[test]
fn plateau_growth_uses_checkpoint_block_medians_instead_of_window_endpoints() {
    let (run, samples) = retrieval_plateau_evidence(100_000, 110_000);

    assert_rejected(run, &samples, "budget");
}

#[test]
fn plateau_growth_at_the_eight_mib_limit_is_accepted() {
    let (run, samples) = retrieval_plateau_evidence(100_000, 108_192);
    let directory = write_evidence(&run, &samples);

    let report = validate_http_memory_evidence(directory.path())
        .expect("the inclusive 8 MiB plateau budget should pass");

    assert_eq!(report.plateau_growth_bytes, Some(8_388_608));
}

#[test]
fn retrieval_plateau_rejects_an_incomplete_checkpoint_population() {
    let (mut run, samples) = retrieval_plateau_evidence(100_000, 100_000);
    run["idle_checkpoints"]
        .as_array_mut()
        .expect("idle checkpoints")
        .pop();

    assert_rejected(run, &samples, "plateau");
}

#[test]
fn retrieval_plateau_rejects_a_checkpoint_with_inflight_requests() {
    let (mut run, samples) = retrieval_plateau_evidence(100_000, 100_000);
    run["idle_checkpoints"][0]["inflight_requests"] = json!(1);

    assert_rejected(run, &samples, "plateau");
}

#[test]
fn retrieval_plateau_rejects_a_checkpoint_without_a_matching_raw_sample() {
    let (mut run, samples) = retrieval_plateau_evidence(100_000, 100_000);
    run["idle_checkpoints"][0]["elapsed_ms"] = json!(2_021);

    assert_rejected(run, &samples, "plateau");
}

#[test]
fn retrieval_plateau_requires_exact_warmup_and_measurement_phases() {
    let (mut run, samples) = retrieval_plateau_evidence(100_000, 100_000);
    run["requests"][0]["phase"] = json!("measured");

    assert_rejected(run, &samples, "plateau");
}

#[test]
fn local_native_requires_a_paired_baseline() {
    assert_rejected(local_native_run(), VALID_SAMPLES, "budget");
}

#[test]
fn local_native_peak_cannot_exceed_its_paired_baseline() {
    let mut run = local_native_run();
    run["baseline"] = json!({
        "image_oci_digest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "peak_rss_bytes": 100_000_000,
        "steady_rss_plus_swap_bytes": 110_000_000
    });
    assert_rejected(run, VALID_SAMPLES, "budget");
}

#[test]
fn local_native_accepts_a_complete_paired_baseline() {
    let mut run = local_native_run();
    run["baseline"] = json!({
        "image_oci_digest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "peak_rss_bytes": 120_000_000,
        "steady_rss_plus_swap_bytes": 110_000_000
    });
    let directory = write_evidence(&run, VALID_SAMPLES);

    let report = validate_http_memory_evidence(directory.path())
        .expect("LocalNative evidence within its paired baseline should validate");

    assert_eq!(report.peak_rss_bytes, 101_000 * 1024);
    assert_eq!(report.steady_rss_plus_swap_bytes, Some(101_000 * 1024));
}

#[test]
fn disabled_mode_cannot_run_ann_case() {
    let mut run = valid_run();
    run["case"]["mode"] = json!({ "kind": "disabled_empty", "dimension": 384 });
    run["case"]["workload"] = json!("ann");
    run["provider"]["resolved_dimension"] = json!(384);
    run["provider"]["stored_dimensions"] = json!([]);
    run["provider"]["observed_dimensions"] = json!([]);
    run["provider"]["index_dimension"] = json!(384);
    run["provider"]["trap_listener_started"] = json!(true);
    set_disabled_identity(&mut run, 384, &[]);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn disabled_empty_mode_records_its_positive_schema_dimension() {
    let mut run = valid_run();
    run["case"]["mode"] = json!({ "kind": "disabled_empty", "dimension": 384 });
    run["provider"]["resolved_dimension"] = json!(384);
    run["provider"]["stored_dimensions"] = json!([]);
    run["provider"]["observed_dimensions"] = json!([]);
    run["provider"]["index_dimension"] = json!(384);
    run["provider"]["trap_listener_started"] = json!(true);
    set_disabled_identity(&mut run, 384, &[]);
    let directory = write_evidence(&run, VALID_SAMPLES);

    validate_http_memory_evidence(directory.path())
        .expect("disabled-empty evidence must pin the positive configured schema width");
}

#[test]
fn disabled_empty_mode_rejects_a_zero_schema_dimension() {
    let mut run = valid_run();
    run["case"]["mode"] = json!({ "kind": "disabled_empty", "dimension": 0 });
    run["provider"]["resolved_dimension"] = json!(0);
    run["provider"]["stored_dimensions"] = json!([]);
    run["provider"]["observed_dimensions"] = json!([]);
    run["provider"]["index_dimension"] = json!(0);
    run["provider"]["trap_listener_started"] = json!(true);
    set_disabled_identity(&mut run, 0, &[]);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn disabled_empty_mode_rejects_stored_vectors() {
    let mut run = valid_run();
    run["case"]["mode"] = json!({ "kind": "disabled_empty", "dimension": 384 });
    run["provider"]["resolved_dimension"] = json!(384);
    run["provider"]["stored_dimensions"] = json!([384]);
    run["provider"]["observed_dimensions"] = json!([384]);
    run["provider"]["index_dimension"] = json!(384);
    run["provider"]["trap_listener_started"] = json!(true);
    set_disabled_identity(&mut run, 384, &[384]);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn disabled_stored_mode_rejects_observed_ann_queries() {
    let mut run = valid_run();
    run["case"]["mode"] = json!({ "kind": "disabled_stored", "dimension": 3072 });
    run["provider"]["resolved_dimension"] = json!(3072);
    run["provider"]["stored_dimensions"] = json!([3072]);
    run["provider"]["observed_dimensions"] = json!([3072]);
    run["provider"]["index_dimension"] = json!(3072);
    run["provider"]["trap_listener_started"] = json!(true);
    run["provider"]["ann_queries"] = json!(1);
    set_disabled_identity(&mut run, 3072, &[3072]);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn disabled_mode_requires_an_active_trap_listener() {
    let mut run = valid_run();
    run["case"]["mode"] = json!({ "kind": "disabled_empty", "dimension": 384 });
    run["provider"]["resolved_dimension"] = json!(384);
    run["provider"]["stored_dimensions"] = json!([]);
    run["provider"]["observed_dimensions"] = json!([]);
    run["provider"]["index_dimension"] = json!(384);
    set_disabled_identity(&mut run, 384, &[]);

    assert_rejected(run, VALID_SAMPLES, "provider");
}

#[test]
fn disabled_stored_mode_accepts_zero_ann_and_trap_requests() {
    let mut run = valid_run();
    run["case"]["mode"] = json!({ "kind": "disabled_stored", "dimension": 3072 });
    run["provider"]["resolved_dimension"] = json!(3072);
    run["provider"]["stored_dimensions"] = json!([3072]);
    run["provider"]["observed_dimensions"] = json!([3072]);
    run["provider"]["index_dimension"] = json!(3072);
    run["provider"]["trap_listener_started"] = json!(true);
    set_disabled_identity(&mut run, 3072, &[3072]);
    let directory = write_evidence(&run, VALID_SAMPLES);

    validate_http_memory_evidence(directory.path())
        .expect("disabled stored vectors should be valid with an active zero-call trap");
}

#[test]
fn wrong_resolved_width_invalidates_seed() {
    let mut run = valid_run();
    run["provider"]["resolved_dimension"] = json!(2048);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn remote_evidence_requires_provider_model_revision_and_embedding_signature() {
    let mut run = valid_run();
    run["provider"]["provider_id"] = json!(null);
    run["provider"]["model_revision"] = json!(null);
    run["provider"]["configured_signature"] = json!(null);

    assert_rejected(run, VALID_SAMPLES, "provider");
}

#[test]
fn remote_evidence_rejects_a_provider_label_from_another_mode() {
    let mut run = valid_run();
    run["provider"]["provider_id"] = json!("local-candle");

    assert_rejected(run, VALID_SAMPLES, "provider");
}

#[test]
fn remote_evidence_rejects_configured_dimension_mismatch() {
    let mut run = valid_run();
    run["provider"]["configured_dimension"] = json!(768);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn remote_evidence_rejects_resolved_provider_dimension_mismatch() {
    let mut run = valid_run();
    run["provider"]["provider_dimension"] = json!(768);

    assert_rejected(run, VALID_SAMPLES, "provider");
}

#[test]
fn remote_evidence_rejects_schema_dimension_mismatch() {
    let mut run = valid_run();
    run["provider"]["schema_dimension"] = json!(768);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn stable_remote_evidence_requires_persisted_signature_to_match_provider_signature() {
    let mut run = valid_run();
    run["provider"]["persisted_embeddings"] = json!([
        { "dimension": 384, "signature": "sig:old-model:d384" }
    ]);

    assert_rejected(run, VALID_SAMPLES, "provider");
}

#[test]
fn wrong_stored_vector_width_invalidates_seed() {
    let mut run = valid_run();
    run["provider"]["stored_dimensions"] = json!([2048]);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn wrong_ann_index_width_invalidates_seed() {
    let mut run = valid_run();
    run["provider"]["index_dimension"] = json!(2048);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn reembed_in_progress_accepts_only_the_source_and_target_vector_widths() {
    let mut run = valid_run();
    run["case"]["id"] = json!("http-remote2048-reembed-d384-to-d2048-r1");
    run["expected_case_ids"] = json!(["http-remote2048-reembed-d384-to-d2048-r1"]);
    run["executed_case_ids"] = json!(["http-remote2048-reembed-d384-to-d2048-r1"]);
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 2048 });
    run["case"]["phase"] = json!("in_progress");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 2048 });
    run["provider"]["resolved_dimension"] = json!(2048);
    run["provider"]["stored_dimensions"] = json!([384, 2048]);
    run["provider"]["observed_dimensions"] = json!([384, 2048]);
    run["provider"]["index_dimension"] = json!(2048);
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[
            (384, "sig:test-model@v1:d384"),
            (2048, "sig:test-model@v1:d2048"),
        ],
    );
    run["rewrite"] = valid_running_rewrite_evidence();
    let directory = write_evidence(&run, VALID_SAMPLES);

    validate_http_memory_evidence(directory.path())
        .expect("a running reembed may contain source and target widths only");
}

#[test]
fn reembed_transition_supports_provider_defined_embedding_widths() {
    let mut run = valid_run();
    let case_id = "http-remote3072-reembed-d768-to-d3072-r1";
    run["case"]["id"] = json!(case_id);
    run["expected_case_ids"] = json!([case_id]);
    run["executed_case_ids"] = json!([case_id]);
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 768, "to": 3072 });
    run["case"]["phase"] = json!("in_progress");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 3072 });
    run["provider"]["resolved_dimension"] = json!(3072);
    run["provider"]["stored_dimensions"] = json!([768, 3072]);
    run["provider"]["observed_dimensions"] = json!([768, 3072]);
    run["provider"]["index_dimension"] = json!(3072);
    set_remote_identity(
        &mut run,
        3072,
        "test-model@v1",
        &[
            (768, "sig:test-model@v1:d768"),
            (3072, "sig:test-model@v1:d3072"),
        ],
    );
    run["rewrite"] = valid_running_rewrite_evidence();
    let directory = write_evidence(&run, VALID_SAMPLES);

    validate_http_memory_evidence(directory.path())
        .expect("the transition gate must accept valid provider-defined dimensions");
}

#[test]
fn dimension_transition_rejects_a_zero_source_width() {
    let mut run = valid_run();
    let case_id = "http-remote384-reembed-d0-to-d384-r1";
    run["case"]["id"] = json!(case_id);
    run["expected_case_ids"] = json!([case_id]);
    run["executed_case_ids"] = json!([case_id]);
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 0, "to": 384 });
    run["case"]["phase"] = json!("in_progress");
    run["provider"]["stored_dimensions"] = json!([0, 384]);
    run["provider"]["observed_dimensions"] = json!([0, 384]);
    run["rewrite"] = valid_running_rewrite_evidence();

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn dimension_transition_rejects_an_unchanged_width() {
    let mut run = valid_run();
    let case_id = "http-remote384-reembed-d384-to-d384-r1";
    run["case"]["id"] = json!(case_id);
    run["expected_case_ids"] = json!([case_id]);
    run["executed_case_ids"] = json!([case_id]);
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 384 });
    run["case"]["phase"] = json!("in_progress");
    run["rewrite"] = valid_running_rewrite_evidence();

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn signature_transition_accepts_a_same_width_reembed() {
    let mut run = valid_run();
    let case_id = "http-remote384-reembed-signature-d384-r1";
    run["case"]["id"] = json!(case_id);
    run["expected_case_ids"] = json!([case_id]);
    run["executed_case_ids"] = json!([case_id]);
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "signature", "dimension": 384 });
    run["case"]["phase"] = json!("in_progress");
    run["rewrite"] = valid_running_rewrite_evidence();
    set_remote_identity(
        &mut run,
        384,
        "test-model@v2",
        &[
            (384, "sig:test-model@v1:d384"),
            (384, "sig:test-model@v2:d384"),
        ],
    );
    let directory = write_evidence(&run, VALID_SAMPLES);

    validate_http_memory_evidence(directory.path())
        .expect("a signature change may reembed without changing vector width");
}

#[test]
fn signature_transition_requires_old_and_new_persisted_signatures_while_running() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "signature", "dimension": 384 });
    run["case"]["phase"] = json!("in_progress");
    set_remote_identity(
        &mut run,
        384,
        "test-model@v2",
        &[(384, "sig:test-model@v2:d384")],
    );

    assert_rejected(run, VALID_SAMPLES, "provider");
}

#[test]
fn reembed_case_cannot_claim_a_steady_run_without_a_transition() {
    let mut run = valid_run();
    run["case"]["id"] = json!("http-remote384-reembed-steady-r1");
    run["expected_case_ids"] = json!(["http-remote384-reembed-steady-r1"]);
    run["executed_case_ids"] = json!(["http-remote384-reembed-steady-r1"]);
    run["case"]["scenario"] = json!("reembed");

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn interrupted_resume_finishes_at_the_target_width_after_observing_both_widths() {
    let mut run = valid_run();
    run["case"]["id"] = json!("http-remote2048-interrupted-resume-d384-to-d2048-r1");
    run["expected_case_ids"] = json!(["http-remote2048-interrupted-resume-d384-to-d2048-r1"]);
    run["executed_case_ids"] = json!(["http-remote2048-interrupted-resume-d384-to-d2048-r1"]);
    run["case"]["scenario"] = json!("interrupted_resume");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 2048 });
    run["case"]["phase"] = json!("after_resume");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 2048 });
    run["provider"]["resolved_dimension"] = json!(2048);
    run["provider"]["stored_dimensions"] = json!([2048]);
    run["provider"]["observed_dimensions"] = json!([384, 2048]);
    run["provider"]["index_dimension"] = json!(2048);
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[(2048, "sig:test-model@v1:d2048")],
    );
    run["rewrite"] = valid_interrupted_resume_evidence();
    let directory = write_evidence(&run, VALID_SAMPLES);

    validate_http_memory_evidence(directory.path())
        .expect("resume must finish with only the target width while recording its transition");
}

#[test]
fn reembed_in_progress_rejects_an_unrelated_stored_width() {
    let mut run = valid_run();
    run["case"]["id"] = json!("http-remote2048-reembed-d384-to-d2048-r1");
    run["expected_case_ids"] = json!(["http-remote2048-reembed-d384-to-d2048-r1"]);
    run["executed_case_ids"] = json!(["http-remote2048-reembed-d384-to-d2048-r1"]);
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 2048 });
    run["case"]["phase"] = json!("in_progress");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 2048 });
    run["provider"]["resolved_dimension"] = json!(2048);
    run["provider"]["stored_dimensions"] = json!([384, 1536, 2048]);
    run["provider"]["observed_dimensions"] = json!([384, 1536, 2048]);
    run["provider"]["index_dimension"] = json!(2048);
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[
            (384, "sig:test-model@v1:d384"),
            (1536, "sig:test-model@v1:d1536"),
            (2048, "sig:test-model@v1:d2048"),
        ],
    );

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn completed_reembed_requires_a_durable_rewrite_report() {
    let mut run = valid_run();
    run["case"]["id"] = json!("http-remote2048-reembed-d384-to-d2048-after-r1");
    run["expected_case_ids"] = json!(["http-remote2048-reembed-d384-to-d2048-after-r1"]);
    run["executed_case_ids"] = json!(["http-remote2048-reembed-d384-to-d2048-after-r1"]);
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 2048 });
    run["case"]["phase"] = json!("after");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 2048 });
    run["provider"]["resolved_dimension"] = json!(2048);
    run["provider"]["stored_dimensions"] = json!([2048]);
    run["provider"]["observed_dimensions"] = json!([384, 2048]);
    run["provider"]["index_dimension"] = json!(2048);
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[(2048, "sig:test-model@v1:d2048")],
    );

    assert_rejected(run, VALID_SAMPLES, "rewrite");
}

#[test]
fn completed_reembed_accepts_all_205_durable_facts_in_three_batches() {
    let mut run = valid_run();
    run["case"]["id"] = json!("http-remote2048-reembed-d384-to-d2048-after-r1");
    run["expected_case_ids"] = json!(["http-remote2048-reembed-d384-to-d2048-after-r1"]);
    run["executed_case_ids"] = json!(["http-remote2048-reembed-d384-to-d2048-after-r1"]);
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 2048 });
    run["case"]["phase"] = json!("after");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 2048 });
    run["provider"]["resolved_dimension"] = json!(2048);
    run["provider"]["stored_dimensions"] = json!([2048]);
    run["provider"]["observed_dimensions"] = json!([384, 2048]);
    run["provider"]["index_dimension"] = json!(2048);
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[(2048, "sig:test-model@v1:d2048")],
    );
    run["rewrite"] = valid_complete_rewrite_evidence();
    let directory = write_evidence(&run, VALID_SAMPLES);

    validate_http_memory_evidence(directory.path())
        .expect("completed rewrite evidence should prove all durable source facts were preserved");
}

#[test]
fn completed_reembed_rejects_a_changed_fact_identity() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 2048 });
    run["case"]["phase"] = json!("after");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 2048 });
    run["provider"]["resolved_dimension"] = json!(2048);
    run["provider"]["stored_dimensions"] = json!([2048]);
    run["provider"]["observed_dimensions"] = json!([384, 2048]);
    run["provider"]["index_dimension"] = json!(2048);
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[(2048, "sig:test-model@v1:d2048")],
    );
    run["rewrite"] = valid_complete_rewrite_evidence();
    run["rewrite"]["final_fact_ids"][204] = json!("fact:storage_memory_replaced");

    assert_rejected(run, VALID_SAMPLES, "rewrite");
}

#[test]
fn completed_reembed_rejects_a_batch_schedule_that_differs_from_the_public_flow() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 2048 });
    run["case"]["phase"] = json!("after");
    run["case"]["mode"] = json!({ "kind": "remote", "dimension": 2048 });
    run["provider"]["resolved_dimension"] = json!(2048);
    run["provider"]["stored_dimensions"] = json!([2048]);
    run["provider"]["observed_dimensions"] = json!([384, 2048]);
    run["provider"]["index_dimension"] = json!(2048);
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[(2048, "sig:test-model@v1:d2048")],
    );
    run["rewrite"] = valid_complete_rewrite_evidence();
    run["rewrite"]["batch_sizes"] = json!([100, 99, 6]);

    assert_rejected(run, VALID_SAMPLES, "rewrite");
}

#[test]
fn unlisted_mode_does_not_silently_skip() {
    let mut run = valid_run();
    run["case"]["mode"] = json!({ "kind": "unlisted_native_mode" });

    assert_rejected(run, VALID_SAMPLES, "schema");
}

#[test]
fn unexpected_error_cannot_be_marked_as_injected() {
    let mut run = valid_run();
    run["case"]["scenario"] = json!("error_release");
    run["expected_faults"] = json!(["unexpected"]);
    run["requests"] = json!([
        { "request_id": "fault", "outcome": "expected_fault", "category": "unexpected" },
        { "request_id": "recovered", "outcome": "success", "category": null }
    ]);

    assert_rejected(run, VALID_SAMPLES, "request");
}

#[test]
fn missing_required_case_is_incomplete() {
    let mut run = valid_run();
    run["expected_case_ids"] = json!([
        "http-remote384-cold-decay-r1",
        "http-remote384-cold-decay-r2"
    ]);

    assert_rejected(run, VALID_SAMPLES, "manifest");
}

#[test]
fn valid_suite_returns_one_validated_report_per_case() {
    let first_id = "http-remote384-cold-decay-r1";
    let second_id = "http-disabled3072-cold-decay-r1";
    let first = valid_run();
    let mut second = valid_run();
    second["case"]["id"] = json!(second_id);
    second["case"]["mode"] = json!({ "kind": "disabled_stored", "dimension": 3072 });
    second["provider"]["resolved_dimension"] = json!(3072);
    second["provider"]["stored_dimensions"] = json!([3072]);
    second["provider"]["observed_dimensions"] = json!([3072]);
    second["provider"]["index_dimension"] = json!(3072);
    second["provider"]["trap_listener_started"] = json!(true);
    set_disabled_identity(&mut second, 3072, &[3072]);
    let directory = write_suite(&[(first_id, first), (second_id, second)]);

    let reports = validate_http_memory_suite(
        directory.path(),
        &[first_id.to_string(), second_id.to_string()],
    )
    .expect("a complete suite with independently valid cases should pass");

    assert_eq!(reports.len(), 2);
    assert!(reports.iter().all(|report| report.samples > 0));
}

#[test]
fn suite_rejects_run_artifact_stored_under_another_case_id() {
    let first_id = "http-remote384-cold-decay-r1";
    let second_id = "http-disabled3072-cold-decay-r1";
    let mut first = valid_run();
    first["case"]["id"] = json!(second_id);
    first["expected_case_ids"] = json!([second_id]);
    first["executed_case_ids"] = json!([second_id]);
    let mut second = valid_run();
    second["case"]["id"] = json!(first_id);
    second["expected_case_ids"] = json!([first_id]);
    second["executed_case_ids"] = json!([first_id]);
    let directory = write_suite(&[(first_id, first), (second_id, second)]);

    let error = validate_http_memory_suite(
        directory.path(),
        &[first_id.to_string(), second_id.to_string()],
    )
    .expect_err("case directory and run identity must agree");

    assert!(error.contains("manifest"), "{error}");
}

#[test]
fn wrong_source_image_bundle_is_invalid() {
    let mut run = valid_run();
    run["image"]["source_bundle_digest"] =
        json!("sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd");

    assert_rejected(run, VALID_SAMPLES, "bundle");
}

#[test]
fn oci_hash_mismatch_is_invalid() {
    let mut run = valid_run();
    run["image"]["oci_digest"] =
        json!("sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

    assert_rejected(run, VALID_SAMPLES, "image");
}

#[test]
fn instrumented_app_features_are_refused() {
    let mut run = valid_run();
    run["image"]["app_features"] = json!([
        "streamable-http",
        "mcp-apps",
        "eval-support",
        "test-fixtures"
    ]);

    assert_rejected(run, VALID_SAMPLES, "features");
}

#[test]
fn memory_evidence_requires_the_system_allocator() {
    let mut run = valid_run();
    run["image"]["app_features"] = json!(["streamable-http", "mcp-apps", "fs-watch", "mimalloc"]);

    assert_rejected(run, VALID_SAMPLES, "features");
}

#[test]
fn composition_evidence_cannot_omit_the_measured_binary_and_transport() {
    let mut run = valid_run();
    run["case"]["composition"] = json!("stdio");
    run["image"]["app_features"] = json!(["mcp-apps", "fs-watch"]);

    assert_rejected(run, VALID_SAMPLES, "process");
}

#[test]
fn a_retrieval_workload_cannot_be_attached_to_cold_decay() {
    let mut run = valid_run();
    run["case"]["workload"] = json!("lexical_fallback");

    assert_rejected(run, VALID_SAMPLES, "manifest");
}

#[test]
fn dashboard_workload_requires_http_composition() {
    let mut run = valid_run();
    run["case"]["composition"] = json!("stdio");
    run["case"]["scenario"] = json!("retrieval_misses");
    run["case"]["workload"] = json!("dashboard");
    run["image"]["app_features"] = json!(["mcp-apps", "fs-watch"]);
    run["process"]["binary_name"] = json!("memory_mcp");
    run["process"]["transport"] = json!("stdio_mcp");
    run["requests"] = json!(
        (0..100)
            .map(|index| json!({
                "request_id": format!("dashboard-{index}"),
                "outcome": "success",
                "category": null
            }))
            .collect::<Vec<_>>()
    );
    run["db_reads"]["events"] = json!(
        (0..100)
            .map(|index| json!({
                "request_id": format!("dashboard-{index}"),
                "foreground": true,
                "statement_kind": "SELECT",
                "cache_hit": false
            }))
            .collect::<Vec<_>>()
    );

    assert_rejected(run, VALID_SAMPLES, "manifest");
}

#[test]
fn disabled_mode_cannot_claim_an_embedding_rewrite() {
    let mut run = valid_run();
    let case_id = "http-disabled-empty-reembed-r1";
    run["case"]["id"] = json!(case_id);
    run["expected_case_ids"] = json!([case_id]);
    run["executed_case_ids"] = json!([case_id]);
    run["case"]["mode"] = json!({ "kind": "disabled_empty", "dimension": 384 });
    run["case"]["scenario"] = json!("reembed");
    run["case"]["transition"] = json!({ "kind": "dimension", "from": 384, "to": 768 });
    run["case"]["phase"] = json!("in_progress");
    run["provider"]["resolved_dimension"] = json!(384);
    run["provider"]["stored_dimensions"] = json!([]);
    run["provider"]["observed_dimensions"] = json!([]);
    run["provider"]["index_dimension"] = json!(384);
    run["provider"]["trap_listener_started"] = json!(true);
    run["rewrite"] = valid_running_rewrite_evidence();
    set_disabled_identity(&mut run, 384, &[]);

    assert_rejected(run, VALID_SAMPLES, "mode");
}

#[test]
fn cli_backfill_is_not_misrepresented_as_a_public_operator_flow() {
    let mut run = cli_rewrite_run();
    run["case"]["scenario"] = json!("backfill");
    run["case"]["transition"] = json!({ "kind": "backfill" });
    run["provider"]["stored_dimensions"] = json!([]);
    run["provider"]["observed_dimensions"] = json!([2048]);
    run["rewrite"]["source_fact_ids"] = json!(seeded_rewrite_fact_ids());
    run["rewrite"]["final_fact_ids"] = json!(seeded_rewrite_fact_ids());
    set_remote_identity(&mut run, 2048, "test-model@v1", &[]);

    assert_rejected(run, VALID_SAMPLES, "manifest");
}

#[test]
fn cli_cannot_claim_the_http_durable_interrupted_resume_flow() {
    let mut run = cli_rewrite_run();
    run["case"]["id"] = json!("cli-remote2048-interrupted-resume-r1");
    run["case"]["scenario"] = json!("interrupted_resume");
    run["case"]["phase"] = json!("after_resume");
    run["provider"]["stored_dimensions"] = json!([2048]);
    run["rewrite"] = valid_interrupted_resume_evidence();
    run["expected_case_ids"] = json!(["cli-remote2048-interrupted-resume-r1"]);
    run["executed_case_ids"] = json!(["cli-remote2048-interrupted-resume-r1"]);
    set_remote_identity(
        &mut run,
        2048,
        "test-model@v1",
        &[(2048, "sig:test-model@v1:d2048")],
    );

    assert_rejected(run, VALID_SAMPLES, "manifest");
}

#[test]
fn cli_reembed_action_can_be_measured_without_a_long_lived_plateau_window() {
    let mut run = cli_rewrite_run();
    run["sample_count"] = json!(1);
    run["process"]["steady_window_ms"] = json!(0);
    let sample = "{\"binary_name\":\"memory_mcp\",\"process_alive\":true,\"elapsed_ms\":0,\"rss_kib\":100000,\"swap_kib\":0,\"hwm_kib\":100000,\"cgroup_current_bytes\":120000000,\"cgroup_swap_bytes\":0,\"cgroup_memory_peak_bytes\":200000000,\"cgroup_oom\":0,\"cgroup_oom_kill\":0}\n";
    let directory = write_evidence(&run, sample);

    let report = validate_http_memory_evidence(directory.path())
        .expect("one-shot CLI reembed is bounded by its full action lifetime");

    assert_eq!(report.samples, 1);
    assert_eq!(report.steady_rss_plus_swap_bytes, None);
    assert_eq!(report.plateau_growth_bytes, None);
    assert_eq!(report.peak_rss_bytes, 100_000 * 1024);
}

#[test]
fn cli_wrapper_sample_cannot_stand_in_for_the_measured_binary() {
    let mut run = cli_rewrite_run();
    run["sample_count"] = json!(1);
    run["process"]["steady_window_ms"] = json!(0);
    let sample = "{\"binary_name\":\"memory_exec_barrier\",\"process_alive\":true,\"elapsed_ms\":0,\"rss_kib\":100000,\"swap_kib\":0,\"hwm_kib\":100000,\"cgroup_current_bytes\":120000000,\"cgroup_swap_bytes\":0,\"cgroup_memory_peak_bytes\":200000000,\"cgroup_oom\":0,\"cgroup_oom_kill\":0}\n";

    assert_rejected(run, sample, "sampler");
}

#[test]
fn exec_barrier_holds_the_app_until_release_and_preserves_process_identity() {
    let directory = tempfile::tempdir().expect("barrier tempdir should be available");
    let wrapper_source = directory.path().join("memory_exec_barrier.rs");
    let wrapper = directory.path().join("memory_exec_barrier");
    fs::write(
        &wrapper_source,
        include_str!("fixtures/memory_exec_barrier.rs"),
    )
    .expect("wrapper source should be copied into the isolated fixture");
    let compile = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&wrapper_source)
        .arg("-o")
        .arg(&wrapper)
        .output()
        .expect("pinned rustc should compile the std-only wrapper");
    assert!(
        compile.status.success(),
        "wrapper compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let gate = directory.path().join("release.gate");
    let report = directory.path().join("child.json");
    let listener = TcpListener::bind("127.0.0.1:0").expect("child probe listener should bind");
    let probe_address = listener
        .local_addr()
        .expect("listener address should be available");
    let (connected, arrived) = mpsc::channel();
    std::thread::spawn(move || {
        let result = listener.accept().and_then(|(mut stream, _)| {
            let mut bytes = [0_u8; 4096];
            let read = stream.read(&mut bytes)?;
            String::from_utf8(bytes[..read].to_vec()).map_err(std::io::Error::other)
        });
        let _ = connected.send(result);
    });

    let mut child = Command::new(&wrapper)
        .arg("--gate")
        .arg(&gate)
        .arg("--deadline-secs")
        .arg("5")
        .arg("--")
        .arg(std::env::current_exe().expect("test executable path is available"))
        .arg("--exact")
        .arg("memory_exec_barrier_child_probe")
        .arg("--nocapture")
        .env("MEMORY_EXEC_PROBE_REPORT", &report)
        .env("MEMORY_EXEC_PROBE_GATE", &gate)
        .env("MEMORY_EXEC_PROBE_ADDRESS", probe_address.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("barrier process should start");
    let child_pid = child.id();
    let stdout = child.stdout.take().expect("wrapper stdout should be piped");
    let mut stdout = BufReader::new(stdout);
    let mut waiting = String::new();
    stdout
        .read_line(&mut waiting)
        .expect("wrapper should announce its blocked state");
    assert!(
        waiting.starts_with("MEMORY_EXEC_BARRIER_WAITING pid="),
        "wrapper did not report its waiting state: {waiting:?}"
    );
    let waiting_fields = waiting
        .split_whitespace()
        .filter_map(|field| field.split_once('='))
        .collect::<std::collections::HashMap<_, _>>();
    let reported_pid = waiting_fields
        .get("pid")
        .expect("waiting event should include PID")
        .parse::<u32>()
        .expect("waiting PID should be numeric");
    let reported_start_time = waiting_fields
        .get("start_time_ticks")
        .expect("waiting event should include start time")
        .parse::<u64>()
        .expect("waiting start time should be numeric");
    assert_eq!(reported_pid, child_pid);

    let early_probe = arrived.recv_timeout(Duration::from_millis(250));
    assert!(
        matches!(early_probe, Err(mpsc::RecvTimeoutError::Timeout)),
        "the application reached exec before release: {early_probe:?}"
    );
    assert!(
        !gate.exists(),
        "the test must hold the explicit gate closed"
    );
    fs::write(&gate, "release\n").expect("test should release the gate explicitly");

    let body = arrived
        .recv_timeout(Duration::from_secs(5))
        .expect("child should report after the gate is released")
        .expect("child probe should connect successfully");
    let child_report: Value =
        serde_json::from_str(&body).expect("child report should be valid JSON");
    assert_eq!(child_report["gate_present_at_entry"], true);
    assert_eq!(child_report["pid"].as_u64(), Some(u64::from(child_pid)));
    assert_eq!(
        child_report["start_time_ticks"].as_u64(),
        Some(reported_start_time)
    );
    let status = child
        .wait()
        .expect("exec'd probe should exit and be reaped");
    assert!(status.success(), "exec'd probe exited with {status}");
}

#[test]
fn memory_exec_barrier_child_probe() {
    let (Ok(report), Ok(gate), Ok(address)) = (
        std::env::var("MEMORY_EXEC_PROBE_REPORT"),
        std::env::var("MEMORY_EXEC_PROBE_GATE"),
        std::env::var("MEMORY_EXEC_PROBE_ADDRESS"),
    ) else {
        return;
    };
    let start_time_ticks = std::env::var("MEMORY_EXEC_BARRIER_START_TICKS")
        .expect("wrapper should forward its process start time")
        .parse::<u64>()
        .expect("wrapper start time should be numeric");
    #[cfg(target_os = "linux")]
    assert_eq!(
        linux_process_start_ticks(std::process::id())
            .expect("Linux process start time should be readable"),
        start_time_ticks,
        "exec must retain the same Linux process start time"
    );
    let payload = json!({
        "gate_present_at_entry": std::path::Path::new(&gate).exists(),
        "pid": std::process::id(),
        "start_time_ticks": start_time_ticks
    });
    fs::write(&report, payload.to_string()).expect("child report should be written");
    let mut stream = TcpStream::connect(address).expect("child should signal the parent listener");
    stream
        .write_all(payload.to_string().as_bytes())
        .expect("child report should reach the parent");
}

#[cfg(target_os = "linux")]
fn linux_process_start_ticks(pid: u32) -> std::io::Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let close = stat.rfind(')').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "process stat lacks comm terminator",
        )
    })?;
    stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "process start time missing",
            )
        })?
        .parse()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}
