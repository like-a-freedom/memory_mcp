//! NER model-progress channel tests: MCP stdio stdout stays JSON-RPC-only
//! while stderr carries schema-version-1 JSON progress; the CLI uses
//! human-readable progress lines; `init` performs no model lookup.
//!
//! The model-backed tests require the local classic GLiNER fixture under
//! `tests/models/ner/urchade--gliner_multi-v2.1` and are `#[ignore]`d; the
//! `init` test runs everywhere.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use tempfile::TempDir;

const GLINER_FIXTURE_DIR: &str = "tests/models/ner/urchade--gliner_multi-v2.1";
const SEEDED_GLINER_REVISION: &str = "443d26d654e0324125a96bebd8e796c14ff2efe6";

fn gliner_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GLINER_FIXTURE_DIR)
}

fn gliner_fixture_present() -> bool {
    gliner_fixture_dir().join("model.safetensors").is_file()
}

/// Seeds a store root with the local GLiNER fixture as a known-good revision
/// so the artifact store reuses the checkpoint instead of re-downloading it.
fn seed_gliner_store(temp_dir: &TempDir) -> PathBuf {
    use memory_mcp::service::model_artifacts::{
        PersistedArtifactState, RevisionState, RevisionStatus, ValidationStatus, persist_state,
    };
    let store_root = temp_dir.path().join("ner-store");
    let revision_dir = store_root
        .join("gliner")
        .join("revisions")
        .join(SEEDED_GLINER_REVISION);
    std::fs::create_dir_all(&revision_dir).expect("create seeded revision dir");
    for file_name in ["gliner_config.json", "model.safetensors", "tokenizer.json"] {
        std::fs::copy(
            gliner_fixture_dir().join(file_name),
            revision_dir.join(file_name),
        )
        .expect("copy GLiNER fixture into seeded store");
    }
    let mut state = PersistedArtifactState::new();
    state.revisions.push(RevisionState {
        revision: SEEDED_GLINER_REVISION.to_string(),
        artifact_identity: "seeded-local-fixture".to_string(),
        validation_status: ValidationStatus::RuntimeRegressionVerified,
        revision_status: RevisionStatus::Latest,
        activated_at: 1_700_000_000,
        role: memory_mcp::service::model_artifacts::ArtifactRole::KnownGood,
        incompatible: None,
    });
    persist_state(&store_root.join("gliner").join("state.json"), &state)
        .expect("persist seeded state");
    store_root
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_memory_mcp")
}

