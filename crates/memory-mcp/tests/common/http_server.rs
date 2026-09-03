//! Shared HTTP server subprocess fixture (ADR-0053, Task 4).
//!
//! Spins up the real `memory_mcp_http` binary, waits for it to
//! report its bound address, and exposes a small handle for tests.
//! Storage defaults to `mem://`; tests that need durable composition
//! pass `storage_url = "rocksdb://..."` and get a `TempDir` that
//! survives `restart`.

#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]
#![allow(dead_code)] // Shared fixture: only some test crates use each helper.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct TestTenant {
    pub name: String,
    pub api_key: String,
}

impl TestTenant {
    pub fn new(name: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            api_key: api_key.into(),
        }
    }
}

pub struct HttpServerConfig {
    pub tenants: Vec<TestTenant>,
    pub extra_env: Vec<(String, String)>,
    /// Storage URL the binary connects to. Defaults to `mem://`; tests
    /// that need durable composition pass `rocksdb://...`.
    pub storage_url: String,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            tenants: Vec::new(),
            extra_env: Vec::new(),
            storage_url: "mem://".to_string(),
        }
    }
}

impl HttpServerConfig {
    pub fn with_tenant(mut self, tenant: TestTenant) -> Self {
        self.tenants.push(tenant);
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    pub fn with_storage_url(mut self, url: impl Into<String>) -> Self {
        self.storage_url = url.into();
        self
    }
}

pub struct HttpServerFixture {
    child: Child,
    pub base_url: String,
    pub config: HttpServerConfig,
    storage: Option<tempfile::TempDir>,
    _stderr_drain: JoinHandle<()>,
    client: reqwest::Client,
}

impl HttpServerFixture {
    pub async fn spawn(config: HttpServerConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");

        // "rocksdb" is the placeholder that means "allocate a fresh
        // tempdir". An explicit "rocksdb:///path" keeps the path across
        // restart so durability tests can prove the data survives.
        let storage = if config.storage_url == "rocksdb" {
            Some(tempfile::tempdir().expect("rocksdb tempdir"))
        } else {
            None
        };
        let resolved_storage_url = match &storage {
            Some(dir) => format!("rocksdb://{}", dir.path().display()),
            None => config.storage_url.clone(),
        };

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("RUST_LOG", "info");
        for (k, v) in build_env(&config, &resolved_storage_url) {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn memory_mcp_http");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let bound = tokio::task::spawn_blocking(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let line = line.expect("read stdout");
                if let Some(addr) = line.strip_prefix("memory_mcp_http bound=") {
                    return addr.to_string();
                }
            }
            panic!("server exited before printing bound line");
        });
        let stderr_drain = tokio::task::spawn_blocking(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let line = line.expect("read stderr");
                eprintln!("server stderr: {line}");
            }
        });
        let addr = bound.await.expect("join bound line");
        let base_url = format!("http://{addr}");

        Self {
            child,
            base_url,
            config,
            storage,
            _stderr_drain: stderr_drain,
            client,
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Poll /health/ready until it returns 200 or the deadline expires.
    /// `spawn` already waits for the bind line; this is an extra
    /// readiness probe for tests that want to know the registry has
    /// been opened before sending traffic.
    pub async fn wait_ready(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let ready = matches!(
                self.client
                    .get(format!("{}/health/ready", self.base_url))
                    .header("host", "localhost")
                    .send()
                    .await,
                Ok(resp) if resp.status() == 200
            );
            if ready {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("server did not become ready within 10s");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // RocksDB releases its LOCK file lazily after the
        // process is reaped; tests that immediately
        // spawn another subprocess against the same
        // path benefit from a brief wait so the kernel
        // releases the file descriptor. 500ms is
        // empirically bounded on macOS and Linux; the
        // LOCK file is gone well before any production
        // scheduler tick.
        std::thread::sleep(Duration::from_millis(500));
    }

    /// Restart the server while preserving the durable storage
    /// directory. A fresh process binds to a new ephemeral port;
    /// callers should use the new `base_url` after this returns.
    pub async fn restart(&mut self) {
        self.kill();
        // If storage_url is the "rocksdb" placeholder, the next spawn
        // will allocate a fresh tempdir; explicit "rocksdb:///path"
        // strings are preserved verbatim by the next spawn, so the
        // directory and its contents survive the restart.
        let new = Self::spawn(std::mem::take(&mut self.config)).await;
        let _ = std::mem::replace(self, new);
    }
}

impl Drop for HttpServerFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn build_env(config: &HttpServerConfig, storage_url: &str) -> Vec<(String, String)> {
    let zeros = "0".repeat(64);
    let bootstrap = config
        .tenants
        .iter()
        .map(|t| format!("{}={}", t.name, t.api_key))
        .collect::<Vec<_>>()
        .join(",");
    let mut env: Vec<(String, String)> = vec![
        ("MEMORY_MCP_HTTP_BIND".into(), "127.0.0.1:0".into()),
        (
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL".into(),
            "http://localhost".into(),
        ),
        ("ALLOWED_HOSTS".into(), "localhost,127.0.0.1".into()),
        ("ALLOWED_ORIGINS".into(), "http://localhost".into()),
        ("MEMORY_MCP_API_KEY_PEPPER".into(), "x".repeat(40)),
        ("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_SIGNUP_MODE".into(), "invite_only".into()),
        ("MEMORY_MCP_HTTP_CSRF_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_OIDC_STATE_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_OIDC_NONCE_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_SESSION_KEY".into(), zeros),
        ("SURREALDB_CONTROL_URL".into(), storage_url.to_string()),
        ("SURREALDB_CONTROL_USERNAME".into(), "root".into()),
        ("SURREALDB_CONTROL_PASSWORD".into(), "root".into()),
        ("SURREALDB_CONTROL_DB".into(), "control".into()),
        ("SURREALDB_CONTROL_NAMESPACE".into(), "control".into()),
        ("SURREALDB_TENANT_URL".into(), storage_url.to_string()),
        ("SURREALDB_TENANT_USERNAME".into(), "root".into()),
        ("SURREALDB_TENANT_PASSWORD".into(), "root".into()),
        ("SURREALDB_TENANT_DB".into(), "tenant".into()),
        ("SURREALDB_TENANT_NAMESPACE".into(), "tenant".into()),
        (
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE".into(),
            "false".into(),
        ),
        (
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI".into(),
            "false".into(),
        ),
        ("MEMORY_MCP_HTTP_TEST_BOOTSTRAP".into(), bootstrap),
    ];
    for (k, v) in &config.extra_env {
        env.push((k.clone(), v.clone()));
    }
    env
}

pub fn modern_meta() -> Value {
    serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {
            "name": "memory-mcp-test",
            "version": "0.0.0",
        },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

pub async fn mcp_call(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    method: &str,
    params: Value,
) -> Value {
    let mut params = params;
    params
        .as_object_mut()
        .expect("params object")
        .insert("_meta".into(), modern_meta());
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("host", "localhost")
        .header("authorization", format!("Bearer {api_key}"))
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", method)
        .header(
            "mcp-name",
            params.get("name").and_then(Value::as_str).unwrap_or(""),
        )
        .body(serde_json::to_string(&body).unwrap())
        .send()
        .await
        .expect("send request");
    let status = resp.status();
    let text = resp.text().await.unwrap();
    if text.starts_with("event:") || text.starts_with("data:") {
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ")
                && let Ok(val) = serde_json::from_str::<Value>(data)
            {
                return serde_json::json!({
                    "http_status": status.as_u16(),
                    "payload": val
                });
            }
        }
    }
    let payload = serde_json::from_str(&text).unwrap_or(serde_json::json!({"raw": text}));
    serde_json::json!({
        "http_status": status.as_u16(),
        "payload": payload
    })
}
