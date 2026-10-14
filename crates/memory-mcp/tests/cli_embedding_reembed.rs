#![cfg(feature = "eval-support")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use memory_mcp::config::{EmbeddingConfig, SurrealConfigBuilder, build_embedding_signature};
use memory_mcp::knowledge::queries::FACT_TEMPORAL_FIELDS;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const NAMESPACE: &str = "org";
const DATABASE: &str = "memory";
const FACT_ID: &str = "fact:cli-dimension-transition";
const MODEL: &str = "fixture-model";
const FIXTURE_DATA_DIR_ENV: &str = "MEMORY_MCP_TEST_REEMBED_DATA_DIR";
const FIXTURE_DIMENSION_ENV: &str = "MEMORY_MCP_TEST_REEMBED_DIMENSION";

static DIMENSION_TRANSITION_GATE: Mutex<()> = Mutex::const_new(());

fn storage_config(data_dir: &Path, dimension: usize) -> memory_mcp::config::SurrealConfig {
    SurrealConfigBuilder::new()
        .db_name(DATABASE)
        .namespace(NAMESPACE)
        .credentials("root", "root")
        .embedded(true)
        .data_dir(data_dir.display().to_string())
        .embedding_config(EmbeddingConfig {
            dimension_override: Some(dimension),
            ..EmbeddingConfig::default()
        })
        .build()
        .expect("build isolated RocksDB fixture config")
}

async fn seed_stale_fact(data_dir: &Path, dimension: usize) {
    let db = SurrealDbClient::connect(&storage_config(data_dir, dimension))
        .await
        .expect("connect isolated RocksDB fixture");
    db.apply_migrations(NAMESPACE)
        .await
        .expect("apply fixture migrations");
    let now = chrono::Utc::now();
    DbClient::create(
        &db,
        FACT_ID,
        json!({
            "fact_id": FACT_ID,
            "fact_type": "note",
            "content": "CLI reembed transition fixture",
            "quote": "CLI reembed transition fixture",
            "source_episode": "episode:cli-reembed-fixture",
            "t_valid": now,
            "t_ingested": now,
            "confidence": 0.9,
            "index_keys": [],
            "access_count": 0,
            "entity_links": [],
            "scope": NAMESPACE,
            "policy_tags": [],
            "provenance": {"source_episode": "episode:cli-reembed-fixture"},
            "embedding": vec![0.125_f64; dimension],
            "embedding_provider": "legacy-provider",
            "embedding_model": "legacy-model",
            "embedding_dimension": dimension,
            "embedding_signature": "embsig:old-cli-fixture",
            "embedding_updated_at": now,
        }),
        NAMESPACE,
        FACT_TEMPORAL_FIELDS,
    )
    .await
    .expect("seed stale fact at its declared source width");
    drop(db);
}

async fn read_request(stream: &mut TcpStream) -> Result<(Vec<u8>, usize), String> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .ok_or_else(|| "embedding request omitted Content-Length".to_string())?;
            if request.len() >= header_end + 4 + content_length {
                return Ok((request, header_end + 4));
            }
        }

        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("read embedding request: {error}"))?;
        if read == 0 {
            return Err("embedding request closed before its body arrived".to_string());
        }
        request.extend_from_slice(&chunk[..read]);
    }
}