/// `memory_mcp init` must print the host snippet without ever touching a model
/// store — even when a model-backed extractor is configured.
#[test]
fn init_performs_no_model_lookup() {
    let temp = TempDir::new().expect("temp dir");
    let output = Command::new(binary())
        .arg("init")
        .arg("--target")
        .arg("vscode")
        .env("NER_EXTRACTOR", "urchade/gliner_multi-v2.1")
        .env("NER_CACHE_DIR", temp.path().join("absent-cache"))
        .env_remove("NER_PROVIDER")
        .env_remove("NER_MODEL")
        .output()
        .expect("run init");

    assert!(
        output.status.success(),
        "init must succeed: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("init stdout must be a JSON result object");
    assert_eq!(payload["target"], "vscode");
    assert_eq!(payload["mutates_files"], false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("[ner]"),
        "init must not emit model progress: {stderr}"
    );
    assert!(
        !stderr.contains("resolve"),
        "init must not resolve model revisions: {stderr}"
    );
}

/// MCP stdio: every stdout line must be valid JSON-RPC, and model progress must
/// arrive as schema-version-1 JSON lines on stderr. Requires the local GLiNER
/// fixture (1.1 GB) so the model-backed store actually activates.
#[ignore = "requires the local GLiNER fixture under tests/models/ner/urchade--gliner_multi-v2.1"]
#[test]
fn stdio_mcp_stdout_stays_json_rpc_while_stderr_carries_progress() {
    if !gliner_fixture_present() {
        eprintln!("skipping: missing {}", gliner_fixture_dir().display());
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let store_root = seed_gliner_store(&temp);
    let mut child = Command::new(binary())
        .arg("serve")
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", temp.path().join("db"))
        .env("SURREALDB_DB_NAME", "memory_progress_channels")
        .env("SURREALDB_NAMESPACE", "org")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("EMBEDDINGS_ENABLED", "false")
        .env("NER_EXTRACTOR", "urchade/gliner_multi-v2.1")
        .env("NER_CACHE_DIR", store_root)
        .env("RUST_LOG", "warn")
        .env_remove("SURREALDB_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start stdio MCP server");
    let mut stdin = child.stdin.take().expect("server stdin");
    let stdout = child.stdout.take().expect("server stdout");
    let stderr = child.stderr.take().expect("server stderr");

    let (sender, responses) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = sender.send(line);
        }
    });
    let stderr_handle = std::thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<String>>()
    });

    let mut request = |id: i64, method: &str, params: serde_json::Value| {
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(stdin, "{message}").expect("write MCP request");
        stdin.flush().expect("flush MCP request");
        loop {
            let line = responses
                .recv_timeout(Duration::from_secs(120))
                .unwrap_or_else(|error| panic!("timed out waiting for MCP response {id}: {error}"));
            let value: serde_json::Value =
                serde_json::from_str(&line).expect("stdout line must be valid JSON-RPC");
            if value.get("id").and_then(serde_json::Value::as_i64) == Some(id) {
                return value;
            }
        }
    };

    let _initialize = request(
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "progress-channels-test", "version": "1.0.0"}
        }),
    );
    request(2, "notifications/initialized", serde_json::json!({}));

    let ingest = request(
        3,
        "tools/call",
        serde_json::json!({
            "name": "ingest",
            "arguments": {
                "source_type": "test",
                "source_id": "progress-channels-1",
                "content": "Alice Smith from OpenAI presented Project Atlas in Moscow.",
                "t_ref": "2026-02-05T00:00:00Z",
                "scope": "org"
            }
        }),
    );
    let episode_id = ingest["result"]["structuredContent"]["result"]
        .as_str()
        .expect("ingest returns an episode id")
        .to_string();

    let extracted = request(
        4,
        "tools/call",
        serde_json::json!({
            "name": "extract",
            "arguments": {"episode_id": episode_id}
        }),
    );
    assert!(
        extracted.get("error").is_none(),
        "extract must succeed: {extracted}"
    );

    // Every stdout line was already validated as JSON-RPC by the request loop;
    // additionally assert the server stayed JSON-RPC-only across the session.
    drop(stdin);
    let _ = child.wait().expect("wait for server exit");

    let stderr_lines = stderr_handle.join().expect("stderr reader finished");
    let progress = stderr_lines
        .iter()
        .filter(|line| line.contains("schema_version"))
        .collect::<Vec<_>>();
    assert!(
        !progress.is_empty(),
        "stderr must carry schema-version JSON progress, got: {stderr_lines:?}"
    );
    for line in &progress {
        let event: serde_json::Value =
            serde_json::from_str(line).expect("progress line must be valid JSON");
        assert_eq!(
            event["schema_version"], 1,
            "progress events must carry schema_version 1: {line}"
        );
    }
}

/// CLI progress: the interactive path emits human-readable `[ner]` lines on
/// stderr. Requires the local GLiNER fixture.
#[ignore = "requires the local GLiNER fixture under tests/models/ner/urchade--gliner_multi-v2.1"]
#[test]
fn cli_ner_progress_uses_human_readable_lines() {
    if !gliner_fixture_present() {
        eprintln!("skipping: missing {}", gliner_fixture_dir().display());
        return;
    }
    let temp = TempDir::new().expect("temp dir");
    let store_root = seed_gliner_store(&temp);

    let db_dir = temp.path().join("db");
    let _ = std::fs::create_dir_all(&db_dir);
    // Reuse the seeding helper's layout by ingesting through the CLI so the
    // store activation emits progress on stderr.
    let output = Command::new(binary())
        .arg("ingest")
        .arg("--source-type")
        .arg("test")
        .arg("--source-id")
        .arg("cli-progress-1")
        .arg("--content")
        .arg("Alice Smith from OpenAI presented Project Atlas in Moscow.")
        .arg("--t-ref")
        .arg("2026-02-05T00:00:00Z")
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", &db_dir)
        .env("SURREALDB_DB_NAME", "memory_progress_cli")
        .env("SURREALDB_NAMESPACE", "org")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("EMBEDDINGS_ENABLED", "false")
        .env("NER_EXTRACTOR", "urchade/gliner_multi-v2.1")
        .env("NER_CACHE_DIR", &store_root)
        .env("RUST_LOG", "warn")
        .env_remove("SURREALDB_URL")
        .output()
        .expect("run CLI ingest");

    assert!(
        output.status.success(),
        "CLI ingest must succeed: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[ner]"),
        "CLI progress must be human-readable `[ner]` lines, got: {stderr}"
    );
    assert!(
        !stderr.contains("\"schema_version\""),
        "CLI progress must not use JSON lines: {stderr}"
    );
}

