//! Isolated Streamable HTTP fixture for paired agent trials.

use std::collections::BTreeMap;
#[cfg(unix)]
use std::io::Read;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::EvalError;
use crate::paired_agent::{SeedEpisode, SeedMemoryOutput};

const HTTP_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const HTTP_READINESS_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_API_KEY: &str = "memoryeval";
const HTTP_PROTOCOL_VERSION: &str = "2026-07-28";

pub struct IsolatedHttpMemoryServer {
    pub(crate) launch_profile: BTreeMap<String, String>,
    child: Child,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
    client: reqwest::Client,
    pub endpoint: String,
    pub bearer_token: String,
    host_header: String,
    next_request_id: i64,
}

impl IsolatedHttpMemoryServer {
    pub fn spawn(
        binary: &Path,
        database_path: &Path,
        claim_rollout_stage: &str,
    ) -> Result<Self, EvalError> {
        Self::spawn_inner(binary, database_path, claim_rollout_stage, false)
    }

    pub(crate) fn spawn_deterministic(
        binary: &Path,
        database_path: &Path,
        claim_rollout_stage: &str,
    ) -> Result<Self, EvalError> {
        Self::spawn_inner(binary, database_path, claim_rollout_stage, true)
    }

    fn spawn_inner(
        binary: &Path,
        database_path: &Path,
        claim_rollout_stage: &str,
        deterministic_extraction: bool,
    ) -> Result<Self, EvalError> {
        if !binary.is_absolute() || !binary.is_file() {
            return Err(EvalError::InvalidConfig(
                "paired HTTP memory server must be an existing absolute binary".into(),
            ));
        }
        if !database_path.is_absolute() || !database_path.is_dir() {
            return Err(EvalError::InvalidConfig(
                "paired HTTP database path must be an existing absolute directory".into(),
            ));
        }
        if !matches!(
            claim_rollout_stage,
            "disabled" | "shadow" | "relations" | "evidence"
        ) {
            return Err(EvalError::InvalidConfig(
                "paired HTTP memory server claim rollout stage is unsupported".into(),
            ));
        }

        let bind_address = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .map_err(|error| {
                EvalError::Suite(format!("select isolated HTTP bind address: {error}"))
            })?;
        let bearer_token = ephemeral_api_key()?;
        let client = reqwest::Client::builder()
            .timeout(HTTP_REQUEST_TIMEOUT)
            .build()
            .map_err(|error| EvalError::Suite(format!("build HTTP MCP client: {error}")))?;
        let storage_url = format!("surrealkv://{}", database_path.display());
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TMPDIR", std::env::temp_dir())
            .env("RUST_LOG", "warn")
            .env("MEMORY_MCP_HTTP_BIND", bind_address.to_string())
            .env("MEMORY_MCP_HTTP_PUBLIC_BASE_URL", "http://localhost")
            .env("ALLOWED_HOSTS", format!("localhost,{bind_address}"))
            .env("ALLOWED_ORIGINS", "http://localhost")
            .env("MEMORY_MCP_API_KEY_PEPPER", "x".repeat(40))
            .env("MEMORY_MCP_HTTP_SIGNUP_MODE", "invite_only")
            .env("MEMORY_MCP_HTTP_CSRF_KEY", "0".repeat(64))
            .env("MEMORY_MCP_HTTP_SESSION_KEY", "0".repeat(64))
            .env("SURREALDB_CONTROL_URL", &storage_url)
            .env("SURREALDB_CONTROL_USERNAME", "root")
            .env("SURREALDB_CONTROL_PASSWORD", "root")
            .env("SURREALDB_CONTROL_DB", "control")
            .env("SURREALDB_CONTROL_NAMESPACE", "control")
            .env("SURREALDB_TENANT_URL", &storage_url)
            .env("SURREALDB_TENANT_USERNAME", "root")
            .env("SURREALDB_TENANT_PASSWORD", "root")
            .env("SURREALDB_TENANT_DB", "tenant")
            .env("SURREALDB_TENANT_NAMESPACE", "tenant")
            .env("MEMORY_MCP_HTTP_AUTH_METHODS", "local")
            .env("MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION", "1")
            .env("MEMORY_MCP_HTTP_MAX_INGESTED_BYTES", "1073741824")
            .env("MEMORY_MCP_HTTP_MAX_EPISODE_COUNT", "100000")
            .env("MEMORY_MCP_HTTP_INGEST_PER_MINUTE", "60")
            .env("MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS", "32")
            .env("MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS", "5")
            .env("MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY", "4")
            .env("MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY", "2")
            .env(
                "MEMORY_MCP_HTTP_TEST_BOOTSTRAP",
                format!("{HTTP_API_KEY}={bearer_token}"),
            )
            .env("MEMORY_CLAIM_ROLLOUT_STAGE", claim_rollout_stage)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if deterministic_extraction {
            command
                .env("NER_EXTRACTOR", "regex")
                .env("EMBEDDINGS_ENABLED", "false");
        }

        let launch_profile = crate::suites::memory_decisions::stdio::launch_profile(&command);
        let mut child = command.spawn().map_err(|source| EvalError::Io {
            path: binary.to_path_buf(),
            source,
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EvalError::Suite("HTTP memory server stdout was not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| EvalError::Suite("HTTP memory server stderr was not piped".into()))?;
        let (bound_sender, bound_receiver) = mpsc::sync_channel(1);
        let stdout_reader = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            let mut sent = false;
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if !sent && let Some(address) = line.strip_prefix("memory_mcp_http bound=") {
                    let _ = bound_sender.send(Ok(address.to_string()));
                    sent = true;
                }
            }
            if !sent {
                let _ = bound_sender.send(Err(
                    "HTTP memory server exited before reporting its bound address".to_string(),
                ));
            }
        });
        let stderr_reader = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if line.is_err() {
                    break;
                }
            }
        });
        let address = match bound_receiver.recv_timeout(HTTP_STARTUP_TIMEOUT) {
            Ok(Ok(address)) => address,
            Ok(Err(message)) => {
                stop_child(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(EvalError::Suite(message));
            }
            Err(error) => {
                stop_child(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(EvalError::Suite(format!(
                    "HTTP memory server startup did not complete within {} seconds: {error}",
                    HTTP_STARTUP_TIMEOUT.as_secs()
                )));
            }
        };
        Ok(Self {
            launch_profile,
            child,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            client,
            endpoint: format!("http://{address}/mcp"),
            bearer_token,
            host_header: address,
            next_request_id: 1,
        })
    }

    pub async fn wait_ready(&self) -> Result<(), EvalError> {
        let health = self.endpoint.trim_end_matches("/mcp").to_string() + "/health/ready";
        let deadline = tokio::time::Instant::now() + HTTP_READINESS_TIMEOUT;
        loop {
            let ready = self
                .client
                .get(&health)
                .header("host", &self.host_header)
                .send()
                .await
                .is_ok_and(|response| response.status().is_success());
            if ready {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(EvalError::Suite(
                    "HTTP memory server did not become ready before its startup deadline".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn seed(
        &mut self,
        episodes: &[SeedEpisode],
    ) -> Result<(BTreeMap<String, String>, Value), EvalError> {
        let seeded = self.seed_inner(episodes, None).await?;
        Ok((seeded.episode_id_to_event_id, seeded.tool_schemas))
    }

    pub(crate) async fn seed_with_revision_checkpoint(
        &mut self,
        episodes: &[SeedEpisode],
        checkpoint_before_index: usize,
        valid_at: DateTime<Utc>,
    ) -> Result<SeedMemoryOutput, EvalError> {
        let seeded = self
            .seed_inner(episodes, Some((checkpoint_before_index, valid_at)))
            .await?;
        if seeded.known_revision.is_some() {
            Ok(seeded)
        } else {
            Err(EvalError::InvalidInput(
                "HTTP seed omitted its requested revision checkpoint".into(),
            ))
        }
    }

    async fn seed_inner(
        &mut self,
        episodes: &[SeedEpisode],
        checkpoint: Option<(usize, DateTime<Utc>)>,
    ) -> Result<SeedMemoryOutput, EvalError> {
        let listed = self
            .request("tools/list", json!({ "_meta": modern_meta() }))
            .await?;
        let tools = listed
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .ok_or_else(|| EvalError::InvalidInput("HTTP tools/list lacks result.tools".into()))?;
        for required in ["ingest", "extract", "assemble_context"] {
            if !tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some(required)
                    && tool.get("inputSchema").is_some()
            }) {
                return Err(EvalError::InvalidInput(format!(
                    "HTTP memory server does not advertise `{required}` with an input schema"
                )));
            }
        }
        let schemas = Value::Array(tools.clone());
        let mut bindings = BTreeMap::new();
        if checkpoint.is_some_and(|(index, _)| index > episodes.len()) {
            return Err(EvalError::InvalidInput(
                "HTTP revision checkpoint lies beyond the seeded history".into(),
            ));
        }
        let mut checkpoint_revision = None;
        for (index, episode) in episodes.iter().enumerate() {
            if let Some((checkpoint_index, valid_at)) = checkpoint
                && index == checkpoint_index
            {
                checkpoint_revision = Some(self.read_known_revision(valid_at).await?);
            }
            let ingest = self
                .call_tool(
                    "ingest",
                    json!({
                        "source_type": episode.source_type,
                        "source_id": episode.source_id,
                        "content": episode.content,
                        "t_ref": episode.valid_at.to_rfc3339(),
                    }),
                )
                .await?;
            let episode_id = ingest
                .pointer("/result/structuredContent/result")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| {
                    EvalError::InvalidInput("HTTP ingest did not return an episode ID".into())
                })?
                .to_string();
            if bindings.contains_key(&episode_id) {
                return Err(EvalError::InvalidInput(
                    "HTTP ingest returned a duplicate episode ID".into(),
                ));
            }
            bindings.insert(episode_id.clone(), episode.event_id.clone());
            self.call_tool("extract", json!({"episode_id":episode_id}))
                .await?;
        }
        if let Some((checkpoint_index, valid_at)) = checkpoint
            && checkpoint_index == episodes.len()
        {
            checkpoint_revision = Some(self.read_known_revision(valid_at).await?);
        }
        Ok(SeedMemoryOutput {
            episode_id_to_event_id: bindings,
            tool_schemas: schemas,
            known_revision: checkpoint_revision,
        })
    }

    async fn read_known_revision(&mut self, valid_at: DateTime<Utc>) -> Result<i64, EvalError> {
        let response = self
            .call_tool(
                "assemble_context",
                json!({
                    "query":"",
                    "valid_at":valid_at.to_rfc3339(),
                    "budget":1,
                    "compact":true
                }),
            )
            .await?;
        response
            .pointer("/result/structuredContent/context/snapshot/known_revision")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                EvalError::InvalidInput(
                    "HTTP assemble_context checkpoint response lacks known_revision".into(),
                )
            })
    }

    pub(crate) async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<Value, EvalError> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments, "_meta": modern_meta() }),
            )
            .await?;
        if result.pointer("/result/isError").and_then(Value::as_bool) == Some(true) {
            return Err(EvalError::Suite(format!(
                "HTTP MCP tool `{name}` returned isError=true"
            )));
        }
        Ok(result)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, EvalError> {
        let request_id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or_else(|| EvalError::Suite("HTTP MCP request id overflowed".into()))?;
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let response = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("host", &self.host_header)
            .header("authorization", format!("Bearer {}", self.bearer_token))
            .header("mcp-protocol-version", HTTP_PROTOCOL_VERSION)
            .header("mcp-method", method)
            .header("mcp-name", name)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .map_err(|error| {
                EvalError::Suite(format!("HTTP MCP {method} request failed: {error}"))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            EvalError::Suite(format!("read HTTP MCP {method} response: {error}"))
        })?;
        if !status.is_success() {
            return Err(EvalError::Suite(format!(
                "HTTP MCP {method} returned HTTP {}",
                status.as_u16()
            )));
        }
        let payload = parse_http_mcp_payload(&body)?;
        if payload.get("error").is_some() {
            return Err(EvalError::Suite(format!(
                "HTTP MCP {method} returned a JSON-RPC error"
            )));
        }
        if payload.get("id").and_then(Value::as_i64) != Some(request_id) {
            return Err(EvalError::Suite(format!(
                "HTTP MCP {method} returned a mismatched response id"
            )));
        }
        Ok(payload)
    }
}