async fn spawn_embedding_endpoint(
    dimension: usize,
) -> (String, Arc<Mutex<Vec<Value>>>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test-only embedding endpoint");
    let address = listener.local_addr().expect("read local endpoint address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed_requests = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = timeout(Duration::from_secs(120), listener.accept())
                .await
                .expect("bounded wait for local provider request")
            else {
                break;
            };
            let Ok((request, body_start)) = read_request(&mut stream).await else {
                continue;
            };
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("request headers are terminated");
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .expect("request body length was checked by read_request");
            let body =
                serde_json::from_slice::<Value>(&request[body_start..body_start + content_length])
                    .expect("provider sends JSON");
            observed_requests.lock().await.push(body);

            let response_body = json!({
                "object": "list",
                "model": MODEL,
                "data": [{"object": "embedding", "index": 0, "embedding": vec![0.25_f64; dimension]}]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            if stream.write_all(response.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    (format!("http://{address}/v1"), requests, server)
}

fn run_bounded_child(mut command: Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bounded test child process");
    let mut stdout = child.stdout.take().expect("capture child stdout");
    let mut stderr = child.stderr.take().expect("capture child stderr");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut stdout, &mut bytes).expect("read child stdout");
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut stderr, &mut bytes).expect("read child stderr");
        bytes
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(120) => {
                std::thread::park_timeout(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("reembed CLI subprocess exceeded its 120-second deadline");
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("poll reembed CLI subprocess: {error}");
            }
        }
    };
    Output {
        status,
        stdout: stdout_reader.join().expect("stdout reader completed"),
        stderr: stderr_reader.join().expect("stderr reader completed"),
    }
}

fn seed_stale_fact_in_child_blocking(data_dir: &Path, dimension: usize) -> Output {
    let mut command =
        Command::new(std::env::current_exe().expect("locate integration test binary"));
    command
        .env_clear()
        .env(FIXTURE_DATA_DIR_ENV, data_dir)
        .env(FIXTURE_DIMENSION_ENV, dimension.to_string())
        .arg("--exact")
        .arg("seed_stale_fact_in_isolated_process")
        .arg("--ignored")
        .arg("--nocapture");
    run_bounded_child(command)
}

async fn seed_stale_fact_from_child(data_dir: &Path, dimension: usize) {
    let data_dir = data_dir.to_path_buf();
    let output = tokio::task::spawn_blocking(move || {
        seed_stale_fact_in_child_blocking(&data_dir, dimension)
    })
    .await
    .expect("bounded fixture process worker completes");
    assert!(
        output.status.success(),
        "fixture seeding process failed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_reembed_cli_blocking(data_dir: &Path, endpoint: &str, target_dimension: usize) -> Output {
    let home = data_dir.join("home");
    std::fs::create_dir_all(&home).expect("create isolated child home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_memory_mcp"));
    command
        .env_clear()
        .env("HOME", home)
        .env("RUST_LOG", "warn")
        .env("SURREALDB_EMBEDDED", "true")
        .env("SURREALDB_DATA_DIR", data_dir)
        .env("SURREALDB_DB_NAME", DATABASE)
        .env("SURREALDB_NAMESPACE", NAMESPACE)
        .env("SURREALDB_USERNAME", "root")
        .env("SURREALDB_PASSWORD", "root")
        .env(
            "SURREALDB_EMBEDDING_DIMENSION",
            target_dimension.to_string(),
        )
        .env("EMBEDDINGS_ENABLED", "true")
        .env("EMBEDDINGS_PROVIDER", "openai-compatible")
        .env("EMBEDDINGS_MODEL", MODEL)
        .env("EMBEDDINGS_BASE_URL", endpoint)
        .env("EMBEDDINGS_API_KEY", "test-only")
        .env("EMBEDDINGS_AUTO_RECOVERY", "false")
        .env("EMBEDDINGS_TIMEOUT_SECS", "5")
        .env("NER_EXTRACTOR", "anno")
        .arg("reembed")
        .arg("--max-failures")
        .arg("0");
    run_bounded_child(command)
}

async fn run_reembed_cli(data_dir: &Path, endpoint: &str, target_dimension: usize) -> Output {
    let data_dir = data_dir.to_path_buf();
    let endpoint = endpoint.to_string();
    tokio::task::spawn_blocking(move || {
        run_reembed_cli_blocking(&data_dir, &endpoint, target_dimension)
    })
    .await
    .expect("bounded CLI process worker completes")
}

async fn declared_index_dimension(db: &SurrealDbClient) -> Option<usize> {
    let raw = db
        .query("INFO FOR TABLE fact", None, NAMESPACE)
        .await
        .expect("read persisted fact-index definition");
    let record = raw.as_array()?.first()?;
    let define = record
        .get("indexes")?
        .get("fact_embedding_hnsw")?
        .as_str()?;
    let after = &define[define.find("DIMENSION ")? + "DIMENSION ".len()..];
    let width: String = after.chars().take_while(char::is_ascii_digit).collect();
    width.parse().ok()
}

async fn assert_cli_dimension_transition(from: usize, to: usize) {
    let _gate = DIMENSION_TRANSITION_GATE.lock().await;
    let temp_dir = tempfile::tempdir().expect("isolated CLI RocksDB root");
    seed_stale_fact_from_child(temp_dir.path(), from).await;
    let (endpoint, requests, server) = spawn_embedding_endpoint(to).await;

    let output = run_reembed_cli(temp_dir.path(), &endpoint, to).await;
    server.abort();
    let _ = server.await;
    assert!(
        output.status.success(),
        "CLI reembed from {from} to {to} failed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let observed_requests = requests.lock().await.clone();
    assert_eq!(
        observed_requests.len(),
        1,
        "the explicit dimension override avoids a startup probe; only reembed calls the provider"
    );
    assert_eq!(observed_requests[0]["model"], MODEL);
    assert_eq!(
        observed_requests[0]["input"],
        "note\nCLI reembed transition fixture\nCLI reembed transition fixture"
    );

    let db = SurrealDbClient::connect(&storage_config(temp_dir.path(), from))
        .await
        .expect("reopen persisted RocksDB after the child process exits");
    let fact = db
        .select_one(FACT_ID, NAMESPACE)
        .await
        .expect("read persisted fact")
        .expect("fact remains durable");
    assert_eq!(fact["embedding"].as_array().map(Vec::len), Some(to));
    let first_value = fact["embedding"]
        .as_array()
        .and_then(|vector| vector[0].as_f64());
    let expected_normalized_value = 1.0 / (to as f64).sqrt();
    assert!(first_value.is_some_and(|value| (value - expected_normalized_value).abs() < 1e-12));
    assert_eq!(fact["embedding_dimension"], to);
    assert_eq!(
        fact["embedding_signature"],
        build_embedding_signature("openai-compatible", Some(MODEL), Some(&endpoint), to)
    );
    assert_eq!(fact["embedding_provider"], "openai-compatible");
    assert_eq!(fact["embedding_model"], MODEL);
    assert_eq!(declared_index_dimension(&db).await, Some(to));

    let job = db
        .select_one("embedding_job:fact_reembed", NAMESPACE)
        .await
        .expect("read durable CLI reembed job")
        .expect("CLI reembed job is persisted");
    assert_eq!(job["status"], "completed");
    assert_eq!(
        job["namespace_progress"][NAMESPACE]["last_completed_fact_id"],
        FACT_ID
    );
}

#[tokio::test]
#[ignore = "fixture seeding runs separately so the CLI can acquire the RocksDB lock"]
async fn seed_stale_fact_in_isolated_process() {
    let data_dir = PathBuf::from(
        std::env::var_os(FIXTURE_DATA_DIR_ENV).expect("fixture process data directory"),
    );
    let dimension = std::env::var(FIXTURE_DIMENSION_ENV)
        .expect("fixture process embedding dimension")
        .parse::<usize>()
        .expect("fixture dimension is an integer");
    seed_stale_fact(&data_dir, dimension).await;
}

#[tokio::test]
async fn cli_reembed_384_to_2048_rewrites_vector_and_index() {
    assert_cli_dimension_transition(384, 2048).await;
}

#[tokio::test]
async fn cli_reembed_2048_to_384_rewrites_vector_and_index() {
    assert_cli_dimension_transition(2048, 384).await;
}

#[tokio::test]
async fn cli_reembed_1536_to_3072_rewrites_vector_and_index() {
    assert_cli_dimension_transition(1536, 3072).await;
}

#[tokio::test]
async fn cli_reembed_changes_signature_at_768() {
    assert_cli_dimension_transition(768, 768).await;
}