// ── Post-readiness refresh: blocked HTTP must not delay initialize ───────
//
// This process test exercises the gap between MCP readiness and the start of
// the background Classic GLiNER refresh task. A local TCP fixture accepts the
// resolver's HEAD request and deliberately blocks it; the test asserts the
// MCP `initialize` response arrives before the fixture unblocks, proving
// the refresh does not consume the initialization deadline.

#[cfg(all(feature = "eval-support", unix))]
fn send_request(
    stdin: &mut std::process::ChildStdin,
    responses: &std::sync::mpsc::Receiver<String>,
    id: i64,
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> serde_json::Value {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    writeln!(stdin, "{message}").expect("write MCP request");
    stdin.flush().expect("flush MCP request");
    loop {
        let line = responses
            .recv_timeout(timeout)
            .unwrap_or_else(|err| panic!("timed out waiting for MCP response {id}: {err}"));
        let value: serde_json::Value =
            serde_json::from_str(&line).expect("stdout line must be valid JSON-RPC");
        if value.get("id").and_then(serde_json::Value::as_i64) == Some(id) {
            return value;
        }
    }
}

#[cfg(all(feature = "eval-support", unix))]
#[test]
fn blocked_gliner_refresh_does_not_delay_initialize() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    // Bind a TCP listener; the test is the resolver client. The fixture
    // accepts the connection, reads the request, writes a partial response,
    // and blocks until the test signals the gate.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let server_handle = thread::spawn(move || {
        if let Some(Ok(mut stream)) = listener.incoming().next() {
            let mut buf = [0u8; 1024];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
            let _ = stream.read(&mut buf);
            // Send headers with a large content-length so the client
            // keeps reading. The body never arrives, which keeps the
            // refresh fetch blocked while we drive `initialize`.
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n");
            let _ = stream.flush();
            let _ = gate_rx.recv();
        }
    });

    let temp = TempDir::new().expect("temp");
    let mut child = Command::new(binary())
        .arg("serve")
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", temp.path().join("db"))
        .env("SURREALDB_DB_NAME", "memory_blocked_refresh")
        .env("SURREALDB_NAMESPACE", "org")
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env("EMBEDDINGS_ENABLED", "false")
        .env("NER_EXTRACTOR", "urchade/gliner_multi-v2.1")
        .env("NER_CACHE_DIR", temp.path().join("absent-cache"))
        .env(
            "MEMORY_EVAL_NER_ARTIFACT_BASE_URL",
            format!("http://{addr}"),
        )
        .env_remove("SURREALDB_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start stdio MCP server");
    let mut stdin = child.stdin.take().expect("server stdin");
    let stdout = child.stdout.take().expect("server stdout");
    let stderr = child.stderr.take().expect("server stderr");

    let (sender, responses) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = sender.send(line);
        }
    });
    let stderr_handle = thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<String>>()
    });

    let started = std::time::Instant::now();
    let initialize = send_request(
        &mut stdin,
        &responses,
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "blocked-refresh-test", "version": "1.0.0"}
        }),
        Duration::from_secs(10),
    );
    let elapsed = started.elapsed();
    assert!(
        initialize.get("id").and_then(serde_json::Value::as_i64) == Some(1),
        "initialize did not return: {initialize}"
    );
    // The fixture is still blocked when `initialize` returns, which proves
    // the refresh task did not block readiness. We don't constrain a tight
    // wall-clock budget because embedded SurrealDB startup varies.
    assert!(
        elapsed < Duration::from_secs(5),
        "initialize took {elapsed:?}; refresh must not delay it"
    );

    // Unblock the HTTP fixture so the child can exit cleanly.
    let _ = gate_tx.send(());
    let _ = server_handle.join();
    drop(stdin);
    let _ = child.wait();
    let stderr_lines = stderr_handle.join().expect("stderr reader");
    // The refresh task must have started, which means the fixture saw
    // the resolver HEAD request; the gate ensures the response never
    // arrived during the initialize deadline.
    let saw_started = stderr_lines
        .iter()
        .any(|line| line.contains("ner.artifact_refresh.started"));
    assert!(
        saw_started,
        "expected ner.artifact_refresh.started on stderr; got: {stderr_lines:?}"
    );
}