impl Drop for IsolatedHttpMemoryServer {
    fn drop(&mut self) {
        stop_child(&mut self.child);
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

fn stop_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": HTTP_PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientInfo": {
            "name": "memory-mcp-paired-eval",
            "version": "1",
        },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

fn parse_http_mcp_payload(body: &str) -> Result<Value, EvalError> {
    if body.trim().is_empty() {
        return Err(EvalError::InvalidInput(
            "HTTP MCP response body is empty".into(),
        ));
    }
    if body.trim_start().starts_with("event:") || body.trim_start().starts_with("data:") {
        for line in body.lines() {
            if let Some(data) = line.strip_prefix("data:")
                && let Ok(value) = serde_json::from_str::<Value>(data.trim())
            {
                return Ok(value);
            }
        }
        return Err(EvalError::InvalidInput(
            "HTTP MCP event stream did not contain a JSON response".into(),
        ));
    }
    serde_json::from_str(body).map_err(EvalError::from)
}

fn ephemeral_api_key() -> Result<String, EvalError> {
    #[cfg(unix)]
    let entropy = {
        let mut source = std::fs::File::open("/dev/urandom")
            .map_err(|error| EvalError::Suite(format!("open OS entropy source: {error}")))?;
        let mut entropy = [0_u8; 32];
        source
            .read_exact(&mut entropy)
            .map_err(|error| EvalError::Suite(format!("read OS entropy source: {error}")))?;
        entropy
    };
    #[cfg(not(unix))]
    let entropy = {
        return Err(EvalError::InvalidConfig(
            "paired HTTP evaluation requires a Unix OS entropy source".into(),
        ));
    };
    Ok(api_key_from_entropy(entropy))
}

fn api_key_from_entropy(mut entropy: [u8; 32]) -> String {
    entropy[6] = (entropy[6] & 0x0f) | 0x40;
    entropy[8] = (entropy[8] & 0x3f) | 0x80;
    let mut uuid = hex::encode(&entropy[..16]);
    uuid.insert(8, '-');
    uuid.insert(13, '-');
    uuid.insert(18, '-');
    uuid.insert(23, '-');
    let secret = hex::encode(&entropy[16..]);
    format!("mem_sk_ak_{uuid}_{secret}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[test]
    fn ephemeral_http_key_formats_os_entropy_as_a_uuid_v4_api_key() {
        let key = api_key_from_entropy([0_u8; 32]);

        assert_eq!(
            key,
            "mem_sk_ak_00000000-0000-4000-8000-000000000000_00000000000000000000000000000000"
        );
    }

    #[test]
    fn ephemeral_http_key_keeps_random_secret_bytes_outside_the_uuid() {
        let mut entropy = [0_u8; 32];
        entropy[16..].copy_from_slice(&[0xAB; 16]);

        let key = api_key_from_entropy(entropy);

        assert!(key.ends_with("ab".repeat(16).as_str()));
    }

    #[tokio::test]
    async fn isolated_http_seed_uses_the_public_stateless_mcp_contract() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local MCP simulator");
        let address = listener.local_addr().expect("read simulator address");
        let observed = Arc::new(Mutex::new(Vec::<(String, String, Value)>::new()));
        let server_observed = Arc::clone(&observed);
        let server = tokio::spawn(async move {
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().await.expect("accept MCP request");
                let (headers, request) = read_request(&mut stream).await;
                let method = if headers
                    .lines()
                    .next()
                    .is_some_and(|line| line.starts_with("get /health/ready "))
                {
                    "GET /health/ready".to_string()
                } else {
                    request
                        .pointer("/method")
                        .and_then(Value::as_str)
                        .expect("JSON-RPC method")
                        .to_string()
                };
                let name = request
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                server_observed.lock().expect("capture request").push((
                    method.clone(),
                    name.clone(),
                    request.clone(),
                ));

                let response = match method.as_str() {
                    "GET /health/ready" => ("application/json", r#"{"status":"ready"}"#.into()),
                    "tools/list" => (
                        "application/json",
                        serde_json::json!({
                            "jsonrpc":"2.0",
                            "id":request["id"],
                            "result":{"tools":[
                                {"name":"ingest","inputSchema":{"type":"object"}},
                                {"name":"extract","inputSchema":{"type":"object"}},
                                {"name":"assemble_context","inputSchema":{"type":"object"}}
                            ]}
                        })
                        .to_string(),
                    ),
                    "tools/call" if name == "ingest" => (
                        "application/json",
                        serde_json::json!({
                            "jsonrpc":"2.0",
                            "id":request["id"],
                            "result":{"structuredContent":{"result":"episode-1"}}
                        })
                        .to_string(),
                    ),
                    "tools/call" if name == "extract" => (
                        "text/event-stream",
                        format!(
                            "event: message\ndata: {}\n\n",
                            serde_json::json!({
                                "jsonrpc":"2.0",
                                "id":request["id"],
                                "result":{"structuredContent":{"result":"extracted"}}
                            })
                        ),
                    ),
                    "tools/call" if name == "assemble_context" => (
                        "application/json",
                        serde_json::json!({
                            "jsonrpc":"2.0",
                            "id":request["id"],
                            "result":{"structuredContent":{"context":{"snapshot":{"known_revision":1}}}}
                        })
                        .to_string(),
                    ),
                    other => panic!("unexpected MCP request {other} {name}"),
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response.0,
                    response.1.len(),
                    response.1
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("send MCP response");
                if method != "GET /health/ready" {
                    assert!(headers.contains(&format!("host: {}", address).to_ascii_lowercase()));
                    assert!(headers.contains("authorization: bearer mem_sk_ak_"));
                    assert!(headers.contains("mcp-protocol-version: 2026-07-28"));
                }
            }
        });

        let temporary = tempfile::tempdir().expect("create HTTP fixture directory");
        let binary = temporary.path().join("memory_mcp_http_fixture.sh");
        std::fs::write(
            &binary,
            format!("#!/bin/sh\nprintf 'memory_mcp_http bound={address}\\n'\nexec /bin/sleep 30\n"),
        )
        .expect("write fake HTTP process");
        let mut permissions = std::fs::metadata(&binary)
            .expect("read fake process mode")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions).expect("make fake HTTP process executable");
        let database = temporary.path().join("database");
        std::fs::create_dir(&database).expect("create isolated database directory");

        let mut fixture = IsolatedHttpMemoryServer::spawn(&binary, &database, "shadow")
            .expect("start isolated HTTP fixture");
        fixture.wait_ready().await.expect("health readiness");
        let valid_at = chrono::DateTime::parse_from_rfc3339("2026-11-20T10:00:00Z")
            .expect("parse fixture time")
            .with_timezone(&Utc);
        let seeded = fixture
            .seed_with_revision_checkpoint(
                &[SeedEpisode {
                    event_id: "event-1".into(),
                    source_id: "source-1".into(),
                    source_type: "conversation".into(),
                    content: "The train code is FJ-38.".into(),
                    valid_at,
                }],
                1,
                valid_at,
            )
            .await
            .expect("seed memory and capture its public revision over HTTP MCP");

        assert_eq!(seeded.known_revision, Some(1));
        assert_eq!(
            seeded
                .episode_id_to_event_id
                .get("episode-1")
                .map(String::as_str),
            Some("event-1")
        );
        assert_eq!(seeded.tool_schemas.as_array().map(Vec::len), Some(3));
        server.await.expect("MCP simulator completes");
        let requests = observed.lock().expect("read captured requests");
        assert_eq!(
            requests
                .iter()
                .map(|(method, _, _)| method.as_str())
                .collect::<Vec<_>>(),
            [
                "GET /health/ready",
                "tools/list",
                "tools/call",
                "tools/call",
                "tools/call"
            ]
        );
        assert_eq!(requests[2].1, "ingest");
        assert_eq!(requests[3].1, "extract");
        assert_eq!(requests[4].1, "assemble_context");
        assert_eq!(
            requests[4].2["params"]["arguments"]["valid_at"],
            valid_at.to_rfc3339()
        );
        assert_eq!(requests[4].2["params"]["arguments"]["budget"], 1);
        assert_eq!(
            requests[2]
                .2
                .pointer("/params/arguments/content")
                .and_then(Value::as_str),
            Some("The train code is FJ-38.")
        );
    }

    #[tokio::test]
    #[ignore = "requires a memory_mcp_http binary built with streamable-http,test-fixtures"]
    async fn live_http_binary_accepts_an_isolated_paired_seed() {
        let binary = std::env::var_os("MEMORY_MCP_HTTP_EVAL_BINARY")
            .map(std::path::PathBuf::from)
            .expect("set MEMORY_MCP_HTTP_EVAL_BINARY to the built HTTP fixture binary");
        let temporary = tempfile::tempdir().expect("create HTTP fixture directory");
        let database = temporary.path().join("database");
        std::fs::create_dir(&database).expect("create isolated database directory");
        let mut server = IsolatedHttpMemoryServer::spawn(&binary, &database, "shadow")
            .expect("start isolated HTTP memory server");
        server.wait_ready().await.expect("server readiness");
        let valid_at = chrono::DateTime::parse_from_rfc3339("2026-11-20T10:00:00Z")
            .expect("parse fixture time")
            .with_timezone(&Utc);
        let seeded = server
            .seed_with_revision_checkpoint(
                &[SeedEpisode {
                    event_id: "event-1".into(),
                    source_id: "source-1".into(),
                    source_type: "conversation".into(),
                    content: "The train code is FJ-38.".into(),
                    valid_at,
                }],
                1,
                valid_at,
            )
            .await
            .expect("seed the live HTTP memory server and capture its revision");

        assert!(seeded.known_revision.is_some_and(|revision| revision > 0));
        assert_eq!(seeded.episode_id_to_event_id.len(), 1);
        assert!(
            seeded
                .episode_id_to_event_id
                .values()
                .any(|event_id| event_id == "event-1")
        );
        assert!(seeded.tool_schemas.as_array().is_some_and(|tools| {
            ["ingest", "extract", "assemble_context"]
                .iter()
                .all(|required| tools.iter().any(|tool| tool["name"] == *required))
        }));
    }

    async fn read_request(stream: &mut TcpStream) -> (String, Value) {
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 4096];
            let count = stream.read(&mut chunk).await.expect("read request");
            assert_ne!(count, 0, "request connection closed early");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let header = String::from_utf8(bytes[..header_end].to_vec()).expect("request headers utf8");
        let content_length = header
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("valid content length"))
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let mut chunk = [0_u8; 4096];
            let count = stream.read(&mut chunk).await.expect("read request body");
            assert_ne!(count, 0, "request body closed early");
            bytes.extend_from_slice(&chunk[..count]);
        }
        let request = if content_length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[header_end..header_end + content_length])
                .expect("valid MCP JSON request")
        };
        (header.to_ascii_lowercase(), request)
    }
}
