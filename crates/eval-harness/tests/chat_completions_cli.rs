use std::fs;
use std::process::Command;

use eval_harness::paired_agent::chat_completions::{
    ChatCompletionsSampling, request_parameters_config_sha256, response_format_config_sha256,
};
use serde_json::{Value, json};

fn write_hash_inputs(directory: &std::path::Path, sampling: &str) -> (String, String) {
    let tasks = directory.join("tasks.json");
    let sampling_config = directory.join("sampling.json");
    fs::write(
        &tasks,
        include_str!("../../../evals/paired/source-authority-local-chat-completions-v4-qualification/tasks-v4.json"),
    )
    .expect("write registered synthetic tasks");
    fs::write(&sampling_config, sampling).expect("write sampler config");
    (
        tasks.display().to_string(),
        sampling_config.display().to_string(),
    )
}

#[test]
fn hash_chat_completions_tasks_emits_the_sampler_fingerprint() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let (tasks, sampling_config) = write_hash_inputs(
        directory.path(),
        r#"{"temperature":1.0,"top_p":0.95,"top_k":20,"min_p":0.0,"presence_penalty":0.0,"repeat_penalty":1.0}"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "hash-chat-completions-tasks",
            "--tasks",
            &tasks,
            "--sampling-config",
            &sampling_config,
        ])
        .output()
        .expect("run task fingerprint command");

    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).expect("parse fingerprint JSON");
    let expected = ChatCompletionsSampling {
        temperature: 1.0,
        top_p: Some(0.95),
        top_k: Some(20),
        min_p: Some(0.0),
        presence_penalty: Some(0.0),
        repeat_penalty: Some(1.0),
    }
    .fingerprint_sha256()
    .expect("valid sampler has a fingerprint");

    assert_eq!(result["chat_completions_sampling_config_sha256"], expected);
}

#[test]
fn hash_chat_completions_tasks_rejects_an_invalid_sampler_config() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let (tasks, sampling_config) = write_hash_inputs(
        directory.path(),
        r#"{"temperature":1.0,"top_p":1.5,"top_k":20}"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "hash-chat-completions-tasks",
            "--tasks",
            &tasks,
            "--sampling-config",
            &sampling_config,
        ])
        .output()
        .expect("run task fingerprint command");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("invalid Chat Completions sampling configuration")
    );
}

#[test]
fn hash_chat_completions_tasks_emits_the_response_format_fingerprint() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let tasks = directory.path().join("tasks.json");
    let response_format_config = directory.path().join("response-format.json");
    fs::write(
        &tasks,
        include_str!("../../../evals/paired/source-authority-local-chat-completions-v4-qualification/tasks-v4.json"),
    )
    .expect("write registered synthetic tasks");
    let response_format = json!({"type":"json_object"});
    fs::write(&response_format_config, response_format.to_string())
        .expect("write response-format config");

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "hash-chat-completions-tasks",
            "--tasks",
            tasks.to_str().unwrap(),
            "--response-format-config",
            response_format_config.to_str().unwrap(),
        ])
        .output()
        .expect("run task fingerprint command");

    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).expect("parse fingerprint JSON");
    assert_eq!(
        result["chat_completions_response_format_config_sha256"],
        response_format_config_sha256(&response_format).unwrap()
    );
}

#[test]
fn hash_chat_completions_tasks_rejects_an_invalid_response_format_config() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let tasks = directory.path().join("tasks.json");
    let response_format_config = directory.path().join("response-format.json");
    fs::write(
        &tasks,
        include_str!("../../../evals/paired/source-authority-local-chat-completions-v4-qualification/tasks-v4.json"),
    )
    .expect("write registered synthetic tasks");
    fs::write(&response_format_config, r#"{"type":"unsupported"}"#)
        .expect("write unsupported response-format config");

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "hash-chat-completions-tasks",
            "--tasks",
            tasks.to_str().unwrap(),
            "--response-format-config",
            response_format_config.to_str().unwrap(),
        ])
        .output()
        .expect("run task fingerprint command");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("invalid Chat Completions response-format configuration")
    );
}

#[test]
fn hash_chat_completions_tasks_emits_provider_request_parameter_fingerprint() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let tasks = directory.path().join("tasks.json");
    let request_parameters_config = directory.path().join("request-parameters.json");
    fs::write(
        &tasks,
        include_str!("../../../evals/paired/source-authority-local-chat-completions-v4-qualification/tasks-v4.json"),
    )
    .expect("write registered synthetic tasks");
    let request_parameters = json!({
        "chat_template_kwargs": {"enable_thinking": false}
    });
    fs::write(&request_parameters_config, request_parameters.to_string())
        .expect("write provider request parameters");

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "hash-chat-completions-tasks",
            "--tasks",
            tasks.to_str().unwrap(),
            "--request-parameters-config",
            request_parameters_config.to_str().unwrap(),
        ])
        .output()
        .expect("run task fingerprint command");

    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).expect("parse fingerprint JSON");
    assert_eq!(
        result["chat_completions_request_parameters_config_sha256"],
        request_parameters_config_sha256(&request_parameters).unwrap()
    );
}

#[test]
fn hash_chat_completions_tasks_rejects_request_parameters_that_override_protocol_fields() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let tasks = directory.path().join("tasks.json");
    let request_parameters_config = directory.path().join("request-parameters.json");
    fs::write(
        &tasks,
        include_str!("../../../evals/paired/source-authority-local-chat-completions-v4-qualification/tasks-v4.json"),
    )
    .expect("write registered synthetic tasks");
    fs::write(&request_parameters_config, r#"{"messages":[]}"#)
        .expect("write conflicting request parameters");

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "hash-chat-completions-tasks",
            "--tasks",
            tasks.to_str().unwrap(),
            "--request-parameters-config",
            request_parameters_config.to_str().unwrap(),
        ])
        .output()
        .expect("run task fingerprint command");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("invalid Chat Completions request-parameters configuration")
    );
}
